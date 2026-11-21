//! iRacing adapter.
//!
//! iRacing publishes telemetry through a named shared memory block
//! (`Local\IRSDKMemMapFileName`) laid out as:
//!
//! * a fixed header (version, status, tick rate, session-info location,
//!   variable-table location, up to four rotating data buffers),
//! * a table of variable headers (name, type, offset, count),
//! * a YAML "session info" string (drivers, track, sessions),
//! * the rotating data buffers themselves.
//!
//! The parsing here works on plain byte slices so it is testable on any OS;
//! only [`shm`] touches the Win32 API.

mod convert;
mod layout;
mod session_info;
pub mod shm;
mod source;

pub use layout::{Header, VarHeader, VarTable, VarType};
pub use session_info::{SessionInfo, sanitize_yaml};
pub use shm::SharedMemory;
pub use source::IRacingSource;

/// Open the live iRacing telemetry (Windows only).
pub fn open_live() -> Result<IRacingSource<shm::Mapped>, vr_core::SourceError> {
    Ok(IRacingSource::new(shm::Mapped::open()?))
}
