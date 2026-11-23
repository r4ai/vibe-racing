use std::time::Duration;

use vr_core::{
    Capabilities, CarState, CarTelemetry, Channel, Control, Extras, Flag, Frame, PitState,
    PlayerTelemetry, Poll, SessionKind, SessionPhase, SessionState, Setup, SetupItem, SourceError,
    TelemetrySource, TrackInfo, TrackSurface, Tyre, Tyres, Warning, Weather,
};

use crate::Pacer;

/// How the player's car misbehaves, in seconds per lap.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Balance {
    /// Steering more without turning more, off the pedals.
    pub understeer_mid_s: f64,
    /// Steering against the turn, on the throttle.
    pub oversteer_exit_s: f64,
    pub rear_lock_s: f64,
    pub wheelspin_s: f64,
}

/// Parameters of the generated race.
#[derive(Debug, Clone)]
pub struct SynthConfig {
    pub cars: usize,
    pub laps: i32,
    /// Index of the player's car.
    pub player_idx: usize,
    /// Simulation step in seconds of session time.
    pub dt: f64,
    pub fuel_start_l: f64,
    pub fuel_capacity_l: f64,
    pub fuel_per_lap_l: f64,
    /// Lap on which the player pits (enters at the end of it).
    pub player_pit_lap: Option<i32>,
    /// Lap on which the car directly ahead at the start pits.
    pub rival_pit_lap: Option<i32>,
    /// Session-time window of a local yellow.
    pub yellow: Option<(f64, f64)>,
    /// Session time the player picks up a 2x incident.
    pub incident_at: Option<f64>,
    /// Lap time lost per lap of tyre age, in seconds; cars differ by ±40 %.
    pub tyre_deg_s: f64,
    /// Lap times vary by up to this, in seconds.
    pub lap_noise_s: f64,
    /// One lap in this many has a mistake costing 1.5–4 s (0: none).
    pub mistake_one_in: u32,
    /// More stops: car index and the lap it pits at the end of.
    pub stops: Vec<(usize, i32)>,
    /// Rain: the track is dry before the first session time and fully wet
    /// from the second one; lap times grow by up to `rain_loss_s`.
    pub rain: Option<(f64, f64)>,
    pub rain_loss_s: f64,
    /// Corners in the player's telemetry; `None`: driving straight on.
    pub balance: Option<Balance>,
    /// Session time the player's car is damaged, and how long the crew needs.
    pub damage: Option<(f64, f64)>,
}

impl Default for SynthConfig {
    fn default() -> Self {
        Self {
            cars: 12,
            laps: 20,
            player_idx: 3,
            dt: 0.1,
            fuel_start_l: 30.0,
            fuel_capacity_l: 60.0,
            fuel_per_lap_l: 2.5,
            player_pit_lap: Some(9),
            rival_pit_lap: Some(7),
            yellow: Some((300.0, 330.0)),
            incident_at: Some(150.0),
            tyre_deg_s: 0.0,
            lap_noise_s: 0.4,
            mistake_one_in: 0,
            stops: Vec::new(),
            rain: None,
            rain_loss_s: 8.0,
            balance: None,
            damage: None,
        }
    }
}

const PIT_LOSS_S: f64 = 25.0;
const PIT_ROAD_PCT: f64 = 0.03;
const DRIVERS: &[&str] = &[
    "A. Senna",
    "B. Hart",
    "C. Ito",
    "You",
    "D. Kowalski",
    "E. Rossi",
    "F. Mueller",
    "G. Dubois",
    "H. Tanaka",
    "I. Novak",
    "J. Smith",
    "K. Lee",
    "L. Silva",
    "M. Garcia",
    "N. Berg",
    "O. Kim",
];

#[derive(Debug, Clone)]
struct SynthCar {
    idx: usize,
    base_lap: f64,
    laps_completed: i32,
    pct: f64,
    lap_start_t: f64,
    last_lap: Option<f64>,
    best_lap: Option<f64>,
    pit_laps: Vec<i32>,
    /// Laps completed when the tyres were last changed.
    stint_start: i32,
    /// Tyre wear of this car, in seconds per lap of age.
    deg_s: f64,
    finished: bool,
    finish_rank: Option<usize>,
}

