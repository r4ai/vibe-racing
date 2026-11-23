//! Assetto Corsa's pages → normalized [`vr_core::Frame`].

use vr_core::{
    CarExtra, CarState, CarTelemetry, Control, Damage, Flag, Frame, PitState, PitWindow,
    PlayerTelemetry, SessionKind, SessionPhase, SessionState, TrackInfo, TrackSurface, Tyre, Tyres,
    Warning, Weather, Wheel, Wheels, WindowUnit,
};

use crate::layout::{CREWCHIEF, GRAPHICS, PHYSICS, STATIC, VEHICLE, View};
use crate::session::{Clock, Sample, Tracker};

pub const GAME: &str = "ac";
/// Assetto Corsa's `carId` of the player's car.
pub const PLAYER_IDX: usize = 0;
const G: f64 = 9.806_65;
const KPA_PER_PSI: f64 = 6.894_757;
/// Lap times longer than this are "none yet".
const MAX_LAP_MS: i32 = 36_000_000;
/// Off the track with this many wheels.
const OFF_TRACK_WHEELS: i32 = 3;

// AC_STATUS
pub const STATUS_OFF: i32 = 0;
pub const STATUS_REPLAY: i32 = 1;
pub const STATUS_LIVE: i32 = 2;
pub const STATUS_PAUSE: i32 = 3;

// AC_FLAG_TYPE
const FLAG_BLUE: i32 = 1;
const FLAG_YELLOW: i32 = 2;
const FLAG_BLACK: i32 = 3;
const FLAG_WHITE: i32 = 4;
const FLAG_CHECKERED: i32 = 5;
const FLAG_PENALTY: i32 = 6;

/// A copy of every page at one moment.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub physics: Vec<u8>,
    pub graphics: Vec<u8>,
    pub statics: Vec<u8>,
    /// CrewChief's page of all cars, when its app runs.
    pub crewchief: Option<Vec<u8>>,
}

impl Snapshot {
    pub fn physics(&self) -> View<'_> {
        View::new(&PHYSICS, &self.physics)
    }

    pub fn graphics(&self) -> View<'_> {
        View::new(&GRAPHICS, &self.graphics)
    }

    pub fn statics(&self) -> View<'_> {
        View::new(&STATIC, &self.statics)
    }

    /// The connected cars of the CrewChief page.
    pub fn vehicles(&self) -> Vec<View<'_>> {
        let Some(b) = &self.crewchief else {
            return Vec::new();
        };
        let head = View::new(&CREWCHIEF, b);
        let n = head.i32("numVehicles").unwrap_or(0).clamp(0, 128) as usize;
        (0..n)
            .filter_map(|i| {
                let at = CREWCHIEF.size + i * VEHICLE.size;
                b.get(at..at + VEHICLE.size)
            })
            .map(|b| View::new(&VEHICLE, b))
            .filter(|v| v.flag("isConnected") == Some(true))
            .collect()
    }

    /// The pages' own values, under their own names.
    pub fn channels(&self) -> Vec<vr_core::Channel> {
        let mut out = self.physics().channels("physics");
        out.extend(self.graphics().channels("graphics"));
        out.extend(self.statics().channels("static"));
        if let Some(b) = &self.crewchief {
            out.extend(View::new(&CREWCHIEF, b).channels("crewchief"));
            for v in self.vehicles() {
                let id = v.i32("carId").unwrap_or(-1);
                out.extend(v.channels(&format!("crewchief.vehicle[{id}]")));
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// What the session tracker needs.
    pub fn sample(&self, now_ms: i64) -> Sample {
        let g = self.graphics();
        let s = self.statics();
        let timed = timed(&s, &g);
        Sample {
            now_ms,
            kind: g.i32("session").unwrap_or(-1),
            event: format!(
                "{}@{}/{}",
                s.text("carModel").unwrap_or_default(),
                s.text("track").unwrap_or_default(),
                s.text("trackConfiguration").unwrap_or_default()
            ),
            completed_laps: g.i32("completedLaps").unwrap_or(0),
            distance_m: g.f64("distanceTraveled").unwrap_or(0.0),
            time_left_s: timed.then(|| time_left(&g)).flatten(),
        }
    }
}

pub fn session_kind(v: i32) -> SessionKind {
    match v {
        0 | 3 | 4 => SessionKind::Practice,
        1 => SessionKind::Qualify,
        2 => SessionKind::Race,
        _ => SessionKind::Other,
    }
}

fn flags(flag: i32, racing: bool) -> Vec<Flag> {
    let f = match flag {
        FLAG_BLUE => Some(Flag::Blue),
        FLAG_YELLOW => Some(Flag::Yellow),
        FLAG_BLACK | FLAG_PENALTY => Some(Flag::Black),
        FLAG_WHITE => Some(Flag::White),
        FLAG_CHECKERED => Some(Flag::Checkered),
        _ => None,
    };
    let green = racing && !matches!(f, Some(Flag::Yellow | Flag::Checkered));
    green.then_some(Flag::Green).into_iter().chain(f).collect()
}

fn lap_s(ms: Option<i32>) -> Option<f64> {
    ms.filter(|ms| (1..MAX_LAP_MS).contains(ms))
        .map(|ms| f64::from(ms) / 1000.0)
}

fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|v| *v > 0.0)
}

/// Not zero, which the game writes for "nothing".
fn set(v: Option<f64>) -> Option<f64> {
    v.filter(|v| *v != 0.0)
}

fn timed(s: &View<'_>, g: &View<'_>) -> bool {
    s.flag("isTimedRace") == Some(true) || g.i32("session") != Some(2)
}

