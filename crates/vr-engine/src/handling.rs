//! How the car behaves, from the driver's inputs and the car's motion.
//!
//! Games do not publish "understeer". It is inferred from what the driver
//! does: near the limit, steering more without turning more means that the
//! front has let go; steering against the turn means that the rear has.
//! Wheelspin and locked rear wheels show as engine speed that does not fit
//! the road speed in the current gear. All of it is a tendency, not a
//! measurement, and only compares with the same car in the same session.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use vr_core::Frame;

/// Zones a lap is divided into.
pub const ZONES: usize = 20;
const MAX_LAPS: usize = 100;
/// Slower than this, the geometry says little.
const MIN_SPEED_MPS: f64 = 15.0;
/// Steps longer than this are gaps in the data.
const MAX_DT_S: f64 = 0.25;
/// Time constant the noisy signals are smoothed with.
const SMOOTH_S: f64 = 0.06;
/// Steering and turning are compared with this long ago.
const WINDOW_S: f64 = 0.25;
/// More steering than this (rad) that does not turn the car.
const STEER_MORE: f64 = 0.06;
const MIN_STEER_RAD: f64 = 0.03;
/// Cornering that counts as near the limit, as a share of what the car does
/// at that speed.
const NEAR_LIMIT: f64 = 0.6;
const MIN_LAT: f64 = 8.0;
/// Yaw (rad/s) against the steering: the driver is catching a slide.
const COUNTER_YAW: f64 = 0.15;
/// Speed may not rise: accelerating, a car turns less for the same steering.
const SPEED_RISE: f64 = 1.0;
const SPEED_BIN: f64 = 10.0;
/// Samples before a baseline is trusted.
const MIN_BASE: u32 = 120;
/// Engine speed off the road speed by this share.
const SLIP: f64 = 0.06;
/// Cornering in which the drivetrain baseline is taken.
const STRAIGHT_LAT: f64 = 8.0;
/// A lower lap counts once it is shown for this long.
const LAP_DROP_S: f64 = 1.0;
/// The drivetrain settles after a gear change.
const GEAR_SETTLE_S: f64 = 0.5;

/// Seconds spent in a state, per part of the corner.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Phases {
    /// On the brakes.
    pub entry: f64,
    pub mid: f64,
    /// On the throttle.
    pub exit: f64,
}

impl Phases {
    pub fn total(&self) -> f64 {
        self.entry + self.mid + self.exit
    }

    fn add(&mut self, p: Phase, dt: f64) {
        match p {
            Phase::Entry => self.entry += dt,
            Phase::Mid => self.mid += dt,
            Phase::Exit => self.exit += dt,
        }
    }

    fn scaled(&self, k: f64) -> Self {
        Self {
            entry: self.entry * k,
            mid: self.mid * k,
            exit: self.exit * k,
        }
    }

    fn plus(&self, o: &Self) -> Self {
        Self {
            entry: self.entry + o.entry,
            mid: self.mid + o.mid,
            exit: self.exit + o.exit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Entry,
    Mid,
    Exit,
}

/// Seconds per lap of each tendency.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Behaviour {
    pub understeer: Phases,
    pub oversteer: Phases,
    pub wheelspin_s: f64,
    /// Rear wheels slower than the car under braking.
    pub rear_lock_s: f64,
    pub abs_s: f64,
    /// Largest angle between where the car points and where it goes.
    pub max_slip_deg: f64,
    /// In slow, medium and fast corners.
    #[serde(default)]
    pub by_speed: [Balance; 3],
}

/// Seconds per lap of either side.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Balance {
    pub understeer_s: f64,
    pub oversteer_s: f64,
}

