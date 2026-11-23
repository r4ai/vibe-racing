//! Automobilista 2 adapter.
//!
//! The game publishes its telemetry in a named shared memory block
//! (`$pcars2$`) when Options > System > Shared Memory is set to
//! "Project CARS 2". The block is one C struct (`SharedMemory.h`, version 14)
//! rewritten every rendered frame; a sequence number tells complete copies
//! from torn ones. What every field becomes is listed in
//! `docs/telemetry.md`.
//!
//! The parsing here works on plain byte slices so it is testable on any OS;
//! only [`shm`] touches the Win32 API.

mod convert;
mod layout;
#[cfg(test)]
mod race_tests;
pub mod shm;
mod source;
mod tracker;

use std::sync::{Arc, Mutex, OnceLock};

pub use layout::{KNOWN_VERSION, MIN_VERSION, Snapshot};
pub use shm::SharedMemory;
pub use source::Ams2Source;
pub use tracker::Tracker;

/// What one process has learnt about the session, kept when the live
/// source is opened again.
fn shared_tracker() -> Arc<Mutex<Tracker>> {
    static T: OnceLock<Arc<Mutex<Tracker>>> = OnceLock::new();
    T.get_or_init(Arc::default).clone()
}

/// Open the live Automobilista 2 telemetry (Windows only).
pub fn open_live() -> Result<Ams2Source<shm::Mapped>, vr_core::SourceError> {
    Ok(Ams2Source::with_tracker(
        shm::Mapped::open()?,
        shared_tracker(),
    ))
}
