//! Window chrome: the app draws the title bar, the OS keeps everything else.
//!
//! On Windows the native frame stays (rounded corners, border, shadow,
//! resize edges, Aero Snap): a window subclass only removes the caption in
//! `WM_NCCALCSIZE` and answers `WM_NCHITTEST` so that the drawn title bar
//! behaves natively: `HTCAPTION` to drag, double-click and open the system
//! menu, `HTMAXBUTTON` on the maximize button for the Snap Layouts flyout.
//! Other platforms keep their own title bar.

use std::sync::{LazyLock, OnceLock};

use crate::inbox::Inbox;

/// Height of the drawn title bar (logical px).
pub const TITLE_BAR: f32 = 40.0;
/// Width of a caption button, as on Windows 11 (logical px).
pub const CAPTION_BUTTON: f32 = 46.0;
/// Room left of the caption buttons for the settings button (logical px).
pub const TOOLS: f32 = 44.0;

/// Whether this platform uses the drawn title bar and caption buttons.
pub const CUSTOM_TITLE_BAR: bool = cfg!(windows);

/// Glyphs of the Windows caption buttons (same code points in both fonts).
pub const GLYPH_MINIMIZE: char = '\u{E921}';
pub const GLYPH_MAXIMIZE: char = '\u{E922}';
pub const GLYPH_RESTORE: char = '\u{E923}';
pub const GLYPH_CLOSE: char = '\u{E8BB}';

/// The system font of the caption glyphs: Segoe Fluent Icons on Windows 11,
/// Segoe MDL2 Assets on Windows 10.
pub fn icon_font() -> &'static str {
    static FONT: OnceLock<&'static str> = OnceLock::new();
    FONT.get_or_init(|| {
        let fonts = std::env::var_os("WINDIR")
            .map(|w| std::path::PathBuf::from(w).join("Fonts"))
            .unwrap_or_default();
        icon_font_in(&fonts)
    })
}

fn icon_font_in(fonts: &std::path::Path) -> &'static str {
    if fonts.join("SegoeIcons.ttf").is_file() {
        "Segoe Fluent Icons"
    } else {
        "Segoe MDL2 Assets"
    }
}

/// Native state the drawn caption buttons follow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChromeState {
    pub maximize_hovered: bool,
    pub maximize_pressed: bool,
    pub maximized: bool,
}

impl ChromeState {
    pub fn apply(&mut self, event: ChromeEvent) {
        match event {
            ChromeEvent::MaximizeHovered(h) => self.maximize_hovered = h,
            ChromeEvent::MaximizePressed(p) => self.maximize_pressed = p,
            ChromeEvent::Maximized(m) => self.maximized = m,
        }
    }
}

/// What the native side tells the UI about the maximize button and state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeEvent {
    MaximizeHovered(bool),
    MaximizePressed(bool),
    Maximized(bool),
}

/// Where a point of the window is, as Windows' hit-test codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Client,
    Caption,
    Maximize,
    Top,
    TopLeft,
    TopRight,
}

/// Classifies a point in physical client coordinates.
/// `resize` is the thickness of the top resize edge in physical pixels.
pub fn hit_test(x: f32, y: f32, width: f32, scale: f32, resize: f32, maximized: bool) -> Hit {
    if !maximized && y >= 0.0 && y < resize {
        return if x < resize * 2.0 {
            Hit::TopLeft
        } else if x >= width - resize * 2.0 {
            Hit::TopRight
        } else {
            Hit::Top
        };
    }
    if y < 0.0 || y >= TITLE_BAR * scale {
        return Hit::Client;
    }
    let button = CAPTION_BUTTON * scale;
    let max_left = width - 2.0 * button;
    if x >= max_left && x < width - button {
        return Hit::Maximize;
    }
    if x >= width - 3.0 * button - TOOLS * scale {
        return Hit::Client; // settings, minimize, close: drawn and handled by the app
    }
    Hit::Caption
}

/// Native events, sent from the window procedure and read by the UI.
static EVENTS: LazyLock<Inbox<ChromeEvent>> = LazyLock::new(Inbox::new);

#[cfg_attr(not(windows), allow(dead_code))]
fn emit(event: ChromeEvent) {
    EVENTS.send(event);
}

/// The events, for the one subscription that reads them.
pub fn events() -> Option<tokio::sync::mpsc::UnboundedReceiver<ChromeEvent>> {
    EVENTS.take()
}