impl Behaviour {
    fn plus(&self, o: &Self) -> Self {
        Self {
            understeer: self.understeer.plus(&o.understeer),
            oversteer: self.oversteer.plus(&o.oversteer),
            wheelspin_s: self.wheelspin_s + o.wheelspin_s,
            rear_lock_s: self.rear_lock_s + o.rear_lock_s,
            abs_s: self.abs_s + o.abs_s,
            max_slip_deg: self.max_slip_deg.max(o.max_slip_deg),
            by_speed: std::array::from_fn(|i| Balance {
                understeer_s: self.by_speed[i].understeer_s + o.by_speed[i].understeer_s,
                oversteer_s: self.by_speed[i].oversteer_s + o.by_speed[i].oversteer_s,
            }),
        }
    }

    fn scaled(&self, k: f64) -> Self {
        Self {
            understeer: self.understeer.scaled(k),
            oversteer: self.oversteer.scaled(k),
            wheelspin_s: self.wheelspin_s * k,
            rear_lock_s: self.rear_lock_s * k,
            abs_s: self.abs_s * k,
            max_slip_deg: self.max_slip_deg,
            by_speed: self.by_speed.map(|b| Balance {
                understeer_s: b.understeer_s * k,
                oversteer_s: b.oversteer_s * k,
            }),
        }
    }

    /// Mean of several laps (the largest slip angle stays the largest).
    pub fn mean(laps: &[Behaviour]) -> Option<Self> {
        if laps.is_empty() {
            return None;
        }
        let sum = laps.iter().fold(Self::default(), |a, b| a.plus(b));
        Some(sum.scaled(1.0 / laps.len() as f64))
    }