/// `sessionTimeLeft` is in milliseconds; negative when there is no limit.
fn time_left(g: &View<'_>) -> Option<f64> {
    g.f64("sessionTimeLeft")
        .filter(|t| *t >= 0.0 && *t < 6.0 * 24.0 * 3_600_000.0)
        .map(|t| t / 1000.0)
}

fn driver(s: &View<'_>) -> String {
    let name = [s.text("playerName"), s.text("playerSurname")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        s.text("playerNick").unwrap_or_else(|| "Player".into())
    } else {
        name
    }
}

/// Inner, middle, outer → outer, middle, inner; 0 is "not told".
fn across(p: &View<'_>, i: usize) -> [Option<f64>; 3] {
    ["tyreTempO", "tyreTempM", "tyreTempI"].map(|n| set(p.f64_at(n, i)))
}

fn tyre(p: &View<'_>, i: usize) -> Tyre {
    Tyre {
        cold_kpa: None,
        hot_kpa: positive(p.f64_at("wheelsPressure", i)).map(|psi| psi * KPA_PER_PSI),
        temp_c: across(p, i),
        // The game's wear is what is left of 100.
        tread: [
            None,
            positive(p.f64_at("tyreWear", i)).map(|w| (w / 100.0).clamp(0.0, 1.0)),
            None,
        ],
        distance_m: None,
        core_c: set(p.f64_at("tyreCoreTemperature", i)),
    }
}

fn wheel(p: &View<'_>, s: &View<'_>, i: usize, shock: Option<f64>) -> Wheel {
    // The front ride height for the front wheels, the rear for the rear.
    let axle = i / 2;
    Wheel {
        speed_mps: p
            .f64_at("wheelAngularSpeed", i)
            .zip(positive(s.f64_at("tyreRadius", i)))
            .map(|(w, r)| w.abs() * r),
        pressure_kpa: positive(p.f64_at("wheelsPressure", i)).map(|psi| psi * KPA_PER_PSI),
        surface_c: across(p, i),
        ride_height_m: p.f64_at("rideHeight", axle),
        shock_mps: shock,
        brake_bar: None,
        load_n: p.f64_at("wheelLoad", i),
        camber_rad: p.f64_at("camberRAD", i),
        travel_m: p.f64_at("suspensionTravel", i),
        brake_temp_c: set(p.f64_at("brakeTemp", i)),
    }
}

fn damage(p: &View<'_>) -> Option<Damage> {
    // Only the first four are used by the game.
    let [front, rear, left, right] = [0, 1, 2, 3].map(|i| p.f64_at("carDamage", i));
    front.is_some().then_some(Damage {
        front,
        rear,
        left,
        right,
        centre: None,
    })
}

fn controls(p: &View<'_>, s: &View<'_>) -> Vec<Control> {
    let ers = s.flag("hasERS") == Some(true);
    let round = |v: f64| (v * 1e4).round() / 1e4;
    [
        (
            "brake_bias",
            positive(p.f64("brakeBias")).map(|b| b * 100.0),
        ),
        ("traction_control", p.f64("tc")),
        ("abs", p.f64("abs")),
        (
            "engine_brake",
            p.f64("engineBrake")
                .filter(|_| s.i32("engineBrakeSettingsCount").unwrap_or(0) > 0),
        ),
        ("ers_recovery", p.f64("ersRecoveryLevel").filter(|_| ers)),
        (
            "ers_power",
            p.f64("ersPowerLevel")
                .filter(|_| ers && s.i32("ersPowerControllerCount").unwrap_or(0) > 0),
        ),
    ]
    .into_iter()
    .filter_map(|(id, v)| {
        Some(Control {
            id: id.into(),
            value: round(v?),
        })
    })
    .collect()
}

fn pit_window(s: &View<'_>) -> Option<PitWindow> {
    let (from, to) = (s.i32("PitWindowStart")?, s.i32("PitWindowEnd")?);
    (to > 0 && to >= from).then(|| PitWindow {
        from: f64::from(from),
        to: f64::from(to),
        unit: if s.flag("isTimedRace") == Some(true) {
            WindowUnit::Minute
        } else {
            WindowUnit::Lap
        },
    })
}

fn car_telemetry(snap: &Snapshot, tracker: &mut Tracker, time_s: f64) -> CarTelemetry {
    let (p, g, s) = (snap.physics(), snap.graphics(), snap.statics());
    let mut warnings = Vec::new();
    if p.flag("pitLimiterOn") == Some(true) {
        warnings.push(Warning::PitLimiter);
    }
    let damage = damage(&p);
    if damage.as_ref().is_some_and(|d| !d.hurt().is_empty()) {
        warnings.push(Warning::Damage);
    }
    let shock = tracker.shock(p.arr("suspensionTravel"), time_s);
    let window = pit_window(&s);
    let has = |n: &str| s.flag(n) == Some(true);
    CarTelemetry {
        // The pedal is pressed at 1, when the clutch is open.
        clutch: p.f64("clutch").map(|c| (1.0 - c).clamp(0.0, 1.0)),
        // A -1..1 input; the steering lock is not told.
        steering_rad: None,
        // Left-handed axes (x right, y up, z forward): left is negative x.
        yaw_rate_rps: p.f64_at("localAngularVel", 1).map(|r| -r),
        lat_accel_mps2: p.f64_at("accG", 0).map(|a| -a * G),
        long_accel_mps2: p.f64_at("accG", 2).map(|a| a * G),
        velocity_x_mps: p.f64_at("localVelocity", 2),
        velocity_y_mps: p.f64_at("localVelocity", 0).map(|v| -v),
        abs_active: None,
        warnings,
        tyres: Tyres {
            lf: tyre(&p, 0),
            rf: tyre(&p, 1),
            lr: tyre(&p, 2),
            rr: tyre(&p, 3),
        },
        tyre_compound: g.text("tyreCompound"),
        pit: PitState {
            in_stall: g.flag("isInPit"),
            mandatory_done: window.and(g.flag("mandatoryPitDone")),
            window,
            ..Default::default()
        },
        controls: controls(&p, &s),
        drs_active: has("hasDRS")
            .then(|| p.flag("drsEnabled").or(p.f64("drs").map(|d| d > 0.5)))
            .flatten(),
        battery_pct: (has("hasKERS") || has("hasERS"))
            .then(|| p.f64("kersCharge"))
            .flatten(),
        weight_penalty_kg: p.f64("ballast"),
        wheels: Some(Wheels {
            lf: wheel(&p, &s, 0, shock[0]),
            rf: wheel(&p, &s, 1, shock[1]),
            lr: wheel(&p, &s, 2, shock[2]),
            rr: wheel(&p, &s, 3, shock[3]),
        }),
        damage,
        penalty_s: g.f64("penaltyTime").filter(|t| *t >= 0.0),
        ..Default::default()
    }
}

