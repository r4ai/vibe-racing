//! Race state tracking and analysis.
//!
//! [`RaceState`] ingests normalized [`vr_core::Frame`]s one at a time and keeps
//! the history needed to answer race-engineer questions (lap history, fuel per
//! lap, gap trends, events). [`RaceApi`] is the single query surface shared by
//! every frontend (CLI, HTTP server, future MCP server).

mod advice;
mod api;
mod archive;
mod events;
mod handling;
pub mod render;
mod render_car;
mod report;
mod state;
mod util;

pub use advice::{Advice, MIN_LAPS as ADVICE_MIN_LAPS, Place};
pub use api::*;
pub use archive::{CarLaps, SessionRecord, fmt_utc};
pub use events::*;
pub use handling::{Behaviour, HandlingLap, Phases, Tendency, Zone};
pub use report::*;
pub use state::*;
pub use util::now_ms;
