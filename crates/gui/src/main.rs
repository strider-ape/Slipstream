//! Slipstream desktop app: the download list with add, pause, resume and delete,
//! settings, and a popup when the browser hands a download over. A client of the
//! daemon. Drawn on the CPU (Slint's software renderer): no GPU stack is loaded.

// No console window on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod link;
#[cfg(windows)]
mod tray;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use link::Command;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use slipstream_core::ipc::{AppSettings, DownloadInfo, Status};

slint::include_modules!();

/// Choices in the "Connections per file" box.
const CONNECTIONS: [usize; 7] = [1, 2, 4, 8, 16, 24, 32];

thread_local! {
    /// The latest list from the daemon (UI thread only).
    static LIST: RefCell<Vec<DownloadInfo>> = RefCell::default();
    /// Settings as last loaded, so saving keeps what the form doesn't show.
    static SETTINGS: RefCell<Option<AppSettings>> = RefCell::default();
    /// The download the "caught" popup is about.
    static CAUGHT: Cell<i64> = const { Cell::new(-1) };
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let popup = CaughtPopup::new()?;
    ui.set_rows(ModelRc::new(Rc::new(VecModel::<DownloadRow>::default())));
    ui.set_segments(ModelRc::new(Rc::new(VecModel::<SegmentBar>::default())));
    let tx = link::start(ui.as_weak(), popup.as_weak());
    let send = move |c: Command| {
        let _ = tx.send(c);
    };

    let s = send.clone();
    ui.on_add_url(move |url| {
        let url = url.trim().to_owned();
        if !url.is_empty() {
            s(Command::Add(url));
        }
    });
    let s = send.clone();
    ui.on_pause(move |id| s(Command::Pause(id.into())));
    let s = send.clone();
    ui.on_resume(move |id| s(Command::Resume(id.into())));
    let s = send.clone();
    ui.on_remove(move |id| {
        // Unfinished: drop the partial file too. Finished: keep the file.
        let done = with_download(id.into(), |d| d.status == Status::Complete);
        s(Command::Remove {
            id: id.into(),
            delete_file: !done.unwrap_or(false),
        });
    });
    ui.on_open_folder(|id| {
        with_download(id.into(), open_folder);
    });
    let weak = ui.as_weak();
    ui.on_selection_changed(move |_| {
        if let Some(ui) = weak.upgrade() {
            refresh_details(&ui);
        }
    });
    let (s, weak) = (send.clone(), ui.as_weak());
    ui.on_save_settings(move || {
        let Some(ui) = weak.upgrade() else { return };
        match settings_from_form(&ui) {
            Ok(settings) => s(Command::SaveSettings(settings)),
            Err(e) => ui.set_status_text(e.into()),
        }
    });

    let weak = popup.as_weak();
    popup.on_dismiss(move || {
        if let Some(p) = weak.upgrade() {
            let _ = p.hide();
        }
    });
    let (s, weak) = (send.clone(), popup.as_weak());
    popup.on_cancel_download(move || {
        s(Command::Remove {
            id: CAUGHT.get(),
            delete_file: true,
        });
        if let Some(p) = weak.upgrade() {
            let _ = p.hide();
        }
    });
    let weak = popup.as_weak();
    popup.on_open_folder(move || {
        with_download(CAUGHT.get(), open_folder);
        if let Some(p) = weak.upgrade() {
            let _ = p.hide();
        }
    });

    run(&ui)
}

/// Windows: closing the window hides it to the tray; Quit in the tray menu ends.
#[cfg(windows)]
fn run(ui: &MainWindow) -> Result<(), slint::PlatformError> {
    let _tray = tray::Tray::new(ui.as_weak()).map_err(slint::PlatformError::Other)?;
    ui.window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
    ui.show()?;
    tray::taskbar_icon(ui.window());
    slint::run_event_loop_until_quit()
}

/// Linux: no tray yet, so closing the window ends the app (downloads go on in the daemon).
#[cfg(not(windows))]
fn run(ui: &MainWindow) -> Result<(), slint::PlatformError> {
    ui.run()
}

fn with_download<T>(id: i64, f: impl FnOnce(&DownloadInfo) -> T) -> Option<T> {
    LIST.with_borrow(|list| list.iter().find(|d| d.id == id).map(f))
}

/// A fresh list from the daemon.
pub fn show_list(ui: &MainWindow, list: Vec<DownloadInfo>) {
    let rows: Vec<DownloadRow> = list.iter().map(row).collect();
    if let Some(model) = ui
        .get_rows()
        .as_any()
        .downcast_ref::<VecModel<DownloadRow>>()
    {
        model.set_vec(rows);
    }
    LIST.set(list);
    refresh_details(ui);
}

fn row(d: &DownloadInfo) -> DownloadRow {
    let progress = match (d.status, d.size) {
        (Status::Complete, _) => 1.0,
        (_, Some(size)) if size > 0 => d.downloaded as f32 / size as f32,
        _ => 0.0,
    };
    let active = d.status == Status::Downloading && d.speed > 0;
    let eta = match d.size {
        Some(size) if active => {
            let secs = size.saturating_sub(d.downloaded) / d.speed.max(1);
            format!("{}:{:02}", secs / 60, secs % 60)
        }
        _ => String::new(),
    };
    let status = match d.status {
        Status::Downloading if d.connections > 0 => format!("{} conn", d.connections),
        Status::Downloading => "Starting".into(),
        Status::Queued => "Queued".into(),
        Status::Paused => "Paused".into(),
        Status::Complete => "Done".into(),
        Status::Error => "Failed".into(),
    };
    DownloadRow {
        id: d.id as i32,
        name: d.name.clone().unwrap_or_else(|| d.url.clone()).into(),
        size: d.size.map(human).unwrap_or_default().into(),
        progress,
        speed: if active {
            format!("{}/s", human(d.speed))
        } else {
            String::new()
        }
        .into(),
        eta: eta.into(),
        status: status.into(),
    }
}

