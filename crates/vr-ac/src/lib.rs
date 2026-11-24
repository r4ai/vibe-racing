//! Assetto Corsa adapter.
//!
//! The game writes three shared-memory pages (`Local\acpmf_physics`,
//! `Local\acpmf_graphics`, `Local\acpmf_static`) about the player's car and
//! answers UDP clients on port 9996. CrewChief's in-game app adds a fourth
//! page, `Local\acpmf_crewchief`, with every car on track. After a session the
//! game leaves its results in `Documents\Assetto Corsa\out\race_out.json`.
//!
//! See `docs/data.md` for every value and where it goes. The parsing works on
//! plain byte slices so it is testable on any OS; only [`pages`] touches the
//! Win32 API.

mod convert;
pub mod layout;
pub mod pages;
mod session;
mod source;
pub mod udp;

use std::path::Path;

pub use convert::{GAME, Snapshot, build_frame};
pub use pages::{Mapped, MemoryPages, Page, Pages};
pub use session::Tracker;
pub use source::{AcSource, results_path};
pub use udp::UdpSource;

/// Open the live game (Windows only). With the user's documents folder, the
/// results the game writes after each session come along.
pub fn open_live(documents: Option<&Path>) -> Result<AcSource<Mapped>, vr_core::SourceError> {
    let src = AcSource::new(Mapped::open()?);
    Ok(match documents {
        Some(d) => src.with_results(results_path(d)),
        None => src,
    })
}

/// Assetto Corsa's UDP telemetry from the game's PC, e.g. `127.0.0.1` or
/// `gamepc:9996`.
pub fn open_udp(host: &str) -> Result<UdpSource, vr_core::SourceError> {
    UdpSource::open(udp::resolve(host)?)
}
