//! Buttons of steering wheels and other game controllers.
//!
//! Windows Raw Input with `RIDEV_INPUTSINK`: reports arrive while another
//! window (the game) has the focus, for any number of buttons, without
//! opening the device.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

/// A button of a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// Hardware id, the same whatever the USB port (see [`device_key`]).
    pub device: String,
    /// Product name, for display.
    pub name: String,
    /// Button number, starting at 1.
    pub button: u16,
}

impl Binding {
    pub fn same_button(&self, other: &Binding) -> bool {
        self.device == other.device && self.button == other.button
    }
}

impl std::fmt::Display for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ボタン {}", self.name, self.button)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonEvent {
    pub button: Binding,
    pub pressed: bool,
}

/// The hardware id part of a device path:
/// `\\?\HID#VID_346E&PID_0006&MI_02#8&1a2b&0&0000#{guid}` →
/// `VID_346E&PID_0006&MI_02`. The instance part changes with the port.
pub fn device_key(path: &str) -> String {
    path.split('#').nth(1).unwrap_or(path).to_ascii_uppercase()
}

/// Buttons held per device and report id, to turn reports into changes.
#[derive(Debug, Default)]
pub struct Held {
    held: HashMap<(usize, u8), BTreeSet<u16>>,
}

impl Held {
    /// Changes since the last report of the same id: (button, pressed).
    pub fn report(&mut self, device: usize, report: u8, now: BTreeSet<u16>) -> Vec<(u16, bool)> {
        let before = self.held.entry((device, report)).or_default();
        let released = before.difference(&now).map(|b| (*b, false));
        let pressed = now.difference(before).map(|b| (*b, true));
        let changes = released.chain(pressed).collect();
        *before = now;
        changes
    }

    /// An unplugged device releases everything it held.
    pub fn remove(&mut self, device: usize) -> Vec<u16> {
        let mut released = BTreeSet::new();
        self.held.retain(|(d, _), buttons| {
            if *d == device {
                released.append(buttons);
            }
            *d != device
        });
        released.into_iter().collect()
    }
}

/// Starts listening on a thread; a no-op where Raw Input does not exist.
pub fn listen(events: tokio::sync::mpsc::UnboundedSender<ButtonEvent>) {
    #[cfg(windows)]
    raw_input::spawn(events);
    #[cfg(not(windows))]
    drop(events);
}

#[cfg(windows)]
mod raw_input {
    use std::collections::{BTreeSet, HashMap};
    use std::ptr::{null, null_mut};

    use tokio::sync::mpsc::UnboundedSender;
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HID_USAGE_PAGE_BUTTON, HID_USAGE_PAGE_GENERIC, HidD_GetProductString, HidP_GetUsages,
        HidP_Input,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::UI::Input::{
        GetRawInputData, GetRawInputDeviceInfoW, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
        RAWINPUTHEADER, RID_INPUT, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDI_DEVICENAME,
        RIDI_PREPARSEDDATA, RIM_TYPEHID, RegisterRawInputDevices,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GIDC_REMOVAL, GetMessageW, HWND_MESSAGE, MSG, WM_INPUT,
        WM_INPUT_DEVICE_CHANGE,
    };

    use super::{Binding, ButtonEvent, Held, device_key};

    const HIDP_STATUS_SUCCESS: i32 = 0x0011_0000;
    /// More than any controller reports at once.
    const MAX_PRESSED: usize = 256;

    struct Device {
        key: String,
        name: String,
        /// `u64` elements: the HID parser expects an aligned buffer.
        preparsed: Vec<u64>,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    pub fn spawn(events: UnboundedSender<ButtonEvent>) {
        let thread = std::thread::Builder::new().name("vr-radio-wheel".into());
        if let Err(e) = thread.spawn(move || run(&events)) {
            tracing::warn!("wheel buttons unavailable: {e}");
        }
    }

    fn run(events: &UnboundedSender<ButtonEvent>) {
        let class = wide("STATIC");
        // SAFETY: a message-only window of a system class, owned by this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                null_mut(),
                null(),
            )
        };
        // Joystick, gamepad and multi-axis controller.
        let devices = [0x04u16, 0x05, 0x08].map(|usage| RAWINPUTDEVICE {
            usUsagePage: HID_USAGE_PAGE_GENERIC,
            usUsage: usage,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: hwnd,
        });
        // SAFETY: `devices` is a valid array of the given length.
        let ok = !hwnd.is_null()
            && unsafe {
                RegisterRawInputDevices(
                    devices.as_ptr(),
                    devices.len() as u32,
                    size_of::<RAWINPUTDEVICE>() as u32,
                )
            } != 0;
        if !ok {
            let e = std::io::Error::last_os_error();
            tracing::warn!("wheel buttons unavailable: {e}");
            return;
        }

