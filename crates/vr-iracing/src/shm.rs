//! Access to iRacing's shared memory.

use std::time::Duration;

use vr_core::SourceError;

/// A readable memory image plus a "new data" signal. Implemented by the live
/// Win32 mapping and by in-memory images in tests.
pub trait SharedMemory: Send {
    /// Copy `len` bytes at `offset`, or `None` if out of bounds.
    fn read(&self, offset: usize, len: usize) -> Option<Vec<u8>>;
    /// Block until the producer signals new data or `timeout` elapses.
    fn wait(&self, timeout: Duration);
}

pub const MEM_MAP_NAME: &str = "Local\\IRSDKMemMapFileName";
pub const DATA_EVENT_NAME: &str = "Local\\IRSDKDataValidEvent";

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Memory::{
        FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        OpenFileMappingW, UnmapViewOfFile, VirtualQuery,
    };
    use windows_sys::Win32::System::Threading::{OpenEventW, WaitForSingleObject};

    const SYNCHRONIZE: u32 = 0x0010_0000;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The live iRacing mapping.
    pub struct Mapped {
        mapping: HANDLE,
        view: MEMORY_MAPPED_VIEW_ADDRESS,
        size: usize,
        event: HANDLE,
    }

    // The handles and the read-only view are safe to use from any thread.
    unsafe impl Send for Mapped {}

    impl Mapped {
        pub fn open() -> Result<Self, SourceError> {
            let name = wide(MEM_MAP_NAME);
            // SAFETY: plain Win32 calls with valid NUL-terminated names; every
            // returned handle/view is checked and released in Drop.
            unsafe {
                let mapping = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
                if mapping.is_null() {
                    return Err(SourceError::NotRunning(
                        "iRacing telemetry not found (is the sim running and are you in a session?)".into(),
                    ));
                }
                let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
                if view.Value.is_null() {
                    CloseHandle(mapping);
                    return Err(SourceError::Io(std::io::Error::last_os_error()));
                }
                let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                let got = VirtualQuery(
                    view.Value,
                    &mut info,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                );
                let size = if got == 0 { 0 } else { info.RegionSize };
                let ev_name = wide(DATA_EVENT_NAME);
                let event = OpenEventW(SYNCHRONIZE, 0, ev_name.as_ptr());
                Ok(Self {
                    mapping,
                    view,
                    size,
                    event,
                })
            }
        }
    }

    impl SharedMemory for Mapped {
        fn read(&self, offset: usize, len: usize) -> Option<Vec<u8>> {
            let end = offset.checked_add(len)?;
            if end > self.size {
                return None;
            }
            let mut out = vec![0u8; len];
            // SAFETY: bounds checked against the mapped region size above.
            // The producer may write concurrently; callers validate
            // consistency via the buffer tick counts.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (self.view.Value as *const u8).add(offset),
                    out.as_mut_ptr(),
                    len,
                );
            }
            Some(out)
        }

        fn wait(&self, timeout: Duration) {
            if self.event.is_null() {
                std::thread::sleep(timeout.min(Duration::from_millis(16)));
                return;
            }
            // SAFETY: valid event handle owned by self.
            unsafe {
                WaitForSingleObject(self.event, timeout.as_millis().min(u32::MAX as u128) as u32);
            }
        }
    }

    impl Drop for Mapped {
        fn drop(&mut self) {
            // SAFETY: releasing resources acquired in `open`.
            unsafe {
                UnmapViewOfFile(self.view);
                CloseHandle(self.mapping);
                if !self.event.is_null() {
                    CloseHandle(self.event);
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    /// iRacing only runs on Windows.
    pub struct Mapped(());

    impl Mapped {
        pub fn open() -> Result<Self, SourceError> {
            Err(SourceError::Unsupported(
                "iRacing telemetry is only available on Windows".into(),
            ))
        }
    }

    impl SharedMemory for Mapped {
        fn read(&self, _offset: usize, _len: usize) -> Option<Vec<u8>> {
            None
        }
        fn wait(&self, timeout: Duration) {
            std::thread::sleep(timeout);
        }
    }
}

pub use imp::Mapped;

/// In-memory image, for tests and tooling.
#[derive(Debug, Clone, Default)]
pub struct MemoryImage(pub std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl SharedMemory for MemoryImage {
    fn read(&self, offset: usize, len: usize) -> Option<Vec<u8>> {
        let b = self.0.lock().ok()?;
        b.get(offset..offset.checked_add(len)?).map(<[u8]>::to_vec)
    }

    fn wait(&self, _timeout: Duration) {}
}