impl SynthCar {
    fn current_lap(&self) -> i32 {
        self.laps_completed + 1
    }

    fn progress(&self) -> f64 {
        self.laps_completed as f64 + self.pct
    }

    fn lap_target(&self, cfg: &SynthConfig, t: f64) -> f64 {
        let lap = self.current_lap();
        let mut target = self.base_lap + noise(self.idx as u64, lap as u64) * cfg.lap_noise_s;
        target += self.deg_s * f64::from((lap - 1 - self.stint_start).max(0));
        if cfg.mistake_one_in > 0 {
            // Uniform in [0, 1): a mistake on 1 lap in `mistake_one_in`.
            let u = (noise(self.idx as u64 + 1000, lap as u64) + 1.0) / 2.0;
            if u * f64::from(cfg.mistake_one_in) < 1.0 {
                target += 1.5 + 2.5 * u * f64::from(cfg.mistake_one_in);
            }
        }
        target + cfg.rain_loss_s * wetness(cfg, t)
    }

    /// Progress-rate multiplier: pit lane time loss only applies on pit road.
    fn pace_factor(&self) -> f64 {
        if self.on_pit_road() {
            let lane = 2.0 * PIT_ROAD_PCT * self.base_lap;
            lane / (lane + PIT_LOSS_S)
        } else {
            1.0
        }
    }

    fn on_pit_road(&self) -> bool {
        (self.pit_laps.contains(&self.current_lap()) && self.pct > 1.0 - PIT_ROAD_PCT)
            || (self.pit_laps.contains(&self.laps_completed) && self.pct < PIT_ROAD_PCT)
    }
}

/// How wet the track is, from 0 (dry) to 1.
fn wetness(cfg: &SynthConfig, t: f64) -> f64 {
    match cfg.rain {
        Some((a, b)) if b > a => ((t - a) / (b - a)).clamp(0.0, 1.0),
        Some((a, _)) => f64::from(u8::from(t >= a)),
        None => 0.0,
    }
}

/// The game's words for a wetness.
fn wetness_word(w: f64) -> &'static str {
    match w {
        w if w < 0.05 => "dry",
        w if w < 0.35 => "lightly wet",
        w if w < 0.7 => "moderately wet",
        _ => "very wet",
    }
}

/// Deterministic pseudo-random in [-1, 1].
fn noise(a: u64, b: u64) -> f64 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    (x % 20_001) as f64 / 10_000.0 - 1.0
}

/// Deterministic generated race for demos and tests.
pub struct SynthSource {
    cfg: SynthConfig,
    t: f64,
    tick: u64,
    cars: Vec<SynthCar>,
    fuel_l: f64,
    pacer: Pacer,
    done: bool,
}

impl SynthSource {
    pub fn new(mut cfg: SynthConfig, speed: f64) -> Self {
        cfg.cars = cfg.cars.max(1);
        cfg.player_idx = cfg.player_idx.min(cfg.cars - 1);
        let cars = (0..cfg.cars)
            .map(|i| {
                let base_lap = if i == cfg.player_idx {
                    // Faster than the car ahead so the gap closes.
                    90.0 + (i as f64 - 2.0) * 0.35
                } else {
                    90.0 + i as f64 * 0.35
                };
                let first = if i == cfg.player_idx {
                    cfg.player_pit_lap
                } else if i + 1 == cfg.player_idx {
                    cfg.rival_pit_lap
                } else {
                    None
                };
                let mut pit_laps: Vec<i32> = first.into_iter().collect();
                pit_laps.extend(cfg.stops.iter().filter(|(c, _)| *c == i).map(|(_, l)| *l));
                SynthCar {
                    idx: i,
                    base_lap,
                    laps_completed: 0,
                    // Grid: staggered just behind the line.
                    pct: 1.0 - 0.02 * (i as f64 + 1.0),
                    lap_start_t: 0.0,
                    last_lap: None,
                    best_lap: None,
                    pit_laps,
                    stint_start: 0,
                    deg_s: cfg.tyre_deg_s * (1.0 + 0.4 * noise(i as u64, 99)),
                    finished: false,
                    finish_rank: None,
                }
            })
            .map(|mut c| {
                c.laps_completed = -1;
                c
            })
            .collect();
        Self {
            fuel_l: cfg.fuel_start_l,
            cfg,
            t: 0.0,
            tick: 0,
            cars,
            pacer: Pacer::new(speed),
            done: false,
        }
    }

