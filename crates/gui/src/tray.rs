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

/// The Windows tray is always there once created.
pub fn shown() -> bool {
    true
}

fn show(ui: &Weak<MainWindow>) {
    if let Some(ui) = ui.upgrade() {
        let _ = ui.show();
    }
}

/// The app icon that build.rs put into the .exe (resource 1, all sizes).
fn icon() -> Result<Icon, String> {
    Icon::from_resource(1, None).map_err(|e| e.to_string())
}

/// Give a window the large icon from the .exe as well. Slint only sets the small
/// title-bar one; the taskbar and Alt+Tab use the large one.
pub fn taskbar_icon(window: &slint::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, IMAGE_ICON, LR_SHARED, LoadImageW, SM_CXICON, SM_CYICON,
        SendMessageW, WM_SETICON,
    };

    let slint_handle = window.window_handle();
    let Ok(handle) = slint_handle.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    // SAFETY: plain Win32 calls on our own window; resource 1 is the icon build.rs embeds.
    unsafe {
        let icon = LoadImageW(
            GetModuleHandleW(std::ptr::null()),
            1 as _,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXICON),
            GetSystemMetrics(SM_CYICON),
            LR_SHARED,
        );
        if !icon.is_null() {
            SendMessageW(
                h.hwnd.get() as _,
                WM_SETICON,
                ICON_BIG as usize,
                icon as isize,
            );
        }
    }
}
