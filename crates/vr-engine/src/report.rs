//! Serializable query responses. These are the wire format of the HTTP API
//! and the JSON output of the CLI, so keep changes backwards compatible.

use serde::{Deserialize, Serialize};
use vr_core::{
    Capabilities, CarTelemetry, Channel, Control, Flag, Frame, SessionKind, SessionPhase, Setup,
};

use crate::Event;
use crate::advice::Advice;
use crate::garage::Experiment;
use crate::handling::{Behaviour, HandlingLap, Zone};

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
    /// The place in class as the cars run on track now; races only. The
    /// official one changes at the line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_position: Option<u32>,
    /// Where on the lap the car is now, 0..1: what a short call is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lap_pct: Option<f64>,
    /// The corner the car is in, and the last one it drove through, on a
    /// track whose corners have names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_now: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_before: Option<String>,
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
    /// Race only: the gap on track at this moment, from when the two passed
    /// the same place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_s: Option<f64>,
    /// How `live_s` changed since the lap before, at the same place. Negative
    /// = shrinking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_change_s: Option<f64>,
    /// Whether the car runs ahead at this moment, whatever the places say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_ahead: Option<bool>,
    /// Its push-to-pass (OTS) left, and whether it is on now, where the car
    /// has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2p_left: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2p_active: Option<bool>,
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
    /// Track and air as the game reports them (absent in older reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weather: Option<vr_core::Weather>,
    /// Whether the rain grows over the last two minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rain_trend: Option<RainTrend>,
    /// The player's car (absent in older reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub car: Option<CarTelemetry>,
    /// The leader of the class, when it is not the car ahead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader: Option<GapInfo>,
    /// The cars ahead and behind as they run now, when the official places
    /// say otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_ahead: Option<GapInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_behind: Option<GapInfo>,
    /// The class by the tyres they are on, when they are not all on the same
    /// or the track is wet: who is on what, and how fast on it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_tyres: Vec<TyreGroup>,
}

/// The cars of the class on one kind of tyre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TyreGroup {
    pub compound: String,
    pub cars: Vec<CarRef>,
    /// The driver is on it.
    pub you: bool,
    /// The median of the cars' paces on it: each car's median of its clean
    /// laps since its last stop.
    pub pace_s: Option<f64>,
    /// The fastest car on it.
    pub fastest: Option<CarRef>,
    pub fastest_s: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RainTrend {
    /// Change of precipitation (0..1) per minute.
    pub per_min: f64,
    pub direction: RainDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RainDirection {
    Rising,
    Steady,
    Easing,
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
    /// The pace at the start of the race against the latest, of all laps
    /// (not only those shown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trend: Option<LapTrend>,
}

/// Clean laps at the start against the latest ones: not the first lap, not
/// the ones a mistake made slow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapTrend {
    pub early_laps: (i32, i32),
    pub early_s: f64,
    pub recent_laps: (i32, i32),
    pub recent_s: f64,
}

/// Laps averaged at each end.
const TREND_LAPS: usize = 3;
/// Slower than the median by more than this is a mistake, not the pace.
const TREND_SLOW: f64 = 1.015;

impl LapTrend {
    /// `None` until there are enough laps for both ends.
    pub fn of(laps: &[LapRecord]) -> Option<Self> {
        let clean: Vec<(i32, f64)> = laps
            .iter()
            .filter(|l| l.is_clean() && l.lap > 1)
            .filter_map(|l| Some((l.lap, l.time_s?)))
            .collect();
        let mut times: Vec<f64> = clean.iter().map(|c| c.1).collect();
        times.sort_by(f64::total_cmp);
        let median = *times.get(times.len() / 2)?;
        let pace: Vec<(i32, f64)> = clean
            .into_iter()
            .filter(|c| c.1 <= median * TREND_SLOW)
            .collect();
        if pace.len() < 2 * TREND_LAPS {
            return None;
        }
        let avg = |part: &[(i32, f64)]| part.iter().map(|c| c.1).sum::<f64>() / part.len() as f64;
        let early = &pace[..TREND_LAPS];
        let recent = &pace[pace.len() - TREND_LAPS..];
        Some(Self {
            early_laps: (early[0].0, early[TREND_LAPS - 1].0),
            early_s: avg(early),
            recent_laps: (recent[0].0, recent[TREND_LAPS - 1].0),
            recent_s: avg(recent),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectorBasis {
    /// The game tells where the sectors begin; the times are worked out from
    /// the positions of the cars.
    GameLines,
    /// It does not: thirds of the lap, worked out likewise.
    Thirds,
    /// No history to work with.
    None,
}

/// Sector times of one car; `None` where they are not known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectorRow {
    pub car: CarRef,
    /// `"you"`, `"ahead"`, `"behind"`, `"leader"` or `"asked"`.
    pub role: String,
    pub last_lap_s: Option<f64>,
    /// The last lap driven from line to line.
    pub lap: Option<i32>,
    pub last: Vec<Option<f64>>,
    pub best: Vec<Option<f64>>,
    /// Of the lap being driven.
    pub current: Vec<Option<f64>>,
}

/// A stretch of the lap against another car.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StretchLoss {
    /// Lap fractions.
    pub from: f64,
    pub to: f64,
    /// From the line, where the length of the track is known.
    pub from_m: Option<f64>,
    pub to_m: Option<f64>,
    /// Sector it begins in, from 1.
    pub sector: usize,
    /// Positive: lost, negative: won.
    pub lost_s: f64,
    /// The corner it is in, on a known track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Sector times, and where the last lap was lost or won.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectorsReport {
    pub meta: Meta,
    pub basis: SectorBasis,
    /// Lap fractions at which the sectors begin.
    pub starts: Vec<f64>,
    pub rows: Vec<SectorRow>,
    /// The car the stretches are compared with.
    pub versus: Option<CarRef>,
    /// Your last lap against theirs, sector by sector. Positive = slower.
    pub delta: Vec<Option<f64>>,
    /// Costliest first.
    pub losses: Vec<StretchLoss>,
    /// Most won first.
    pub gains: Vec<StretchLoss>,
    /// The corners in each sector, on a known track: "sector 4" means
    /// nothing to a driver, "Spoon" does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corners: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsReport {
    pub meta: Meta,
    /// Pass this as `since` to get only newer events next time.
    pub last_seq: u64,
    pub events: Vec<Event>,
}

/// A change of an in-car setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Adjustment {
    pub session_time_s: f64,
    pub lap: Option<i32>,
    pub control: String,
    pub from: f64,
    pub to: f64,
}

/// The player's car: engine, tyres, damage, pit service, settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarReport {
    pub meta: Meta,
    pub car_name: Option<String>,
    pub car: Option<CarTelemetry>,
    pub water_temp_c: Option<f64>,
    pub oil_temp_c: Option<f64>,
    pub incidents: Option<i32>,
    /// In-car changes of this session, oldest first.
    pub adjustments: Vec<Adjustment>,
}

