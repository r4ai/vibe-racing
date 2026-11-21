//! Game-agnostic race telemetry model.
//!
//! Every game adapter converts its native telemetry into [`Frame`]s and exposes
//! itself through [`TelemetrySource`]. Everything above this crate (analysis,
//! CLI, HTTP, MCP) only ever sees these types.
//!
//! Units are SI unless the field name says otherwise: seconds, metres, litres,
//! degrees Celsius, metres per second.

mod frame;
mod source;

pub use frame::*;
pub use source::*;
