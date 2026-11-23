//! Tray icon and native window visibility.
//!
//! eframe's Windows viewport cannot reliably be shown again after
//! `ViewportCommand::Visible(false)`, so the app keeps the viewport alive and
//! hides the native HWND with `ShowWindow`. The event loop remains alive, the
//! service listeners keep their ports, and the background pollers keep their
//! threads. The tray restores the same HWND with `ShowWindow(SW_SHOW)`.

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::Arc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW,
};

pub struct Tray {
    _icon: TrayIcon,
    _menu: Menu,
    control: Arc<WindowControl>,
}

struct WindowControl {
    hwnd: isize,
    ctx: egui::Context,
}

impl WindowControl {
    fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOW);
            SetForegroundWindow(self.hwnd);
        }
        // The window was hidden, so no redraw is guaranteed to be queued.
        self.ctx.request_repaint();
    }

    fn hide(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }

}

impl Tray {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let handle = cc.window_handle().map_err(|err| err.to_string())?;
        let raw = handle.as_raw();
        let hwnd = match raw {
            RawWindowHandle::Win32(handle) => handle.hwnd.get(),
            _ => return Err("eframe did not expose a Win32 window handle".to_string()),
        };
        let control = Arc::new(WindowControl {
            hwnd,
            ctx: cc.egui_ctx.clone(),
        });

        let menu = Menu::new();
        let open = MenuItem::new("Open GMO TempTools", true, None);
        let quit = MenuItem::new("Quit GMO TempTools", true, None);
        let separator = PredefinedMenuItem::separator();
        menu.append_items(&[&open, &separator, &quit])
            .map_err(|err| format!("cannot build tray menu: {err}"))?;

        let open_id = open.id().clone();
        let quit_id = quit.id().clone();
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(false)
            .with_tooltip("GMO TempTools — running in background")
            .with_icon(icon()?)
            .build()
            .map_err(|err| format!("cannot create tray icon: {err}"))?;

        let menu_control = control.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id() == &open_id {
                menu_control.show();
            } else if event.id() == &quit_id {
                // A hidden viewport does not service a close command. All state
                // writes are atomic and there is no unsaved user data, so exit
                // directly instead of showing the window just to close it.
                std::process::exit(0);
            }
        }));

        let click_control = control.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                click_control.show();
            }
        }));

        Ok(Self {
            _icon: tray,
            _menu: menu,
            control,
        })
    }

    pub fn hide(&self) {
        self.control.hide();
    }
}

fn icon() -> Result<Icon, String> {
    const SIZE: u32 = 32;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let distance = (dx * dx + dy * dy).sqrt();
            let color = if distance <= 14.5 {
                [64, 214, 155, 255]
            } else {
                [0, 0, 0, 0]
            };
            let index = ((y * SIZE + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&color);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|err| format!("cannot build tray icon: {err}"))
}