    fn leader_laps(&self) -> i32 {
        self.cars
            .iter()
            .map(|c| c.laps_completed)
            .max()
            .unwrap_or(0)
    }

    fn step(&mut self) {
        let dt = self.cfg.dt;
        self.t += dt;
        let laps_total = self.cfg.laps;
        let checkered = self.leader_laps() >= laps_total;
        let player_idx = self.cfg.player_idx;
        let mut finishers = self.cars.iter().filter(|c| c.finished).count();
        for c in &mut self.cars {
            if c.finished {
                continue;
            }
            let target = c.lap_target(&self.cfg, self.t);
            c.pct += dt / target * c.pace_factor();
            if c.idx == player_idx {
                self.fuel_l = (self.fuel_l - self.cfg.fuel_per_lap_l * dt / target).max(0.0);
            }
            if c.pct >= 1.0 {
                c.pct -= 1.0;
                c.laps_completed += 1;
                if c.laps_completed > 0 {
                    let lap = self.t - c.lap_start_t;
                    c.last_lap = Some(lap);
                    c.best_lap = Some(c.best_lap.map_or(lap, |b: f64| b.min(lap)));
                }
                c.lap_start_t = self.t;
                if c.pit_laps.contains(&c.laps_completed) {
                    c.stint_start = c.laps_completed;
                    if c.idx == player_idx {
                        self.fuel_l = self.cfg.fuel_capacity_l;
                    }
                }
                if checkered || c.laps_completed >= laps_total {
                    c.finished = true;
                    c.finish_rank = Some(finishers);
                    finishers += 1;
                }
            }
        }
        if self.cars.iter().all(|c| c.finished) {
            self.done = true;
        }
    }

    fn frame(&self) -> Frame {
        let mut order: Vec<&SynthCar> = self.cars.iter().collect();
        // Finished cars keep their finishing order; the rest run on progress.
        order.sort_by(|a, b| match (a.finish_rank, b.finish_rank) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.progress().total_cmp(&a.progress()),
        });
        let leader = order[0];
        let leader_lap_s = leader.base_lap;

        let cars = self
            .cars
            .iter()
            .map(|c| {
                let pos = order.iter().position(|o| o.idx == c.idx).unwrap() as u32 + 1;
                let on_pit = c.on_pit_road();
                CarState {
                    idx: c.idx,
                    number: format!("{}", c.idx * 7 % 90 + 2),
                    driver: DRIVERS[c.idx % DRIVERS.len()].to_string(),
                    team: None,
                    car_name: Some("GT3 Demo".into()),
                    class_name: Some("GT3".into()),
                    class_id: Some(1),
                    rating: Some(2500 - c.idx as i32 * 50),
                    is_player: c.idx == self.cfg.player_idx,
                    position: Some(pos),
                    class_position: Some(pos),
                    lap: Some(c.current_lap().max(0)),
                    laps_completed: Some(c.laps_completed.max(0)),
                    lap_dist_pct: Some(c.pct),
                    on_pit_road: on_pit,
                    surface: if on_pit {
                        TrackSurface::ApproachingPits
                    } else {
                        TrackSurface::OnTrack
                    },
                    last_lap_s: c.last_lap,
                    best_lap_s: c.best_lap,
                    gap_to_leader_s: Some((leader.progress() - c.progress()) * leader_lap_s),
                    est_lap_s: Some(90.0),
                    extra: Default::default(),
                }
            })
            .collect();

        let mut flags = Vec::new();
        let phase = if self.leader_laps() >= self.cfg.laps {
            flags.push(Flag::Checkered);
            SessionPhase::Checkered
        } else {
            if self.leader_laps() == self.cfg.laps - 1 {
                flags.push(Flag::White);
            }
            match self.cfg.yellow {
                Some((a, b)) if self.t >= a && self.t < b => flags.push(Flag::YellowWaving),
                _ => flags.push(Flag::Green),
            }
            SessionPhase::Racing
        };