    /// The tendency most of the time goes to, for a zone.
    fn worst(&self) -> (Tendency, f64) {
        [
            (Tendency::UndersteerEntry, self.understeer.entry),
            (Tendency::UndersteerMid, self.understeer.mid),
            (Tendency::UndersteerExit, self.understeer.exit),
            (Tendency::OversteerEntry, self.oversteer.entry),
            (Tendency::OversteerMid, self.oversteer.mid),
            (Tendency::OversteerExit, self.oversteer.exit),
            (Tendency::Wheelspin, self.wheelspin_s),
            (Tendency::RearLock, self.rear_lock_s),
        ]
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("the list is not empty")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tendency {
    UndersteerEntry,
    UndersteerMid,
    UndersteerExit,
    OversteerEntry,
    OversteerMid,
    OversteerExit,
    Wheelspin,
    RearLock,
}

impl Tendency {
    pub fn label(self) -> &'static str {
        match self {
            Self::UndersteerEntry => "understeer on entry",
            Self::UndersteerMid => "understeer mid-corner",
            Self::UndersteerExit => "understeer on exit",
            Self::OversteerEntry => "oversteer on entry",
            Self::OversteerMid => "oversteer mid-corner",
            Self::OversteerExit => "oversteer on exit",
            Self::Wheelspin => "wheelspin",
            Self::RearLock => "rear locking",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandlingLap {
    pub lap: i32,
    pub behaviour: Behaviour,
}

/// A part of the track and what happens there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    pub from_pct: f64,
    pub to_pct: f64,
    pub tendency: Tendency,
    /// Seconds per lap.
    pub seconds: f64,
}

/// A quantile followed without keeping the samples.
#[derive(Debug, Clone, Copy, Default)]
struct Quantile {
    n: u32,
    value: f64,
}

impl Quantile {
    /// Moves by `step` towards the `q` quantile.
    fn add(&mut self, x: f64, q: f64, step: f64) {
        if self.n == 0 {
            self.value = x;
        }
        self.n = self.n.saturating_add(1);
        self.value += if x > self.value {
            step * q
        } else {
            -step * (1.0 - q)
        };
    }

    fn get(&self) -> Option<f64> {
        (self.n >= MIN_BASE).then_some(self.value)
    }
}

/// Smoothed signals at one moment.
#[derive(Debug, Clone, Copy, Default)]
struct Motion {
    t: f64,
    steer: f64,
    yaw: f64,
    lat: f64,
    speed: f64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct HandlingTracker {
    laps: VecDeque<HandlingLap>,
    lap: Option<i32>,
    /// Since when a lower lap is shown than before.
    dropped_since: Option<f64>,
    current: Behaviour,
    /// Seconds of the lap in progress that were judged.
    driven_s: f64,
    zones: Vec<Behaviour>,
    /// Laps the zones were collected over.
    zone_laps: f64,
    /// Lateral acceleration the car reaches, per speed bin.
    grip: HashMap<u32, Quantile>,
    /// Engine speed per road speed, per gear.
    gears: HashMap<i32, Quantile>,
    /// The last moments, oldest first.
    recent: VecDeque<Motion>,
    last_t: Option<f64>,
    gear: Option<i32>,
    gear_since: f64,
    last_pct: Option<f64>,
}

impl HandlingTracker {
    pub fn laps(&self) -> impl DoubleEndedIterator<Item = &HandlingLap> {
        self.laps.iter()
    }

    /// Whether there is enough data to judge the balance at all.
    pub fn ready(&self) -> bool {
        self.grip.values().any(|g| g.get().is_some())
    }

    /// Zones with the most time in a tendency, worst first.
    pub fn zones(&self, n: usize) -> Vec<Zone> {
        if self.zone_laps < 0.5 {
            return Vec::new();
        }
        let mut z: Vec<Zone> = self
            .zones
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let (tendency, s) = b.worst();
                Zone {
                    from_pct: i as f64 / ZONES as f64,
                    to_pct: (i + 1) as f64 / ZONES as f64,
                    tendency,
                    seconds: s / self.zone_laps,
                }
            })
            .filter(|z| z.seconds >= 0.05)
            .collect();
        z.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
        z.truncate(n);
        z
    }

    pub fn ingest(&mut self, f: &Frame) {
        let t = f.session.time_s;
        let dt = self.last_t.map(|l| t - l);
        self.last_t = Some(t);
        let (Some(p), Some(car)) = (&f.player, f.player_car()) else {
            return;
        };
        self.next_lap(car.lap, t);
        if p.gear != self.gear {
            self.gear = p.gear;
            self.gear_since = t;
        }
        let Some(dt) = dt.filter(|d| *d > 0.0 && *d <= MAX_DT_S) else {
            self.recent.clear();
            return;
        };
        let speed = p.speed_mps.unwrap_or(0.0);
        if !p.on_track || car.on_pit_road || speed < MIN_SPEED_MPS {
            self.recent.clear();
            return;
        }
        self.driven_s += dt;
        let pct = car.lap_dist_pct.unwrap_or(0.0);
        if let Some(last) = self.last_pct {
            let d = pct - last;
            if (0.0..0.1).contains(&d) {
                self.zone_laps += d;
            }
        }
        self.last_pct = Some(pct);

        let throttle = p.throttle.unwrap_or(0.0);
        let brake = p.brake.unwrap_or(0.0);
        let phase = if brake > 0.1 {
            Phase::Entry
        } else if throttle > 0.3 {
            Phase::Exit
        } else {
            Phase::Mid
        };
        let mut step = Behaviour::default();
        self.balance(f, speed, phase, dt, &mut step);
        self.traction(f, speed, throttle, brake, dt, t, &mut step);
        if p.car.abs_active == Some(true) {
            step.abs_s += dt;
        }
        self.current = self.current.plus(&step);
        if self.zones.is_empty() {
            self.zones = vec![Behaviour::default(); ZONES];
        }
        let z = ((pct * ZONES as f64) as usize).min(ZONES - 1);
        self.zones[z] = self.zones[z].plus(&step);
    }

    fn next_lap(&mut self, lap: Option<i32>, t: f64) {
        let Some(lap) = lap else { return };
        match self.lap {
            Some(l) if l == lap => self.dropped_since = None,
            // Shown for a moment while the car is towed.
            Some(l) if lap < l && t - *self.dropped_since.get_or_insert(t) < LAP_DROP_S => {}
            Some(l) => {
                self.dropped_since = None;
                let behaviour = std::mem::take(&mut self.current);
                let driven_s = std::mem::take(&mut self.driven_s);
                // Going back means a reset: nothing was completed.
                if lap > l && driven_s > 0.0 {
                    if self.laps.len() == MAX_LAPS {
                        self.laps.pop_front();
                    }
                    self.laps.push_back(HandlingLap { lap: l, behaviour });
                }
                self.lap = Some(lap);
            }
            None => self.lap = Some(lap),
        }
    }

    fn balance(&mut self, f: &Frame, speed: f64, phase: Phase, dt: f64, step: &mut Behaviour) {
        let c = &f.player.as_ref().expect("checked by the caller").car;
        if let (Some(vx), Some(vy)) = (c.velocity_x_mps, c.velocity_y_mps)
            && vx > MIN_SPEED_MPS
        {
            step.max_slip_deg = vy.atan2(vx).to_degrees().abs();
        }
        let (Some(steer), Some(yaw), Some(lat)) =
            (c.steering_rad, c.yaw_rate_rps, c.lat_accel_mps2)
        else {
            return;
        };
        let t = f.session.time_s;
        let k = 1.0 - (-dt / SMOOTH_S).exp();
        let now = match self.recent.back() {
            Some(l) => Motion {
                t,
                steer: l.steer + (steer - l.steer) * k,
                yaw: l.yaw + (yaw - l.yaw) * k,
                lat: l.lat + (lat - l.lat) * k,
                speed: l.speed + (speed - l.speed) * k,
            },
            None => Motion {
                t,
                steer,
                yaw,
                lat,
                speed,
            },
        };
        self.recent.push_back(now);
        // Keep one sample that is at least as old as the window.
        while self.recent.len() > 2 && t - self.recent[1].t >= WINDOW_S {
            self.recent.pop_front();
        }

        let class = crate::chassis::speed_class(now.speed);
        let same_way = now.steer * now.yaw > 0.0;
        if !same_way && now.yaw.abs() > COUNTER_YAW && now.steer.abs() > MIN_STEER_RAD {
            step.oversteer.add(phase, dt);
            step.by_speed[class].oversteer_s += dt;
            return;
        }
        let bin = (now.speed / SPEED_BIN) as u32;
        let grip = self.grip.entry(bin).or_default();
        // The grip the car shows at its best; single spikes do not count.
        if now.lat.abs() > 3.0 {
            grip.add(now.lat.abs(), 0.95, 0.1);
        }
        let Some(limit) = grip.get() else { return };
        let then = self.recent[0];
        if same_way
            && now.lat.abs() > (NEAR_LIMIT * limit).max(MIN_LAT)
            && t - then.t >= WINDOW_S
            && then.steer * now.steer > 0.0
            && now.steer.abs() - then.steer.abs() > STEER_MORE
            && now.yaw.abs() < then.yaw.abs()
            && now.speed - then.speed < SPEED_RISE
        {
            step.understeer.add(phase, dt);
            step.by_speed[class].understeer_s += dt;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn traction(
        &mut self,
        f: &Frame,
        speed: f64,
        throttle: f64,
        brake: f64,
        dt: f64,
        t: f64,
        step: &mut Behaviour,
    ) {
        let p = f.player.as_ref().expect("checked by the caller");
        let (Some(rpm), Some(gear)) = (p.rpm.filter(|r| *r > 0.0), p.gear.filter(|g| *g > 0))
        else {
            return;
        };
        let settled = t - self.gear_since >= GEAR_SETTLE_S;
        if !settled || p.car.clutch.is_some_and(|c| c < 0.95) {
            return;
        }
        let ratio = rpm / speed;
        let lat = p.car.lat_accel_mps2.unwrap_or(0.0).abs();
        let base = self.gears.entry(gear).or_default();
        if brake < 0.02 && throttle > 0.2 && lat < STRAIGHT_LAT {
            base.add(ratio, 0.5, ratio * 0.002);
        }
        let Some(base) = base.get() else { return };
        let slip = ratio / base - 1.0;
        if throttle > 0.5 && slip > SLIP {
            step.wheelspin_s += dt;
        } else if brake > 0.2 && slip < -SLIP {
            step.rear_lock_s += dt;
        }
    }
}
