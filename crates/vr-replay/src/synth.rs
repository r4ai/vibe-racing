use std::time::Duration;

use vr_core::{
    Capabilities, CarState, Flag, Frame, PlayerTelemetry, Poll, SessionKind, SessionPhase,
    SessionState, SourceError, TelemetrySource, TrackInfo, TrackSurface, Weather,
};

use crate::Pacer;

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
    pit_lap: Option<i32>,
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

    fn lap_target(&self) -> f64 {
        self.base_lap + noise(self.idx as u64, self.current_lap() as u64) * 0.4
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
        (self.pit_lap == Some(self.current_lap()) && self.pct > 1.0 - PIT_ROAD_PCT)
            || (self.pit_lap == Some(self.laps_completed) && self.pct < PIT_ROAD_PCT)
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
                let pit_lap = if i == cfg.player_idx {
                    cfg.player_pit_lap
                } else if i + 1 == cfg.player_idx {
                    cfg.rival_pit_lap
                } else {
                    None
                };
                SynthCar {
                    idx: i,
                    base_lap,
                    laps_completed: 0,
                    // Grid: staggered just behind the line.
                    pct: 1.0 - 0.02 * (i as f64 + 1.0),
                    lap_start_t: 0.0,
                    last_lap: None,
                    best_lap: None,
                    pit_lap,
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
            let target = c.lap_target();
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
                if c.idx == player_idx && c.pit_lap == Some(c.laps_completed) {
                    self.fuel_l = self.cfg.fuel_capacity_l;
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
                }),
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
            }),
            cars,
            weather: Some(Weather {
                air_temp_c: Some(22.0),
                track_temp_c: Some(31.0),
                wetness: Some("dry".into()),
                precipitation: Some(0.0),
            }),
        }
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
}
