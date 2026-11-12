//! Background-mode plumbing: a system tray icon with a menu, so SnipXTool can
//! keep running (and keep its Print Screen hotkey alive) without a window.

use eframe::egui;
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    Snip,
    Settings,
    Quit,
}

pub struct Tray {
    rx: Receiver<TrayAction>,
    /// HWND of the main window, set by `App` once known. Hidden windows do not
    /// receive repaint requests, so callbacks show the window via Win32 first.
    hwnd: std::sync::Arc<std::sync::atomic::AtomicIsize>,
    #[cfg(windows)]
    _icon: tray_icon::TrayIcon,
}

impl Tray {
    /// Builds the tray icon. Returns `None` if the platform has no tray support
    /// (or Windows refused to create the icon).
    pub fn new(ctx: egui::Context) -> Option<Self> {
        let hwnd = std::sync::Arc::new(std::sync::atomic::AtomicIsize::new(0));
        #[cfg(windows)]
        {
            win::build(ctx, hwnd.clone())
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
            None
        }
    }

    /// Drains pending tray actions. Call this once per frame.
    pub fn actions(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();
        while let Ok(action) = self.rx.try_recv() {
            actions.push(action);
        }
        actions
    }

    /// Lets the tray callbacks show the main window themselves.
    pub fn set_hwnd(&self, hwnd: isize) {
        self.hwnd.store(hwnd, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use tray_icon::menu::{Menu, MenuEvent as TrayMenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    };

    fn wake(hwnd: &std::sync::Arc<std::sync::atomic::AtomicIsize>) {
        let h = hwnd.load(std::sync::atomic::Ordering::Relaxed);
        if h != 0 {
            #[link(name = "user32")]
            extern "system" {
                fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
                fn SetForegroundWindow(hwnd: isize) -> i32;
            }
            unsafe {
                // Hidden windows never get repaint requests, so wake the UI loop.
                ShowWindow(h, 5); // SW_SHOW
                SetForegroundWindow(h);
            }
        }
    }

    pub fn build(ctx: egui::Context, hwnd: std::sync::Arc<std::sync::atomic::AtomicIsize>) -> Option<Tray> {
        let (tx, rx) = channel();

        let open = MenuItem::new("Open SnipXTool", true, None);
        let snip = MenuItem::new("Take a snip\tPrint Screen", true, None);
        let settings = MenuItem::new("Settings...", true, None);
        let quit = MenuItem::new("Quit SnipXTool", true, None);

        let menu = Menu::new();
        let separator = PredefinedMenuItem::separator();
        if menu.append_items(&[&open, &snip, &separator, &settings, &quit]).is_err() {
            return None;
        }

        let (rgba, width, height) = crate::logo_rgba(32);
        let icon = match Icon::from_rgba(rgba, width, height) {
            Ok(icon) => icon,
            Err(_) => return None,
        };
        let tray = match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("SnipXTool - press Print Screen to snip")
            .with_icon(icon)
            .build()
        {
            Ok(tray) => tray,
            Err(_) => return None,
        };

        let open_id = open.id().clone();
        let snip_id = snip.id().clone();
        let settings_id = settings.id().clone();
        let quit_id = quit.id().clone();

        let menu_tx = tx.clone();
        let menu_ctx = ctx.clone();
        let menu_hwnd = hwnd.clone();
        TrayMenuEvent::set_event_handler(Some(move |event: TrayMenuEvent| {
            let action = if event.id == open_id {
                Some(TrayAction::Open)
            } else if event.id == snip_id {
                Some(TrayAction::Snip)
            } else if event.id == settings_id {
                Some(TrayAction::Settings)
            } else if event.id == quit_id {
                Some(TrayAction::Quit)
            } else {
                None
            };
            if let Some(action) = action {
                if action == TrayAction::Quit {
                    // Hidden windows cannot process a "quit" message, so exit here.
                    std::process::exit(0);
                }
                wake(&menu_hwnd);
                let _ = menu_tx.send(action);
                menu_ctx.request_repaint();
            }
        }));

        let click_ctx = ctx;
        let click_hwnd = hwnd.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let activate = matches!(
                event,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) || matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            );
            if activate {
                wake(&click_hwnd);
                let _ = tx.send(TrayAction::Open);
                click_ctx.request_repaint();
            }
        }));

        Some(Tray { rx, hwnd, _icon: tray })
    }
}
