//! Serializable query responses. These are the wire format of the HTTP API
//! and the JSON output of the CLI, so keep changes backwards compatible.

use serde::{Deserialize, Serialize};
use vr_core::{Capabilities, Flag, Frame, SessionKind, SessionPhase};

use crate::Event;

/// How the data behind a response was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Long-running collector (`vr serve`) attached to a live game.
    Live,
    /// A single sample read on demand; no history.
    Oneshot,
    /// Recorded telemetry replayed from a file.
    Replay,
    /// Generated demo data.
    Synthetic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub game: String,
    pub source: SourceKind,
    pub connected: bool,
    /// Milliseconds since the last sample was ingested.
    pub age_ms: Option<i64>,
    /// Whether lap history / trends are available (false for one-shot reads).
    pub history: bool,
    pub session_time_s: Option<f64>,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarRef {
    pub idx: usize,
    pub number: String,
    pub driver: String,
    pub class_name: Option<String>,
    pub position: Option<u32>,
    pub class_position: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub kind: SessionKind,
    pub phase: SessionPhase,
    pub track: Option<String>,
    /// Lap the player is on.
    pub lap: Option<i32>,
    pub laps_total: Option<i32>,
    pub laps_remaining: Option<i32>,
    pub time_remaining_s: Option<f64>,
    pub flags: Vec<Flag>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerSummary {
    pub car: CarRef,
    pub cars_total: usize,
    pub cars_in_class: usize,
    pub last_lap_s: Option<f64>,
    pub best_lap_s: Option<f64>,
    /// Average of the last few clean laps.
    pub avg_lap_s: Option<f64>,
    pub incidents: Option<i32>,
    pub on_pit_road: bool,
    pub on_track: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapMode {
    /// Race: time difference on the road (from time-behind-leader).
    RaceTime,
    /// Race fallback: lap progress difference × reference lap time.
    TrackProgress,
    /// Practice / qualifying: best lap time difference.
    BestLap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GapInfo {
    pub car: CarRef,
    /// Positive seconds; direction is implied by ahead/behind.
    pub gap_s: Option<f64>,
    /// Whole laps between the cars (0 = same lap).
    pub laps: i32,
    pub mode: GapMode,
    /// Change of the gap per lap over recent laps. Negative = gap shrinking.
    pub trend_s_per_lap: Option<f64>,
    pub their_last_lap_s: Option<f64>,
    /// Their last lap minus ours. Negative = they were faster.
    pub last_lap_delta_s: Option<f64>,
    pub on_pit_road: bool,
}

/// A car physically near the player on track (regardless of position).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelativeCar {
    pub car: CarRef,
    /// Positive = ahead on track, negative = behind.
    pub rel_s: f64,
    pub same_class: bool,
    /// Positive = car is laps ahead of us in the race, negative = laps down.
    pub lap_diff: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuelBasis {
    /// Measured from completed clean laps.
    History,
    /// Estimated from instantaneous consumption.
    Instant,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuelInfo {
    pub fuel_l: Option<f64>,
    pub capacity_l: Option<f64>,
    pub per_lap_l: Option<f64>,
    pub basis: FuelBasis,
    pub laps_sampled: usize,
    /// Laps the current fuel lasts at `per_lap_l`.
    pub laps_of_fuel: Option<f64>,
    /// Laps (including the current partial lap) until the chequered flag.
    pub laps_to_finish: Option<f64>,
    /// Fuel to add to finish. Negative = surplus.
    pub to_finish_l: Option<f64>,
    /// `to_finish_l` plus a safety margin of half a lap, clamped to ≥ 0.
    pub add_with_margin_l: Option<f64>,
    /// Last lap number the car can complete before running dry.
    pub last_lap_on_fuel: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Brief {
    pub meta: Meta,
    pub session: Option<SessionSummary>,
    pub player: Option<PlayerSummary>,
    pub ahead: Option<GapInfo>,
    pub behind: Option<GapInfo>,
    /// Nearest cars on track within a few seconds.
    pub nearby: Vec<RelativeCar>,
    pub fuel: Option<FuelInfo>,
    pub recent_events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandingRow {
    pub car: CarRef,
    pub is_player: bool,
    pub laps_completed: Option<i32>,
    pub gap_to_leader_s: Option<f64>,
    /// Gap to the car one position ahead in the same scope.
    pub interval_s: Option<f64>,
    pub last_lap_s: Option<f64>,
    pub best_lap_s: Option<f64>,
    pub on_pit_road: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandingsReport {
    pub meta: Meta,
    pub class_only: bool,
    pub rows: Vec<StandingRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GapReport {
    pub meta: Meta,
    pub ahead: Option<GapInfo>,
    pub behind: Option<GapInfo>,
    /// Present when a specific car was requested.
    pub target: Option<GapInfo>,
    /// Present when a specific car was requested: positive = target is ahead.
    pub target_is_ahead: Option<bool>,
    pub nearby: Vec<RelativeCar>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuelReport {
    pub meta: Meta,
    pub fuel: Option<FuelInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapRecord {
    pub lap: i32,
    pub time_s: Option<f64>,
    /// Player only.
    pub fuel_used_l: Option<f64>,
    /// Touched pit road during the lap (in-lap / out-lap).
    pub pit: bool,
    /// Caution / yellow was shown during the lap.
    pub caution: bool,
    pub position: Option<u32>,
    pub session_time_s: f64,
}

impl LapRecord {
    /// Representative of race pace (no pit, no caution, has a time).
    pub fn is_clean(&self) -> bool {
        !self.pit && !self.caution && self.time_s.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapsReport {
    pub meta: Meta,
    pub car: Option<CarRef>,
    pub laps: Vec<LapRecord>,
    pub best_lap_s: Option<f64>,
    pub avg_clean_lap_s: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsReport {
    pub meta: Meta,
    /// Pass this as `since` to get only newer events next time.
    pub last_seq: u64,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotReport {
    pub meta: Meta,
    pub frame: Option<Frame>,
}
