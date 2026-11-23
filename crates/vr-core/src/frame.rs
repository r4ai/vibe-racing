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
    /// Changes when the game restarts a session under the same number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<i64>,
    /// Scheduled length of a timed session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_total_s: Option<f64>,
    /// In-game time of day, seconds after midnight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_of_day_s: Option<f64>,
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
    /// Before the cars may drive.
    GetInCar,
    /// On the grid or warming up, before the start.
    Warmup,
    /// Formation or pace laps.
    ParadeLaps,
    /// The session is running.
    Racing,
    /// The chequered flag is out.
    Checkered,
    /// After the session.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TrackInfo {
    pub name: String,
    pub config: Option<String>,
    pub length_m: Option<f64>,
    /// Where the sectors begin, as lap fractions, the first at 0; empty when
    /// the game does not tell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sectors: Vec<f64>,
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
    /// Car number as shown on the car, e.g. `"23"`; the game's slot of the
    /// car when it gives none.
    pub number: String,
    pub driver: String,
    pub team: Option<String>,
    pub car_name: Option<String>,
    pub class_name: Option<String>,
    pub class_id: Option<i32>,
    /// The driver's skill rating on the game's own scale (iRating...).
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
    /// What else the game tells about any car (absent in older recordings).
    #[serde(default)]
    pub extra: CarExtra,
}

/// Details of any car on track, the player's or not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CarExtra {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tyre_compound: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_lap_num: Option<i32>,
    /// Push-to-pass / overtake uses or seconds left (the unit is the game's).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p2p_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p2p_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_repairs_used: Option<i32>,
    /// Flags shown to this car only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<Flag>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gear: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpm: Option<f64>,
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
    /// Incident points, as the game counts them for its penalties.
    pub incidents: Option<i32>,
    pub current_lap_s: Option<f64>,
    pub delta_to_best_s: Option<f64>,
    pub water_temp_c: Option<f64>,
    pub oil_temp_c: Option<f64>,
    /// Everything else about the player's car (absent in older recordings).
    #[serde(default)]
    pub car: CarTelemetry,
}

/// The player's car in detail. Every field is `None` (or empty) when the game
/// or the car does not provide it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CarTelemetry {
    /// 0.0 disengaged .. 1.0 fully engaged.
    pub clutch: Option<f64>,
    /// Steering wheel angle in radians, positive to the left.
    pub steering_rad: Option<f64>,
    /// Positive to the left.
    pub yaw_rate_rps: Option<f64>,
    pub lat_accel_mps2: Option<f64>,
    pub long_accel_mps2: Option<f64>,
    /// Velocity in the car's frame: forward, and to the left.
    pub velocity_x_mps: Option<f64>,
    pub velocity_y_mps: Option<f64>,
    pub abs_active: Option<bool>,

    pub oil_press_bar: Option<f64>,
    pub fuel_press_bar: Option<f64>,
    pub oil_level_l: Option<f64>,
    pub water_level_l: Option<f64>,
    pub voltage: Option<f64>,
    /// Warning lights that are on.
    pub warnings: Vec<Warning>,
    pub shift_rpm: Option<f64>,

    pub tyres: Tyres,
    pub tyre_compound: Option<String>,
    pub tyre_sets_used: Option<i32>,
    /// `None`: unlimited or unknown.
    pub tyre_sets_available: Option<i32>,

    pub pit: PitState,
    /// Adjustments the driver can make while driving (brake bias, traction
    /// control...).
    pub controls: Vec<Control>,

    /// Push-to-pass / OTS left in races (the unit is the game's).
    pub p2p_count: Option<i32>,
    pub p2p_active: Option<bool>,
    pub drs_active: Option<bool>,
    /// 0.0..1.0
    pub battery_pct: Option<f64>,

    /// Success ballast and the like.
    pub weight_penalty_kg: Option<f64>,
    pub power_adjust_pct: Option<f64>,
    /// Against the best lap of the session, anyone's.
    pub delta_to_session_best_s: Option<f64>,
    pub delta_to_optimal_s: Option<f64>,
    /// Cars alongside, e.g. `"left"`, as the game's spotter sees them.
    pub alongside: Option<String>,
    pub team_incidents: Option<i32>,
    /// What each wheel does, where the game tells it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wheels: Option<Wheels>,
    /// Damage to the body, where the game tells how much.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage: Option<Damage>,
    /// Seconds of time penalty not yet served.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub penalty_s: Option<f64>,
}

/// How badly the car is hurt: each part from 0.0 (whole) to 1.0 (destroyed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Damage {
    /// The body, which carries the aero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub front: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rear: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<f64>,
    /// Each wheel's suspension: LF, RF, LR, RR.
    #[serde(skip_serializing_if = "all_none")]
    pub suspension: [Option<f64>; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<f64>,
}