/// Events emitted since the last call, while nobody reads them.
pub fn drain() -> Vec<ChromeEvent> {
    EVENTS.drain()
}

/// Installs the native behavior on `window` (main thread, once).
pub fn install_native_chrome(window: &dyn iced::window::Window) {
    #[cfg(windows)]
    {
        use raw_window_handle::RawWindowHandle;
        if let Ok(handle) = window.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            // SAFETY: called on the thread that owns the window, with its handle.
            unsafe { win32::install(h.hwnd.get() as _) };
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg(windows)]
mod win32 {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT,
        HTMAXBUTTON, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsZoomed, NCCALCSIZE_PARAMS,
        SIZE_MAXIMIZED, SIZE_RESTORED, SM_CXPADDEDBORDER, SM_CYFRAME, SW_MAXIMIZE, SW_RESTORE,
        SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, ShowWindow,
        WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
        WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SIZE,
    };

    use super::{ChromeEvent, Hit, emit, hit_test};

    const SUBCLASS_ID: usize = 0x5652_5244; // "VRRD"

    pub unsafe fn install(hwnd: HWND) {
        // SAFETY: `hwnd` is a live window of this thread.
        unsafe {
            SetWindowSubclass(hwnd, Some(proc), SUBCLASS_ID, 0);
            // Recompute the frame now that the caption is gone.
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            );
        }
    }

    /// Thickness of the resize frame (and of the part of a maximized window
    /// that lies off screen).
    unsafe fn frame(hwnd: HWND) -> i32 {
        // SAFETY: plain queries on a live window.
        unsafe {
            let dpi = GetDpiForWindow(hwnd);
            GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
        }
    }

    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        // SAFETY: called by Windows for our window; pointers come from Windows.
        unsafe {
            match msg {
                WM_NCCALCSIZE if wparam != 0 => {
                    let params = lparam as *mut NCCALCSIZE_PARAMS;
                    let top = (*params).rgrc[0].top;
                    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
                    if result != 0 {
                        return result;
                    }
                    // Keep the side and bottom frames; drop the caption.
                    (*params).rgrc[0].top = top;
                    if IsZoomed(hwnd) != 0 {
                        (*params).rgrc[0].top += frame(hwnd);
                    }
                    0
                }
                WM_NCHITTEST => {
                    let native = DefSubclassProc(hwnd, msg, wparam, lparam);
                    if matches!(
                        native as u32,
                        HTLEFT | HTRIGHT | HTBOTTOM | HTBOTTOMLEFT | HTBOTTOMRIGHT
                    ) {
                        return native;
                    }
                    let mut point = POINT {
                        x: (lparam & 0xffff) as i16 as i32,
                        y: ((lparam >> 16) & 0xffff) as i16 as i32,
                    };
                    ScreenToClient(hwnd, &mut point);
                    let mut rect: RECT = std::mem::zeroed();
                    GetClientRect(hwnd, &mut rect);
                    let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
                    let hit = hit_test(
                        point.x as f32,
                        point.y as f32,
                        rect.right as f32,
                        scale,
                        frame(hwnd) as f32,
                        IsZoomed(hwnd) != 0,
                    );
                    (match hit {
                        Hit::Client => HTCLIENT,
                        Hit::Caption => HTCAPTION,
                        Hit::Maximize => HTMAXBUTTON,
                        Hit::Top => HTTOP,
                        Hit::TopLeft => HTTOPLEFT,
                        Hit::TopRight => HTTOPRIGHT,
                    }) as LRESULT
                }
                WM_NCMOUSEMOVE => {
                    let over = wparam as u32 == HTMAXBUTTON;
                    emit(ChromeEvent::MaximizeHovered(over));
                    if over {
                        let mut track = TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE | TME_NONCLIENT,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        TrackMouseEvent(&mut track);
                    }
                    DefSubclassProc(hwnd, msg, wparam, lparam)
                }
                WM_NCMOUSELEAVE => {
                    emit(ChromeEvent::MaximizeHovered(false));
                    emit(ChromeEvent::MaximizePressed(false));
                    DefSubclassProc(hwnd, msg, wparam, lparam)
                }
                // The button is drawn by the app: no classic button tracking.
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if wparam as u32 == HTMAXBUTTON => {
                    emit(ChromeEvent::MaximizePressed(true));
                    0
                }
                WM_NCLBUTTONUP if wparam as u32 == HTMAXBUTTON => {
                    emit(ChromeEvent::MaximizePressed(false));
                    ShowWindow(
                        hwnd,
                        if IsZoomed(hwnd) != 0 {
                            SW_RESTORE
                        } else {
                            SW_MAXIMIZE
                        },
                    );
                    0
                }
                WM_SIZE => {
                    match wparam as u32 {
                        SIZE_MAXIMIZED => emit(ChromeEvent::Maximized(true)),
                        SIZE_RESTORED => emit(ChromeEvent::Maximized(false)),
                        _ => {}
                    }
                    DefSubclassProc(hwnd, msg, wparam, lparam)
                }
                _ => DefSubclassProc(hwnd, msg, wparam, lparam),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 1000 px wide window at 150 % with an 8 px resize edge.
    const W: f32 = 1000.0;
    const S: f32 = 1.5;
    const R: f32 = 8.0;

    #[test]
    fn top_edge_resizes_unless_maximized() {
        assert_eq!(hit_test(500.0, 0.0, W, S, R, false), Hit::Top);
        assert_eq!(hit_test(500.0, 7.9, W, S, R, false), Hit::Top);
        assert_eq!(hit_test(15.9, 2.0, W, S, R, false), Hit::TopLeft);
        assert_eq!(hit_test(16.0, 2.0, W, S, R, false), Hit::Top);
        assert_eq!(hit_test(984.0, 2.0, W, S, R, false), Hit::TopRight);
        assert_eq!(hit_test(983.9, 2.0, W, S, R, false), Hit::Top);
        assert_eq!(hit_test(500.0, 2.0, W, S, R, true), Hit::Caption);
        // The edge ends exactly at `resize`.
        assert_eq!(hit_test(500.0, 8.0, W, S, R, false), Hit::Caption);
        // Row 0 of a maximized window is still the title bar.
        assert_eq!(hit_test(500.0, 0.0, W, S, R, true), Hit::Caption);
    }

    #[test]
    fn title_bar_areas() {
        // Caption buttons are 69 px wide at 150 %: close 931..1000,
        // maximize 862..931, minimize 793..862, settings from 727.
        assert_eq!(hit_test(931.0, 20.0, W, S, R, false), Hit::Client);
        assert_eq!(hit_test(930.9, 20.0, W, S, R, false), Hit::Maximize);
        assert_eq!(hit_test(862.0, 20.0, W, S, R, false), Hit::Maximize);
        assert_eq!(hit_test(861.9, 20.0, W, S, R, false), Hit::Client);
        assert_eq!(hit_test(727.0, 20.0, W, S, R, false), Hit::Client);
        assert_eq!(hit_test(726.9, 20.0, W, S, R, false), Hit::Caption);
        assert_eq!(hit_test(100.0, 59.9, W, S, R, false), Hit::Caption);
        assert_eq!(hit_test(100.0, 60.0, W, S, R, false), Hit::Client);
        assert_eq!(hit_test(100.0, -1.0, W, S, R, true), Hit::Client);
    }

    #[test]
    fn state_follows_events() {
        let mut s = ChromeState::default();
        s.apply(ChromeEvent::MaximizeHovered(true));
        s.apply(ChromeEvent::MaximizePressed(true));
        s.apply(ChromeEvent::Maximized(true));
        assert_eq!(
            s,
            ChromeState {
                maximize_hovered: true,
                maximize_pressed: true,
                maximized: true
            }
        );
        s.apply(ChromeEvent::MaximizeHovered(false));
        s.apply(ChromeEvent::MaximizePressed(false));
        s.apply(ChromeEvent::Maximized(false));
        assert_eq!(s, ChromeState::default());
    }

    #[test]
    fn icon_font_prefers_fluent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(icon_font_in(dir.path()), "Segoe MDL2 Assets");
        std::fs::write(dir.path().join("SegoeIcons.ttf"), "").unwrap();
        assert_eq!(icon_font_in(dir.path()), "Segoe Fluent Icons");
        assert!(["Segoe Fluent Icons", "Segoe MDL2 Assets"].contains(&icon_font()));
    }

    #[test]
    fn events_round_trip() {
        emit(ChromeEvent::Maximized(true));
        emit(ChromeEvent::MaximizeHovered(false));
        assert_eq!(
            drain(),
            vec![
                ChromeEvent::Maximized(true),
                ChromeEvent::MaximizeHovered(false)
            ]
        );
        assert_eq!(drain(), vec![]);
    }
}
