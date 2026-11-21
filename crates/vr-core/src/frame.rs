use serde::{Deserialize, Serialize};

/// One normalized telemetry sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// Adapter id, e.g. `"iracing"`.
    pub game: String,
    /// Monotonic sample counter from the source.
    pub tick: u64,
    /// Wall clock time the sample was taken (unix epoch milliseconds).
    pub timestamp_ms: i64,
    pub session: SessionState,
    /// Index into [`Frame::cars`] (by `CarState::idx`) of the player's car.
    pub player_car_idx: Option<usize>,
    pub player: Option<PlayerTelemetry>,
    /// All cars in the session (pace car and spectators excluded).
    pub cars: Vec<CarState>,
    pub weather: Option<Weather>,
}

impl Frame {
    pub fn car(&self, idx: usize) -> Option<&CarState> {
        self.cars.iter().find(|c| c.idx == idx)
    }

    pub fn player_car(&self) -> Option<&CarState> {
        self.player_car_idx.and_then(|i| self.car(i))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    /// Changes whenever the game moves to a new session (practice → qualify → race).
    pub session_num: i32,
    pub kind: SessionKind,
    pub phase: SessionPhase,
    /// Seconds since the session started.
    pub time_s: f64,
    pub time_remaining_s: Option<f64>,
    pub laps_remaining: Option<i32>,
    pub laps_total: Option<i32>,
    pub flags: Vec<Flag>,
    pub track: Option<TrackInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Practice,
    Qualify,
    Warmup,
    Race,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Unknown,
    GetInCar,
    Warmup,
    ParadeLaps,
    Racing,
    Checkered,
    CoolDown,
}

/// Normalized flag set. Adapters map their native bitfields onto these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    Green,
    Yellow,
    /// Local waving yellow (incident near the car).
    YellowWaving,
    /// Full course caution / safety car.
    Caution,
    Red,
    Blue,
    White,
    Checkered,
    Black,
    /// Meatball: car must pit for repairs.
    Repair,
    Disqualify,
    Debris,
    OneLapToGreen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackInfo {
    pub name: String,
    pub config: Option<String>,
    pub length_m: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackSurface {
    NotInWorld,
    OffTrack,
    InPitStall,
    ApproachingPits,
    OnTrack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarState {
    pub idx: usize,
    /// Car number as shown on the car, e.g. `"23"`.
    pub number: String,
    pub driver: String,
    pub team: Option<String>,
    pub car_name: Option<String>,
    pub class_name: Option<String>,
    pub class_id: Option<i32>,
    pub rating: Option<i32>,
    pub is_player: bool,
    /// Overall position (1-based). `None` before positions are assigned.
    pub position: Option<u32>,
    pub class_position: Option<u32>,
    /// Lap the car is currently on.
    pub lap: Option<i32>,
    pub laps_completed: Option<i32>,
    /// Progress around the current lap, 0.0..1.0.
    pub lap_dist_pct: Option<f64>,
    pub on_pit_road: bool,
    pub surface: TrackSurface,
    pub last_lap_s: Option<f64>,
    pub best_lap_s: Option<f64>,
    /// Race only: time behind the leader.
    pub gap_to_leader_s: Option<f64>,
    /// Game's estimated lap time for this car/class, used for relative gaps.
    pub est_lap_s: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PlayerTelemetry {
    pub on_track: bool,
    pub speed_mps: Option<f64>,
    pub rpm: Option<f64>,
    pub gear: Option<i32>,
    pub throttle: Option<f64>,
    pub brake: Option<f64>,
    pub fuel_l: Option<f64>,
    pub fuel_capacity_l: Option<f64>,
    /// Instantaneous consumption in litres per hour.
    pub fuel_use_lph: Option<f64>,
    pub incidents: Option<i32>,
    pub current_lap_s: Option<f64>,
    pub delta_to_best_s: Option<f64>,
    pub water_temp_c: Option<f64>,
    pub oil_temp_c: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Weather {
    pub air_temp_c: Option<f64>,
    pub track_temp_c: Option<f64>,
    /// Free-form wetness description (e.g. `"dry"`, `"moderately wet"`).
    pub wetness: Option<String>,
    /// 0.0..1.0
    pub precipitation: Option<f64>,
}
