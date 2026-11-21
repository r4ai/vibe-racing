//! iRacing variables → normalized [`vr_core::Frame`].

use vr_core::{
    CarState, Flag, Frame, PlayerTelemetry, SessionKind, SessionPhase, SessionState, TrackInfo,
    TrackSurface, Weather,
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

pub fn build_frame(v: &VarView<'_>, si: &SessionInfo, tick: u64, timestamp_ms: i64) -> Frame {
    let session_num = v.i32("SessionNum").unwrap_or(0);
    let def = si.session(session_num);
    let kind = def.map(|d| d.kind).unwrap_or(SessionKind::Other);
    let player_idx = v
        .i32("PlayerCarIdx")
        .map(|i| i as usize)
        .or(si.player_car_idx);

    let mut cars = Vec::new();
    for d in &si.drivers {
        if d.is_pace_car || d.is_spectator || d.car_idx >= MAX_CARS {
            continue;
        }
        let i = d.car_idx;
        let pos = v
            .i32_at("CarIdxPosition", i)
            .filter(|p| *p > 0)
            .map(|p| p as u32);
        let cpos = v
            .i32_at("CarIdxClassPosition", i)
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
            is_player: Some(i) == player_idx,
            position: pos,
            class_position: cpos,
            lap: v.i32_at("CarIdxLap", i).filter(|l| *l >= 0),
            laps_completed: v.i32_at("CarIdxLapCompleted", i).filter(|l| *l >= 0),
            lap_dist_pct: v
                .f64_at("CarIdxLapDistPct", i)
                .filter(|p| (0.0..=1.0).contains(p)),
            on_pit_road: v.bool_at("CarIdxOnPitRoad", i).unwrap_or(false),
            surface: surface(v.i32_at("CarIdxTrackSurface", i)),
            last_lap_s: v.f64_at("CarIdxLastLapTime", i).filter(|t| *t > 0.0),
            best_lap_s: v.f64_at("CarIdxBestLapTime", i).filter(|t| *t > 0.0),
            gap_to_leader_s: if kind == SessionKind::Race {
                v.f64_at("CarIdxF2Time", i)
                    .filter(|t| *t >= 0.0 && pos.is_some())
            } else {
                None
            },
            est_lap_s: d.est_lap_s,
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
            }),
        },
        player_car_idx: player_idx,
        player: Some(player),
        cars,
        weather: Some(Weather {
            air_temp_c: v.f64("AirTemp"),
            track_temp_c: v.f64("TrackTempCrew").or_else(|| v.f64("TrackTemp")),
            wetness: wetness(v.i32("TrackWetness")),
            precipitation: v.f64("Precipitation"),
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