        let incidents = match self.cfg.incident_at {
            Some(at) if self.t >= at => 2,
            _ => 0,
        };
        let player = &self.cars[self.cfg.player_idx];
        Frame {
            game: "synthetic".into(),
            tick: self.tick,
            timestamp_ms: (self.t * 1000.0) as i64,
            session: SessionState {
                session_num: 0,
                kind: SessionKind::Race,
                phase,
                time_s: self.t,
                time_remaining_s: None,
                laps_remaining: Some((self.cfg.laps - self.leader_laps().max(0)).max(0)),
                laps_total: Some(self.cfg.laps),
                flags,
                track: Some(TrackInfo {
                    name: "Demo Raceway".into(),
                    config: Some("GP".into()),
                    length_m: Some(4500.0),
                    sectors: vec![0.0, 0.3, 0.7],
                }),
                uid: None,
                time_total_s: None,
                time_of_day_s: None,
            },
            player_car_idx: Some(self.cfg.player_idx),
            player: Some(PlayerTelemetry {
                on_track: true,
                speed_mps: Some(if player.on_pit_road() { 22.0 } else { 50.0 }),
                rpm: Some(7000.0),
                gear: Some(4),
                throttle: Some(0.8),
                brake: Some(0.0),
                fuel_l: Some(self.fuel_l),
                fuel_capacity_l: Some(self.cfg.fuel_capacity_l),
                fuel_use_lph: Some(self.cfg.fuel_per_lap_l * 3600.0 / 90.0),
                incidents: Some(incidents),
                current_lap_s: Some(self.t - player.lap_start_t),
                delta_to_best_s: None,
                water_temp_c: Some(88.0),
                oil_temp_c: Some(102.0),
                car: self.car_telemetry(),
            })
            .map(|p| self.driven(p)),
            cars,
            weather: {
                let w = wetness(&self.cfg, self.t);
                Some(Weather {
                    air_temp_c: Some(22.0 - 3.0 * w),
                    track_temp_c: Some(31.0 - 8.0 * w),
                    wetness: Some(wetness_word(w).into()),
                    precipitation: Some(w),
                    ..Default::default()
                })
            },
        }
    }
}

impl SynthSource {
    /// A steady car: balanced, with slowly wearing tyres.
    fn car_telemetry(&self) -> CarTelemetry {
        let player = &self.cars[self.cfg.player_idx];
        let age = f64::from((player.laps_completed - player.stint_start).max(0)) + player.pct;
        let tread = (1.0 - 0.01 * age).max(0.0);
        let tyre = Tyre {
            cold_kpa: Some(160.0),
            hot_kpa: None,
            temp_c: [Some(84.0), Some(86.0), Some(90.0)],
            tread: [Some(tread); 3],
            distance_m: Some(age * 4500.0),
        };
        CarTelemetry {
            clutch: Some(1.0),
            steering_rad: Some(0.0),
            yaw_rate_rps: Some(0.0),
            lat_accel_mps2: Some(0.0),
            long_accel_mps2: Some(0.0),
            velocity_x_mps: Some(50.0),
            velocity_y_mps: Some(0.0),
            abs_active: Some(false),
            tyres: Tyres {
                lf: tyre.clone(),
                rf: tyre.clone(),
                lr: tyre.clone(),
                rr: tyre,
            },
            pit: PitState {
                open: Some(true),
                fast_repairs_available: Some(1),
                fast_repairs_used: Some(0),
                repair_left_s: self
                    .cfg
                    .damage
                    .filter(|(at, _)| self.t >= *at)
                    .map(|(_, s)| s),
                ..Default::default()
            },
            controls: vec![
                Control {
                    id: "brake_bias".into(),
                    value: 54.0,
                },
                Control {
                    id: "traction_control".into(),
                    value: 3.0,
                },
            ],
            warnings: match self.cfg.damage {
                Some((at, _)) if self.t >= at => vec![Warning::Damage],
                _ => Vec::new(),
            },
            p2p_count: Some(200),
            p2p_active: Some(false),
            ..Default::default()
        }
    }
}

/// Corners of a lap, all alike.
const CORNERS: f64 = 10.0;
/// Understeer shows only after the car was followed for this long, a slide
/// once the steering is smoothed.
const NOTICE_S: f64 = 0.2;
const SMOOTH_S: f64 = 0.1;

