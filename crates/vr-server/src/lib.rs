//! `vr serve`: a collector thread that reads a `TelemetrySource` at full
//! rate into a shared `RaceState`, plus a localhost HTTP API exposing
//! `RaceApi` so short-lived CLI calls (and later an MCP server) can query
//! live state with history.

mod collector;
mod discovery;
mod http;
mod sessions;

pub use collector::*;
pub use discovery::*;
pub use http::*;
pub use sessions::*;

pub const DEFAULT_PORT: u16 = 47800;
pub const API_PREFIX: &str = "/v1";
