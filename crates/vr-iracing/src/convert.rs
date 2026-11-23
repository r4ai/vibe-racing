//! iRacing variables → normalized [`vr_core::Frame`].

use vr_core::{
    CarExtra, CarState, CarTelemetry, Control, Flag, Frame, PitState, PlayerTelemetry, SessionKind,
    SessionPhase, SessionState, TrackInfo, TrackSurface, Tyre, Tyres, Warning, Weather, Wheel,
    Wheels,
};

use crate::layout::VarView;
use crate::session_info::SessionInfo;

pub const MAX_CARS: usize = 64;

// irsdk_Flags
const CHECKERED: u32 = 0x0001;
const WHITE: u32 = 0x0002;
const GREEN: u32 = 0x0004;
const YELLOW: u32 = 0x0008;
const RED: u32 = 0x0010;
const BLUE: u32 = 0x0020;
const DEBRIS: u32 = 0x0040;
const YELLOW_WAVING: u32 = 0x0100;
const ONE_LAP_TO_GREEN: u32 = 0x0200;
const CAUTION: u32 = 0x4000;
const CAUTION_WAVING: u32 = 0x8000;
const BLACK: u32 = 0x0001_0000;
const DISQUALIFY: u32 = 0x0002_0000;
const REPAIR: u32 = 0x0010_0000;

pub fn flags(bits: u32) -> Vec<Flag> {
    let table = [
        (GREEN, Flag::Green),
        (YELLOW, Flag::Yellow),
        (YELLOW_WAVING, Flag::YellowWaving),
        (CAUTION | CAUTION_WAVING, Flag::Caution),
        (RED, Flag::Red),
        (BLUE, Flag::Blue),
        (WHITE, Flag::White),
        (CHECKERED, Flag::Checkered),
        (BLACK, Flag::Black),
        (REPAIR, Flag::Repair),
        (DISQUALIFY, Flag::Disqualify),
        (DEBRIS, Flag::Debris),
        (ONE_LAP_TO_GREEN, Flag::OneLapToGreen),
    ];
    table
        .iter()
        .filter(|(m, _)| bits & m != 0)
        .map(|(_, f)| *f)
        .collect()
}

fn phase(v: Option<i32>) -> SessionPhase {
    match v {
        Some(1) => SessionPhase::GetInCar,
        Some(2) => SessionPhase::Warmup,
        Some(3) => SessionPhase::ParadeLaps,
        Some(4) => SessionPhase::Racing,
        Some(5) => SessionPhase::Checkered,
        Some(6) => SessionPhase::CoolDown,
        _ => SessionPhase::Unknown,
    }
}

fn surface(v: Option<i32>) -> TrackSurface {
    match v {
        Some(0) => TrackSurface::OffTrack,
        Some(1) => TrackSurface::InPitStall,
        Some(2) => TrackSurface::ApproachingPits,
        Some(3) => TrackSurface::OnTrack,
        _ => TrackSurface::NotInWorld,
    }
}

fn wetness(v: Option<i32>) -> Option<String> {
    Some(
        match v? {
            1 => "dry",
            2 => "mostly dry",
            3 => "very lightly wet",
            4 => "lightly wet",
            5 => "moderately wet",
            6 => "very wet",
            7 => "extremely wet",
            _ => return None,
        }
        .to_string(),
    )
}

/// Values iRacing uses for "unlimited" / "not set".
fn sane_time(v: Option<f64>) -> Option<f64> {
    v.filter(|t| *t >= 0.0 && *t < 6.0 * 24.0 * 3600.0)
}

fn sane_laps(v: Option<i32>) -> Option<i32> {
    v.filter(|l| (0..10_000).contains(l))
}

/// Bits of irsdk_EngineWarnings.
fn warnings(bits: u32) -> Vec<Warning> {
    let table = [
        (0x001, Warning::WaterTemp),
        (0x002, Warning::FuelPressure),
        (0x004, Warning::OilPressure),
        (0x008, Warning::Stalled),
        (0x010, Warning::PitLimiter),
        (0x020, Warning::RevLimiter),
        (0x040, Warning::OilTemp),
        // Mandatory or optional repairs needed.
        (0x180, Warning::Damage),
    ];
    table
        .iter()
        .filter(|(m, _)| bits & m != 0)
        .map(|(_, w)| *w)
        .collect()
}

/// irsdk_PitSvFlags.
fn pit_request(bits: u32) -> Vec<String> {
    [
        (0x01, "lf tyre"),
        (0x02, "rf tyre"),
        (0x04, "lr tyre"),
        (0x08, "rr tyre"),
        (0x10, "fuel"),
        (0x20, "windshield tearoff"),
        (0x40, "fast repair"),
    ]
    .iter()
    .filter(|(m, _)| bits & m != 0)
    .map(|(_, s)| s.to_string())
    .collect()
}

/// irsdk_PitSvStatus.
fn pit_status(v: Option<i32>) -> Option<String> {
    Some(
        match v? {
            1 => "in progress",
            2 => "complete",
            100 => "too far left",
            101 => "too far right",
            102 => "too far forward",
            103 => "too far back",
            104 => "bad angle",
            105 => "cannot be repaired",
            _ => return None,
        }
        .to_string(),
    )
}