impl SynthSource {
    /// The inputs and motion of a corner of `balance`: brakes, coasts, gets on
    /// the throttle. Each tendency lasts its share of the lap.
    fn driven(&self, mut p: PlayerTelemetry) -> PlayerTelemetry {
        let Some(b) = self.cfg.balance else { return p };
        let car = &self.cars[self.cfg.player_idx];
        let corner_s = car.base_lap / CORNERS;
        // Seconds since the corner began.
        let s = (car.pct * CORNERS).fract() * corner_s;
        let (brake_s, mid_s, exit_s) = (1.5, 2.5, 2.0);
        let c = &mut p.car;
        let turning = |c: &mut CarTelemetry, steer: f64, yaw: f64| {
            c.steering_rad = Some(steer);
            c.yaw_rate_rps = Some(yaw);
            c.lat_accel_mps2 = Some(20.0);
        };
        p.throttle = Some(0.9);
        if s < brake_s {
            (p.throttle, p.brake) = (Some(0.0), Some(0.8));
            if s < b.rear_lock_s / CORNERS {
                p.rpm = p.rpm.map(|r| r * 0.85);
            }
        } else if s < brake_s + mid_s {
            let s = s - brake_s;
            p.throttle = Some(0.0);
            // The wheel goes on turning, the car does not.
            let more = if b.understeer_mid_s > 0.0 {
                s.min(b.understeer_mid_s / CORNERS + NOTICE_S)
            } else {
                0.0
            };
            turning(c, 0.1 + 0.5 * more, 0.4 - 0.05 * more);
        } else if s < brake_s + mid_s + exit_s {
            let s = s - brake_s - mid_s;
            let caught = b.oversteer_exit_s > 0.0 && s < b.oversteer_exit_s / CORNERS + SMOOTH_S;
            turning(c, if caught { -0.1 } else { 0.1 }, 0.4);
        } else if s < brake_s + mid_s + exit_s + b.wheelspin_s / CORNERS {
            p.rpm = p.rpm.map(|r| r * 1.15);
        }
        p
    }
}

impl TelemetrySource for SynthSource {
    fn game(&self) -> &str {
        "synthetic"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::ALL
    }

    fn poll(&mut self, timeout: Duration) -> Result<Poll, SourceError> {
        if self.done {
            return Ok(Poll::Disconnected);
        }
        if !self.pacer.wait_until(self.t + self.cfg.dt, timeout) {
            return Ok(Poll::Idle);
        }
        self.step();
        self.tick += 1;
        Ok(Poll::Frame(Box::new(self.frame())))
    }

    fn extras(&mut self) -> Option<Extras> {
        let item = |path: &str, value: &str| SetupItem {
            path: path.into(),
            value: value.into(),
        };
        Some(Extras {
            channels: vec![Channel {
                name: "SessionTime".into(),
                unit: "s".into(),
                description: "Seconds since session start".into(),
                value: self.t.into(),
            }],
            info: Some(serde_json::json!({ "WeekendInfo": { "TrackName": "demo" } })),
            setup: Some(Setup {
                name: Some("baseline".into()),
                modified: Some(false),
                passed_tech: Some(true),
                items: vec![
                    item("Chassis/Front/ArbBlades", "3"),
                    item("Chassis/Rear/WingAngle", "7 deg"),
                ],
                unavailable: Vec::new(),
            }),
        })
    }

    fn reconnectable(&self) -> bool {
        false
    }
}

