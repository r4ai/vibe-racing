//! Race state tracking and analysis.
//!
//! [`RaceState`] ingests normalized [`vr_core::Frame`]s one at a time and keeps
//! the history needed to answer race-engineer questions (lap history, fuel per
//! lap, gap trends, events). [`RaceApi`] is the single query surface shared by
//! every frontend (CLI, HTTP server, future MCP server).

mod api;
mod events;
pub mod render;
mod report;
mod state;
mod util;

pub use api::*;
pub use events::*;
pub use report::*;
pub use state::*;
pub use util::now_ms;
