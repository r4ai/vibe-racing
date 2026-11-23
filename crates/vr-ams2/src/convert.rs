//! Automobilista 2 shared memory → normalized [`vr_core::Frame`].

use vr_core::{
    CarExtra, CarState, CarTelemetry, Control, Damage, Flag, Frame, PitState, PlayerTelemetry,
    SessionKind, SessionPhase, SessionState, TrackInfo, TrackSurface, Tyre, Tyres, Warning,
    Weather, Wheel, Wheels,
};

use crate::layout::{PARTICIPANTS, Snapshot};
use crate::tracker::{
    GAME_INGAME_INMENU_TIME_TICKING, GAME_INGAME_PAUSED, GAME_INGAME_PLAYING, Tracker,
};

// (Type#2) Session state
const SESSION_PRACTICE: u32 = 1;
const SESSION_TEST: u32 = 2;
const SESSION_QUALIFY: u32 = 3;
const SESSION_FORMATION_LAP: u32 = 4;
const SESSION_RACE: u32 = 5;
const SESSION_TIME_ATTACK: u32 = 6;

// (Type#3) Race state
const RACESTATE_NOT_STARTED: u32 = 1;
const RACESTATE_RACING: u32 = 2;
const RACESTATE_FINISHED: u32 = 3;
const RACESTATE_DISQUALIFIED: u32 = 4;
const RACESTATE_RETIRED: u32 = 5;
const RACESTATE_DNF: u32 = 6;

// (Type#7) Pit mode
const PIT_MODE_DRIVING_INTO_PITS: u32 = 1;
const PIT_MODE_IN_PIT: u32 = 2;
const PIT_MODE_IN_GARAGE: u32 = 4;
const PIT_MODE_DRIVING_OUT_OF_GARAGE: u32 = 5;

// (Type#8) Pit schedule
const PIT_SCHEDULE_DRIVE_THROUGH: u32 = 5;
const PIT_SCHEDULE_STOP_GO: u32 = 6;

// (Type#9) Car flags
const CAR_ENGINE_ACTIVE: u32 = 0x02;
const CAR_ENGINE_WARNING: u32 = 0x04;
const CAR_SPEED_LIMITER: u32 = 0x08;
const CAR_ABS: u32 = 0x10;
const CAR_TCS: u32 = 0x40;

// (Type#10) Tyre flags
const TYRE_ATTACHED: u32 = 0x01;
/// Attached and inflated (`TYRE_INFLATED` is 0x02).
const TYRE_SOUND: u32 = 0x03;

// (Type#12) Crash state
const CRASH_DAMAGE_OFFTRACK: u32 = 1;

// (Type#14) DRS state
const DRS_INSTALLED: u32 = 0x01;
/// Available on the next zone (0x04) or now (0x08).
const DRS_AVAILABLE: u32 = 0x0c;
const DRS_ACTIVE: u32 = 0x10;

// (Type#16) Yellow flag state (full course yellow)
const YFS_PENDING: i32 = 1;
const YFS_PITS_CLOSED: i32 = 2;
const YFS_PIT_LEAD_LAP: i32 = 3;
const YFS_LAST_LAP: i32 = 6;
const YFS_RESUME: i32 = 7;
const YFS_RACE_HALT: i32 = 8;

const PSI_KPA: f64 = 6.894_757;
const KELVIN: f64 = 273.15;
/// Damage from which the car wants mending.
const DAMAGE_WARNING: f64 = 0.2;
/// A wheel rolls freely below this throttle and side force, off the brake.
const FREE_THROTTLE: f64 = 0.25;
const FREE_LAT_MPS2: f64 = 3.0;

fn kind(state: Option<u32>) -> SessionKind {
    match state {
        Some(SESSION_PRACTICE | SESSION_TEST | SESSION_TIME_ATTACK) => SessionKind::Practice,
        Some(SESSION_QUALIFY) => SessionKind::Qualify,
        Some(SESSION_FORMATION_LAP | SESSION_RACE) => SessionKind::Race,
        _ => SessionKind::Other,
    }
}

fn phase(session: Option<u32>, race: Option<u32>) -> SessionPhase {
    if session == Some(SESSION_FORMATION_LAP) {
        return SessionPhase::ParadeLaps;
    }
    match race {
        Some(RACESTATE_NOT_STARTED) => SessionPhase::GetInCar,
        Some(RACESTATE_RACING) => SessionPhase::Racing,
        Some(RACESTATE_FINISHED | RACESTATE_DISQUALIFIED | RACESTATE_RETIRED | RACESTATE_DNF) => {
            SessionPhase::Checkered
        }
        _ => SessionPhase::Unknown,
    }
}

/// (Type#5) Flag colours.
fn colour_flags(colour: u32) -> Vec<Flag> {
    match colour {
        1 => vec![Flag::Green],
        2 => vec![Flag::Blue],
        3 => vec![Flag::SlowCar],
        4 => vec![Flag::White],
        5 => vec![Flag::Red],
        6 => vec![Flag::YellowWaving],
        7 => vec![Flag::Yellow, Flag::YellowWaving],
        8 => vec![Flag::BlackWhite],
        9 => vec![Flag::Repair],
        // AMS2 shows black to a disqualified car.
        10 => vec![Flag::Disqualify],
        11 => vec![Flag::Checkered],
        _ => Vec::new(),
    }
}

/// Full course yellow.
fn caution_flags(state: Option<i32>) -> Vec<Flag> {
    match state {
        Some(YFS_LAST_LAP | YFS_RESUME) => vec![Flag::Caution, Flag::OneLapToGreen],
        Some(YFS_RACE_HALT) => vec![Flag::Caution, Flag::Red],
        Some(s) if (YFS_PENDING..YFS_LAST_LAP).contains(&s) => vec![Flag::Caution],
        _ => Vec::new(),
    }
}

fn penalty(schedule: Option<u32>) -> bool {
    matches!(
        schedule,
        Some(PIT_SCHEDULE_DRIVE_THROUGH | PIT_SCHEDULE_STOP_GO)
    )
}

fn sorted(mut v: Vec<Flag>) -> Vec<Flag> {
    v.sort();
    v.dedup();
    v
}

/// (Type#8) Pit schedule.
fn planned(schedule: Option<u32>) -> Option<String> {
    Some(
        match schedule? {
            1 => "player requested",
            2 => "engineer requested",
            3 => "damage",
            4 => "mandatory",
            5 => "drive through",
            6 => "stop and go",
            7 => "pit spot occupied",
            _ => return None,
        }
        .to_string(),
    )
}

fn on_pit_road(mode: Option<u32>) -> bool {
    matches!(
        mode,
        Some(PIT_MODE_DRIVING_INTO_PITS..=PIT_MODE_DRIVING_OUT_OF_GARAGE)
    )
}

fn surface(mode: Option<u32>) -> TrackSurface {
    match mode {
        Some(PIT_MODE_IN_PIT | PIT_MODE_IN_GARAGE) => TrackSurface::InPitStall,
        Some(PIT_MODE_DRIVING_INTO_PITS) => TrackSurface::ApproachingPits,
        _ => TrackSurface::OnTrack,
    }
}

/// (Type#11) Terrain materials a car races on (CrewChief's list, plus the
/// tarmac run-offs, which some tracks use on the racing line too).
fn racing_surface(t: u32) -> bool {
    // Road, low grip, three bumpy roads, marbles, rumble strips, drains,
    // pavement, exit rumble strips, cobbles, sand road, bumpy dirt road,
    // run-off road.
    matches!(t, 0..=5 | 10 | 11 | 18 | 19 | 21 | 25 | 29 | 30 | 46)
}

/// Stable small id for a class name.
fn class_id(name: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    // Positive and never 0 (0 reads as "no class" elsewhere).
    (h % 0x7fff_fffe) as i32 + 1
}

fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|x| *x > 0.0)
}

