//! Linux tray icon, using the StatusNotifierItem standard over D-Bus: KDE, XFCE,
//! Cinnamon, and GNOME with the AppIndicator extension (on by default in Ubuntu).
//! Click the icon to open the window; the menu has Open / Quit.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use ksni::menu::StandardItem;
use ksni::{MenuItem, OfflineReason, TrayMethods};
use slint::Weak;

use crate::MainWindow;

/// Whether a panel shows our icon right now. It can go away, for example when the
/// desktop shell restarts; then closing the window must end the app instead.
static SHOWN: AtomicBool = AtomicBool::new(false);

pub fn shown() -> bool {
    SHOWN.load(Ordering::Relaxed)
}

/// The icon lives as long as the app: it runs on its own thread.
pub struct Tray;

impl Tray {
    /// Fails when the desktop has no tray (no D-Bus, or no panel that shows these icons).
    pub fn new(ui: Weak<MainWindow>) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => return drop(tx.send(Err(e.to_string()))),
                };
                rt.block_on(async move {
                    match (Item { ui }).spawn().await {
                        Ok(_handle) => {
                            SHOWN.store(true, Ordering::Relaxed);
                            let _ = tx.send(Ok(()));
                            std::future::pending::<()>().await;
                        }
                        Err(e) => drop(tx.send(Err(e.to_string()))),
                    }
                });
            })
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())??;
        Ok(Self)
    }
}

struct Item {
    ui: Weak<MainWindow>,
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        "slipstream".into()
    }

    fn title(&self) -> String {
        "Slipstream".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [
            &include_bytes!("../../../assets/icons/slipstream-16.png")[..],
            include_bytes!("../../../assets/icons/slipstream-32.png"),
            include_bytes!("../../../assets/icons/slipstream-48.png"),
        ]
        .into_iter()
        .filter_map(argb)
        .collect()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        show(&self.ui);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open Slipstream".into(),
                activate: Box::new(|item: &mut Self| show(&item.ui)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|_| drop(slint::quit_event_loop())),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        SHOWN.store(true, Ordering::Relaxed);
    }

    fn watcher_offline(&self, _reason: OfflineReason) -> bool {
        // The icon is gone: bring the window back so the app isn't stranded, and wait
        // for the panel to return.
        SHOWN.store(false, Ordering::Relaxed);
        show(&self.ui);
        true
    }
}

/// The tray runs on its own thread; the window belongs to the UI thread.
fn show(ui: &Weak<MainWindow>) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        let _ = slint::ComponentHandle::show(&ui);
    });
}

/// A PNG as the tray wants it: ARGB, 8 bits each.
fn argb(png: &[u8]) -> Option<ksni::Icon> {
    let mut reader = png::Decoder::new(Cursor::new(png)).read_info().ok()?;
    if reader.output_color_type() != (png::ColorType::Rgba, png::BitDepth::Eight) {
        return None;
    }
    let mut data = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut data).ok()?;
    data.truncate(info.buffer_size());
    for px in data.as_chunks_mut::<4>().0 {
        px.rotate_right(1);
    }
    Some(ksni::Icon {
        width: info.width as i32,
        height: info.height as i32,
        data,
    })
}