/// The line and segment bar under the list, for the selected download.
fn refresh_details(ui: &MainWindow) {
    let id = ui.get_selected_id() as i64;
    let found = with_download(id, |d| {
        let size = d
            .size
            .unwrap_or_else(|| d.segments.iter().map(|s| s[1] + 1).max().unwrap_or(0));
        let bars: Vec<SegmentBar> = d
            .segments
            .iter()
            .filter(|_| size > 0)
            .map(|&[start, end, done]| {
                let len = (end + 1).saturating_sub(start).max(1);
                SegmentBar {
                    x: start as f32 / size as f32,
                    w: len as f32 / size as f32,
                    fill: (done as f32 / len as f32).min(1.0),
                }
            })
            .collect();
        let mut line = format!(
            "{}  ·  {}",
            d.name.as_deref().unwrap_or(&d.url),
            d.dir.display()
        );
        if let Some(error) = &d.error {
            line += &format!("  ·  {error}");
        }
        (line, bars)
    });
    let (line, bars) = found.unwrap_or_default();
    if found_none(ui, id) {
        ui.set_selected_id(-1);
    }
    ui.set_details(line.into());
    if let Some(model) = ui
        .get_segments()
        .as_any()
        .downcast_ref::<VecModel<SegmentBar>>()
    {
        model.set_vec(bars);
    }
}

fn found_none(ui: &MainWindow, id: i64) -> bool {
    id >= 0 && with_download(id, |_| ()).is_none() && ui.get_rows().row_count() > 0
}

pub fn show_settings(ui: &MainWindow, s: AppSettings) {
    ui.set_s_folder(s.download_dir.display().to_string().into());
    let index = CONNECTIONS
        .iter()
        .position(|&c| c == s.max_connections)
        .unwrap_or(3);
    ui.set_s_connections_index(index as i32);
    ui.set_s_limit(rate_text(s.speed_limit).into());
    ui.set_s_catch(s.catch_enabled);
    SETTINGS.set(Some(s));
}

fn settings_from_form(ui: &MainWindow) -> Result<AppSettings, String> {
    let index = ui
        .get_s_connections_index()
        .clamp(0, CONNECTIONS.len() as i32 - 1);
    let folder = ui.get_s_folder().trim().to_owned();
    if folder.is_empty() {
        return Err("Choose a download folder".into());
    }
    Ok(AppSettings {
        download_dir: folder.into(),
        max_connections: CONNECTIONS[index as usize],
        speed_limit: parse_rate(&ui.get_s_limit())?,
        catch_enabled: ui.get_s_catch(),
    })
}

pub fn show_caught(popup: &CaughtPopup, d: DownloadInfo) {
    CAUGHT.set(d.id);
    let name = d
        .name
        .clone()
        .unwrap_or_else(|| d.url.rsplit('/').next().unwrap_or(&d.url).to_owned());
    let size = d
        .size
        .map(human)
        .unwrap_or_else(|| "size not known yet".into());
    popup.set_name(name.into());
    popup.set_info(format!("{size}  →  {}", d.dir.display()).into());
    let _ = popup.show();
    #[cfg(windows)]
    tray::taskbar_icon(popup.window());
}

/// Show the file in the system file manager.
fn open_folder(d: &DownloadInfo) {
    let file = d
        .name
        .as_ref()
        .map(|n| d.dir.join(n))
        .filter(|f| f.exists());
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("explorer");
        match file {
            Some(f) => cmd.arg(format!("/select,{}", f.display())),
            None => cmd.arg(&d.dir),
        };
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        let _ = std::process::Command::new("xdg-open").arg(&d.dir).spawn();
    }
}

/// 1536 → "1.5 KiB".
fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// "" / "none" / "0" → 0, "500K" → 512000, "2M" → 2097152.
fn parse_rate(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") || s == "0" {
        return Ok(0);
    }
    let (num, mult) = match s.to_ascii_uppercase().chars().last() {
        Some('K') => (&s[..s.len() - 1], 1024.0),
        Some('M') => (&s[..s.len() - 1], 1024.0 * 1024.0),
        _ => (s, 1.0),
    };
    let n: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("Speed limit not understood: {s}"))?;
    Ok((n * mult) as u64)
}

fn rate_text(rate: u64) -> String {
    match rate {
        0 => String::new(),
        r if r % (1024 * 1024) == 0 => format!("{}M", r / (1024 * 1024)),
        r if r % 1024 == 0 => format!("{}K", r / 1024),
        r => r.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_round_trip() {
        for text in ["", "500K", "2M", "1000"] {
            assert_eq!(rate_text(parse_rate(text).unwrap()), text);
        }
        assert_eq!(parse_rate("none"), Ok(0));
        assert!(parse_rate("fast").is_err());
    }
}