fn some_text(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

/// Tyre pressure in kPa; the header says PSI, older builds gave kPa.
fn kpa(v: Option<f64>) -> Option<f64> {
    positive(v).map(|p| if p < 70.0 { p * PSI_KPA } else { p })
}

/// Ride height in metres; the header says cm, recordings show metres.
fn metres(v: Option<f64>) -> Option<f64> {
    v.map(|h| if h.abs() > 1.0 { h / 100.0 } else { h })
}

fn celsius(kelvin: Option<f64>) -> Option<f64> {
    positive(kelvin).map(|k| k - KELVIN)
}

/// The left and right edge as outer and inner: swapped on the right tyres.
fn across(i: usize, left: Option<f64>, mid: Option<f64>, right: Option<f64>) -> [Option<f64>; 3] {
    if i.is_multiple_of(2) {
        [left, mid, right]
    } else {
        [right, mid, left]
    }
}

fn tyre(s: &Snapshot<'_>, i: usize) -> Tyre {
    let t = |name: &str| s.f64_at(name, i).filter(|x| *x != 0.0);
    let mid = t("mTyreTempCenter").or_else(|| t("mTyreTemp"));
    Tyre {
        cold_kpa: None,
        hot_kpa: kpa(s.f64_at("mAirPressure", i)),
        temp_c: across(i, t("mTyreTempLeft"), mid, t("mTyreTempRight")),
        tread: [
            None,
            s.f64_at("mTyreWear", i).map(|w| (1.0 - w).clamp(0.0, 1.0)),
            None,
        ],
        distance_m: None,
        carcass_c: celsius(s.f64_at("mTyreCarcassTemp", i)),
        brake_c: s.f64_at("mBrakeTempCelsius", i),
    }
}

fn compound(s: &Snapshot<'_>) -> Option<String> {
    let names: Vec<String> = (0..4)
        .filter_map(|i| some_text(s.str_at("mTyreCompound", i)))
        .collect();
    let (front, rear) = (names.first()?, names.last()?);
    Some(if front == rear {
        front.clone()
    } else {
        format!("{front}/{rear}")
    })
}

fn wheels(s: &Snapshot<'_>, t: &mut Tracker, car_mps: f64, free: bool) -> Wheels {
    let mut w = std::array::from_fn::<Wheel, 4, _>(|i| Wheel {
        speed_mps: None,
        pressure_kpa: kpa(s.f64_at("mAirPressure", i)),
        surface_c: across(
            i,
            s.f64_at("mTyreTempLeft", i),
            s.f64_at("mTyreTempCenter", i),
            s.f64_at("mTyreTempRight", i),
        ),
        ride_height_m: metres(s.f64_at("mRideHeight", i)),
        shock_mps: s.f64_at("mSuspensionVelocity", i),
        brake_bar: None,
    });
    for (i, wheel) in w.iter_mut().enumerate() {
        if let Some(rps) = s.f64_at("mTyreRPS", i) {
            wheel.speed_mps = t.wheel_speed(i, rps, car_mps, free);
        }
    }
    let [lf, rf, lr, rr] = w;
    Wheels { lf, rf, lr, rr }
}

fn damage(s: &Snapshot<'_>) -> Damage {
    Damage {
        aero: s.f64("mAeroDamage"),
        engine: s.f64("mEngineDamage"),
        suspension: s.tyres("mSuspensionDamage"),
        brakes: s.tyres("mBrakeDamage"),
    }
}

fn controls(s: &Snapshot<'_>) -> Vec<Control> {
    let mut out = Vec::new();
    let mut put = |id: &str, v: Option<f64>| {
        if let Some(v) = v {
            out.push(Control {
                id: id.into(),
                value: (v * 1e4).round() / 1e4,
            });
        }
    };
    put(
        "brake_bias",
        s.f64("mBrakeBias").filter(|b| *b >= 0.0).map(|b| b * 100.0),
    );
    put("abs", s.f64("mAntiLockSetting").filter(|v| *v >= 0.0));
    put(
        "traction_control",
        s.f64("mTractionControlSetting").filter(|v| *v >= 0.0),
    );
    put("ers_mode", s.f64("mErsDeploymentMode").filter(|v| *v > 0.0));
    out
}

/// Cars beside the player, as a spotter calls them. The game's frame: Y is
/// up, the car points to −Z and +X is its left (as CrewChief reads it).
fn alongside(s: &Snapshot<'_>, player: usize, cars: &[CarState]) -> Option<String> {
    const LENGTH_M: f64 = 4.5;
    const WIDE_M: (f64, f64) = (1.5, 6.0);
    const LEVEL_M: f64 = 3.0;
    let pos = |i: usize| -> Option<[f64; 3]> {
        Some([
            s.part_f64(i, "mWorldPosition", 0)?,
            s.part_f64(i, "mWorldPosition", 1)?,
            s.part_f64(i, "mWorldPosition", 2)?,
        ])
    };
    let me = pos(player)?;
    let yaw = s.f64_at("mOrientation", 1)?;
    let (sin, cos) = yaw.sin_cos();
    let (mut left, mut right) = (0, 0);
    for c in cars.iter().filter(|c| c.idx != player && !c.on_pit_road) {
        let Some(p) = pos(c.idx) else { continue };
        let (dx, dy, dz) = (p[0] - me[0], p[1] - me[1], p[2] - me[2]);
        let side = cos * dx - sin * dz;
        let behind = sin * dx + cos * dz;
        if dy.abs() > LEVEL_M || behind.abs() > LENGTH_M {
            continue;
        }
        if (WIDE_M.0..WIDE_M.1).contains(&side) {
            left += 1;
        } else if (WIDE_M.0..WIDE_M.1).contains(&-side) {
            right += 1;
        }
    }
    Some(
        match (left, right) {
            (0, 0) => return None,
            (l, r) if l > 0 && r > 0 => "left and right",
            (1, 0) => "left",
            (_, 0) => "two on the left",
            (0, 1) => "right",
            _ => "two on the right",
        }
        .to_string(),
    )
}

fn class_positions(cars: &mut [CarState]) {
    let keys: Vec<(Option<i32>, Option<u32>)> =
        cars.iter().map(|c| (c.class_id, c.position)).collect();
    for c in cars.iter_mut() {
        let Some(me) = c.position else { continue };
        let ahead = keys
            .iter()
            .filter(|(class, pos)| *class == c.class_id && pos.is_some_and(|p| p < me))
            .count();
        c.class_position = Some(ahead as u32 + 1);
    }
}

/// The sample as normalized telemetry. `now_s` is a monotonic clock.
pub fn build_frame(
    s: &Snapshot<'_>,
    t: &mut Tracker,
    tick: u64,
    timestamp_ms: i64,
    now_s: f64,
) -> Frame {
    let m = t.begin(s, now_s);
    let session_state = s.u32("mSessionState");
    let kind = kind(session_state);
    let race = kind == SessionKind::Race;
    let length = positive(s.f64("mTrackLength"));
    let count = s
        .i32("mNumParticipants")
        .and_then(|n| usize::try_from(n).ok())
        .map_or(PARTICIPANTS, |n| n.min(PARTICIPANTS));
    let game = s.u32("mGameState").unwrap_or(0);
    let num_sectors = s.i32("mNumSectors").and_then(|n| usize::try_from(n).ok());

    let mut cars = Vec::new();
    for i in 0..count {
        if s.part_u32(i, "mIsActive") != Some(1) {
            continue;
        }
        let is_player = Some(i) == m.player;
        let laps = s.part_u32(i, "mLapsCompleted");
        let pct = s
            .part_f64(i, "mCurrentLapDistance", 0)
            .zip(length)
            .map(|(d, l)| (d / l).clamp(0.0, 1.0));
        let pit_mode = s.u32_at("mPitModes", i);
        let pit = on_pit_road(pit_mode);
        let gap = laps.and_then(|l| t.gap(i, l, m.time_s));
        if let (Some(sector), Some(p)) = (s.part_f64(i, "mCurrentSector", 0), pct)
            && !pit
        {
            t.sector(i, sector as i32, p);
        }
        let position = s.part_u32(i, "mRacePosition").filter(|p| *p > 0);
        let class_name = some_text(s.str_at("mCarClassNames", i)).or_else(|| {
            is_player
                .then(|| some_text(s.str("mCarClassName")))
                .flatten()
        });
        let race_state = s.u32_at("mRaceStates", i);
        let mut flags = colour_flags(s.u32_at("mHighestFlagColours", i).unwrap_or(0));
        if penalty(s.u32_at("mPitSchedules", i)) {
            flags.push(Flag::Black);
        }
        match race_state {
            Some(RACESTATE_FINISHED) => flags.push(Flag::Checkered),
            Some(RACESTATE_DISQUALIFIED) => flags.push(Flag::Disqualify),
            _ => {}
        }
        let sectors = |names: [&str; 3]| -> Vec<Option<f64>> {
            let v: Vec<Option<f64>> = names.iter().map(|n| positive(s.f64_at(n, i))).collect();
            if v.iter().any(Option::is_some) {
                v
            } else {
                Vec::new()
            }
        };
        cars.push(CarState {
            idx: i,
            number: (i + 1).to_string(),
            driver: s.part_str(i, "mName").unwrap_or_default(),
            team: None,
            car_name: some_text(s.str_at("mCarNames", i))
                .or_else(|| is_player.then(|| some_text(s.str("mCarName"))).flatten()),
            class_id: class_name.as_deref().map(class_id),
            class_name,
            rating: None,
            is_player,
            position,
            class_position: None,
            lap: s
                .part_u32(i, "mCurrentLap")
                .filter(|l| *l > 0)
                .map(|l| l as i32),
            laps_completed: laps.map(|l| l as i32),
            lap_dist_pct: pct,
            on_pit_road: pit,
            surface: surface(pit_mode),
            last_lap_s: positive(s.f64_at("mLastLapTimes", i))
                .or_else(|| is_player.then(|| positive(s.f64("mLastLapTime"))).flatten()),
            best_lap_s: positive(s.f64_at("mFastestLapTimes", i))
                .or_else(|| is_player.then(|| positive(s.f64("mBestLapTime"))).flatten()),
            gap_to_leader_s: gap.filter(|_| race && position.is_some()),
            est_lap_s: None,
            extra: CarExtra {
                tyre_compound: is_player.then(|| compound(s)).flatten(),
                flags: sorted(flags),
                speed_mps: s.f64_at("mSpeeds", i),
                lap_invalidated: if is_player {
                    s.bool("mLapInvalidated")
                } else {
                    s.bool_at("mLapsInvalidated", i)
                },
                retired: race_state.map(|r| matches!(r, RACESTATE_RETIRED | RACESTATE_DNF)),
                sector_s: sectors([
                    "mCurrentSector1Times",
                    "mCurrentSector2Times",
                    "mCurrentSector3Times",
                ]),
                best_sector_s: sectors([
                    "mFastestSector1Times",
                    "mFastestSector2Times",
                    "mFastestSector3Times",
                ]),
                ..Default::default()
            },
        });
    }
    class_positions(&mut cars);

    let speed = s.f64("mSpeed");
    let car_flags = s.u32("mCarFlags").unwrap_or(0);
    let local = |name: &str, i: usize| s.f64_at(name, i);
    let lat = local("mLocalAcceleration", 0);
    let throttle = s.f64("mThrottle");
    let brake = s.f64("mBrake");
    let free = throttle.is_some_and(|x| x < FREE_THROTTLE)
        && brake == Some(0.0)
        && lat.is_some_and(|a| a.abs() < FREE_LAT_MPS2);
    let terrain: Vec<u32> = (0..4).filter_map(|i| s.u32_at("mTerrain", i)).collect();
    let off_track = (terrain.len() == 4 && !terrain.iter().any(|t| racing_surface(*t)))
        || s.u32("mCrashState") == Some(CRASH_DAMAGE_OFFTRACK);
    let pit_mode = s.u32("mPitMode");
    let in_session = matches!(
        game,
        GAME_INGAME_PLAYING | GAME_INGAME_PAUSED | GAME_INGAME_INMENU_TIME_TICKING
    );
    let fuel_capacity = positive(s.f64("mFuelCapacity"));
    let fuel_l = s.f64("mFuelLevel").zip(fuel_capacity).map(|(f, c)| f * c);
    let damage = damage(s);
    let tyre_flags: Vec<u32> = (0..4).filter_map(|i| s.u32_at("mTyreFlags", i)).collect();
    let flat = tyre_flags.iter().any(|f| f & TYRE_ATTACHED != 0)
        && tyre_flags.iter().any(|f| f & TYRE_SOUND != TYRE_SOUND);
    let player_car = m.player.and_then(|p| cars.iter_mut().find(|c| c.idx == p));
    let on_track = in_session && pit_mode != Some(PIT_MODE_IN_GARAGE) && player_car.is_some();
    if let Some(c) = player_car {
        if off_track && !c.on_pit_road {
            c.surface = TrackSurface::OffTrack;
        }
        if c.car_name.is_none() {
            c.car_name = some_text(s.str("mCarName"));
        }
    }
    let mut warnings = Vec::new();
    let mut warn = |on: bool, w: Warning| {
        if on {
            warnings.push(w);
        }
    };
    warn(car_flags & CAR_ENGINE_WARNING != 0, Warning::Engine);
    warn(car_flags & CAR_SPEED_LIMITER != 0, Warning::PitLimiter);
    warn(
        on_track && car_flags & CAR_ENGINE_ACTIVE == 0 && s.f64("mRpm").is_some_and(|r| r < 50.0),
        Warning::Stalled,
    );
    warn(
        damage.worst().is_some_and(|d| d >= DAMAGE_WARNING),
        Warning::Damage,
    );
    warn(flat, Warning::FlatTyre);
    warn(s.bool("mClutchOverheated") == Some(true), Warning::Clutch);
    warnings.sort();

    let yellow = s.i32("mYellowFlagState");
    let leader_laps = cars
        .iter()
        .filter(|c| c.position == Some(1))
        .find_map(|c| c.laps_completed);
    let my_laps = cars
        .iter()
        .find(|c| c.is_player)
        .and_then(|c| c.laps_completed);
    let pits_open = match yellow {
        Some(YFS_PENDING | YFS_PITS_CLOSED) => Some(false),
        Some(YFS_PIT_LEAD_LAP) => Some(leader_laps.is_some() && leader_laps == my_laps),
        Some(y) if y >= 0 => Some(true),
        _ => None,
    };
    let drs = s.u32("mDrsState").filter(|d| d & DRS_INSTALLED != 0);
    let boost = s.f64("mBoostAmount").filter(|a| *a > 0.0);
    let boost_on = s.bool("mBoostActive").filter(|on| *on || boost.is_some());
    let schedule = s.u32("mPitSchedule");
    let fuel_use_lph = t.fuel_flow(m.time_s, fuel_l.filter(|_| on_track));
    let forward = |name: &str| local(name, 2).map(|v| -v);

    let car = CarTelemetry {
        clutch: s.f64("mClutch").map(|c| 1.0 - c),
        yaw_rate_rps: local("mAngularVelocity", 1).map(|v| -v),
        lat_accel_mps2: lat,
        long_accel_mps2: forward("mLocalAcceleration"),
        velocity_x_mps: forward("mLocalVelocity"),
        velocity_y_mps: local("mLocalVelocity", 0),
        abs_active: s
            .bool("mAntiLockActive")
            .map(|a| a || car_flags & CAR_ABS != 0),
        oil_press_bar: positive(s.f64("mOilPressureKPa")).map(|k| k / 100.0),
        fuel_press_bar: positive(s.f64("mFuelPressureKPa")).map(|k| k / 100.0),
        warnings,
        tyres: Tyres {
            lf: tyre(s, 0),
            rf: tyre(s, 1),
            lr: tyre(s, 2),
            rr: tyre(s, 3),
            live: true,
        },
        tyre_compound: compound(s),
        pit: PitState {
            open: pits_open,
            in_stall: pit_mode.map(|p| p == PIT_MODE_IN_PIT),
            service_active: pit_mode.map(|p| p == PIT_MODE_IN_PIT),
            planned: planned(schedule),
            mandatory_lap: s.i32("mEnforcedPitStopLap").filter(|l| *l > 0),
            ..Default::default()
        },
        controls: controls(s),
        p2p_count: boost.map(|a| a.round() as i32),
        p2p_active: boost_on,
        drs_active: drs.map(|d| d & DRS_ACTIVE != 0),
        drs_available: drs.map(|d| d & DRS_AVAILABLE != 0),
        tc_active: Some(car_flags & CAR_TCS != 0),
        alongside: m.player.and_then(|p| alongside(s, p, &cars)),
        wheels: Some(wheels(s, t, speed.unwrap_or(0.0), free)),
        damage: Some(damage),
        ..Default::default()
    };
    let player = PlayerTelemetry {
        on_track,
        speed_mps: speed,
        rpm: s.f64("mRpm"),
        gear: s.i32("mGear"),
        throttle,
        brake,
        fuel_l,
        fuel_capacity_l: fuel_capacity,
        fuel_use_lph,
        incidents: None,
        current_lap_s: s.f64("mCurrentTime").filter(|t| *t >= 0.0),
        delta_to_best_s: None,
        water_temp_c: positive(s.f64("mWaterTempCelsius")),
        oil_temp_c: positive(s.f64("mOilTempCelsius")),
        car,
    };

    let mut flags = m
        .player
        .and_then(|p| cars.iter().find(|c| c.idx == p))
        .map(|c| c.extra.flags.clone())
        .unwrap_or_default();
    flags.extend(colour_flags(s.u32("mHighestFlagColour").unwrap_or(0)));
    flags.extend(caution_flags(yellow));
    if penalty(schedule) {
        flags.push(Flag::Black);
    }

    let duration = positive(s.f64("mSessionDuration")).map(|m| m * 60.0);
    let laps_total = s.u32("mLapsInEvent").filter(|l| *l > 0).map(|l| l as i32);
    Frame {
        game: "ams2".into(),
        tick,
        timestamp_ms,
        session: SessionState {
            session_num: m.session_num,
            kind,
            phase: phase(session_state, s.u32("mRaceState")),
            time_s: m.time_s,
            time_remaining_s: s.f64("mEventTimeRemaining").filter(|t| *t >= 0.0),
            laps_remaining: laps_total
                .zip(leader_laps)
                .map(|(total, done)| (total - done).max(0)),
            laps_total,
            flags: sorted(flags),
            track: some_text(s.str("mTrackLocation")).map(|name| TrackInfo {
                name,
                config: some_text(s.str("mTrackVariation")),
                length_m: length,
                sectors: num_sectors.map_or_else(Vec::new, |n| t.sector_starts(n)),
            }),
            uid: Some(m.uid),
            time_total_s: duration,
            time_of_day_s: None,
        },
        player_car_idx: m.player.filter(|p| cars.iter().any(|c| c.idx == *p)),
        player: Some(player),
        cars,
        weather: Some(Weather {
            air_temp_c: s.f64("mAmbientTemperature"),
            track_temp_c: s.f64("mTrackTemperature"),
            precipitation: s.f64("mRainDensity").map(|r| r.clamp(0.0, 1.0)),
            wind_mps: s.f64("mWindSpeed").filter(|w| *w >= 0.0),
            ..Default::default()
        }),
    }
}

/// The game's own description of the session, for `Extras.info`.
pub fn info(s: &Snapshot<'_>) -> serde_json::Value {
    let count = s
        .i32("mNumParticipants")
        .and_then(|n| usize::try_from(n).ok())
        .map_or(PARTICIPANTS, |n| n.min(PARTICIPANTS));
    let participants: Vec<serde_json::Value> = (0..count)
        .filter(|i| s.part_u32(*i, "mIsActive") == Some(1))
        .map(|i| {
            serde_json::json!({
                "index": i,
                "name": s.part_str(i, "mName"),
                "car": s.str_at("mCarNames", i),
                "class": s.str_at("mCarClassNames", i),
                "nationality": s.u32_at("mNationalities", i),
            })
        })
        .collect();
    serde_json::json!({
        "version": s.version,
        "build": s.u32("mBuildVersionNumber"),
        "track": s.str("mTrackLocation"),
        "variation": s.str("mTrackVariation"),
        "track_translated": s.str("mTranslatedTrackLocation"),
        "variation_translated": s.str("mTranslatedTrackVariation"),
        "track_length_m": s.f64("mTrackLength"),
        "car": s.str("mCarName"),
        "class": s.str("mCarClassName"),
        "laps_in_event": s.u32("mLapsInEvent"),
        "session_duration_min": s.f64("mSessionDuration"),
        "additional_laps": s.i32("mSessionAdditionalLaps"),
        "sectors": s.i32("mNumSectors"),
        "private": s.bool("mSessionIsPrivate"),
        "participants": participants,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::testutil::Image;

    const ME: usize = 1;

    /// Two cars at Interlagos, the player second.
    fn race() -> Image {
        let mut img = Image::new();
        img.set_u32("mGameState", 0, GAME_INGAME_PLAYING);
        img.set_u32("mSessionState", 0, SESSION_RACE);
        img.set_u32("mRaceState", 0, RACESTATE_RACING);
        img.set_i32("mNumParticipants", 0, 3);
        img.set_i32("mViewedParticipantIndex", 0, ME as i32);
        img.set_str("mTrackLocation", 0, "Interlagos");
        img.set_str("mTrackVariation", 0, "GP");
        img.set_f32("mTrackLength", 0, 4000.0);
        img.set_i32("mNumSectors", 0, 3);
        img.set_u32("mLapsInEvent", 0, 10);
        img.set_str("mCarName", 0, "Formula Ultimate");
        img.set_str("mCarClassName", 0, "F-Ultimate");
        img.set_u32("mCarFlags", 0, CAR_ENGINE_ACTIVE);
        img.set_f32("mRpm", 0, 9000.0);
        img.set_i32("mEnforcedPitStopLap", 0, -1);
        img.set_f32("mBrakeBias", 0, -1.0);
        img.set_i32("mAntiLockSetting", 0, -1);
        img.set_i32("mTractionControlSetting", 0, -1);
        img.set_i32("mYellowFlagState", 0, -1);
        for i in 0..4 {
            img.set_u32("mTyreFlags", i, TYRE_SOUND | 4);
        }
        for (car, name, pos) in [(0, "Leader", 1), (ME, "Me", 2), (2, "Third", 3)] {
            img.part_bool(car, "mIsActive", true);
            img.part_str(car, "mName", name);
            img.part_u32(car, "mRacePosition", pos);
            img.part_u32(car, "mLapsCompleted", 2);
            img.part_u32(car, "mCurrentLap", 3);
            img.part_f32(car, "mCurrentLapDistance", 0, 1000.0);
            img.set_str("mCarClassNames", car, "F-Ultimate");
            img.set_str("mCarNames", car, "Formula Ultimate");
            img.set_f32("mLastLapTimes", car, -1.0);
            img.set_f32("mFastestLapTimes", car, -1.0);
        }
        img
    }

    fn frame(img: &Image, t: &mut Tracker, now: f64) -> Frame {
        build_frame(&img.snapshot(), t, 7, 1_000, now)
    }

    fn me(f: &Frame) -> &CarState {
        f.player_car().unwrap()
    }

    fn car(f: &Frame) -> &CarTelemetry {
        &f.player.as_ref().unwrap().car
    }

    #[test]
    fn session_kinds_and_phases() {
        let kinds = [
            (None, SessionKind::Other),
            (Some(0), SessionKind::Other),
            (Some(SESSION_PRACTICE), SessionKind::Practice),
            (Some(SESSION_TEST), SessionKind::Practice),
            (Some(SESSION_QUALIFY), SessionKind::Qualify),
            (Some(SESSION_FORMATION_LAP), SessionKind::Race),
            (Some(SESSION_RACE), SessionKind::Race),
            (Some(SESSION_TIME_ATTACK), SessionKind::Practice),
            (Some(7), SessionKind::Other),
        ];
        for (v, want) in kinds {
            assert_eq!(kind(v), want, "{v:?}");
        }
        let phases = [
            (Some(0), SessionPhase::Unknown),
            (Some(RACESTATE_NOT_STARTED), SessionPhase::GetInCar),
            (Some(RACESTATE_RACING), SessionPhase::Racing),
            (Some(RACESTATE_FINISHED), SessionPhase::Checkered),
            (Some(RACESTATE_DISQUALIFIED), SessionPhase::Checkered),
            (Some(RACESTATE_RETIRED), SessionPhase::Checkered),
            (Some(RACESTATE_DNF), SessionPhase::Checkered),
            (Some(7), SessionPhase::Unknown),
            (None, SessionPhase::Unknown),
        ];
        for (v, want) in phases {
            assert_eq!(phase(Some(SESSION_RACE), v), want, "{v:?}");
        }
        assert_eq!(
            phase(Some(SESSION_FORMATION_LAP), Some(RACESTATE_RACING)),
            SessionPhase::ParadeLaps
        );
    }

    #[test]
    fn flags_and_full_course_yellow() {
        let colours = [
            (0, vec![]),
            (1, vec![Flag::Green]),
            (2, vec![Flag::Blue]),
            (3, vec![Flag::SlowCar]),
            (4, vec![Flag::White]),
            (5, vec![Flag::Red]),
            (6, vec![Flag::YellowWaving]),
            (7, vec![Flag::Yellow, Flag::YellowWaving]),
            (8, vec![Flag::BlackWhite]),
            (9, vec![Flag::Repair]),
            (10, vec![Flag::Disqualify]),
            (11, vec![Flag::Checkered]),
            (12, vec![]),
        ];
        for (c, want) in colours {
            assert_eq!(colour_flags(c), want, "{c}");
        }
        let fcy = [
            (None, vec![]),
            (Some(-1), vec![]),
            (Some(0), vec![]),
            (Some(1), vec![Flag::Caution]),
            (Some(5), vec![Flag::Caution]),
            (Some(6), vec![Flag::Caution, Flag::OneLapToGreen]),
            (Some(7), vec![Flag::Caution, Flag::OneLapToGreen]),
            (Some(8), vec![Flag::Caution, Flag::Red]),
            (Some(9), vec![]),
        ];
        for (s, want) in fcy {
            assert_eq!(caution_flags(s), want, "{s:?}");
        }
        let plans = [
            (1, "player requested"),
            (2, "engineer requested"),
            (3, "damage"),
            (4, "mandatory"),
            (5, "drive through"),
            (6, "stop and go"),
            (7, "pit spot occupied"),
        ];
        for (v, want) in plans {
            assert_eq!(planned(Some(v)).as_deref(), Some(want));
        }
        assert_eq!(planned(Some(0)), None);
        assert_eq!(planned(Some(8)), None);
        assert!(!penalty(Some(4)) && penalty(Some(5)) && penalty(Some(6)) && !penalty(Some(7)));

        let mut img = race();
        img.set_u32("mHighestFlagColour", 0, 7);
        img.set_u32("mHighestFlagColours", ME, 2);
        img.set_u32("mPitSchedule", 0, PIT_SCHEDULE_DRIVE_THROUGH);
        img.set_i32("mYellowFlagState", 0, YFS_PITS_CLOSED);
        let f = frame(&img, &mut Tracker::default(), 0.0);
        assert_eq!(
            f.session.flags,
            vec![
                Flag::Yellow,
                Flag::YellowWaving,
                Flag::Caution,
                Flag::Blue,
                Flag::Black
            ]
        );
        assert_eq!(car(&f).pit.open, Some(false));
        assert_eq!(car(&f).pit.planned.as_deref(), Some("drive through"));
    }

    #[test]
    fn pits_open_by_yellow_state() {
        let mut img = race();
        let open = |img: &Image| car(&frame(img, &mut Tracker::default(), 0.0)).pit.open;
        assert_eq!(open(&img), None, "-1: no full course yellow logic");
        for (state, want) in [(0, true), (1, false), (2, false), (4, true), (8, true)] {
            img.set_i32("mYellowFlagState", 0, state);
            assert_eq!(open(&img), Some(want), "{state}");
        }
        img.set_i32("mYellowFlagState", 0, YFS_PIT_LEAD_LAP);
        assert_eq!(open(&img), Some(true), "on the lead lap");
        img.part_u32(ME, "mLapsCompleted", 1);
        assert_eq!(open(&img), Some(false), "a lap down");
    }

    #[test]
    fn cars_positions_pits_and_states() {
        let mut img = race();
        img.set_str("mCarClassNames", 2, "F-Reiza");
        img.part_u32(2, "mRacePosition", 3);
        img.set_u32("mPitModes", 0, PIT_MODE_IN_PIT);
        img.set_u32("mPitModes", 2, PIT_MODE_DRIVING_INTO_PITS);
        img.set_u32("mRaceStates", 0, RACESTATE_FINISHED);
        img.set_u32("mRaceStates", 2, RACESTATE_DNF);
        img.set_u32("mPitSchedules", 2, PIT_SCHEDULE_STOP_GO);
        img.set_f32("mLastLapTimes", 0, 80.5);
        img.set_f32("mFastestLapTimes", 0, 79.25);
        img.set_f32("mSpeeds", 0, 12.5);
        img.set_bool("mLapsInvalidated", 0, true);
        img.set_f32("mCurrentSector1Times", 0, 25.5);
        img.set_f32("mFastestSector2Times", 0, 26.0);
        img.set_f32("mLastLapTime", 0, 81.0);
        img.set_f32("mBestLapTime", 0, 80.0);
        img.set_bool("mLapInvalidated", 0, true);
        img.part_u32(1, "mCurrentLap", 0);
        img.part_bool(3, "mIsActive", true);
        img.set_i32("mNumParticipants", 0, 3);
        let f = frame(&img, &mut Tracker::default(), 0.0);
        assert_eq!(f.cars.len(), 3, "the 4th slot is beyond mNumParticipants");

        let leader = f.car(0).unwrap();
        assert_eq!(leader.number, "1");
        assert!(leader.on_pit_road);
        assert_eq!(leader.surface, TrackSurface::InPitStall);
        assert_eq!(leader.extra.flags, vec![Flag::Checkered]);
        assert_eq!(leader.extra.retired, Some(false));
        assert_eq!(leader.last_lap_s, Some(80.5));
        assert_eq!(leader.best_lap_s, Some(79.25));
        assert_eq!(leader.extra.speed_mps, Some(12.5));
        assert_eq!(leader.extra.lap_invalidated, Some(true));
        assert_eq!(leader.extra.sector_s, vec![Some(25.5), None, None]);
        assert_eq!(leader.extra.best_sector_s, vec![None, Some(26.0), None]);
        assert_eq!(leader.lap, Some(3));
        assert_eq!(leader.laps_completed, Some(2));
        assert_eq!(leader.lap_dist_pct, Some(0.25));
        assert_eq!(leader.class_position, Some(1));

        let me = me(&f);
        assert!(me.is_player);
        assert_eq!(me.lap, None, "lap 0 is unset");
        assert_eq!(me.surface, TrackSurface::OnTrack);
        assert_eq!(me.last_lap_s, Some(81.0), "the player's own fields fill in");
        assert_eq!(me.best_lap_s, Some(80.0));
        assert_eq!(me.extra.lap_invalidated, Some(true));
        assert!(me.extra.sector_s.is_empty());
        assert_eq!(me.extra.tyre_compound, None);
        assert_eq!(me.class_position, Some(2));
        assert_eq!(me.car_name.as_deref(), Some("Formula Ultimate"));

        let third = f.car(2).unwrap();
        assert_eq!(third.surface, TrackSurface::ApproachingPits);
        assert!(third.on_pit_road);
        assert_eq!(third.class_name.as_deref(), Some("F-Reiza"));
        assert_ne!(third.class_id, me.class_id);
        assert_eq!(third.class_position, Some(1), "alone in its class");
        assert_eq!(third.extra.retired, Some(true));
        assert_eq!(third.extra.flags, vec![Flag::Black]);

        img.set_u32("mRaceStates", 0, RACESTATE_DISQUALIFIED);
        let f = frame(&img, &mut Tracker::default(), 0.0);
        assert_eq!(f.car(0).unwrap().extra.flags, vec![Flag::Disqualify]);

        for (mode, pit, s) in [
            (0, false, TrackSurface::OnTrack),
            (
                PIT_MODE_DRIVING_INTO_PITS,
                true,
                TrackSurface::ApproachingPits,
            ),
            (PIT_MODE_IN_PIT, true, TrackSurface::InPitStall),
            (3, true, TrackSurface::OnTrack),
            (PIT_MODE_IN_GARAGE, true, TrackSurface::InPitStall),
            (PIT_MODE_DRIVING_OUT_OF_GARAGE, true, TrackSurface::OnTrack),
            (6, false, TrackSurface::OnTrack),
        ] {
            assert_eq!(on_pit_road(Some(mode)), pit, "{mode}");
            assert_eq!(surface(Some(mode)), s, "{mode}");
        }
        assert!(class_id("a") > 0 && class_id("a") != class_id("b"));
        assert_eq!(class_id("F-Ultimate"), class_id("F-Ultimate"));
    }

    #[test]
    fn gaps_only_in_races() {
        let mut img = race();
        let mut t = Tracker::default();
        frame(&img, &mut t, 0.0);
        img.part_u32(0, "mLapsCompleted", 3);
        frame(&img, &mut t, 1.0);
        img.part_u32(ME, "mLapsCompleted", 3);
        let f = frame(&img, &mut t, 2.5);
        assert_eq!(f.car(0).unwrap().gap_to_leader_s, Some(0.0));
        assert_eq!(me(&f).gap_to_leader_s, Some(1.5));
        assert_eq!(f.car(2).unwrap().gap_to_leader_s, None);
        // Without a position the gap means nothing.
        img.part_u32(ME, "mRacePosition", 0);
        let f = frame(&img, &mut t, 3.0);
        assert_eq!(me(&f).gap_to_leader_s, None);
        assert_eq!(me(&f).position, None);

        let mut img = race();
        img.set_u32("mSessionState", 0, SESSION_QUALIFY);
        let mut t = Tracker::default();
        frame(&img, &mut t, 0.0);
        img.part_u32(ME, "mLapsCompleted", 3);
        assert_eq!(me(&frame(&img, &mut t, 1.0)).gap_to_leader_s, None);
    }

    #[test]
    fn sectors_once_seen() {
        let mut img = race();
        let mut t = Tracker::default();
        let f = frame(&img, &mut t, 0.0);
        assert!(f.session.track.as_ref().unwrap().sectors.is_empty());
        for (car, d1, d2) in [(0, 1200.0, 2600.0), (2, 1240.0, 2640.0)] {
            img.part_f32(car, "mCurrentSector", 0, 0.0);
            frame(&img, &mut t, 1.0);
            img.part_f32(car, "mCurrentLapDistance", 0, d1);
            img.part_u32(car, "mCurrentSector", 1);
            frame(&img, &mut t, 2.0);
            img.part_f32(car, "mCurrentLapDistance", 0, d2);
            img.part_u32(car, "mCurrentSector", 2);
            frame(&img, &mut t, 3.0);
        }
        let f = frame(&img, &mut t, 4.0);
        let track = f.session.track.unwrap();
        assert_eq!(track.sectors, vec![0.0, 0.31, 0.66]);
        assert_eq!(track.name, "Interlagos");
        assert_eq!(track.config.as_deref(), Some("GP"));
        assert_eq!(track.length_m, Some(4000.0));
        // In the pits the distance is not the lap's.
        img.set_u32("mPitModes", 0, PIT_MODE_IN_PIT);
        img.part_u32(0, "mCurrentSector", 0);
        frame(&img, &mut t, 5.0);
        img.part_f32(0, "mCurrentLapDistance", 0, 10.0);
        img.part_u32(0, "mCurrentSector", 1);
        frame(&img, &mut t, 6.0);
        assert_eq!(t.sector_starts(3), vec![0.0, 0.31, 0.66]);
    }

    #[test]
    fn the_players_car() {
        let mut img = race();
        img.set_f32("mFuelLevel", 0, 0.25);
        img.set_f32("mFuelCapacity", 0, 120.0);
        img.set_f32("mSpeed", 0, 50.0);
        img.set_i32("mGear", 0, 5);
        img.set_f32("mThrottle", 0, 1.0);
        img.set_f32("mBrake", 0, 0.0);
        img.set_f32("mClutch", 0, 0.25);
        img.set_f32("mCurrentTime", 0, 33.5);
        img.set_f32("mWaterTempCelsius", 0, 92.0);
        img.set_f32("mOilTempCelsius", 0, 0.0);
        img.set_f32("mOilPressureKPa", 0, 450.0);
        img.set_f32("mFuelPressureKPa", 0, 0.0);
        img.set_f32("mLocalVelocity", 0, 0.5);
        img.set_f32("mLocalVelocity", 2, -50.0);
        img.set_f32("mLocalAcceleration", 0, -9.0);
        img.set_f32("mLocalAcceleration", 2, 4.0);
        img.set_f32("mAngularVelocity", 1, 0.3);
        img.set_bool("mAntiLockActive", 0, false);
        img.set_u32("mCarFlags", 0, CAR_ENGINE_ACTIVE | CAR_ABS | CAR_TCS);
        img.set_f32("mBrakeBias", 0, 0.5625);
        img.set_i32("mAntiLockSetting", 0, 3);
        img.set_i32("mTractionControlSetting", 0, 0);
        img.set_i32("mErsDeploymentMode", 0, 0);
        img.set_i32("mEnforcedPitStopLap", 0, 12);
        img.set_u32("mPitMode", 0, PIT_MODE_IN_PIT);
        img.set_u32("mPitSchedule", 0, 1);
        img.set_u32("mDrsState", 0, DRS_INSTALLED | 0x08);
        img.set_f32("mBoostAmount", 0, 42.4);
        img.set_bool("mBoostActive", 0, true);
        let mut t = Tracker::default();
        let f = frame(&img, &mut t, 0.0);
        let p = f.player.as_ref().unwrap();
        assert!(p.on_track);
        assert_eq!(p.fuel_l, Some(30.0));
        assert_eq!(p.fuel_capacity_l, Some(120.0));
        assert_eq!(p.speed_mps, Some(50.0));
        assert_eq!(p.gear, Some(5));
        assert_eq!(p.current_lap_s, Some(33.5));
        assert_eq!(p.water_temp_c, Some(92.0));
        assert_eq!(p.oil_temp_c, None, "0 is unset");
        assert_eq!(p.incidents, None);
        let c = &p.car;
        assert_eq!(c.clutch, Some(0.75));
        assert_eq!(c.steering_rad, None);
        assert_eq!(c.velocity_x_mps, Some(50.0));
        assert_eq!(c.velocity_y_mps, Some(0.5));
        assert_eq!(c.long_accel_mps2, Some(-4.0));
        assert_eq!(c.lat_accel_mps2, Some(-9.0));
        assert_eq!(c.yaw_rate_rps.map(|y| (y * 1e6).round() / 1e6), Some(-0.3));
        assert_eq!(c.abs_active, Some(true), "the car flag says so");
        assert_eq!(c.tc_active, Some(true));
        assert_eq!(c.oil_press_bar, Some(4.5));
        assert_eq!(c.fuel_press_bar, None);
        assert_eq!(
            c.controls,
            vec![
                Control {
                    id: "brake_bias".into(),
                    value: 56.25
                },
                Control {
                    id: "abs".into(),
                    value: 3.0
                },
                Control {
                    id: "traction_control".into(),
                    value: 0.0
                },
            ]
        );
        assert_eq!(c.pit.in_stall, Some(true));
        assert_eq!(c.pit.service_active, Some(true));
        assert_eq!(c.pit.planned.as_deref(), Some("player requested"));
        assert_eq!(c.pit.mandatory_lap, Some(12));
        assert_eq!(c.drs_active, Some(false));
        assert_eq!(c.drs_available, Some(true));
        assert_eq!((c.p2p_count, c.p2p_active), (Some(42), Some(true)));
        assert!(c.tyres.live);
        assert_eq!(c.warnings, vec![]);
        assert_eq!(f.weather.as_ref().unwrap().wind_mps, Some(0.0));

        img.set_i32("mErsDeploymentMode", 0, 3);
        img.set_u32("mDrsState", 0, 0);
        img.set_f32("mBoostAmount", 0, 0.0);
        img.set_bool("mBoostActive", 0, false);
        img.set_u32("mPitMode", 0, PIT_MODE_IN_GARAGE);
        let f = frame(&img, &mut t, 0.1);
        let c = car(&f);
        assert_eq!(c.controls.last().unwrap().id, "ers_mode");
        assert_eq!(c.controls.last().unwrap().value, 3.0);
        assert_eq!((c.drs_active, c.drs_available), (None, None), "no DRS");
        assert_eq!((c.p2p_count, c.p2p_active), (None, None), "no boost");
        assert!(!f.player.as_ref().unwrap().on_track, "in the garage");
    }

    #[test]
    fn warnings_from_the_car() {
        let mut img = race();
        let warnings = |img: &Image| {
            car(&frame(img, &mut Tracker::default(), 0.0))
                .warnings
                .clone()
        };
        img.set_u32(
            "mCarFlags",
            0,
            CAR_ENGINE_ACTIVE | CAR_ENGINE_WARNING | CAR_SPEED_LIMITER,
        );
        img.set_bool("mClutchOverheated", 0, true);
        assert_eq!(
            warnings(&img),
            vec![Warning::PitLimiter, Warning::Engine, Warning::Clutch]
        );
        img.set_u32("mCarFlags", 0, 0);
        img.set_bool("mClutchOverheated", 0, false);
        assert_eq!(warnings(&img), vec![], "turning: not stalled");
        img.set_f32("mRpm", 0, 49.0);
        assert_eq!(warnings(&img), vec![Warning::Stalled]);
        img.set_u32("mGameState", 0, GAME_INGAME_PAUSED);
        assert_eq!(
            warnings(&img),
            vec![Warning::Stalled],
            "paused, still in the car"
        );
        img.set_u32("mPitMode", 0, PIT_MODE_IN_GARAGE);
        assert_eq!(warnings(&img), vec![], "in the garage");
        img.set_u32("mPitMode", 0, 0);
        img.set_u32("mCarFlags", 0, CAR_ENGINE_ACTIVE);

        img.set_f32("mAeroDamage", 0, 0.19);
        assert_eq!(warnings(&img), vec![]);
        img.set_f32("mAeroDamage", 0, 0.2);
        assert_eq!(warnings(&img), vec![Warning::Damage]);
        img.set_f32("mAeroDamage", 0, 0.0);
        img.set_f32("mSuspensionDamage", 3, 0.5);
        img.set_f32("mBrakeDamage", 0, 0.1);
        img.set_f32("mEngineDamage", 0, 0.05);
        let f = frame(&img, &mut Tracker::default(), 0.0);
        let d = car(&f).damage.clone().unwrap();
        assert_eq!(d.suspension, [Some(0.0), Some(0.0), Some(0.0), Some(0.5)]);
        assert_eq!(d.brakes[0].map(|b| (b * 1e6).round() / 1e6), Some(0.1));
        assert_eq!(d.worst(), Some(0.5));
        img.set_f32("mSuspensionDamage", 3, 0.0);

        img.set_u32("mTyreFlags", 2, TYRE_ATTACHED);
        assert_eq!(warnings(&img), vec![Warning::FlatTyre], "deflated");
        img.set_u32("mTyreFlags", 2, 0x02);
        assert_eq!(warnings(&img), vec![Warning::FlatTyre], "came off");
        for i in 0..4 {
            img.set_u32("mTyreFlags", i, 0);
        }
        assert_eq!(warnings(&img), vec![], "no tyre data at all");
    }

    #[test]
    fn tyres_and_wheels() {
        let mut img = race();
        img.set_tyres("mTyreTempLeft", [80.0, 70.0, 82.0, 72.0]);
        img.set_tyres("mTyreTempCenter", [85.0, 75.0, 0.0, 77.0]);
        img.set_tyres("mTyreTempRight", [90.0, 79.0, 92.0, 78.0]);
        img.set_tyres("mTyreTemp", [1.0, 1.0, 86.0, 1.0]);
        img.set_tyres("mTyreWear", [0.25, 0.0, 1.5, 0.5]);
        img.set_tyres("mAirPressure", [26.0, 180.0, 0.0, 69.9]);
        img.set_tyres("mTyreCarcassTemp", [373.15, 0.0, 353.15, 363.15]);
        img.set_tyres("mBrakeTempCelsius", [450.0, 440.0, 300.0, 310.0]);
        img.set_tyres("mRideHeight", [5.5, 0.055, -2.0, 1.0]);
        img.set_tyres("mSuspensionVelocity", [0.1, -0.1, 0.0, 0.2]);
        img.set_str("mTyreCompound", 0, "Soft");
        img.set_str("mTyreCompound", 1, "Soft");
        img.set_str("mTyreCompound", 2, "Soft");
        img.set_str("mTyreCompound", 3, "Soft");
        let mut t = Tracker::default();
        let f = frame(&img, &mut t, 0.0);
        let c = car(&f);
        let r = |v: Option<f64>| v.map(|x| (x * 1000.0).round() / 1000.0);
        assert_eq!(c.tyres.lf.temp_c, [Some(80.0), Some(85.0), Some(90.0)]);
        assert_eq!(
            c.tyres.rf.temp_c,
            [Some(79.0), Some(75.0), Some(70.0)],
            "right: swapped"
        );
        assert_eq!(
            c.tyres.lr.temp_c,
            [Some(82.0), Some(86.0), Some(92.0)],
            "fallback"
        );
        assert_eq!(c.tyres.lf.tread, [None, Some(0.75), None]);
        assert_eq!(c.tyres.rf.min_tread(), Some(1.0));
        assert_eq!(c.tyres.lr.min_tread(), Some(0.0), "clamped");
        assert_eq!(r(c.tyres.lf.hot_kpa), Some(179.264), "psi");
        assert_eq!(c.tyres.rf.hot_kpa, Some(180.0), "kPa");
        assert_eq!(c.tyres.lr.hot_kpa, None);
        assert_eq!(r(c.tyres.rr.hot_kpa), Some(481.944), "69.9 is psi");
        assert_eq!(r(c.tyres.lf.carcass_c), Some(100.0));
        assert_eq!(c.tyres.rf.carcass_c, None);
        assert_eq!(c.tyres.lf.brake_c, Some(450.0));
        assert_eq!(c.tyre_compound.as_deref(), Some("Soft"));
        assert_eq!(me(&f).extra.tyre_compound.as_deref(), Some("Soft"));
        let w = c.wheels.as_ref().unwrap();
        assert_eq!(r(w.lf.ride_height_m), Some(0.055), "cm");
        assert_eq!(r(w.rf.ride_height_m), Some(0.055), "m");
        assert_eq!(r(w.lr.ride_height_m), Some(-0.02));
        assert_eq!(r(w.rr.ride_height_m), Some(1.0), "1 is not over 1");
        assert_eq!(r(w.rr.shock_mps), Some(0.2));
        assert_eq!(w.lf.surface_c, [Some(80.0), Some(85.0), Some(90.0)]);
        assert_eq!(w.lf.speed_mps, None, "radius not learnt");

        img.set_str("mTyreCompound", 2, "Medium");
        img.set_str("mTyreCompound", 3, "Medium");
        img.set_f32("mSpeed", 0, 40.0);
        img.set_tyres("mTyreRPS", [-125.0, -125.0, -130.0, -130.0]);
        for i in 0..30 {
            frame(&img, &mut t, f64::from(i));
        }
        let f = frame(&img, &mut t, 31.0);
        let c = car(&f);
        assert_eq!(c.tyre_compound.as_deref(), Some("Soft/Medium"));
        let w = c.wheels.as_ref().unwrap();
        assert_eq!(r(w.lf.speed_mps), Some(40.0));
        // Locking the fronts under braking: not learnt from, shows the lock.
        img.set_f32("mBrake", 0, 1.0);
        img.set_tyres("mTyreRPS", [-12.5, -12.5, -130.0, -130.0]);
        let f = frame(&img, &mut t, 32.0);
        let w = car(&f).wheels.clone().unwrap();
        assert_eq!(r(w.lf.speed_mps), Some(4.0));
        assert_eq!(r(w.rr.speed_mps), Some(40.0));
        assert_eq!(compound(&Image::new().snapshot()), None);
    }

    #[test]
    fn off_track_by_terrain_or_crash() {
        let mut img = race();
        let surface = |img: &Image| me(&frame(img, &mut Tracker::default(), 0.0)).surface;
        for (terrain, off) in [
            (0, false),
            (5, false),
            (6, true),
            (7, true),
            (8, true),
            (10, false),
            (11, false),
            (18, false),
            (19, false),
            (21, false),
            (25, false),
            (29, false),
            (30, false),
            (46, false),
            (47, true),
        ] {
            for i in 0..4 {
                img.set_u32("mTerrain", i, terrain);
            }
            let want = if off {
                TrackSurface::OffTrack
            } else {
                TrackSurface::OnTrack
            };
            assert_eq!(surface(&img), want, "{terrain}");
        }
        img.set_u32("mTerrain", 0, 0);
        assert_eq!(
            surface(&img),
            TrackSurface::OnTrack,
            "one wheel on the road"
        );
        img.set_u32("mCrashState", 0, CRASH_DAMAGE_OFFTRACK);
        assert_eq!(surface(&img), TrackSurface::OffTrack);
        img.set_u32("mPitModes", ME, PIT_MODE_DRIVING_INTO_PITS);
        assert_eq!(
            surface(&img),
            TrackSurface::ApproachingPits,
            "pit lane wins"
        );
    }

    #[test]
    fn cars_alongside() {
        let mut img = race();
        let beside = |img: &Image| {
            car(&frame(img, &mut Tracker::default(), 0.0))
                .alongside
                .clone()
        };
        // Facing −Z (yaw 0): +X is left.
        let put = |img: &mut Image, car: usize, x: f32, y: f32, z: f32| {
            img.part_f32(car, "mWorldPosition", 0, x);
            img.part_f32(car, "mWorldPosition", 1, y);
            img.part_f32(car, "mWorldPosition", 2, z);
        };
        put(&mut img, 0, 100.0, 0.0, 100.0);
        assert_eq!(beside(&img), None);
        put(&mut img, ME, 97.0, 0.0, 100.0);
        assert_eq!(beside(&img).as_deref(), Some("left"));
        put(&mut img, 2, 102.0, 0.0, 100.0);
        assert_eq!(beside(&img).as_deref(), Some("two on the left"));
        put(&mut img, 2, 98.0, 0.0, 104.0);
        assert_eq!(
            beside(&img).as_deref(),
            Some("left"),
            "1 m across: not beside"
        );
        put(&mut img, ME, 103.0, 0.0, 100.0);
        assert_eq!(beside(&img).as_deref(), Some("two on the right"));
        put(&mut img, 2, 106.0, 0.0, 102.0);
        assert_eq!(beside(&img).as_deref(), Some("left and right"));
        put(&mut img, 2, 100.0, 0.0, 95.0);
        assert_eq!(beside(&img).as_deref(), Some("right"), "5 m ahead");
        put(&mut img, 2, 98.0, 0.0, 104.5);
        assert_eq!(
            beside(&img).as_deref(),
            Some("two on the right"),
            "4.5 m behind"
        );
        put(&mut img, 2, 98.0, 5.0, 100.0);
        assert_eq!(beside(&img).as_deref(), Some("right"), "on a bridge");
        put(&mut img, 2, 109.0, 0.0, 100.0);
        assert_eq!(beside(&img).as_deref(), Some("right"), "6 m: not alongside");
        img.set_u32("mPitModes", 0, PIT_MODE_IN_PIT);
        assert_eq!(beside(&img), None, "the pit lane is not the track");
        // Turned half round (yaw π): the left is now −X.
        img.set_u32("mPitModes", 0, 0);
        put(&mut img, 2, 120.0, 0.0, 100.0);
        img.set_f32("mOrientation", 1, std::f32::consts::PI);
        assert_eq!(beside(&img).as_deref(), Some("left"));
    }

    #[test]
    fn weather_and_session_info() {
        let mut img = race();
        img.set_f32("mAmbientTemperature", 0, 24.5);
        img.set_f32("mTrackTemperature", 0, 38.0);
        img.set_f32("mRainDensity", 0, 1.5);
        img.set_f32("mWindSpeed", 0, -1.0);
        img.set_f32("mSessionDuration", 0, 30.0);
        img.set_f32("mEventTimeRemaining", 0, 1200.0);
        img.set_u32("mLapsInEvent", 0, 0);
        img.set_u32("mNationalities", ME, 76);
        img.set_bool("mSessionIsPrivate", 0, true);
        let f = frame(&img, &mut Tracker::default(), 0.0);
        let w = f.weather.unwrap();
        assert_eq!(w.air_temp_c, Some(24.5));
        assert_eq!(w.track_temp_c, Some(38.0));
        assert_eq!(w.precipitation, Some(1.0));
        assert_eq!(w.wind_mps, None);
        assert_eq!(f.session.time_total_s, Some(1800.0));
        assert_eq!(f.session.time_remaining_s, Some(1200.0));
        assert_eq!(f.session.time_s, 600.0);
        assert_eq!(f.session.laps_total, None);
        assert_eq!(f.session.laps_remaining, None);
        assert_eq!(f.tick, 7);
        assert_eq!(f.timestamp_ms, 1_000);

        let i = info(&img.snapshot());
        assert_eq!(i["version"], 14);
        assert_eq!(i["track"], "Interlagos");
        assert_eq!(i["private"], true);
        assert_eq!(i["participants"].as_array().unwrap().len(), 3);
        assert_eq!(i["participants"][1]["nationality"], 76);
        assert_eq!(i["participants"][1]["class"], "F-Ultimate");
        img.set_i32("mNumParticipants", 0, -1);
        assert_eq!(
            info(&img.snapshot())["participants"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn no_player_and_no_track() {
        let mut img = Image::new();
        img.set_i32("mViewedParticipantIndex", 0, -1);
        let f = frame(&img, &mut Tracker::default(), 0.0);
        assert!(f.cars.is_empty());
        assert_eq!(f.player_car_idx, None);
        assert_eq!(f.session.track, None);
        assert!(!f.player.unwrap().on_track);
    }

    #[test]
    fn exact_ids_and_units() {
        assert_eq!(class_id("GT3"), 1_812_731_170);
        assert_eq!(class_id("F-Ultimate"), 1_925_234_198);
        assert_eq!(kpa(Some(70.0)), Some(70.0), "70 reads as kPa");
        assert_eq!(kpa(Some(-1.0)), None);
        assert_eq!(metres(Some(-1.5)), Some(-0.015));
        assert_eq!(celsius(Some(0.0)), None);
    }

    #[test]
    fn edges_of_the_players_car() {
        let mut img = race();
        img.set_f32("mFuelPressureKPa", 0, 350.0);
        img.set_u32("mCarFlags", 0, CAR_ENGINE_ACTIVE | CAR_TCS);
        img.set_bool("mAntiLockActive", 0, false);
        img.set_f32("mBoostAmount", 0, 42.0);
        img.set_bool("mBoostActive", 0, false);
        img.set_i32("mEnforcedPitStopLap", 0, 0);
        img.set_u32("mDrsState", 0, DRS_INSTALLED);
        img.set_f32("mRpm", 0, 50.0);
        let f = frame(&img, &mut Tracker::default(), 0.0);
        let c = car(&f);
        assert_eq!(c.fuel_press_bar, Some(3.5));
        assert_eq!(c.abs_active, Some(false));
        assert_eq!((c.p2p_count, c.p2p_active), (Some(42), Some(false)));
        assert_eq!(c.pit.mandatory_lap, None);
        assert_eq!((c.drs_active, c.drs_available), (Some(false), Some(false)));
        img.set_u32("mDrsState", 0, DRS_INSTALLED | 0x04 | DRS_ACTIVE);
        img.set_u32("mCarFlags", 0, 0);
        let f = frame(&img, &mut Tracker::default(), 0.0);
        let c = car(&f);
        assert_eq!((c.drs_active, c.drs_available), (Some(true), Some(true)));
        assert_eq!(c.warnings, vec![], "50 rpm is turning");
        assert_eq!(c.tc_active, Some(false));
        // The viewed car is not in the race.
        img.set_i32("mViewedParticipantIndex", 0, 5);
        let f = frame(&img, &mut Tracker::default(), 0.0);
        assert_eq!(f.player_car_idx, None);
        assert!(!f.player.unwrap().on_track);
    }

    #[test]
    fn when_a_wheel_rolls_freely() {
        let learnt = |throttle: f32, brake: f32, lat: f32| {
            let mut img = race();
            img.set_f32("mSpeed", 0, 40.0);
            img.set_tyres("mTyreRPS", [-125.0; 4]);
            img.set_f32("mThrottle", 0, throttle);
            img.set_f32("mBrake", 0, brake);
            img.set_f32("mLocalAcceleration", 0, lat);
            let mut t = Tracker::default();
            for i in 0..=30 {
                frame(&img, &mut t, f64::from(i));
            }
            car(&frame(&img, &mut t, 99.0))
                .wheels
                .clone()
                .unwrap()
                .lf
                .speed_mps
                .is_some()
        };
        assert!(learnt(0.24, 0.0, -2.9));
        assert!(!learnt(0.25, 0.0, 0.0), "on the throttle");
        assert!(!learnt(0.0, 0.001, 0.0), "on the brake");
        assert!(!learnt(0.0, 0.0, 3.0), "cornering");
        assert!(!learnt(0.0, 0.0, -3.0), "cornering the other way");
    }

    /// Where a car is that runs `side` m to the left and `behind` m behind a
    /// car at `me` turned by `yaw`.
    fn offset(me: [f32; 3], yaw: f32, side: f32, behind: f32) -> [f32; 3] {
        let (s, c) = yaw.sin_cos();
        [
            me[0] + side * c + behind * s,
            me[1],
            me[2] - side * s + behind * c,
        ]
    }

    #[test]
    fn alongside_at_any_heading_and_height() {
        const YAW: f32 = std::f32::consts::FRAC_PI_4;
        let mut img = race();
        img.set_f32("mOrientation", 1, YAW);
        let me = [100.0, 10.0, 100.0];
        let put = |img: &mut Image, car: usize, p: [f32; 3]| {
            for (i, v) in p.into_iter().enumerate() {
                img.part_f32(car, "mWorldPosition", i, v);
            }
        };
        put(&mut img, ME, me);
        put(&mut img, 2, [500.0, 10.0, 500.0]);
        let beside = |img: &Image| {
            car(&frame(img, &mut Tracker::default(), 0.0))
                .alongside
                .clone()
        };
        put(&mut img, 0, offset(me, YAW, 5.0, 0.0));
        assert_eq!(beside(&img).as_deref(), Some("left"));
        put(&mut img, 0, offset(me, YAW, -2.0, 4.0));
        assert_eq!(beside(&img).as_deref(), Some("right"));
        put(&mut img, 0, offset(me, YAW, 2.0, 6.0));
        assert_eq!(beside(&img), None, "6 m behind");
        put(&mut img, 0, offset(me, YAW, 2.0, -6.0));
        assert_eq!(beside(&img), None, "6 m ahead");
        let mut level = offset(me, YAW, 5.0, 0.0);
        level[1] = 13.0;
        put(&mut img, 0, level);
        assert_eq!(beside(&img).as_deref(), Some("left"), "3 m up still counts");
        level[1] = 13.5;
        put(&mut img, 0, level);
        assert_eq!(beside(&img), None);
    }
}
