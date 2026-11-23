//! Race state tracking and analysis.
//!
//! [`RaceState`] ingests normalized [`vr_core::Frame`]s one at a time and keeps
//! the history needed to answer race-engineer questions (lap history, fuel per
//! lap, gap trends, events). [`RaceApi`] is the single query surface shared by
//! every frontend (CLI, HTTP server, future MCP server).

mod advice;
mod api;
mod archive;
mod chassis;
mod events;
mod forecast;
mod garage;
mod handling;
mod live;
pub mod render;
mod render_car;
mod report;
mod speech;
mod state;
mod strategy;
mod timing;
mod trace;
pub mod tracks;
pub(crate) mod tyres;
mod util;

pub use advice::{Advice, MIN_LAPS as ADVICE_MIN_LAPS, Place};
pub use api::*;
pub use archive::{CarLaps, SessionRecord, fmt_utc};
pub use chassis::{Chassis, Corners, Damper, Ride};
pub use events::*;
pub use forecast::{ForecastCar, ForecastReport};
pub use garage::{
    Area, Change, Comparison, Experiment, Finding, SetupAnalysis, analyse, experiments, findings,
};
pub use handling::{Balance, Behaviour, HandlingLap, Phases, Tendency, Zone};
pub use live::{LIVE_KEYS, LiveReport};
pub use report::*;
pub use speech::{Speech, Topic as SpeechTopic, speak, speak_with};
pub use state::*;
pub use strategy::{
    Basis, Fade, PitLoss, Plan, Rejoin, Rival, StrategyCall, StrategyReport, TyreCall, pit_loss_of,
};
pub use trace::{
    Corner, CornersReport, LapTrace, POINTS as TRACE_POINTS, Speeds, compare as compare_laps,
    compare_on as compare_laps_on,
};
pub use util::now_ms;
