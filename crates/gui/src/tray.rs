//! Windows tray icon: Slipstream stays near the clock when its window is closed.
//! Click the icon to open the window; right-click for Open / Quit.

use std::time::Duration;

use slint::{ComponentHandle, Timer, TimerMode, Weak};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::MainWindow;

/// Keep this alive for as long as the icon should show.
pub struct Tray {
    _icon: TrayIcon,
    _timer: Timer,
}

impl Tray {
    pub fn new(ui: Weak<MainWindow>) -> Result<Self, String> {
        let err = |e: &dyn std::fmt::Display| e.to_string();
        let open = MenuItem::new("Open Slipstream", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append(&open).map_err(|e| err(&e))?;
        menu.append(&quit).map_err(|e| err(&e))?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("Slipstream")
            .with_icon(icon()?)
            .with_menu(Box::new(menu))
            .build()
            .map_err(|e| err(&e))?;

        // The tray reports clicks through channels; check them on the UI thread.
        let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, Duration::from_millis(100), move || {
            while let Ok(e) = MenuEvent::receiver().try_recv() {
                if e.id == quit_id {
                    let _ = slint::quit_event_loop();
                } else if e.id == open_id {
                    show(&ui);
                }
            }
            while let Ok(e) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = e
                {
                    show(&ui);
                }
            }
        });
        Ok(Self {
            _icon: icon,
            _timer: timer,
        })
    }
}

fn show(ui: &Weak<MainWindow>) {
    if let Some(ui) = ui.upgrade() {
        let _ = ui.show();
    }
}

/// The app icon at 32×32, drawn in code: blue circle with a white download arrow.
fn icon() -> Result<Icon, String> {
    const N: u32 = 32;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5 - 16.0, y as f32 + 0.5 - 16.0);
            let inside = fx * fx + fy * fy <= 15.0 * 15.0;
            let shaft = fx.abs() <= 1.8 && (-9.0..=2.0).contains(&fy);
            let head = (0.0..=7.0).contains(&fy) && fx.abs() <= 7.0 - fy;
            let base = (9.0..=11.0).contains(&fy) && fx.abs() <= 8.0;
            let px = match (inside, shaft || head || base) {
                (true, true) => [255, 255, 255, 255],
                (true, false) => [37, 99, 235, 255],
                _ => [0, 0, 0, 0],
            };
            rgba.extend_from_slice(&px);
        }
    }
    Icon::from_rgba(rgba, N, N).map_err(|e| e.to_string())
}