/// The player's car as the graphics page sees it.
fn player_car(snap: &Snapshot) -> CarState {
    let (p, g, s) = (snap.physics(), snap.graphics(), snap.statics());
    let laps = g.i32("completedLaps").filter(|l| *l >= 0);
    let position = g.i32("position").filter(|p| *p > 0).map(|p| p as u32);
    let surface = if g.flag("isInPit") == Some(true) {
        TrackSurface::InPitStall
    } else if p.i32("numberOfTyresOut").unwrap_or(0) >= OFF_TRACK_WHEELS {
        TrackSurface::OffTrack
    } else {
        TrackSurface::OnTrack
    };
    CarState {
        idx: PLAYER_IDX,
        number: PLAYER_IDX.to_string(),
        driver: driver(&s),
        team: None,
        car_name: s.text("carModel"),
        class_name: None,
        class_id: None,
        rating: None,
        is_player: true,
        position,
        class_position: position,
        lap: laps.map(|l| l + 1),
        laps_completed: laps,
        lap_dist_pct: g
            .f64("normalizedCarPosition")
            .filter(|p| (0.0..=1.0).contains(p)),
        on_pit_road: g.flag("isInPitLane") == Some(true) || surface == TrackSurface::InPitStall,
        surface,
        last_lap_s: lap_s(g.i32("iLastTime")),
        best_lap_s: lap_s(g.i32("iBestTime")),
        gap_to_leader_s: None,
        est_lap_s: None,
        extra: CarExtra {
            tyre_compound: g.text("tyreCompound"),
            gear: p.i32("gear").map(|g| g - 1),
            rpm: p.f64("rpms").filter(|r| *r >= 0.0),
            ..Default::default()
        },
    }
}

/// Another car, from CrewChief's page.
fn other_car(v: &View<'_>, race: bool) -> Option<CarState> {
    let idx = usize::try_from(v.i32("carId")?).ok()?;
    let laps = v.i32("lapCount").filter(|l| *l >= 0);
    // The running order counts from 0, the leaderboard from 1.
    let position = if race {
        v.i32("carRealTimeLeaderboardPosition")
            .filter(|p| *p >= 0)
            .map(|p| p as u32 + 1)
    } else {
        v.i32("carLeaderboardPosition")
            .filter(|p| *p > 0)
            .map(|p| p as u32)
    };
    let in_pit = v.flag("isCarInPit") == Some(true);
    Some(CarState {
        idx,
        number: idx.to_string(),
        driver: v.text("driverName").unwrap_or_else(|| format!("Car {idx}")),
        team: None,
        car_name: v.text("carModel"),
        class_name: None,
        class_id: None,
        rating: None,
        is_player: idx == PLAYER_IDX,
        position,
        class_position: position,
        lap: laps.map(|l| l + 1),
        laps_completed: laps,
        lap_dist_pct: v.f64("spLineLength").filter(|p| (0.0..=1.0).contains(p)),
        on_pit_road: in_pit || v.flag("isCarInPitline") == Some(true),
        surface: if in_pit {
            TrackSurface::InPitStall
        } else {
            TrackSurface::OnTrack
        },
        last_lap_s: lap_s(v.i32("lastLapTimeMS")),
        best_lap_s: lap_s(v.i32("bestLapMS")),
        gap_to_leader_s: None,
        est_lap_s: None,
        extra: CarExtra::default(),
    })
}

fn phase(g: &View<'_>, race: bool) -> SessionPhase {
    if g.i32("flag") == Some(FLAG_CHECKERED) {
        return SessionPhase::Checkered;
    }
    // On the grid, the lap clock has not started.
    if race && g.i32("completedLaps") == Some(0) && g.i32("iCurrentTime").unwrap_or(0) <= 0 {
        return SessionPhase::Warmup;
    }
    SessionPhase::Racing
}