/// irsdk_CarLeftRight; `None` when nobody is alongside.
fn alongside(v: Option<i32>) -> Option<String> {
    Some(
        match v? {
            2 => "left",
            3 => "right",
            4 => "left and right",
            5 => "two on the left",
            6 => "two on the right",
            _ => return None,
        }
        .to_string(),
    )
}

fn skies(v: Option<i32>) -> Option<String> {
    Some(
        match v? {
            0 => "clear",
            1 => "partly cloudy",
            2 => "mostly cloudy",
            3 => "overcast",
            _ => return None,
        }
        .to_string(),
    )
}

/// 255 stands for "unlimited".
fn limited(v: Option<i32>) -> Option<i32> {
    v.filter(|n| (0..255).contains(n))
}

/// `"BrakeBias"` → `"brake_bias"`, `"ABS"` → `"abs"`.
fn snake_case(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, ch) in c.iter().enumerate() {
        if ch.is_ascii_uppercase() {
            let after_lower = i > 0 && !c[i - 1].is_ascii_uppercase();
            let starts_word =
                i > 0 && c.get(i + 1).is_some_and(char::is_ascii_lowercase) && c[i - 1] != '_';
            if (after_lower || starts_word) && c[i - 1] != '_' {
                out.push('_');
            }
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// In-car switches that do not change how the car drives.
const NOT_CONTROLS: &[&str] = &[
    "dcStarter",
    "dcTearOffVisor",
    "dcDashPage",
    "dcDashPage2",
    "dcPitSpeedLimiterToggle",
    "dcHeadlightFlash",
    "dcToggleWindshieldWipers",
    "dcTriggerWindshieldWipers",
    "dcPushToPass",
    "dcDRSToggle",
];

fn controls(v: &VarView<'_>) -> Vec<Control> {
    v.names()
        .filter(|n| n.starts_with("dc") && !NOT_CONTROLS.contains(n))
        .filter_map(|n| {
            Some(Control {
                id: snake_case(&n[2..]),
                // Four decimals: 57.2519, not 57.25186538696289.
                value: (v.f64(n)? * 1e4).round() / 1e4,
            })
        })
        .collect()
}

/// `corner` is `"LF"`...; iRacing numbers the tread from the left edge.
fn tyre(v: &VarView<'_>, corner: &str) -> Tyre {
    let across = |what: &str| {
        let [l, m, r] = ["L", "M", "R"].map(|edge| v.f64(&format!("{corner}{what}{edge}")));
        if corner.starts_with('L') {
            [l, m, r]
        } else {
            [r, m, l]
        }
    };
    Tyre {
        cold_kpa: v.f64(&format!("{corner}coldPressure")),
        hot_kpa: v.f64(&format!("{corner}pressure")),
        temp_c: across("tempC"),
        tread: across("wear"),
        distance_m: v.f64(&format!("{corner}odometer")),
    }
}

/// Only in the telemetry files, not in shared memory.
fn wheel(v: &VarView<'_>, corner: &str) -> Wheel {
    let of = |what: &str| v.f64(&format!("{corner}{what}"));
    let [l, m, r] = ["tempL", "tempM", "tempR"].map(of);
    Wheel {
        speed_mps: of("speed"),
        pressure_kpa: of("pressure"),
        surface_c: if corner.starts_with('L') {
            [l, m, r]
        } else {
            [r, m, l]
        },
        ride_height_m: of("rideHeight"),
        // Some cars have third springs ("SH") instead.
        shock_mps: of("shockVel").or_else(|| of("SHshockVel")),
        brake_bar: of("brakeLinePress"),
    }
}

fn wheels(v: &VarView<'_>) -> Option<Wheels> {
    v.f64("LFspeed")?;
    Some(Wheels {
        lf: wheel(v, "LF"),
        rf: wheel(v, "RF"),
        lr: wheel(v, "LR"),
        rr: wheel(v, "RR"),
    })
}

fn car_telemetry(v: &VarView<'_>, si: &SessionInfo) -> CarTelemetry {
    let delta = |name: &str| {
        v.bool(&format!("{name}_OK"))
            .unwrap_or(false)
            .then(|| v.f64(name))
            .flatten()
    };
    CarTelemetry {
        clutch: v.f64("Clutch"),
        steering_rad: v.f64("SteeringWheelAngle"),
        yaw_rate_rps: v.f64("YawRate"),
        lat_accel_mps2: v.f64("LatAccel"),
        long_accel_mps2: v.f64("LongAccel"),
        velocity_x_mps: v.f64("VelocityX"),
        velocity_y_mps: v.f64("VelocityY"),
        abs_active: v.bool("BrakeABSactive"),
        oil_press_bar: v.f64("OilPress"),
        fuel_press_bar: v.f64("FuelPress"),
        oil_level_l: v.f64("OilLevel"),
        water_level_l: v.f64("WaterLevel"),
        voltage: v.f64("Voltage"),
        warnings: warnings(v.u32("EngineWarnings").unwrap_or(0)),
        shift_rpm: v.f64("PlayerCarSLShiftRPM").filter(|r| *r > 0.0),
        tyres: Tyres {
            lf: tyre(v, "LF"),
            rf: tyre(v, "RF"),
            lr: tyre(v, "LR"),
            rr: tyre(v, "RR"),
        },
        tyre_compound: compound(si, v.i32("PlayerTireCompound"), true),
        tyre_sets_used: v.i32("TireSetsUsed"),
        tyre_sets_available: limited(v.i32("TireSetsAvailable")),
        pit: PitState {
            open: v.bool("PitsOpen"),
            in_stall: v.bool("PlayerCarInPitStall"),
            service_active: v.bool("PitstopActive"),
            status: pit_status(v.i32("PlayerCarPitSvStatus")),
            speed_limit_mps: si.pit_speed_limit_mps,
            repair_left_s: v.f64("PitRepairLeft"),
            optional_repair_left_s: v.f64("PitOptRepairLeft"),
            tow_s: v.f64("PlayerCarTowTime"),
            fast_repairs_used: v.i32("FastRepairUsed"),
            fast_repairs_available: limited(v.i32("FastRepairAvailable")),
            requested: pit_request(v.u32("PitSvFlags").unwrap_or(0)),
            requested_fuel_l: v.f64("PitSvFuel"),
        },
        controls: controls(v),
        p2p_count: v.i32("P2P_Count"),
        p2p_active: v.bool("P2P_Status"),
        // irsdk: 3 is "active".
        drs_active: v.i32("DRS_Status").map(|s| s == 3),
        battery_pct: v.f64("EnergyERSBatteryPct"),
        weight_penalty_kg: v.f64("PlayerCarWeightPenalty"),
        power_adjust_pct: v.f64("PlayerCarPowerAdjust"),
        delta_to_session_best_s: delta("LapDeltaToSessionBestLap"),
        delta_to_optimal_s: delta("LapDeltaToOptimalLap"),
        alongside: alongside(v.i32("CarLeftRight")),
        team_incidents: v.i32("PlayerCarTeamIncidentCount"),
        wheels: wheels(v),
    }
}

/// The name of a compound; names are known for the player's car model only.
fn compound(si: &SessionInfo, index: Option<i32>, same_car: bool) -> Option<String> {
    let i = usize::try_from(index?).ok()?;
    match si.tyre_compounds.get(i) {
        Some(name) if same_car && !name.is_empty() => Some(name.clone()),
        _ => Some(format!("compound {i}")),
    }
}

/// Flags for one car, not for everybody.
fn personal_flags(bits: u32) -> Vec<Flag> {
    flags(bits & (BLACK | REPAIR | DISQUALIFY | BLUE))
}

pub fn build_frame(v: &VarView<'_>, si: &SessionInfo, tick: u64, timestamp_ms: i64) -> Frame {
    let session_num = v.i32("SessionNum").unwrap_or(0);
    let def = si.session(session_num);
    let kind = def.map(|d| d.kind).unwrap_or(SessionKind::Other);
    let player_idx = v
        .i32("PlayerCarIdx")
        .map(|i| i as usize)
        .or(si.player_car_idx);

    let player_car_name = si
        .drivers
        .iter()
        .find(|d| Some(d.car_idx) == player_idx)
        .and_then(|d| d.car_name.clone());
    let mut cars = Vec::new();
    for d in &si.drivers {
        if d.is_pace_car || d.is_spectator || d.car_idx >= MAX_CARS {
            continue;
        }
        let i = d.car_idx;
        let is_player = Some(i) == player_idx;
        // Recordings on disk (.ibt) have the player's own variables only.
        let own_i32 = |array: &str, own: &str| {
            if is_player && !v.has(array) {
                v.i32(own)
            } else {
                v.i32_at(array, i)
            }
        };
        let own_f64 = |array: &str, own: &str| {
            if is_player && !v.has(array) {
                v.f64(own)
            } else {
                v.f64_at(array, i)
            }
        };
        let pos = own_i32("CarIdxPosition", "PlayerCarPosition")
            .filter(|p| *p > 0)
            .map(|p| p as u32);
        let cpos = own_i32("CarIdxClassPosition", "PlayerCarClassPosition")
            .filter(|p| *p > 0)
            .map(|p| p as u32);
        cars.push(CarState {
            idx: i,
            number: d.number.clone(),
            driver: d.name.clone(),
            team: d.team.clone(),
            car_name: d.car_name.clone(),
            class_name: d.class_name.clone(),
            class_id: v.i32_at("CarIdxClass", i).filter(|c| *c > 0).or(d.class_id),
            rating: d.irating,
            is_player,
            position: pos,
            class_position: cpos,
            lap: own_i32("CarIdxLap", "Lap").filter(|l| *l >= 0),
            laps_completed: own_i32("CarIdxLapCompleted", "LapCompleted").filter(|l| *l >= 0),
            lap_dist_pct: own_f64("CarIdxLapDistPct", "LapDistPct")
                .filter(|p| (0.0..=1.0).contains(p)),
            on_pit_road: own_i32("CarIdxOnPitRoad", "OnPitRoad").is_some_and(|p| p != 0),
            surface: surface(own_i32("CarIdxTrackSurface", "PlayerTrackSurface")),
            last_lap_s: own_f64("CarIdxLastLapTime", "LapLastLapTime").filter(|t| *t > 0.0),
            best_lap_s: own_f64("CarIdxBestLapTime", "LapBestLapTime").filter(|t| *t > 0.0),
            gap_to_leader_s: if kind == SessionKind::Race {
                v.f64_at("CarIdxF2Time", i)
                    .filter(|t| *t >= 0.0 && pos.is_some())
            } else {
                None
            },
            est_lap_s: d.est_lap_s,
            extra: CarExtra {
                tyre_compound: compound(
                    si,
                    v.i32_at("CarIdxTireCompound", i),
                    d.car_name == player_car_name,
                ),
                best_lap_num: v.i32_at("CarIdxBestLapNum", i).filter(|l| *l > 0),
                p2p_count: v.i32_at("CarIdxP2P_Count", i),
                p2p_active: v.bool_at("CarIdxP2P_Status", i),
                fast_repairs_used: v.i32_at("CarIdxFastRepairsUsed", i),
                flags: personal_flags(v.i32_at("CarIdxSessionFlags", i).unwrap_or(0) as u32),
                gear: v.i32_at("CarIdxGear", i),
                rpm: v.f64_at("CarIdxRPM", i).filter(|r| *r >= 0.0),
            },
        });
    }

    let kg_per_l = si.fuel_kg_per_l.filter(|d| *d > 0.0);
    let player = PlayerTelemetry {
        on_track: v.bool("IsOnTrack").unwrap_or(false),
        speed_mps: v.f64("Speed"),
        rpm: v.f64("RPM"),
        gear: v.i32("Gear"),
        throttle: v.f64("Throttle"),
        brake: v.f64("Brake"),
        fuel_l: v.f64("FuelLevel"),
        fuel_capacity_l: si.fuel_capacity_l,
        // FuelUsePerHour is kg/h.
        fuel_use_lph: v
            .f64("FuelUsePerHour")
            .zip(kg_per_l)
            .map(|(kgh, d)| kgh / d),
        incidents: v.i32("PlayerCarMyIncidentCount"),
        current_lap_s: v.f64("LapCurrentLapTime"),
        delta_to_best_s: v
            .bool("LapDeltaToBestLap_OK")
            .unwrap_or(true)
            .then(|| v.f64("LapDeltaToBestLap"))
            .flatten(),
        water_temp_c: v.f64("WaterTemp"),
        oil_temp_c: v.f64("OilTemp"),
        car: car_telemetry(v, si),
    };

    Frame {
        game: "iracing".into(),
        tick,
        timestamp_ms,
        session: SessionState {
            session_num,
            kind,
            phase: phase(v.i32("SessionState")),
            time_s: v.f64("SessionTime").unwrap_or(0.0),
            time_remaining_s: sane_time(v.f64("SessionTimeRemain")),
            laps_remaining: sane_laps(v.i32("SessionLapsRemainEx")),
            laps_total: sane_laps(v.i32("SessionLapsTotal")).or(def.and_then(|d| d.laps)),
            flags: flags(v.u32("SessionFlags").unwrap_or(0)),
            track: si.track_name.clone().map(|name| TrackInfo {
                name,
                config: si.track_config.clone(),
                length_m: si.track_length_m,
                sectors: si.sectors.clone(),
            }),
            uid: v.i32("SessionUniqueID").map(i64::from),
            time_total_s: sane_time(v.f64("SessionTimeTotal")),
            time_of_day_s: v.f64("SessionTimeOfDay"),
        },
        player_car_idx: player_idx,
        player: Some(player),
        cars,
        weather: Some(Weather {
            air_temp_c: v.f64("AirTemp"),
            track_temp_c: v.f64("TrackTempCrew").or_else(|| v.f64("TrackTemp")),
            wetness: wetness(v.i32("TrackWetness")),
            precipitation: v.f64("Precipitation"),
            declared_wet: v.bool("WeatherDeclaredWet"),
            humidity: v.f64("RelativeHumidity"),
            wind_mps: v.f64("WindVel"),
            wind_dir_deg: v.f64("WindDir").map(|r| r.to_degrees().rem_euclid(360.0)),
            skies: skies(v.i32("Skies")),
            fog: v.f64("FogLevel"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_enums_and_sentinels() {
        let phases = [
            (None, SessionPhase::Unknown),
            (Some(0), SessionPhase::Unknown),
            (Some(1), SessionPhase::GetInCar),
            (Some(2), SessionPhase::Warmup),
            (Some(3), SessionPhase::ParadeLaps),
            (Some(4), SessionPhase::Racing),
            (Some(5), SessionPhase::Checkered),
            (Some(6), SessionPhase::CoolDown),
        ];
        for (v, want) in phases {
            assert_eq!(phase(v), want, "{v:?}");
        }
        let surfaces = [
            (Some(-1), TrackSurface::NotInWorld),
            (Some(0), TrackSurface::OffTrack),
            (Some(1), TrackSurface::InPitStall),
            (Some(2), TrackSurface::ApproachingPits),
            (Some(3), TrackSurface::OnTrack),
            (None, TrackSurface::NotInWorld),
        ];
        for (v, want) in surfaces {
            assert_eq!(surface(v), want, "{v:?}");
        }
        let wet = [
            "dry",
            "mostly dry",
            "very lightly wet",
            "lightly wet",
            "moderately wet",
            "very wet",
            "extremely wet",
        ];
        for (i, w) in wet.iter().enumerate() {
            assert_eq!(wetness(Some(i as i32 + 1)).as_deref(), Some(*w));
        }
        assert_eq!(wetness(Some(0)), None);
        assert_eq!(wetness(Some(8)), None);
        assert_eq!(wetness(None), None);

        assert_eq!(sane_time(Some(0.0)), Some(0.0));
        assert_eq!(sane_time(Some(-1.0)), None);
        assert_eq!(sane_time(Some(604_800.0)), None);
        assert_eq!(sane_time(Some(518_399.0)), Some(518_399.0));
        assert_eq!(
            sane_time(Some(518_400.0)),
            None,
            "6 days and more is 'unlimited'"
        );
        assert_eq!(sane_laps(Some(0)), Some(0));
        assert_eq!(sane_laps(Some(9_999)), Some(9_999));
        assert_eq!(sane_laps(Some(32_767)), None);
        assert_eq!(sane_laps(Some(-1)), None);
    }

    #[test]
    fn bits_and_codes_of_the_car_map() {
        assert_eq!(warnings(0), []);
        // What iRacing showed for a damaged car with the engine off.
        assert_eq!(
            warnings(414),
            [
                Warning::FuelPressure,
                Warning::OilPressure,
                Warning::Stalled,
                Warning::PitLimiter,
                Warning::Damage
            ]
        );
        assert_eq!(
            warnings(0x01 | 0x20 | 0x40),
            [Warning::WaterTemp, Warning::RevLimiter, Warning::OilTemp]
        );
        assert_eq!(warnings(0x80), [Warning::Damage]);
        assert_eq!(warnings(0x100), [Warning::Damage]);
        assert_eq!(warnings(0x200), []);
        assert_eq!(
            pit_request(79),
            ["lf tyre", "rf tyre", "lr tyre", "rr tyre", "fast repair"]
        );
        assert_eq!(pit_request(0x30), ["fuel", "windshield tearoff"]);
        let status = [
            (1, "in progress"),
            (2, "complete"),
            (100, "too far left"),
            (101, "too far right"),
            (102, "too far forward"),
            (103, "too far back"),
            (104, "bad angle"),
            (105, "cannot be repaired"),
        ];
        for (v, want) in status {
            assert_eq!(pit_status(Some(v)).as_deref(), Some(want));
        }
        assert_eq!(pit_status(Some(0)), None);
        assert_eq!(pit_status(None), None);
        let near = [
            "left",
            "right",
            "left and right",
            "two on the left",
            "two on the right",
        ];
        for (i, want) in near.iter().enumerate() {
            assert_eq!(alongside(Some(i as i32 + 2)).as_deref(), Some(*want));
        }
        assert_eq!(alongside(Some(0)), None);
        assert_eq!(alongside(Some(1)), None, "clear");
        assert_eq!(alongside(None), None);
        for (i, want) in ["clear", "partly cloudy", "mostly cloudy", "overcast"]
            .iter()
            .enumerate()
        {
            assert_eq!(skies(Some(i as i32)).as_deref(), Some(*want));
        }
        assert_eq!(skies(Some(4)), None);
        assert_eq!(limited(Some(0)), Some(0));
        assert_eq!(limited(Some(254)), Some(254));
        assert_eq!(limited(Some(255)), None, "unlimited");
        assert_eq!(limited(Some(-1)), None);
    }

    #[test]
    fn control_names_become_ids() {
        for (name, id) in [
            ("BrakeBias", "brake_bias"),
            ("TractionControl", "traction_control"),
            ("TractionControl2", "traction_control2"),
            ("ABS", "abs"),
            ("ThrottleShape", "throttle_shape"),
            ("AntiRollFront", "anti_roll_front"),
            ("MGUKDeployMode", "mguk_deploy_mode"),
            ("Boost_Level", "boost_level"),
        ] {
            assert_eq!(snake_case(name), id);
        }
    }

    #[test]
    fn personal_flags_leave_out_those_for_everybody() {
        assert_eq!(
            personal_flags(GREEN | YELLOW | BLUE | BLACK | REPAIR | DISQUALIFY),
            [Flag::Blue, Flag::Black, Flag::Repair, Flag::Disqualify]
        );
        assert_eq!(personal_flags(GREEN), []);
    }

    #[test]
    fn compounds_have_names_for_the_same_car_only() {
        let si = SessionInfo {
            tyre_compounds: vec!["Hard".into(), String::new()],
            ..Default::default()
        };
        assert_eq!(compound(&si, Some(0), true).as_deref(), Some("Hard"));
        assert_eq!(compound(&si, Some(0), false).as_deref(), Some("compound 0"));
        assert_eq!(compound(&si, Some(1), true).as_deref(), Some("compound 1"));
        assert_eq!(compound(&si, Some(7), true).as_deref(), Some("compound 7"));
        assert_eq!(compound(&si, Some(-1), true), None);
        assert_eq!(compound(&si, None, true), None);
    }

    #[test]
    fn every_flag_bit_maps() {
        let cases = [
            (CHECKERED, Flag::Checkered),
            (WHITE, Flag::White),
            (GREEN, Flag::Green),
            (YELLOW, Flag::Yellow),
            (RED, Flag::Red),
            (BLUE, Flag::Blue),
            (DEBRIS, Flag::Debris),
            (YELLOW_WAVING, Flag::YellowWaving),
            (ONE_LAP_TO_GREEN, Flag::OneLapToGreen),
            (CAUTION, Flag::Caution),
            (BLACK, Flag::Black),
            (DISQUALIFY, Flag::Disqualify),
            (REPAIR, Flag::Repair),
        ];
        for (bit, f) in cases {
            assert_eq!(flags(bit), vec![f], "{bit:#x}");
        }
        assert!(flags(0).is_empty());
    }

    #[test]
    fn maps_flags() {
        let f = flags(GREEN | BLUE | 0x1000_0000);
        assert_eq!(f, vec![Flag::Green, Flag::Blue]);
        assert_eq!(flags(CAUTION_WAVING), vec![Flag::Caution]);
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::layout::testutil::{Image, Var};
    use crate::layout::{Header, VarTable};

    const YAML: &str = r#"
WeekendInfo:
 TrackDisplayName: Test Ring
SessionInfo:
 Sessions:
 - SessionNum: 0
   SessionLaps: 15
   SessionTime: unlimited
   SessionType: Race
 - SessionNum: 1
   SessionLaps: unlimited
   SessionTime: 600.0 sec
   SessionType: Practice
DriverInfo:
 DriverCarIdx: 1
 DriverCarFuelKgPerLtr: 0.0
 Drivers:
 - CarIdx: 0
   UserName: Pace
   CarIsPaceCar: 1
 - CarIdx: 1
   UserName: Me
   CarNumber: "1"
   CarClassID: 11
 - CarIdx: 2
   UserName: Spec
   IsSpectator: 1
 - CarIdx: 3
   UserName: Other
   CarNumber: "3"
   CarClassID: 11
 - CarIdx: 64
   UserName: Out Of Range
"#;

    fn vars(with_player_idx: bool) -> Vec<Var> {
        let one = |name, ty| Var { name, ty, count: 1 };
        let arr = |name, ty| Var {
            name,
            ty,
            count: 64,
        };
        let mut v = vec![
            one("SessionNum", 2),
            one("SessionTime", 5),
            one("FuelUsePerHour", 4),
            one("LapDeltaToBestLap", 4),
            one("LapDeltaToBestLap_OK", 1),
            one("TrackTemp", 4),
            arr("CarIdxPosition", 2),
            arr("CarIdxClassPosition", 2),
            arr("CarIdxClass", 2),
            arr("CarIdxLap", 2),
            arr("CarIdxLapCompleted", 2),
            arr("CarIdxLapDistPct", 4),
            arr("CarIdxLastLapTime", 4),
            arr("CarIdxBestLapTime", 4),
            arr("CarIdxF2Time", 4),
        ];
        if with_player_idx {
            v.push(one("PlayerCarIdx", 2));
        }
        v
    }

    fn build(img: &Image) -> Frame {
        let h = Header::parse(&img.bytes).unwrap();
        let vt = VarTable::parse(
            &img.bytes[h.var_header_offset as usize..],
            h.num_vars as usize,
        );
        let si = SessionInfo::parse(YAML).unwrap();
        let (_, b) = h.latest_buf().unwrap();
        let data = &img.bytes[b.offset as usize..b.offset as usize + h.buf_len as usize];
        build_frame(&vt.view(data), &si, 7, 1234)
    }

    #[test]
    fn wheels_are_there_in_telemetry_files_only() {
        let names = [
            "LFspeed",
            "LFpressure",
            "LFtempL",
            "LFtempR",
            "RFtempL",
            "RFtempR",
            "LFrideHeight",
            "LFSHshockVel",
            "LRshockVel",
            "LRbrakeLinePress",
        ];
        let vars = names.map(|name| Var {
            name,
            ty: 4,
            count: 1,
        });
        let mut img = Image::new(&vars, YAML);
        for (i, name) in names.into_iter().enumerate() {
            img.set_f32(name, 0, 1.0 + i as f32);
        }
        let w = build(&img).player.unwrap().car.wheels.unwrap();
        assert_eq!((w.lf.speed_mps, w.lf.pressure_kpa), (Some(1.0), Some(2.0)));
        assert_eq!(w.lf.surface_c, [Some(3.0), None, Some(4.0)]);
        assert_eq!(w.rf.surface_c, [Some(6.0), None, Some(5.0)]);
        assert_eq!((w.lf.ride_height_m, w.lf.shock_mps), (Some(7.0), Some(8.0)));
        assert_eq!((w.lr.shock_mps, w.lr.brake_bar), (Some(9.0), Some(10.0)));
        assert_eq!(w.rr, Wheel::default());
        // Shared memory.
        let img = Image::new(&vars[1..], YAML);
        assert_eq!(build(&img).player.unwrap().car.wheels, None);
    }

    #[test]
    fn the_car_in_detail() {
        let one = |name, ty| Var { name, ty, count: 1 };
        let vars = [
            one("PlayerCarIdx", 2),
            one("SessionUniqueID", 2),
            one("LFtempCL", 4),
            one("LFtempCR", 4),
            one("RFtempCL", 4),
            one("RFtempCR", 4),
            one("RFwearM", 4),
            one("LRcoldPressure", 4),
            one("dcBrakeBias", 4),
            one("dcTractionControl", 4),
            one("dcDashPage", 4),
            one("dcStarter", 1),
            one("P2P_Count", 2),
            one("DRS_Status", 2),
            one("FastRepairAvailable", 2),
            one("LapDeltaToOptimalLap", 4),
            one("LapDeltaToSessionBestLap", 4),
            one("LapDeltaToSessionBestLap_OK", 1),
            one("WindDir", 4),
            one("WeatherDeclaredWet", 1),
        ];
        let mut img = Image::new(&vars, YAML);
        img.set_i32("PlayerCarIdx", 0, 1);
        img.set_i32("SessionUniqueID", 0, 3);
        img.set_f32("LFtempCL", 0, 88.5);
        img.set_f32("LFtempCR", 0, 97.5);
        img.set_f32("RFtempCL", 0, 99.5);
        img.set_f32("RFtempCR", 0, 90.5);
        img.set_f32("RFwearM", 0, 0.5);
        img.set_f32("LRcoldPressure", 0, 131.0);
        img.set_f32("dcBrakeBias", 0, 57.25);
        img.set_f32("dcTractionControl", 0, 2.0);
        img.set_f32("dcDashPage", 0, 1.0);
        img.set_i32("P2P_Count", 0, 138);
        img.set_i32("DRS_Status", 0, 3);
        img.set_i32("FastRepairAvailable", 0, 255);
        img.set_f32("LapDeltaToOptimalLap", 0, 1.0);
        img.set_f32("LapDeltaToSessionBestLap", 0, -0.5);
        img.set_bool("LapDeltaToSessionBestLap_OK", 0, true);
        img.set_f32("WindDir", 0, -std::f32::consts::FRAC_PI_2);
        img.set_bool("WeatherDeclaredWet", 0, true);
        let f = build(&img);
        assert_eq!(f.session.uid, Some(3));
        let c = f.player.unwrap().car;
        // Outer, middle, inner: the left edge is outside on the left only.
        assert_eq!(c.tyres.lf.temp_c, [Some(88.5), None, Some(97.5)]);
        assert_eq!(c.tyres.rf.temp_c, [Some(90.5), None, Some(99.5)]);
        assert_eq!(c.tyres.rf.tread, [None, Some(0.5), None]);
        assert_eq!(c.tyres.rf.min_tread(), Some(0.5));
        assert_eq!(c.tyres.lf.mean_temp_c(), Some(93.0));
        assert_eq!(c.tyres.lr.mean_temp_c(), None);
        assert_eq!(c.tyres.lr.cold_kpa, Some(131.0));
        let controls: Vec<(&str, f64)> = c
            .controls
            .iter()
            .map(|c| (c.id.as_str(), c.value))
            .collect();
        assert_eq!(controls, [("brake_bias", 57.25), ("traction_control", 2.0)]);
        assert_eq!(c.p2p_count, Some(138));
        assert_eq!(c.drs_active, Some(true));
        assert_eq!(c.pit.fast_repairs_available, None);
        assert_eq!(c.delta_to_session_best_s, Some(-0.5));
        assert_eq!(c.delta_to_optimal_s, None, "not marked valid");
        let w = f.weather.unwrap();
        assert_eq!(w.wind_dir_deg.map(f64::round), Some(270.0));
        assert_eq!(w.declared_wet, Some(true));
        assert_eq!(w.humidity, None);
    }

    #[test]
    fn filters_cars_and_uses_session_info_fallbacks() {
        let img = Image::new(&vars(false), YAML);
        let f = build(&img);
        assert_eq!(f.tick, 7);
        assert_eq!(f.timestamp_ms, 1234);
        let idx: Vec<usize> = f.cars.iter().map(|c| c.idx).collect();
        assert_eq!(idx, vec![1, 3], "pace car, spectator and idx >= 64 dropped");
        // No PlayerCarIdx variable: falls back to DriverInfo.
        assert_eq!(f.player_car_idx, Some(1));
        assert!(f.cars[0].is_player && !f.cars[1].is_player);
        // Zero class from telemetry -> class from session info.
        assert_eq!(f.cars[0].class_id, Some(11));
        // laps_total from the session definition when the variable is missing.
        assert_eq!(f.session.laps_total, Some(15));
        assert_eq!(f.session.kind, SessionKind::Race);
        // Zeroed telemetry means "no data" for positions and lap times.
        let c = &f.cars[0];
        assert_eq!((c.position, c.class_position), (None, None));
        assert_eq!((c.last_lap_s, c.best_lap_s), (None, None));
        assert_eq!(c.gap_to_leader_s, None, "no position -> no F2 gap");
        assert_eq!(c.lap, Some(0));
        assert_eq!(c.laps_completed, Some(0));
        assert_eq!(c.lap_dist_pct, Some(0.0));
        // Fuel density 0 -> no L/h conversion.
        assert_eq!(f.player.as_ref().unwrap().fuel_use_lph, None);
        assert_eq!(
            f.weather.as_ref().unwrap().track_temp_c,
            Some(0.0),
            "TrackTemp fallback"
        );
    }

    #[test]
    fn sentinel_values_are_normalized() {
        let mut img = Image::new(&vars(true), YAML);
        img.set_i32("PlayerCarIdx", 0, 3);
        img.set_f32("FuelUsePerHour", 0, 30.0);
        img.set_f32("LapDeltaToBestLap", 0, -0.4);
        img.set_bool("LapDeltaToBestLap_OK", 0, false);
        // Car 1: valid values at the edges.
        img.set_i32("CarIdxPosition", 1, 1);
        img.set_i32("CarIdxClassPosition", 1, 1);
        img.set_i32("CarIdxClass", 1, 99);
        img.set_i32("CarIdxLap", 1, 0);
        img.set_f32("CarIdxLapDistPct", 1, 1.0);
        img.set_f32("CarIdxLastLapTime", 1, 0.001);
        img.set_f32("CarIdxBestLapTime", 1, 0.001);
        img.set_f32("CarIdxF2Time", 1, 0.0);
        // Car 3: invalid sentinels.
        img.set_i32("CarIdxPosition", 3, -1);
        img.set_i32("CarIdxClassPosition", 3, -1);
        img.set_i32("CarIdxClass", 3, -1);
        img.set_i32("CarIdxLap", 3, -1);
        img.set_i32("CarIdxLapCompleted", 3, -1);
        img.set_f32("CarIdxLapDistPct", 3, -1.0);
        img.set_f32("CarIdxLastLapTime", 3, -1.0);
        img.set_f32("CarIdxBestLapTime", 3, -1.0);
        img.set_f32("CarIdxF2Time", 3, -1.0);
        let f = build(&img);
        assert_eq!(f.player_car_idx, Some(3), "telemetry PlayerCarIdx wins");
        let p = f.player.as_ref().unwrap();
        assert_eq!(p.delta_to_best_s, None, "delta not valid");
        let a = f.car(1).unwrap();
        assert_eq!(a.position, Some(1));
        assert_eq!(a.class_position, Some(1));
        assert_eq!(a.class_id, Some(99), "telemetry class wins when set");
        assert_eq!(a.lap, Some(0));
        assert_eq!(a.lap_dist_pct, Some(1.0));
        assert!(a.last_lap_s.unwrap() > 0.0 && a.best_lap_s.unwrap() > 0.0);
        assert_eq!(a.gap_to_leader_s, Some(0.0), "leader gap 0 is valid");
        let b = f.car(3).unwrap();
        assert_eq!(
            (b.position, b.class_position, b.lap, b.laps_completed),
            (None, None, None, None)
        );
        assert_eq!(b.class_id, Some(11));
        assert_eq!(
            (
                b.lap_dist_pct,
                b.last_lap_s,
                b.best_lap_s,
                b.gap_to_leader_s
            ),
            (None, None, None, None)
        );

        // Lap dist above 1 is invalid; F2 time is ignored outside races.
        img.set_f32("CarIdxLapDistPct", 1, 1.01);
        img.set_i32("SessionNum", 0, 1);
        let f = build(&img);
        assert_eq!(f.session.kind, SessionKind::Practice);
        assert_eq!(f.session.laps_total, None, "unlimited laps");
        let a = f.car(1).unwrap();
        assert_eq!(a.lap_dist_pct, None);
        assert_eq!(a.gap_to_leader_s, None);

        // Delta is reported when valid.
        img.set_bool("LapDeltaToBestLap_OK", 0, true);
        let f = build(&img);
        assert!((f.player.unwrap().delta_to_best_s.unwrap() + 0.4).abs() < 1e-6);
    }

    #[test]
    fn unknown_session_number_is_other() {
        let mut img = Image::new(&vars(false), YAML);
        img.set_i32("SessionNum", 0, 9);
        let f = build(&img);
        assert_eq!(f.session.kind, SessionKind::Other);
        assert_eq!(f.session.laps_total, None);
    }
}
