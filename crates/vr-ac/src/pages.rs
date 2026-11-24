//! Access to the shared-memory pages.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use vr_core::SourceError;

use crate::layout::{CREWCHIEF, GRAPHICS, PHYSICS, STATIC, VEHICLE};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Physics,
    Graphics,
    Static,
    /// Written by CrewChief's in-game app, when it runs.
    CrewChief,
}

impl Page {
    pub const ALL: [Self; 4] = [Self::Physics, Self::Graphics, Self::Static, Self::CrewChief];

    pub fn mapping_name(self) -> &'static str {
        match self {
            Self::Physics => "Local\\acpmf_physics",
            Self::Graphics => "Local\\acpmf_graphics",
            Self::Static => "Local\\acpmf_static",
            Self::CrewChief => "Local\\acpmf_crewchief",
        }
    }

    /// What is worth copying: the struct, or for CrewChief the head and up to
    /// 128 cars (the bigger of its two versions).
    pub fn size(self) -> usize {
        match self {
            Self::Physics => PHYSICS.size,
            Self::Graphics => GRAPHICS.size,
            Self::Static => STATIC.size,
            Self::CrewChief => CREWCHIEF.size + 128 * VEHICLE.size,
        }
    }
}

/// The game's pages. Implemented by the live Win32 mappings and by
/// [`MemoryPages`] in tests.
pub trait Pages: Send {
    /// Copy up to `len` bytes of a page from `offset`; `None` when the page
    /// does not exist.
    fn read(&mut self, page: Page, offset: usize, len: usize) -> Option<Vec<u8>>;
}

/// Pages held in memory, shared so a test can change them under a source.
#[derive(Debug, Clone, Default)]
pub struct MemoryPages(pub Arc<Mutex<HashMap<Page, Vec<u8>>>>);

impl MemoryPages {
    pub fn set(&self, page: Page, bytes: Vec<u8>) {
        if let Ok(mut m) = self.0.lock() {
            m.insert(page, bytes);
        }
    }

    pub fn remove(&self, page: Page) {
        if let Ok(mut m) = self.0.lock() {
            m.remove(&page);
        }
    }
}

impl Pages for MemoryPages {
    fn read(&mut self, page: Page, offset: usize, len: usize) -> Option<Vec<u8>> {
        let m = self.0.lock().ok()?;
        let b = m.get(&page)?;
        let end = offset.checked_add(len)?.min(b.len());
        Some(b.get(offset..end)?.to_vec())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Memory::{
        FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        OpenFileMappingW, UnmapViewOfFile, VirtualQuery,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// One read-only mapping.
    struct View {
        mapping: HANDLE,
        view: MEMORY_MAPPED_VIEW_ADDRESS,
        size: usize,
    }

    // The handle and the read-only view are safe to use from any thread.
    unsafe impl Send for View {}

    impl View {
        fn open(name: &str) -> Option<Self> {
            let name = wide(name);
            // SAFETY: plain Win32 calls with a valid NUL-terminated name; the
            // handle and view are checked and released in Drop.
            unsafe {
                let mapping = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
                if mapping.is_null() {
                    return None;
                }
                let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
                if view.Value.is_null() {
                    CloseHandle(mapping);
                    return None;
                }
                let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                let got = VirtualQuery(
                    view.Value,
                    &mut info,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                );
                Some(Self {
                    mapping,
                    view,
                    size: if got == 0 { 0 } else { info.RegionSize },
                })
            }
        }

        fn read(&self, offset: usize, len: usize) -> Option<Vec<u8>> {
            let end = offset.checked_add(len)?.min(self.size);
            let len = end.checked_sub(offset)?;
            let mut out = vec![0u8; len];
            // SAFETY: bounds checked against the mapped region. The game may
            // write meanwhile; the source checks the packet ids around it.
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

    impl Drop for View {
        fn drop(&mut self) {
            // SAFETY: releasing what `open` acquired.
            unsafe {
                UnmapViewOfFile(self.view);
                CloseHandle(self.mapping);
            }
        }
    }

    /// The live pages of a running Assetto Corsa.
    pub struct Mapped {
        views: HashMap<Page, View>,
        /// CrewChief's app may start later; look for it now and then.
        crewchief_tried: Option<Instant>,
    }

    impl Mapped {
        pub fn open() -> Result<Self, SourceError> {
            let mut views = HashMap::new();
            for p in [Page::Physics, Page::Graphics, Page::Static] {
                let v = View::open(p.mapping_name()).ok_or_else(|| {
                    SourceError::NotRunning(
                        "Assetto Corsa shared memory not found (is the game running and are you on track?)".into(),
                    )
                })?;
                views.insert(p, v);
            }
            let mut m = Self {
                views,
                crewchief_tried: None,
            };
            m.find_crewchief();
            Ok(m)
        }

        fn find_crewchief(&mut self) {
            if self.views.contains_key(&Page::CrewChief)
                || self
                    .crewchief_tried
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
            {
                return;
            }
            self.crewchief_tried = Some(Instant::now());
            if let Some(v) = View::open(Page::CrewChief.mapping_name()) {
                self.views.insert(Page::CrewChief, v);
            }
        }
    }

    impl Pages for Mapped {
        fn read(&mut self, page: Page, offset: usize, len: usize) -> Option<Vec<u8>> {
            if page == Page::CrewChief {
                self.find_crewchief();
            }
            self.views.get(&page)?.read(offset, len)
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    /// Assetto Corsa only runs on Windows; elsewhere use its UDP telemetry.
    pub struct Mapped(pub(super) ());

    impl Mapped {
        pub fn open() -> Result<Self, SourceError> {
            Err(SourceError::Unsupported(
                "Assetto Corsa shared memory is only available on Windows (use --game ac-udp from another machine)".into(),
            ))
        }
    }

    impl Pages for Mapped {
        fn read(&mut self, _page: Page, _offset: usize, _len: usize) -> Option<Vec<u8>> {
            None
        }
    }
}

pub use imp::Mapped;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_pages_read_what_exists() {
        let mut m = MemoryPages::default();
        assert_eq!(m.read(Page::Static, 0, 4), None);
        m.set(Page::Static, vec![1, 2, 3, 4, 5]);
        assert_eq!(m.read(Page::Static, 1, 2), Some(vec![2, 3]));
        assert_eq!(m.read(Page::Static, 3, 10), Some(vec![4, 5]), "cut short");
        assert_eq!(m.read(Page::Static, 6, 1), None, "beyond the end");
        m.remove(Page::Static);
        assert_eq!(m.read(Page::Static, 0, 1), None);
        let names: Vec<&str> = Page::ALL.iter().map(|p| p.mapping_name()).collect();
        assert_eq!(
            names,
            [
                "Local\\acpmf_physics",
                "Local\\acpmf_graphics",
                "Local\\acpmf_static",
                "Local\\acpmf_crewchief"
            ]
        );
        assert_eq!(Page::Graphics.size(), 296);
        assert_eq!(Page::CrewChief.size(), 520 + 128 * 228);
    }

    #[cfg(not(windows))]
    #[test]
    fn live_pages_need_windows() {
        let Err(e) = Mapped::open() else {
            panic!("opened")
        };
        assert!(matches!(e, SourceError::Unsupported(_)), "{e}");
        assert_eq!(Mapped(()).read(Page::Physics, 0, 4), None);
    }
}
