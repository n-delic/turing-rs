// System tray: theme picker, brightness, open config, reload, quit.
// trayicon handles both Windows (Shell_NotifyIcon) and Linux (StatusNotifierItem over DBus).
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use trayicon::{MenuBuilder, TrayIconBuilder};

#[derive(Clone, PartialEq, Debug)]
pub enum Cmd {
    Brightness(u8),
    Theme(Option<PathBuf>),
    Settings,
    OpenConfig,
    Reload,
    Quit,
}

const ICO: &[u8] = include_bytes!("../assets/icon.ico");

/// Runs the tray on its own thread; menu events arrive on `tx`.
/// ponytail: check marks reflect state at startup only; rebuild via set_menu if that ever matters.
pub fn start(themes: Vec<(String, PathBuf)>, current: Option<PathBuf>, brightness: u8, tx: Sender<Cmd>) {
    std::thread::spawn(move || {
        let mut tm = MenuBuilder::new().checkable("Built-in dashboard", current.is_none(), Cmd::Theme(None));
        for (name, p) in themes {
            tm = tm.checkable(&name, current.as_ref() == Some(&p), Cmd::Theme(Some(p)));
        }
        let mut bm = MenuBuilder::new();
        for b in [10u8, 25, 50, 75, 100] {
            bm = bm.checkable(&format!("{b}%"), b == brightness, Cmd::Brightness(b));
        }
        let menu = MenuBuilder::new()
            .submenu("Theme", tm)
            .submenu("Brightness", bm)
            .separator()
            .item("Settings…", Cmd::Settings)
            .item("Open config folder", Cmd::OpenConfig)
            .item("Reload", Cmd::Reload)
            .separator()
            .item("Quit", Cmd::Quit);
        let tray = TrayIconBuilder::new()
            .on_click(Cmd::Settings)
            .on_double_click(Cmd::Settings)
            .sender(move |c: &Cmd| {
                let _ = tx.send(c.clone());
            })
            .icon_from_buffer(ICO)
            .tooltip("turing-rs")
            .menu(menu)
            .build();
        let _tray = match tray {
            Ok(t) => t,
            Err(e) => {
                eprintln!("tray unavailable: {e:?}");
                return;
            }
        };
        // Windows delivers tray events through this thread's message queue; Linux runs its own DBus task.
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        #[cfg(not(windows))]
        loop {
            std::thread::park();
        }
    });
}
