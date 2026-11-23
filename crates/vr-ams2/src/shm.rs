//! Access to the game's shared memory.

use vr_core::SourceError;

/// A readable memory image. Implemented by the live Win32 mapping and by
/// in-memory images in tests.
pub trait SharedMemory: Send {
    /// Copy `len` bytes at `offset`, or `None` if out of bounds.
    fn read(&self, offset: usize, len: usize) -> Option<Vec<u8>>;
}

/// The name of the mapping in "Project CARS 2" shared memory mode.
pub const MEM_MAP_NAME: &str = "$pcars2$";

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Memory::{
        FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        OpenFileMappingW, UnmapViewOfFile, VirtualQuery,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The live mapping, read-only.
    pub struct Mapped {
        mapping: HANDLE,
        view: MEMORY_MAPPED_VIEW_ADDRESS,
        size: usize,
    }

    // The handle and the read-only view are safe to use from any thread.
    unsafe impl Send for Mapped {}

    impl Mapped {
        pub fn open() -> Result<Self, SourceError> {
            let name = wide(MEM_MAP_NAME);
            // SAFETY: plain Win32 calls with a valid NUL-terminated name; the
            // handle and view are checked and released in Drop.
            unsafe {
                let mapping = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
                if mapping.is_null() {
                    return Err(SourceError::NotRunning(
                        "Automobilista 2 telemetry not found (is the game running, with \
                         Options > System > Shared Memory set to \"Project CARS 2\"?)"
                            .into(),
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
                Ok(Self {
                    mapping,
                    view,
                    size,
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
            // The game may write concurrently; callers check the sequence
            // number before and after.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (self.view.Value as *const u8).add(offset),
                    out.as_mut_ptr(),
                    len,
                );
            }
            Some(out)
        }
    }

    impl Drop for Mapped {
        fn drop(&mut self) {
            // SAFETY: releasing resources acquired in `open`.
            unsafe {
                UnmapViewOfFile(self.view);
                CloseHandle(self.mapping);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    /// Automobilista 2 only runs on Windows.
    pub struct Mapped(pub(super) ());

    impl Mapped {
        pub fn open() -> Result<Self, SourceError> {
            Err(SourceError::Unsupported(
                "Automobilista 2 telemetry is only available on Windows".into(),
            ))
        }
    }

    impl SharedMemory for Mapped {
        fn read(&self, _offset: usize, _len: usize) -> Option<Vec<u8>> {
            None
        }
    }
}

pub use imp::Mapped;

/// In-memory image, for tests and tooling.
#[derive(Debug, Clone, Default)]
pub struct MemoryImage(pub std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl MemoryImage {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(bytes)))
    }

    pub fn set(&self, bytes: Vec<u8>) {
        if let Ok(mut b) = self.0.lock() {
            *b = bytes;
        }
    }
}

impl SharedMemory for MemoryImage {
    fn read(&self, offset: usize, len: usize) -> Option<Vec<u8>> {
        let b = self.0.lock().ok()?;
        b.get(offset..offset.checked_add(len)?).map(<[u8]>::to_vec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_image_reads_in_bounds_only() {
        let m = MemoryImage::new(vec![1, 2, 3, 4]);
        assert_eq!(m.read(1, 2), Some(vec![2, 3]));
        assert_eq!(m.read(3, 2), None);
        assert_eq!(m.read(usize::MAX, 2), None);
        m.set(vec![9; 8]);
        assert_eq!(m.read(6, 2), Some(vec![9, 9]));
    }

    #[cfg(not(windows))]
    #[test]
    fn live_mapping_is_windows_only() {
        assert!(matches!(Mapped::open(), Err(SourceError::Unsupported(_))));
        assert_eq!(Mapped(()).read(0, 1), None);
    }
}
