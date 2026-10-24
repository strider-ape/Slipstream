//! Throwaway prototype: the classic-table main window in egui, with fake downloads.

use std::time::Duration;

use eframe::egui;
use egui_extras::{Column, TableBuilder};

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Downloading,
    Paused,
    Done,
    Failed,
}

struct Row {
    name: &'static str,
    size_mb: f64,
    status: Status,
    /// Per-connection progress (0..1) of each segment.
    segments: Vec<f32>,
    /// MB/s for each active segment.
    rates: Vec<f32>,
}

impl Row {
    fn new(name: &'static str, size_mb: f64, status: Status, done: f32, conns: usize) -> Self {
        let rates = (0..conns).map(|i| 0.6 + 0.15 * i as f32).collect();
        let segments = (0..conns).map(|i| (done + 0.07 * i as f32).min(1.0)).collect();
        Self { name, size_mb, status, segments, rates }
    }
    fn done(&self) -> f32 {
        self.segments.iter().sum::<f32>() / self.segments.len() as f32
    }
    fn speed(&self) -> f32 {
        if self.status != Status::Downloading {
            return 0.0;
        }
        self.segments.iter().zip(&self.rates).filter(|(s, _)| **s < 1.0).map(|(_, r)| r).sum()
    }
}

struct App {
    rows: Vec<Row>,
    selected: usize,
    search: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::ThemePreference::System);
        Self {
            rows: vec![
                Row::new("ubuntu-24.04.2-desktop-amd64.iso", 5939.0, Status::Downloading, 0.35, 8),
                Row::new("linux-6.12.tar.xz", 145.0, Status::Downloading, 0.10, 4),
                Row::new("quarterly-report.pdf", 2.1, Status::Done, 1.0, 1),
                Row::new("holiday-video.mp4", 840.0, Status::Paused, 0.12, 6),
                Row::new("Fedora-Workstation-41.iso", 2290.0, Status::Paused, 0.58, 8),
                Row::new("broken-link.zip", 0.0, Status::Failed, 0.0, 1),
            ],
            selected: 0,
            search: String::new(),
        }
    }

    /// Fake progress so the window has something live to draw.
    fn tick(&mut self, dt: f32) {
        for row in &mut self.rows {
            if row.status != Status::Downloading {
                continue;
            }
            let size = row.size_mb as f32;
            let n = row.segments.len() as f32;
            for (s, r) in row.segments.iter_mut().zip(&row.rates) {
                *s = (*s + r * dt * n / size).min(1.0);
            }
            if row.segments.iter().all(|s| *s >= 1.0) {
                row.status = Status::Done;
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.tick(ctx.input(|i| i.stable_dt).min(0.1));

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let _ = ui.button("➕ Add URL");
                if ui.button("⏸ Pause").clicked() {
                    self.rows[self.selected].status = Status::Paused;
                }
                if ui.button("▶ Resume").clicked() && self.rows[self.selected].status == Status::Paused {
                    self.rows[self.selected].status = Status::Downloading;
                }
                let _ = ui.button("🗑 Delete");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search").desired_width(180.0));
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("details").show(ui, |ui| {
            let row = &self.rows[self.selected];
            ui.add_space(4.0);
            ui.label(format!(
                "{}  ·  {} connections  ·  {:.1} MB/s",
                row.name,
                row.segments.len(),
                row.speed()
            ));
            segment_bar(ui, &row.segments);
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let search = self.search.to_lowercase();
            let mut clicked = None;
            TableBuilder::new(ui)
                .striped(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::remainder().at_least(200.0).clip(true))
                .column(Column::exact(80.0))
                .column(Column::exact(150.0))
                .column(Column::exact(80.0))
                .column(Column::exact(60.0))
                .column(Column::exact(100.0))
                .header(24.0, |mut header| {
                    for title in ["Name", "Size", "Done", "Speed", "ETA", "Status"] {
                        header.col(|ui| {
                            ui.strong(title);
                        });
                    }
                })
                .body(|mut body| {
                    for (i, r) in self.rows.iter().enumerate() {
                        if !r.name.to_lowercase().contains(&search) {
                            continue;
                        }
                        body.row(26.0, |mut row| {
                            row.set_selected(i == self.selected);
                            let done = r.done();
                            let speed = r.speed();
                            row.col(|ui| {
                                ui.label(r.name);
                            });
                            row.col(|ui| {
                                ui.label(size_text(r.size_mb));
                            });
                            row.col(|ui| {
                                ui.add(egui::ProgressBar::new(done).text(format!("{:.0}%", done * 100.0)));
                            });
                            row.col(|ui| {
                                if speed > 0.0 {
                                    ui.label(format!("{speed:.1} MB/s"));
                                }
                            });
                            row.col(|ui| {
                                if speed > 0.0 {
                                    let secs = (1.0 - done) as f64 * r.size_mb / speed as f64;
                                    ui.label(format!("{}:{:02}", secs as u64 / 60, secs as u64 % 60));
                                }
                            });
                            row.col(|ui| {
                                ui.label(match r.status {
                                    Status::Downloading => format!("{} conn", r.segments.len()),
                                    Status::Paused => "Paused".into(),
                                    Status::Done => "Done".into(),
                                    Status::Failed => "Failed".into(),
                                });
                            });
                            if row.response().clicked() {
                                clicked = Some(i);
                            }
                        });
                    }
                });
            if let Some(i) = clicked {
                self.selected = i;
            }
        });

        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// One cell per connection, filled as far as that connection got.
fn segment_bar(ui: &mut egui::Ui, segments: &[f32]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
    let visuals = ui.visuals();
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);
    let w = rect.width() / segments.len() as f32;
    for (i, s) in segments.iter().enumerate() {
        let min = egui::pos2(rect.left() + i as f32 * w + 1.0, rect.top() + 2.0);
        let fill = egui::Rect::from_min_size(min, egui::vec2((w - 2.0) * s, rect.height() - 4.0));
        painter.rect_filled(fill, 2.0, visuals.selection.bg_fill);
    }
}

fn size_text(mb: f64) -> String {
    match mb {
        0.0 => "?".into(),
        mb if mb >= 1024.0 => format!("{:.1} GB", mb / 1024.0),
        mb => format!("{mb:.1} MB"),
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Slipstream (egui prototype)")
            .with_inner_size([920.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native("slipstream-egui", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