/// Run a synthetic race to completion (or until `until_s`) and collect frames.
pub fn synth_frames(cfg: SynthConfig, until_s: Option<f64>) -> Vec<Frame> {
    let mut src = SynthSource::new(cfg, 0.0);
    let mut out = Vec::new();
    while let Ok(Poll::Frame(f)) = src.poll(Duration::ZERO) {
        let stop = until_s.is_some_and(|u| f.session.time_s >= u);
        out.push(*f);
        if stop {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_race_finishes() {
        let frames = synth_frames(
            SynthConfig {
                laps: 3,
                ..Default::default()
            },
            None,
        );
        let last = frames.last().unwrap();
        assert_eq!(last.session.phase, SessionPhase::Checkered);
        assert!(last.cars.iter().all(|c| c.laps_completed.unwrap() >= 3));
    }

    fn player_laps(cfg: SynthConfig) -> Vec<f64> {
        let mut laps = Vec::new();
        for f in synth_frames(cfg, None) {
            let p = &f.cars[3];
            if let Some(l) = p.last_lap_s
                && laps.len() < p.laps_completed.unwrap_or(0) as usize
            {
                laps.push(l);
            }
        }
        laps
    }

    fn quiet(laps: i32) -> SynthConfig {
        SynthConfig {
            laps,
            lap_noise_s: 0.0,
            player_pit_lap: None,
            rival_pit_lap: None,
            yellow: None,
            ..Default::default()
        }
    }

    /// Defaults keep the race the tests and fixtures were written for.
    #[test]
    fn defaults_add_nothing() {
        let c = SynthConfig::default();
        assert_eq!(
            (c.tyre_deg_s, c.lap_noise_s, c.mistake_one_in),
            (0.0, 0.4, 0)
        );
        assert!(c.stops.is_empty() && c.rain.is_none());
    }

    #[test]
    fn tyres_wear_until_the_stop() {
        let deg = 0.3 * (1.0 + 0.4 * noise(3, 99));
        let laps = player_laps(SynthConfig {
            tyre_deg_s: 0.3,
            stops: vec![(3, 7)],
            ..quiet(10)
        });
        // Laps 1..6 lose `deg` per lap of age (times are in 0.1 s steps).
        let slope = (laps[5] - laps[0]) / 5.0;
        assert!((slope - deg).abs() < 0.05, "{slope} vs {deg}: {laps:?}");
        // The stop costs the in and the out lap; then the tyres are new.
        assert!(laps[6] + laps[7] > 2.0 * laps[5] + 20.0, "{laps:?}");
        assert!((laps[8] - laps[0]).abs() < 0.25, "{laps:?}");
    }

    #[test]
    fn rain_slows_everyone_and_shows_in_the_weather() {
        let cfg = SynthConfig {
            rain: Some((200.0, 380.0)),
            ..quiet(6)
        };
        let laps = player_laps(cfg.clone());
        assert!(laps[5] - laps[0] > 7.0, "{laps:?}");
        let frames = synth_frames(cfg, Some(400.0));
        let w = |t: f64| {
            let f = frames
                .iter()
                .find(|f| f.session.time_s >= t)
                .unwrap()
                .weather
                .clone()
                .unwrap();
            (f.wetness.unwrap(), f.precipitation.unwrap())
        };
        assert_eq!(w(100.0), ("dry".to_string(), 0.0));
        assert_eq!(w(245.0).0, "lightly wet");
        assert_eq!(w(290.0).0, "moderately wet");
        assert_eq!(w(395.0), ("very wet".to_string(), 1.0));
        assert_eq!(wetness_word(0.049), "dry");
        assert_eq!(wetness_word(0.05), "lightly wet");
        assert_eq!(wetness_word(0.35), "moderately wet");
        assert_eq!(wetness_word(0.7), "very wet");
    }

    #[test]
    fn damage_needs_the_crew() {
        let cfg = SynthConfig {
            damage: Some((20.0, 35.0)),
            ..quiet(2)
        };
        let repair = |t: f64| {
            let f = synth_frames(cfg.clone(), Some(t)).pop().unwrap();
            let car = f.player.unwrap().car;
            (car.pit.repair_left_s, car.warnings)
        };
        assert_eq!(repair(19.0), (None, vec![]));
        assert_eq!(repair(20.5), (Some(35.0), vec![Warning::Damage]));
    }

    #[test]
    fn mistakes_cost_whole_seconds_on_a_few_laps() {
        let laps = player_laps(SynthConfig {
            mistake_one_in: 4,
            ..quiet(16)
        });
        let slow = laps
            .iter()
            .filter(|l| **l > laps[0].min(laps[1]) + 1.4)
            .count();
        assert!((1..=8).contains(&slow), "{laps:?}");
        assert!(laps.iter().all(|l| *l < 90.0 + 4.5), "{laps:?}");
    }
}
