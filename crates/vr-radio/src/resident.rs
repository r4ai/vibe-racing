//! Staying resident: tray icon, start with Windows, single instance.

/// From the tray icon or a second launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentEvent {
    Show,
    Quit,
}

/// Command line Windows runs at sign-in. The working directory would be
/// `System32` there, so the agent's is passed explicitly.
pub fn startup_command(exe: &str, cwd: &str) -> String {
    // A trailing backslash would escape the closing quote.
    format!("\"{exe}\" --tray --cwd \"{}\"", cwd.trim_end_matches('\\'))
}

/// RGBA of the tray icon: a green dot in a white ring.
#[cfg_attr(not(windows), allow(dead_code))]
fn icon_rgba(size: u32) -> Vec<u8> {
    let c = (size as f32 - 1.0) / 2.0;
    let r = size as f32 / 2.0;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = (x as f32 - c).hypot(y as f32 - c);
            out.extend_from_slice(&if d < r * 0.55 {
                [0x22, 0xc5, 0x5e, 255]
            } else if d < r * 0.9 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 0]
            });
        }
    }
    out
}

#[cfg(not(windows))]
pub use stub::*;
#[cfg(windows)]
pub use win32::*;

#[cfg(not(windows))]
mod stub {
    use super::ResidentEvent;

    pub const SUPPORTED: bool = false;
    pub fn install_tray() -> bool {
        false
    }
    pub fn poll() -> Option<ResidentEvent> {
        None
    }
    pub fn starts_with_windows() -> bool {
        false
    }
    pub fn set_start_with_windows(_: bool, _: &std::path::Path) -> Result<(), String> {
        Err("Windows only".into())
    }
    pub fn first_instance() -> bool {
        true
    }
    pub fn leave_own_console() {}
}

#[cfg(windows)]
mod win32 {
    use std::cell::RefCell;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, Ordering};

    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };
    use windows_sys::Win32::Foundation::{
        ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, INFINITE, SetEvent, WaitForSingleObject,
    };

    use super::{ResidentEvent, icon_rgba, startup_command};

    pub const SUPPORTED: bool = true;

    /// Closes the console when this process is alone in it (started from the
    /// Start menu or Explorer, not from a terminal).
    pub fn leave_own_console() {
        use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
        let mut pids = [0u32; 2];
        // SAFETY: the buffer holds the given number of process ids.
        let attached = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
        if attached == 1 {
            // SAFETY: plain Win32 call; writes to the console then go nowhere.
            unsafe { FreeConsole() };
        }
    }
    const SHOW: &str = "show";
    const QUIT: &str = "quit";
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_VALUE: &str = "VR Radio";
    static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);

    thread_local! {
        static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// Must be called on the event-loop thread.
    pub fn install_tray() -> bool {
        match build() {
            Ok(tray) => {
                TRAY.set(Some(tray));
                true
            }
            Err(e) => {
                tracing::warn!("tray icon unavailable: {e}");
                false
            }
        }
    }

    fn build() -> Result<TrayIcon, Box<dyn std::error::Error>> {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id(SHOW, "開く", true, None))?;
        menu.append(&MenuItem::with_id(QUIT, "終了", true, None))?;
        Ok(TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("VR Radio")
            .with_icon(Icon::from_rgba(icon_rgba(32), 32, 32)?)
            .build()?)
    }

    pub fn poll() -> Option<ResidentEvent> {
        if SHOW_REQUESTED.swap(false, Ordering::Relaxed) {
            return Some(ResidentEvent::Show);
        }
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            match e.id.0.as_str() {
                SHOW => return Some(ResidentEvent::Show),
                QUIT => return Some(ResidentEvent::Quit),
                _ => {}
            }
        }
        while let Ok(e) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                return Some(ResidentEvent::Show);
            }
        }
        None
    }

    pub fn starts_with_windows() -> bool {
        let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
        // SAFETY: only asks whether the value exists.
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                null_mut(),
            ) == 0
        }
    }

    pub fn set_start_with_windows(on: bool, cwd: &Path) -> Result<(), String> {
        let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
        let status = if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let data = wide(&startup_command(
                &exe.to_string_lossy(),
                &cwd.to_string_lossy(),
            ));
            // SAFETY: NUL-terminated strings and the byte length of `data`.
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    value.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            }
        } else {
            // SAFETY: NUL-terminated strings.
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) }
        };
        match status {
            0 => Ok(()),
            ERROR_FILE_NOT_FOUND if !on => Ok(()),
            e => Err(std::io::Error::from_raw_os_error(e as i32).to_string()),
        }
    }

    /// False when the radio already runs: its window is asked to show up.
    pub fn first_instance() -> bool {
        let name = wide(r"Local\vr-radio-show");
        // SAFETY: an auto-reset event kept open for the life of the process.
        let (event, exists) = unsafe {
            let event = CreateEventW(null(), 0, 0, name.as_ptr());
            (event, GetLastError() == ERROR_ALREADY_EXISTS)
        };
        if event.is_null() {
            return true;
        }
        if exists {
            // SAFETY: a valid event handle.
            unsafe { SetEvent(event) };
            return false;
        }
        let event = event as usize;
        std::thread::spawn(move || {
            // SAFETY: the handle is never closed.
            while unsafe { WaitForSingleObject(event as _, INFINITE) } == 0 {
                SHOW_REQUESTED.store(true, Ordering::Relaxed);
            }
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_command_quotes_paths() {
        assert_eq!(
            startup_command(r"C:\My Apps\vr-radio.exe", r"C:\race data"),
            r#""C:\My Apps\vr-radio.exe" --tray --cwd "C:\race data""#
        );
        assert_eq!(startup_command("a", r"C:\x\"), r#""a" --tray --cwd "C:\x""#);
    }

    #[test]
    fn icon_is_a_ringed_dot() {
        let px = icon_rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        let at = |x: usize, y: usize| <[u8; 4]>::try_from(&px[(y * 32 + x) * 4..][..4]).unwrap();
        assert_eq!(at(16, 16), [0x22, 0xc5, 0x5e, 255]);
        assert_eq!(at(16, 4), [255, 255, 255, 255]);
        assert_eq!(at(4, 16), [255, 255, 255, 255]);
        assert_eq!(at(0, 0), [0, 0, 0, 0]);
    }
}