fn all_none(v: &[Option<f64>; 4]) -> bool {
    v.iter().all(Option::is_none)
}

/// Lighter than this is a scratch, not damage.
pub const DAMAGE_SCRATCH: f64 = 0.05;

impl Damage {
    pub fn parts(&self) -> [(&'static str, Option<f64>); 9] {
        let [lf, rf, lr, rr] = self.suspension;
        [
            ("front", self.front),
            ("rear", self.rear),
            ("left", self.left),
            ("right", self.right),
            ("LF suspension", lf),
            ("RF suspension", rf),
            ("LR suspension", lr),
            ("RR suspension", rr),
            ("engine", self.engine),
        ]
    }

    /// The parts that are more than scratched, the worst first.
    pub fn hurt(&self) -> Vec<(&'static str, f64)> {
        let mut v: Vec<(&'static str, f64)> = self
            .parts()
            .into_iter()
            .filter_map(|(n, d)| Some((n, d.filter(|d| *d >= DAMAGE_SCRATCH)?)))
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Wheels {
    pub lf: Wheel,
    pub rf: Wheel,
    pub lr: Wheel,
    pub rr: Wheel,
}

/// One wheel at this moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Wheel {
    /// Of the tread: what the car would drive if the wheel did not slip.
    pub speed_mps: Option<f64>,
    pub pressure_kpa: Option<f64>,
    /// Outer edge, middle, inner edge.
    pub surface_c: [Option<f64>; 3],
    pub ride_height_m: Option<f64>,
    /// Positive in bump.
    pub shock_mps: Option<f64>,
    pub brake_bar: Option<f64>,
    /// Vertical load on the tyre.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_n: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camber_rad: Option<f64>,
    /// How far the suspension is compressed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub travel_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brake_temp_c: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Warning {
    WaterTemp,
    FuelPressure,
    OilPressure,
    OilTemp,
    Stalled,
    PitLimiter,
    RevLimiter,
    /// The game wants the car repaired, or repairs are pending.
    Damage,
}

impl Warning {
    /// Says that the car is hurt, not just how it is driven.
    pub fn is_damage(self) -> bool {
        matches!(
            self,
            Self::WaterTemp | Self::FuelPressure | Self::OilPressure | Self::OilTemp | Self::Damage
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Tyres {
    pub lf: Tyre,
    pub rf: Tyre,
    pub lr: Tyre,
    pub rr: Tyre,
}

impl Tyres {
    pub fn all(&self) -> [(&'static str, &Tyre); 4] {
        [
            ("LF", &self.lf),
            ("RF", &self.rf),
            ("LR", &self.lr),
            ("RR", &self.rr),
        ]
    }
}

/// Temperatures and tread across the tyre: outer, middle, inner edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Tyre {
    pub cold_kpa: Option<f64>,
    pub hot_kpa: Option<f64>,
    pub temp_c: [Option<f64>; 3],
    /// Tread left, 0.0..1.0.
    pub tread: [Option<f64>; 3],
    pub distance_m: Option<f64>,
    /// Inside the rubber, as it is now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_c: Option<f64>,
    /// The grip wear leaves, 0.0..1.0 of a new tyre's, where the game
    /// models wear as lost grip rather than lost tread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grip: Option<f64>,
}

impl Tyre {
    /// The most worn part.
    pub fn min_tread(&self) -> Option<f64> {
        self.tread.iter().flatten().copied().min_by(f64::total_cmp)
    }

    pub fn mean_temp_c(&self) -> Option<f64> {
        let t: Vec<f64> = self.temp_c.iter().flatten().copied().collect();
        (!t.is_empty()).then(|| t.iter().sum::<f64>() / t.len() as f64)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PitState {
    pub open: Option<bool>,
    pub in_stall: Option<bool>,
    /// The crew is working on the car.
    pub service_active: Option<bool>,
    /// Trouble with the stop, e.g. `"too far left"`.
    pub status: Option<String>,
    pub speed_limit_mps: Option<f64>,
    /// Mandatory repairs left.
    pub repair_left_s: Option<f64>,
    pub optional_repair_left_s: Option<f64>,
    pub tow_s: Option<f64>,
    pub fast_repairs_used: Option<i32>,
    /// `None`: unlimited or unknown.
    pub fast_repairs_available: Option<i32>,
    /// What the next stop will do: `lf tyre`, `rf tyre`, `lr tyre`, `rr tyre`,
    /// `fuel`, `windshield tearoff`, `fast repair`.
    pub requested: Vec<String>,
    pub requested_fuel_l: Option<f64>,
    /// When the rules let the car stop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<PitWindow>,
    /// The stop the rules require has been made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mandatory_done: Option<bool>,
}

/// From and to, both included, in laps or in minutes of the session.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PitWindow {
    pub from: f64,
    pub to: f64,
    pub unit: WindowUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowUnit {
    Lap,
    Minute,
}

/// One in-car adjustment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// Stable id, the same in every game for the same adjustment:
    /// `brake_bias` (% to the front), `traction_control`, `abs`,
    /// `engine_brake`... The value is on the game's own scale.
    pub id: String,
    pub value: f64,
}

/// A value the adapter passes on under the game's own name, for what the
/// normalized model does not cover.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// A boolean, number, text, or an array of them.
    pub value: serde_json::Value,
}

/// A garage setup: a flat list, as its structure differs from car to car.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Setup {
    pub name: Option<String>,
    /// Changed since it was loaded.
    pub modified: Option<bool>,
    pub passed_tech: Option<bool>,
    pub items: Vec<SetupItem>,
    /// Settings the game names without telling their value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupItem {
    /// E.g. `"Chassis/Front/ArbPosition"`.
    pub path: String,
    /// With its unit, as the garage shows it.
    pub value: String,
}

/// What changes rarely or is too big for every frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Extras {
    pub channels: Vec<Channel>,
    /// The game's own description of the session.
    pub info: Option<serde_json::Value>,
    pub setup: Option<Setup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Weather {
    pub air_temp_c: Option<f64>,
    pub track_temp_c: Option<f64>,
    pub wetness: Option<Wetness>,
    /// 0.0..1.0
    pub precipitation: Option<f64>,
    /// The stewards allow rain tyres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_wet: Option<bool>,
    /// 0.0..1.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub humidity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_mps: Option<f64>,
    /// Where the wind comes from, degrees clockwise from north.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_dir_deg: Option<f64>,
    /// `clear`, `partly cloudy`, `mostly cloudy` or `overcast`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skies: Option<String>,
    /// 0.0..1.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog: Option<f64>,
    /// Rubber laid down on the track, 0.0..1.0 (1 is fully grippy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_grip: Option<f64>,
}

/// How wet the track is, from dry to under water.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Wetness {
    #[serde(rename = "dry")]
    Dry,
    #[serde(rename = "mostly dry")]
    MostlyDry,
    #[serde(rename = "very lightly wet")]
    VeryLightlyWet,
    #[serde(rename = "lightly wet")]
    LightlyWet,
    #[serde(rename = "moderately wet")]
    ModeratelyWet,
    #[serde(rename = "very wet")]
    VeryWet,
    #[serde(rename = "extremely wet")]
    ExtremelyWet,
}

impl Wetness {
    pub const ALL: [Self; 7] = [
        Self::Dry,
        Self::MostlyDry,
        Self::VeryLightlyWet,
        Self::LightlyWet,
        Self::ModeratelyWet,
        Self::VeryWet,
        Self::ExtremelyWet,
    ];

    /// As it is written in JSON, e.g. `"mostly dry"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dry => "dry",
            Self::MostlyDry => "mostly dry",
            Self::VeryLightlyWet => "very lightly wet",
            Self::LightlyWet => "lightly wet",
            Self::ModeratelyWet => "moderately wet",
            Self::VeryWet => "very wet",
            Self::ExtremelyWet => "extremely wet",
        }
    }

    /// Wet enough for rain tyres to be a question.
    pub fn is_wet(self) -> bool {
        self > Self::MostlyDry
    }
}

impl std::fmt::Display for Wetness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wetness_is_written_as_before() {
        for w in Wetness::ALL {
            let json = serde_json::to_string(&w).unwrap();
            assert_eq!(json, format!("\"{w}\""));
            assert_eq!(serde_json::from_str::<Wetness>(&json).unwrap(), w);
        }
        let wet: Vec<bool> = Wetness::ALL.iter().map(|w| w.is_wet()).collect();
        assert_eq!(wet, [false, false, true, true, true, true, true]);
        assert!(serde_json::from_str::<Wetness>("\"damp\"").is_err());
    }

    #[test]
    fn damage_tells_what_is_more_than_scratched() {
        let d = Damage {
            front: Some(0.25),
            rear: Some(DAMAGE_SCRATCH),
            left: Some(0.049),
            right: None,
            suspension: [None, Some(0.6), None, None],
            engine: Some(0.0),
        };
        assert_eq!(
            d.hurt(),
            [
                ("RF suspension", 0.6),
                ("front", 0.25),
                ("rear", DAMAGE_SCRATCH)
            ]
        );
        assert!(Damage::default().hurt().is_empty());
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(
            json["suspension"],
            serde_json::json!([null, 0.6, null, null])
        );
        let whole = serde_json::to_value(Damage::default()).unwrap();
        assert_eq!(
            whole,
            serde_json::json!({}),
            "nothing told, nothing written"
        );
    }
}