        let mut known: HashMap<usize, Option<Device>> = HashMap::new();
        let mut held = Held::default();
        let mut msg = MSG::default();
        // SAFETY: the message loop of this thread's window.
        while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
            let changes = match msg.message {
                WM_INPUT => on_input(msg.lParam as HRAWINPUT, &mut known, &mut held),
                WM_INPUT_DEVICE_CHANGE if msg.wParam as u32 == GIDC_REMOVAL => {
                    let handle = msg.lParam as usize;
                    let released = held.remove(handle);
                    match known.remove(&handle).flatten() {
                        Some(d) => released.iter().map(|b| event(&d, *b, false)).collect(),
                        None => Vec::new(),
                    }
                }
                _ => Vec::new(),
            };
            for e in changes {
                if events.send(e).is_err() {
                    return;
                }
            }
            // SAFETY: `msg` came from `GetMessageW`; DefWindowProc frees the input.
            unsafe { DispatchMessageW(&msg) };
        }
    }

    fn event(d: &Device, button: u16, pressed: bool) -> ButtonEvent {
        ButtonEvent {
            button: Binding {
                device: d.key.clone(),
                name: d.name.clone(),
                button,
            },
            pressed,
        }
    }

    fn on_input(
        input: HRAWINPUT,
        known: &mut HashMap<usize, Option<Device>>,
        held: &mut Held,
    ) -> Vec<ButtonEvent> {
        let header = size_of::<RAWINPUTHEADER>() as u32;
        let mut size = 0u32;
        // SAFETY: a null buffer asks for the size.
        unsafe { GetRawInputData(input, RID_INPUT, null_mut(), &mut size, header) };
        // At least a whole RAWINPUT, so that reading its fields is in bounds.
        let bytes = (size as usize).max(size_of::<RAWINPUT>());
        let mut buf = vec![0u64; bytes.div_ceil(8)];
        // SAFETY: `buf` holds at least `size` bytes, aligned for RAWINPUT.
        let read = unsafe {
            GetRawInputData(input, RID_INPUT, buf.as_mut_ptr().cast(), &mut size, header)
        };
        if read == 0 || read == u32::MAX {
            return Vec::new();
        }
        // SAFETY: the zero-initialized buffer holds a whole RAWINPUT.
        let (handle, kind, report_len, count, reports) = unsafe {
            let raw = buf.as_ptr().cast::<RAWINPUT>();
            let hid = &raw const (*raw).data.hid;
            (
                (*raw).header.hDevice,
                (*raw).header.dwType,
                (*hid).dwSizeHid as usize,
                (*hid).dwCount as usize,
                (&raw const (*hid).bRawData).cast::<u8>(),
            )
        };
        let offset = reports as usize - buf.as_ptr() as usize;
        let total = report_len.saturating_mul(count);
        if kind != RIM_TYPEHID
            || report_len == 0
            || total
                .checked_add(offset)
                .is_none_or(|end| end > read as usize)
        {
            return Vec::new();
        }
        let device = known.entry(handle as usize).or_insert_with(|| open(handle));
        let Some(device) = device else {
            return Vec::new();
        };
        // SAFETY: `buf` is initialized; the range was checked against it above.
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), read as usize) };
        let mut out = Vec::new();
        for report in bytes[offset..offset + total].chunks_exact(report_len) {
            let mut usages = [0u16; MAX_PRESSED];
            let mut len = MAX_PRESSED as u32;
            let mut report = report.to_vec();
            // SAFETY: the preparsed data belongs to this device and the
            // buffers have the given lengths.
            let status = unsafe {
                HidP_GetUsages(
                    HidP_Input,
                    HID_USAGE_PAGE_BUTTON,
                    0,
                    usages.as_mut_ptr(),
                    &mut len,
                    device.preparsed.as_ptr() as _,
                    report.as_mut_ptr(),
                    report.len() as u32,
                )
            };
            // Reports of other ids carry no buttons.
            if status != HIDP_STATUS_SUCCESS {
                continue;
            }
            let now: BTreeSet<u16> = usages[..len as usize].iter().copied().collect();
            for (button, pressed) in held.report(handle as usize, report[0], now) {
                out.push(event(device, button, pressed));
            }
        }
        out
    }

    fn open(handle: HANDLE) -> Option<Device> {
        let mut chars = 0u32;
        // SAFETY: null buffers ask for the size; the buffers then have it.
        let (path, preparsed) = unsafe {
            GetRawInputDeviceInfoW(handle, RIDI_DEVICENAME, null_mut(), &mut chars);
            let mut path = vec![0u16; chars as usize];
            GetRawInputDeviceInfoW(
                handle,
                RIDI_DEVICENAME,
                path.as_mut_ptr().cast(),
                &mut chars,
            );
            let mut size = 0u32;
            GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, null_mut(), &mut size);
            if size == 0 {
                return None;
            }
            let mut preparsed = vec![0u64; (size as usize).div_ceil(8)];
            GetRawInputDeviceInfoW(
                handle,
                RIDI_PREPARSEDDATA,
                preparsed.as_mut_ptr().cast(),
                &mut size,
            );
            (path, preparsed)
        };
        let len = path.iter().position(|c| *c == 0).unwrap_or(path.len());
        let path = String::from_utf16_lossy(&path[..len]);
        let key = device_key(&path);
        let name = product(&path).unwrap_or_else(|| "コントローラー".into());
        tracing::debug!("controller: {name} ({key})");
        Some(Device {
            key,
            name,
            preparsed,
        })
    }

    fn product(path: &str) -> Option<String> {
        let wpath = wide(path);
        let mut name = [0u16; 127];
        // SAFETY: opened without access rights, only to read the name, and closed.
        let ok = unsafe {
            let file = CreateFileW(
                wpath.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            );
            if file == INVALID_HANDLE_VALUE {
                return None;
            }
            let ok =
                HidD_GetProductString(file, name.as_mut_ptr().cast(), size_of_val(&name) as u32);
            CloseHandle(file);
            ok
        };
        let len = name.iter().position(|c| *c == 0).unwrap_or(name.len());
        let name = String::from_utf16_lossy(&name[..len]).trim().to_string();
        (ok && !name.is_empty()).then_some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(b: &[u16]) -> BTreeSet<u16> {
        b.iter().copied().collect()
    }

    #[test]
    fn device_key_ignores_the_port() {
        let a = device_key(r"\\?\HID#vid_346e&pid_0006&MI_02#8&1a2b&0&0000#{4d1e}");
        assert_eq!(a, "VID_346E&PID_0006&MI_02");
        let other_port = device_key(r"\\?\HID#VID_346E&PID_0006&MI_02#9&ff&0&0001#{4d1e}");
        assert_eq!(a, other_port);
        assert_eq!(device_key("odd"), "ODD");
    }

    #[test]
    fn reports_become_changes() {
        let mut h = Held::default();
        assert_eq!(h.report(1, 0, set(&[])), vec![]);
        assert_eq!(h.report(1, 0, set(&[3, 40])), vec![(3, true), (40, true)]);
        assert_eq!(h.report(1, 0, set(&[3, 40])), vec![]);
        assert_eq!(h.report(1, 0, set(&[40, 41])), vec![(3, false), (41, true)]);
        // Another report id or device does not release them.
        assert_eq!(h.report(1, 2, set(&[7])), vec![(7, true)]);
        assert_eq!(h.report(2, 0, set(&[9])), vec![(9, true)]);
        assert_eq!(h.remove(1), vec![7, 40, 41]);
        assert_eq!(h.remove(1), Vec::<u16>::new());
        assert_eq!(h.report(2, 0, set(&[])), vec![(9, false)]);
    }

    #[test]
    fn bindings_match_by_device_and_button() {
        let b = |device: &str, name: &str, button| Binding {
            device: device.into(),
            name: name.into(),
            button,
        };
        let a = b("X", "R9", 5);
        assert!(a.same_button(&b("X", "renamed", 5)));
        assert!(!a.same_button(&b("X", "R9", 6)));
        assert!(!a.same_button(&b("Y", "R9", 5)));
        assert_eq!(a.to_string(), "R9 ボタン 5");
    }
}