pub fn build_frame(snap: &Snapshot, tracker: &mut Tracker, tick: u64, now_ms: i64) -> Frame {
    let (p, g, s) = (snap.physics(), snap.graphics(), snap.statics());
    let Clock {
        session_num,
        uid,
        time_s,
    } = tracker.advance(&snap.sample(now_ms));
    let raw_kind = g.i32("session").unwrap_or(-1);
    let kind = session_kind(raw_kind);
    let race = kind == SessionKind::Race;
    let phase = phase(&g, race);

    let mut player = player_car(snap);
    let mut cars = Vec::new();
    for v in snap.vehicles() {
        let Some(c) = other_car(&v, race) else {
            continue;
        };
        if c.idx == PLAYER_IDX {
            // The player's own pages are the more exact; the app knows names.
            if player.driver == "Player" {
                player.driver = c.driver;
            }
            continue;
        }
        if !cars.iter().any(|o: &CarState| o.idx == c.idx) {
            cars.push(c);
        }
    }
    cars.insert(0, player);

    let timed = timed(&s, &g);
    let laps_total = (!timed)
        .then(|| g.i32("numberOfLaps"))
        .flatten()
        .filter(|l| (1..10_000).contains(l));
    let laps_remaining = laps_total
        .filter(|_| race)
        .zip(g.i32("completedLaps"))
        .map(|(t, c)| (t - c).max(0));

    let sectors = tracker.sectors(
        s.i32("sectorCount").unwrap_or(0),
        g.i32("currentSectorIndex").unwrap_or(0),
        g.f64("normalizedCarPosition").unwrap_or(-1.0),
    );
    let car = car_telemetry(snap, tracker, time_s);
    let best = cars[0].best_lap_s;
    let player = PlayerTelemetry {
        on_track: g.i32("status") == Some(STATUS_LIVE),
        speed_mps: p.f64("speedKmh").map(|v| v / 3.6),
        rpm: p.f64("rpms"),
        gear: p.i32("gear").map(|g| g - 1),
        throttle: p.f64("gas"),
        brake: p.f64("brake"),
        fuel_l: p.f64("fuel"),
        fuel_capacity_l: positive(s.f64("maxFuel")),
        fuel_use_lph: None,
        incidents: None,
        current_lap_s: g
            .i32("iCurrentTime")
            .filter(|ms| (0..MAX_LAP_MS).contains(ms))
            .map(|ms| f64::from(ms) / 1000.0),
        delta_to_best_s: best.and(p.f64("performanceMeter")),
        water_temp_c: None,
        oil_temp_c: None,
        car,
    };

    Frame {
        game: GAME.into(),
        tick,
        timestamp_ms: now_ms,
        session: SessionState {
            session_num,
            kind,
            phase,
            time_s,
            time_remaining_s: timed.then(|| time_left(&g)).flatten(),
            laps_remaining,
            laps_total,
            flags: flags(
                g.i32("flag").unwrap_or(0),
                race && phase == SessionPhase::Racing,
            ),
            track: s.text("track").map(|name| TrackInfo {
                name,
                config: s.text("trackConfiguration"),
                length_m: positive(s.f64("trackSPlineLength")),
                sectors,
            }),
            uid: Some(uid),
            time_total_s: None,
            time_of_day_s: None,
        },
        player_car_idx: Some(PLAYER_IDX),
        player: Some(player),
        cars,
        weather: Some(Weather {
            air_temp_c: set(p.f64("airTemp")),
            track_temp_c: set(p.f64("roadTemp")),
            // The wind is set in km/h, its direction in degrees.
            wind_mps: g.f64("windSpeed").filter(|w| *w >= 0.0).map(|w| w / 3.6),
            wind_dir_deg: g
                .f64("windDirection")
                .filter(|_| g.f64("windSpeed").is_some_and(|w| w > 0.0))
                .map(|d| d.rem_euclid(360.0)),
            track_grip: positive(g.f64("surfaceGrip")),
            ..Default::default()
        }),
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use crate::layout::Writer;

    /// A car on track: lap 3 of a 10-lap race at Monza, third.
    pub fn snapshot() -> (Writer<'static>, Writer<'static>, Writer<'static>) {
        let mut p = Writer::new(&PHYSICS);
        p.i32("packetId", 100)
            .f32("gas", 0.75)
            .f32("brake", 0.0)
            .f32("fuel", 31.5)
            .i32("gear", 4)
            .i32("rpms", 7200)
            .f32("steerAngle", -0.1)
            .f32("speedKmh", 180.0)
            .f32s("accG", &[0.5, 1.0, -0.25])
            .f32s("wheelLoad", &[3000.0, 3200.0, 3500.0, 3600.0])
            .f32s("wheelsPressure", &[27.5, 27.5, 26.0, 26.0])
            .f32s("wheelAngularSpeed", &[150.0, 150.0, -160.0, 160.0])
            .f32s("tyreWear", &[98.5, 98.0, 99.0, 97.5])
            .f32s("tyreCoreTemperature", &[85.0, 86.0, 80.0, 81.0])
            .f32s("camberRAD", &[-0.05, -0.05, -0.03, -0.03])
            .f32s("suspensionTravel", &[0.02, 0.02, 0.03, 0.03])
            .f32("tc", 0.12)
            .f32("abs", 0.0)
            .f32s("carDamage", &[0.0, 0.0, 0.0, 0.0, 0.0])
            .f32s("rideHeight", &[0.05, 0.07])
            .f32("ballast", 10.0)
            .f32("airTemp", 24.0)
            .f32("roadTemp", 32.0)
            .f32s("localAngularVel", &[0.0, 0.2, 0.0])
            .f32("performanceMeter", -0.35)
            .f32s("brakeTemp", &[400.0, 410.0, 300.0, 0.0])
            .f32("clutch", 0.0)
            .f32s("tyreTempI", &[90.0, 91.0, 85.0, 86.0])
            .f32s("tyreTempM", &[88.0, 89.0, 84.0, 85.0])
            .f32s("tyreTempO", &[86.0, 87.0, 83.0, 84.0])
            .f32("brakeBias", 0.58)
            .f32s("localVelocity", &[-1.0, 0.0, 49.5]);
        let mut g = Writer::new(&GRAPHICS);
        g.i32("packetId", 50)
            .i32("status", STATUS_LIVE)
            .i32("session", 2)
            .text("lastTime", "1:48.250")
            .i32("completedLaps", 2)
            .i32("position", 3)
            .i32("iCurrentTime", 30_500)
            .i32("iLastTime", 108_250)
            .i32("iBestTime", 107_900)
            .f32("sessionTimeLeft", -1.0)
            .f32("distanceTraveled", 12_000.0)
            .i32("currentSectorIndex", 0)
            .i32("numberOfLaps", 10)
            .text("tyreCompound", "Medium (M)")
            .f32("normalizedCarPosition", 0.3)
            .f32("surfaceGrip", 0.98)
            .f32("windSpeed", 18.0)
            .f32("windDirection", 370.0);
        let mut s = Writer::new(&STATIC);
        s.text("smVersion", "1.7")
            .text("acVersion", "1.16")
            .i32("numberOfSessions", 1)
            .i32("numCars", 20)
            .text("carModel", "ks_ferrari_488_gt3")
            .text("track", "monza")
            .text("playerName", "Ada")
            .text("playerSurname", "Lovelace")
            .i32("sectorCount", 3)
            .i32("maxRpm", 7800)
            .f32("maxFuel", 120.0)
            .f32s("tyreRadius", &[0.34, 0.34, 0.35, 0.35])
            .f32("trackSPlineLength", 5758.0);
        (p, g, s)
    }

    pub fn snap(p: &Writer<'_>, g: &Writer<'_>, s: &Writer<'_>) -> Snapshot {
        Snapshot {
            physics: p.bytes.clone(),
            graphics: g.bytes.clone(),
            statics: s.bytes.clone(),
            crewchief: None,
        }
    }

    /// CrewChief's page with these cars: id, driver, laps, spline, positions.
    pub fn crewchief(cars: &[(i32, &str, i32, f32, i32, i32, bool)]) -> Vec<u8> {
        let mut head = Writer::new(&CREWCHIEF);
        head.i32("numVehicles", cars.len() as i32)
            .text("serverName", "Test Server");
        let mut bytes = head.bytes;
        for &(id, name, laps, spline, board, realtime, pit) in cars {
            let mut v = Writer::new(&VEHICLE);
            v.i32("carId", id)
                .text("driverName", name)
                .text("carModel", "ks_ferrari_488_gt3")
                .i32("lapCount", laps)
                .f32("spLineLength", spline)
                .i32("bestLapMS", 107_000 + id)
                .i32("lastLapTimeMS", 108_000 + id)
                .i32("carLeaderboardPosition", board)
                .i32("carRealTimeLeaderboardPosition", realtime)
                .i32("isCarInPit", i32::from(pit))
                .i32("isCarInPitline", i32::from(pit))
                .i32("isConnected", i32::from(id != 9));
            bytes.extend(v.bytes);
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-4)
    }

    #[test]
    fn the_players_car_in_a_race() {
        let (p, g, s) = snapshot();
        let mut t = Tracker::default();
        let f = build_frame(&snap(&p, &g, &s), &mut t, 7, 1_000);
        assert_eq!(
            (f.game.as_str(), f.tick, f.timestamp_ms),
            ("ac", 7, 1_000)
        );
        let ss = &f.session;
        assert_eq!(
            (ss.kind, ss.phase),
            (SessionKind::Race, SessionPhase::Racing)
        );
        assert_eq!((ss.session_num, ss.uid, ss.time_s), (0, Some(0), 0.0));
        assert_eq!((ss.laps_total, ss.laps_remaining), (Some(10), Some(8)));
        assert_eq!(ss.time_remaining_s, None, "a race of laps");
        assert_eq!(ss.flags, [Flag::Green]);
        let track = ss.track.as_ref().unwrap();
        assert_eq!(track.name, "monza");
        assert_eq!(track.config, None);
        assert_eq!(track.length_m, Some(5758.0));
        assert!(track.sectors.is_empty(), "not learned yet");

        assert_eq!(f.player_car_idx, Some(0));
        assert_eq!(f.cars.len(), 1, "no CrewChief: the player alone");
        let c = f.player_car().unwrap();
        assert_eq!(c.driver, "Ada Lovelace");
        assert_eq!(c.number, "0");
        assert_eq!(c.car_name.as_deref(), Some("ks_ferrari_488_gt3"));
        assert_eq!((c.position, c.class_position), (Some(3), Some(3)));
        assert_eq!((c.lap, c.laps_completed), (Some(3), Some(2)));
        assert!(close(c.lap_dist_pct, 0.3));
        assert_eq!((c.on_pit_road, c.surface), (false, TrackSurface::OnTrack));
        assert_eq!((c.last_lap_s, c.best_lap_s), (Some(108.25), Some(107.9)));
        assert_eq!(c.extra.tyre_compound.as_deref(), Some("Medium (M)"));
        assert_eq!((c.extra.gear, c.extra.rpm), (Some(3), Some(7200.0)));

        let pl = f.player.unwrap();
        assert!(pl.on_track);
        assert!(close(pl.speed_mps, 50.0));
        assert_eq!((pl.gear, pl.rpm), (Some(3), Some(7200.0)));
        assert_eq!((pl.throttle, pl.brake), (Some(0.75), Some(0.0)));
        assert_eq!((pl.fuel_l, pl.fuel_capacity_l), (Some(31.5), Some(120.0)));
        assert_eq!(pl.current_lap_s, Some(30.5));
        assert!(close(pl.delta_to_best_s, -0.35));
        assert_eq!(
            (pl.fuel_use_lph, pl.incidents, pl.water_temp_c),
            (None, None, None)
        );

        let car = pl.car;
        assert_eq!(car.clutch, Some(1.0), "pedal up: engaged");
        assert_eq!(car.steering_rad, None);
        assert!(close(car.yaw_rate_rps, -0.2));
        assert!(close(car.lat_accel_mps2, -0.5 * G));
        assert!(close(car.long_accel_mps2, -0.25 * G));
        assert!(close(car.velocity_x_mps, 49.5));
        assert!(close(car.velocity_y_mps, 1.0));
        assert_eq!(car.warnings, []);
        assert_eq!(car.tyre_compound.as_deref(), Some("Medium (M)"));
        let lf = &car.tyres.lf;
        assert!(close(lf.hot_kpa, 27.5 * KPA_PER_PSI));
        assert_eq!(lf.cold_kpa, None);
        assert_eq!(
            lf.temp_c,
            [Some(86.0), Some(88.0), Some(90.0)],
            "outer first"
        );
        assert!(close(lf.tread[1], 0.985));
        assert_eq!((lf.tread[0], lf.tread[2]), (None, None));
        assert_eq!(lf.core_c, Some(85.0));
        assert!(close(car.tyres.rr.min_tread(), 0.975));
        let controls: Vec<(&str, f64)> = car
            .controls
            .iter()
            .map(|c| (c.id.as_str(), c.value))
            .collect();
        assert_eq!(
            controls,
            [
                ("brake_bias", 58.0),
                ("traction_control", 0.12),
                ("abs", 0.0)
            ]
        );
        assert_eq!(
            (car.drs_active, car.battery_pct),
            (None, None),
            "neither on this car"
        );
        assert_eq!(car.weight_penalty_kg, Some(10.0));
        assert_eq!(car.pit.in_stall, Some(false));
        assert_eq!((car.pit.window, car.pit.mandatory_done), (None, None));
        assert_eq!(car.penalty_s, Some(0.0));
        let d = car.damage.unwrap();
        assert!(d.hurt().is_empty());
        assert_eq!((d.front, d.centre), (Some(0.0), None));
        let w = car.wheels.unwrap();
        assert!(close(w.lf.speed_mps, 51.0));
        assert!(
            close(w.lr.speed_mps, 56.0),
            "turning backwards counts as speed"
        );
        assert!(close(w.lf.ride_height_m, 0.05));
        assert!(close(w.rr.ride_height_m, 0.07));
        assert_eq!(w.lf.shock_mps, None, "one sample");
        assert_eq!(w.lf.load_n, Some(3000.0));
        assert!(close(w.lf.camber_rad, -0.05));
        assert!(close(w.lr.travel_m, 0.03));
        assert_eq!((w.lf.brake_temp_c, w.rr.brake_temp_c), (Some(400.0), None));
        assert_eq!(w.rf.surface_c, [Some(87.0), Some(89.0), Some(91.0)]);
        assert_eq!(w.lf.brake_bar, None);

        let wx = f.weather.unwrap();
        assert_eq!((wx.air_temp_c, wx.track_temp_c), (Some(24.0), Some(32.0)));
        assert!(close(wx.wind_mps, 5.0));
        assert!(close(wx.wind_dir_deg, 10.0));
        assert!(close(wx.track_grip, 0.98));
        assert_eq!(wx.wetness, None);
    }

    #[test]
    fn sessions_flags_and_phases() {
        let (p, mut g, mut s) = snapshot();
        let mut t = Tracker::default();
        let mut build = |g: &crate::layout::Writer<'_>, s: &crate::layout::Writer<'_>| {
            build_frame(&snap(&p, g, s), &mut t, 0, 0)
        };
        for (v, kind) in [
            (-1, SessionKind::Other),
            (0, SessionKind::Practice),
            (1, SessionKind::Qualify),
            (2, SessionKind::Race),
            (3, SessionKind::Practice),
            (4, SessionKind::Practice),
            (5, SessionKind::Other),
            (6, SessionKind::Other),
            (7, SessionKind::Other),
        ] {
            assert_eq!(session_kind(v), kind, "{v}");
        }
        for (flag, want) in [
            (0, vec![Flag::Green]),
            (1, vec![Flag::Green, Flag::Blue]),
            (2, vec![Flag::Yellow]),
            (3, vec![Flag::Green, Flag::Black]),
            (4, vec![Flag::Green, Flag::White]),
            (6, vec![Flag::Green, Flag::Black]),
            (9, vec![Flag::Green]),
        ] {
            g.i32("flag", flag);
            assert_eq!(build(&g, &s).session.flags, want, "{flag}");
        }
        g.i32("flag", 5);
        let f = build(&g, &s);
        assert_eq!(f.session.phase, SessionPhase::Checkered);
        assert_eq!(f.session.flags, [Flag::Checkered]);
        assert_eq!(flags(1, false), [Flag::Blue], "no green outside races");
        assert_eq!(flags(0, false), []);

        // On the grid.
        g.i32("flag", 0)
            .i32("completedLaps", 0)
            .i32("iCurrentTime", 0);
        let f = build(&g, &s);
        assert_eq!(f.session.phase, SessionPhase::Warmup);
        assert_eq!(f.session.flags, [], "not racing yet");
        assert_eq!(f.session.laps_remaining, Some(10));

        // Practice of 30 minutes, 12:30 left.
        g.i32("session", 0)
            .i32("iCurrentTime", 0)
            .f32("sessionTimeLeft", 750_000.0);
        let f = build(&g, &s);
        assert_eq!(f.session.phase, SessionPhase::Racing);
        assert_eq!(f.session.kind, SessionKind::Practice);
        assert_eq!(f.session.session_num, 1, "a new session");
        assert_eq!(f.session.time_remaining_s, Some(750.0));
        assert_eq!(
            (f.session.laps_total, f.session.laps_remaining),
            (None, None)
        );

        // A timed race with a pit window in minutes.
        g.i32("session", 2).i32("iCurrentTime", 5);
        s.i32("isTimedRace", 1)
            .i32("PitWindowStart", 10)
            .i32("PitWindowEnd", 20);
        let f = build(&g, &s);
        assert_eq!(f.session.time_remaining_s, Some(750.0));
        assert_eq!(
            (f.session.laps_total, f.session.laps_remaining),
            (None, None)
        );
        let pit = f.player.unwrap().car.pit;
        assert_eq!(
            pit.window,
            Some(PitWindow {
                from: 10.0,
                to: 20.0,
                unit: WindowUnit::Minute
            })
        );
        assert_eq!(pit.mandatory_done, Some(false));
        s.i32("isTimedRace", 0);
        g.i32("mandatoryPitDone", 1);
        let pit = build(&g, &s).player.unwrap().car.pit;
        assert_eq!(pit.window.unwrap().unit, WindowUnit::Lap);
        assert_eq!(pit.mandatory_done, Some(true));
        s.i32("PitWindowStart", 30).i32("PitWindowEnd", 20);
        assert_eq!(
            build(&g, &s).player.unwrap().car.pit.window,
            None,
            "backwards"
        );

        // Unlimited time; no limit on laps.
        g.f32("sessionTimeLeft", -1.0).i32("numberOfLaps", 0);
        let f = build(&g, &s);
        assert_eq!(
            (f.session.time_remaining_s, f.session.laps_total),
            (None, None)
        );
    }

    #[test]
    fn what_the_game_leaves_at_zero_is_unknown() {
        let (mut p, mut g, mut s) = snapshot();
        g.i32("iLastTime", 0)
            .i32("iBestTime", i32::MAX)
            .i32("position", 0)
            .i32("iCurrentTime", -1)
            .f32("normalizedCarPosition", 1.5)
            .f32("surfaceGrip", 0.0)
            .text("tyreCompound", "");
        p.f32s("tyreTempI", &[0.0; 4])
            .f32s("tyreTempM", &[0.0; 4])
            .f32s("tyreTempO", &[0.0; 4])
            .f32s("wheelsPressure", &[0.0; 4])
            .f32s("tyreWear", &[0.0; 4])
            .f32("brakeBias", 0.0)
            .f32("airTemp", 0.0);
        s.f32("maxFuel", 0.0)
            .f32("trackSPlineLength", 0.0)
            .f32s("tyreRadius", &[0.0; 4])
            .text("playerName", "")
            .text("playerSurname", "")
            .text("playerNick", "Speedy");
        let f = build_frame(&snap(&p, &g, &s), &mut Tracker::default(), 0, 0);
        let c = f.player_car().unwrap();
        assert_eq!((c.last_lap_s, c.best_lap_s, c.position), (None, None, None));
        assert_eq!(
            (c.lap_dist_pct, c.extra.tyre_compound.clone()),
            (None, None)
        );
        assert_eq!(c.driver, "Speedy");
        let pl = f.player.unwrap();
        assert_eq!(pl.current_lap_s, None);
        assert_eq!(pl.delta_to_best_s, None, "no best lap to compare to");
        assert_eq!(pl.fuel_capacity_l, None);
        let lf = &pl.car.tyres.lf;
        assert_eq!(
            (lf.hot_kpa, lf.temp_c, lf.min_tread()),
            (None, [None; 3], None)
        );
        assert!(pl.car.controls.iter().all(|c| c.id != "brake_bias"));
        let w = pl.car.wheels.unwrap();
        assert_eq!((w.lf.speed_mps, w.lf.pressure_kpa), (None, None));
        assert_eq!(f.session.track.unwrap().length_m, None);
        let wx = f.weather.unwrap();
        assert_eq!((wx.air_temp_c, wx.track_grip), (None, None));
        s.text("playerNick", "");
        let f = build_frame(&snap(&p, &g, &s), &mut Tracker::default(), 0, 0);
        assert_eq!(f.player_car().unwrap().driver, "Player");
    }

    #[test]
    fn the_car_in_trouble_and_in_the_pits() {
        let (mut p, mut g, mut s) = snapshot();
        p.f32s("carDamage", &[12.5, 0.0, 3.0, 0.0, 99.0])
            .i32("pitLimiterOn", 1)
            .i32("numberOfTyresOut", 3)
            .f32("clutch", 0.25);
        g.f32("penaltyTime", 5.0);
        let f = build_frame(&snap(&p, &g, &s), &mut Tracker::default(), 0, 0);
        let car = &f.player.as_ref().unwrap().car;
        assert_eq!(car.warnings, [Warning::PitLimiter, Warning::Damage]);
        let d = car.damage.as_ref().unwrap();
        assert_eq!(
            d.hurt(),
            [("front", 12.5), ("left", 3.0)],
            "the fifth is unused"
        );
        assert_eq!(car.penalty_s, Some(5.0));
        assert_eq!(car.clutch, Some(0.75));
        assert_eq!(f.player_car().unwrap().surface, TrackSurface::OffTrack);
        p.i32("numberOfTyresOut", 2);
        g.i32("isInPitLane", 1);
        let f = build_frame(&snap(&p, &g, &s), &mut Tracker::default(), 0, 0);
        let c = f.player_car().unwrap();
        assert_eq!((c.on_pit_road, c.surface), (true, TrackSurface::OnTrack));
        g.i32("isInPitLane", 0).i32("isInPit", 1);
        let f = build_frame(&snap(&p, &g, &s), &mut Tracker::default(), 0, 0);
        let c = f.player_car().unwrap();
        assert_eq!((c.on_pit_road, c.surface), (true, TrackSurface::InPitStall));
        assert_eq!(f.player.unwrap().car.pit.in_stall, Some(true));

        // A hybrid with DRS and engine-brake maps.
        s.i32("hasDRS", 1)
            .i32("hasERS", 1)
            .i32("engineBrakeSettingsCount", 13)
            .i32("ersPowerControllerCount", 3);
        p.i32("drsEnabled", 1)
            .f32("kersCharge", 0.6)
            .i32("engineBrake", 5)
            .i32("ersRecoveryLevel", 2)
            .i32("ersPowerLevel", 1);
        let car = build_frame(&snap(&p, &g, &s), &mut Tracker::default(), 0, 0)
            .player
            .unwrap()
            .car;
        assert_eq!(
            (car.drs_active, car.battery_pct),
            (Some(true), Some(0.6f32 as f64))
        );
        let ids: Vec<(&str, f64)> = car
            .controls
            .iter()
            .map(|c| (c.id.as_str(), c.value))
            .collect();
        assert_eq!(
            ids,
            [
                ("brake_bias", 58.0),
                ("traction_control", 0.12),
                ("abs", 0.0),
                ("engine_brake", 5.0),
                ("ers_recovery", 2.0),
                ("ers_power", 1.0)
            ]
        );
        s.i32("ersPowerControllerCount", 0)
            .i32("hasERS", 0)
            .i32("hasKERS", 1);
        let car = build_frame(&snap(&p, &g, &s), &mut Tracker::default(), 0, 0)
            .player
            .unwrap()
            .car;
        assert_eq!(car.battery_pct, Some(0.6f32 as f64), "KERS");
        assert!(car.controls.iter().all(|c| !c.id.starts_with("ers")));
    }

    #[test]
    fn every_car_from_crewchief() {
        let (p, g, s) = snapshot();
        let mut snapshot = snap(&p, &g, &s);
        snapshot.crewchief = Some(crewchief(&[
            (0, "Ada Lovelace", 2, 0.3, 3, 2, false),
            (4, "Grace Hopper", 2, 0.35, 1, 1, false),
            (7, "Alan Turing", 2, 0.25, 2, 3, true),
            (9, "Left Early", 1, 0.5, 4, 4, false),
            (4, "Twice", 2, 0.35, 1, 1, false),
        ]));
        let f = build_frame(&snapshot, &mut Tracker::default(), 0, 0);
        let ids: Vec<usize> = f.cars.iter().map(|c| c.idx).collect();
        assert_eq!(ids, [0, 4, 7], "disconnected and doubled cars left out");
        let me = &f.cars[0];
        assert_eq!(
            (me.position, me.last_lap_s),
            (Some(3), Some(108.25)),
            "own pages win"
        );
        let c = f.car(4).unwrap();
        assert_eq!(c.driver, "Grace Hopper");
        assert_eq!(c.position, Some(2), "running order, from 0");
        assert_eq!((c.lap, c.laps_completed), (Some(3), Some(2)));
        assert!(close(c.lap_dist_pct, 0.35));
        assert_eq!((c.last_lap_s, c.best_lap_s), (Some(108.004), Some(107.004)));
        assert!(!c.is_player && !c.on_pit_road);
        let c = f.car(7).unwrap();
        assert_eq!((c.on_pit_road, c.surface), (true, TrackSurface::InPitStall));
        assert_eq!(c.position, Some(4));

        // Outside races the leaderboard counts.
        let mut g = g;
        g.i32("session", 1);
        let mut snapshot = snap(&p, &g, &s);
        snapshot.crewchief = Some(crewchief(&[(4, "", 2, 0.35, 1, 1, false)]));
        let f = build_frame(&snapshot, &mut Tracker::default(), 0, 0);
        let c = f.car(4).unwrap();
        assert_eq!(c.position, Some(1));
        assert_eq!(c.driver, "Car 4");

        // The app names the player when the game does not.
        let mut s = s;
        s.text("playerName", "").text("playerSurname", "");
        let mut snapshot = snap(&p, &g, &s);
        snapshot.crewchief = Some(crewchief(&[(0, "Ada L", 2, 0.3, 3, 2, false)]));
        let f = build_frame(&snapshot, &mut Tracker::default(), 0, 0);
        assert_eq!(f.cars[0].driver, "Ada L");

        // Channels name every value.
        let ch = snapshot.channels();
        let names: Vec<&str> = ch.iter().map(|c| c.name.as_str()).collect();
        assert!(names.is_sorted());
        assert!(names.contains(&"physics.tyreContactHeading"));
        assert!(names.contains(&"graphics.windDirection"));
        assert!(names.contains(&"static.PitWindowEnd"));
        assert!(names.contains(&"crewchief.serverName"));
        assert!(names.contains(&"crewchief.vehicle[0].tyreInflation"));
        let n = PHYSICS.fields.len()
            + GRAPHICS.fields.len()
            + STATIC.fields.len()
            + CREWCHIEF.fields.len()
            + VEHICLE.fields.len();
        assert_eq!(ch.len(), n);
    }

    #[test]
    fn sectors_and_dampers_over_a_lap() {
        let (mut p, mut g, s) = snapshot();
        let mut t = Tracker::default();
        let mut sectors = Vec::new();
        for (i, (sector, pos)) in [(0, 0.1), (1, 0.34), (2, 0.67), (0, 0.01)]
            .iter()
            .enumerate()
        {
            g.i32("currentSectorIndex", *sector)
                .f32("normalizedCarPosition", *pos);
            p.f32s("suspensionTravel", &[0.02 + 0.001 * i as f32; 4]);
            let f = build_frame(&snap(&p, &g, &s), &mut t, i as u64, 1_000 + 10 * i as i64);
            sectors = f.session.track.unwrap().sectors;
            if i > 0 {
                let shock = f.player.unwrap().car.wheels.unwrap().lf.shock_mps.unwrap();
                assert!((shock - 0.1).abs() < 1e-3, "{shock}");
            }
        }
        let rounded: Vec<f64> = sectors
            .iter()
            .map(|s| (s * 100.0).round() / 100.0)
            .collect();
        assert_eq!(rounded, [0.0, 0.34, 0.67]);
    }
}