/// Values under the game's own names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelsReport {
    pub meta: Meta,
    /// How many there are in all.
    pub total: usize,
    pub channels: Vec<Channel>,
}

/// The game's own description of the session, or a part of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InfoReport {
    pub meta: Meta,
    pub path: String,
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupReport {
    pub meta: Meta,
    pub car_name: Option<String>,
    pub setup: Option<Setup>,
    /// What can be changed while driving, as it is now.
    pub controls: Vec<Control>,
    pub adjustments: Vec<Adjustment>,
}

/// How the car behaves and what could be changed. Tendencies, not
/// measurements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandlingReport {
    pub meta: Meta,
    /// Enough driving to judge the balance.
    pub ready: bool,
    /// Laps the mean is taken over.
    pub laps_sampled: usize,
    /// Seconds per lap.
    pub mean: Option<Behaviour>,
    pub laps: Vec<HandlingLap>,
    /// Where on the track, worst first.
    pub zones: Vec<Zone>,
    pub controls: Vec<Control>,
    pub adjustments: Vec<Adjustment>,
    pub advice: Vec<Advice>,
    /// What the changes while driving did.
    #[serde(default)]
    pub experiments: Vec<Experiment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotReport {
    pub meta: Meta,
    pub frame: Option<Frame>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lap(lap: i32, time_s: f64) -> LapRecord {
        LapRecord {
            lap,
            time_s: Some(time_s),
            fuel_used_l: None,
            pit: false,
            caution: false,
            position: None,
            session_time_s: 0.0,
        }
    }

    #[test]
    fn the_trend_leaves_out_the_start_stops_and_mistakes() {
        let mut laps: Vec<LapRecord> = [
            (1, 95.0),
            (2, 90.0),
            (3, 90.3),
            (4, 90.6),
            (5, 90.6),
            (6, 93.0),
            (7, 90.9),
            (8, 91.2),
            (9, 91.0),
        ]
        .into_iter()
        .map(|(l, t)| lap(l, t))
        .collect();
        laps[4].pit = true;
        // The median of the clean laps is 90.9: 93.0 is a mistake.
        let t = LapTrend::of(&laps).unwrap();
        assert_eq!((t.early_laps, t.recent_laps), ((2, 4), (7, 9)));
        assert!((t.early_s - 90.3).abs() < 1e-9, "{t:?}");
        assert!((t.recent_s - 273.1 / 3.0).abs() < 1e-9, "{t:?}");
        // One clean lap fewer, and no trend.
        assert_eq!(LapTrend::of(&laps[..8]), None);
        // At the limit the lap still counts, above it not.
        let with = |slow: f64| -> Vec<LapRecord> {
            [90.0, 90.0, 90.0, 90.0, 90.0, slow, 90.0]
                .into_iter()
                .enumerate()
                .map(|(i, t)| lap(i as i32 + 2, t))
                .collect()
        };
        let at = 90.0 * TREND_SLOW;
        assert_eq!(LapTrend::of(&with(at)).unwrap().recent_laps, (6, 8));
        assert_eq!(LapTrend::of(&with(at + 0.01)).unwrap().recent_laps, (5, 8));
    }
}
