//! Throwaway prototype: the classic-table main window in Slint, with fake downloads.

use std::rc::Rc;
use std::time::Duration;

use slint::{Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

slint::slint! {
    import { Button, LineEdit, ListView, Palette, ProgressIndicator } from "std-widgets.slint";

    export struct DownloadRow {
        name: string,
        size: string,
        progress: float,
        speed: string,
        eta: string,
        status: string,
    }

    component Cell inherits Text {
        vertical-alignment: center;
        overflow: elide;
    }

    export component MainWindow inherits Window {
        title: "Slipstream (Slint prototype)";
        preferred-width: 920px;
        preferred-height: 480px;
        in property <[DownloadRow]> rows;
        in property <[float]> segments;
        in property <string> details;
        in-out property <int> selected: 0;
        callback pause();
        callback resume();

        VerticalLayout {
            padding: 6px;
            spacing: 6px;

            HorizontalLayout {
                spacing: 6px;
                Button { text: "+ Add URL"; }
                Button { text: "Pause"; clicked => { root.pause(); } }
                Button { text: "Resume"; clicked => { root.resume(); } }
                Button { text: "Delete"; }
                Rectangle { horizontal-stretch: 1; }
                LineEdit { placeholder-text: "Search"; width: 180px; }
            }

            Rectangle {
                height: 26px;
                background: Palette.alternate-background;
                HorizontalLayout {
                    padding-left: 8px;
                    padding-right: 8px;
                    spacing: 8px;
                    Cell { text: "Name"; font-weight: 600; horizontal-stretch: 1; }
                    Cell { text: "Size"; font-weight: 600; width: 80px; }
                    Cell { text: "Done"; font-weight: 600; width: 150px; }
                    Cell { text: "Speed"; font-weight: 600; width: 80px; }
                    Cell { text: "ETA"; font-weight: 600; width: 60px; }
                    Cell { text: "Status"; font-weight: 600; width: 90px; }
                }
            }

            ListView {
                vertical-stretch: 1;
                for row[i] in root.rows: Rectangle {
                    height: 28px;
                    background: i == root.selected ? Palette.selection-background
                        : Math.mod(i, 2) == 1 ? Palette.alternate-background : transparent;
                    HorizontalLayout {
                        padding-left: 8px;
                        padding-right: 8px;
                        spacing: 8px;
                        Cell { text: row.name; horizontal-stretch: 1;
                               color: i == root.selected ? Palette.selection-foreground : Palette.foreground; }
                        Cell { text: row.size; width: 80px; }
                        HorizontalLayout {
                            width: 150px;
                            spacing: 6px;
                            alignment: center;
                            ProgressIndicator { progress: row.progress; height: 8px; horizontal-stretch: 1; }
                            Cell { text: Math.round(row.progress * 100) + "%"; width: 36px; }
                        }
                        Cell { text: row.speed; width: 80px; }
                        Cell { text: row.eta; width: 60px; }
                        Cell { text: row.status; width: 90px; }
                    }
                    TouchArea { clicked => { root.selected = i; } }
                }
            }

            Cell { text: root.details; height: 20px; }

            Rectangle {
                height: 18px;
                border-radius: 3px;
                background: Palette.alternate-background;
                for s[i] in root.segments: Rectangle {
                    x: parent.width * i / root.segments.length + 1px;
                    y: 2px;
                    width: (parent.width / root.segments.length - 2px) * s;
                    height: parent.height - 4px;
                    border-radius: 2px;
                    background: Palette.accent-background;
                }
            }
        }
    }
}

struct Fake {
    name: &'static str,
    size_mb: f64,
    status: &'static str,
    segments: Vec<f32>,
    rates: Vec<f32>,
}

impl Fake {
    fn new(name: &'static str, size_mb: f64, status: &'static str, done: f32, conns: usize) -> Self {
        let rates = (0..conns).map(|i| 0.6 + 0.15 * i as f32).collect();
        let segments = (0..conns).map(|i| (done + 0.07 * i as f32).min(1.0)).collect();
        Self { name, size_mb, status, segments, rates }
    }
    fn done(&self) -> f32 {
        self.segments.iter().sum::<f32>() / self.segments.len() as f32
    }
    fn speed(&self) -> f32 {
        if self.status != "downloading" {
            return 0.0;
        }
        self.segments.iter().zip(&self.rates).filter(|(s, _)| **s < 1.0).map(|(_, r)| r).sum()
    }
    fn row(&self) -> DownloadRow {
        let (done, speed) = (self.done(), self.speed());
        let eta = if speed > 0.0 {
            let secs = (1.0 - done) as f64 * self.size_mb / speed as f64;
            format!("{}:{:02}", secs as u64 / 60, secs as u64 % 60)
        } else {
            String::new()
        };
        DownloadRow {
            name: self.name.into(),
            size: size_text(self.size_mb).into(),
            progress: done,
            speed: if speed > 0.0 { format!("{speed:.1} MB/s").into() } else { SharedString::new() },
            eta: eta.into(),
            status: match self.status {
                "downloading" => format!("{} conn", self.segments.len()).into(),
                s => {
                    let mut c = s.chars();
                    c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default().into()
                }
            },
        }
    }
}

fn size_text(mb: f64) -> String {
    match mb {
        0.0 => "?".into(),
        mb if mb >= 1024.0 => format!("{:.1} GB", mb / 1024.0),
        mb => format!("{mb:.1} MB"),
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let fakes = Rc::new(std::cell::RefCell::new(vec![
        Fake::new("ubuntu-24.04.2-desktop-amd64.iso", 5939.0, "downloading", 0.35, 8),
        Fake::new("linux-6.12.tar.xz", 145.0, "downloading", 0.10, 4),
        Fake::new("quarterly-report.pdf", 2.1, "done", 1.0, 1),
        Fake::new("holiday-video.mp4", 840.0, "paused", 0.12, 6),
        Fake::new("Fedora-Workstation-41.iso", 2290.0, "paused", 0.58, 8),
        Fake::new("broken-link.zip", 0.0, "failed", 0.0, 1),
    ]));
    let rows = Rc::new(VecModel::from(fakes.borrow().iter().map(Fake::row).collect::<Vec<_>>()));
    let segments = Rc::new(VecModel::<f32>::default());
    ui.set_rows(ModelRc::from(rows.clone()));
    ui.set_segments(ModelRc::from(segments.clone()));

    let set_status = |status: &'static str, from: &'static str| {
        let (weak, fakes) = (ui.as_weak(), fakes.clone());
        move || {
            let ui = weak.unwrap();
            let mut f = fakes.borrow_mut();
            let row = &mut f[ui.get_selected() as usize];
            if row.status == from {
                row.status = status;
            }
        }
    };
    ui.on_pause(set_status("paused", "downloading"));
    ui.on_resume(set_status("downloading", "paused"));

    // Fake progress, 10 times a second.
    let timer = Timer::default();
    let weak = ui.as_weak();
    let dt = 0.1f32;
    timer.start(TimerMode::Repeated, Duration::from_millis(100), move || {
        let ui = weak.unwrap();
        let mut f = fakes.borrow_mut();
        for (i, fake) in f.iter_mut().enumerate() {
            if fake.status == "downloading" {
                let (size, n) = (fake.size_mb as f32, fake.segments.len() as f32);
                for (s, r) in fake.segments.iter_mut().zip(&fake.rates) {
                    *s = (*s + r * dt * n / size).min(1.0);
                }
                if fake.segments.iter().all(|s| *s >= 1.0) {
                    fake.status = "done";
                }
            }
            if rows.row_data(i) != Some(fake.row()) {
                rows.set_row_data(i, fake.row());
            }
        }
        let sel = &f[ui.get_selected() as usize];
        segments.set_vec(sel.segments.clone());
        ui.set_details(
            format!("{}  ·  {} connections  ·  {:.1} MB/s", sel.name, sel.segments.len(), sel.speed()).into(),
        );
    });
    ui.run()
}
