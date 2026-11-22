//! What the wheels do: locking, spinning, ride heights, dampers, tyre
//! temperatures under load. Measured, not inferred, but only where the game
//! tells it (iRacing: in its telemetry files, not while driving).

use serde::{Deserialize, Serialize};
use vr_core::{Frame, TrackSurface, Wheel, Wheels};

/// Slower than this, wheels slip for other reasons.
const MIN_SPEED_MPS: f64 = 15.0;
/// Steps longer than this are gaps in the data.
const MAX_DT_S: f64 = 0.25;
/// Slower than the car by this share on the brakes: locking.
const LOCK: f64 = 0.12;
/// Faster than the car by this share on the throttle: spinning. A driven
/// wheel at the limit slips by half of it.
const SPIN: f64 = 0.10;
const BRAKING: f64 = 0.2;
const ON_POWER: f64 = 0.4;
/// Cornering that loads the outer tyres.
const LOADED_LAT: f64 = 12.0;
/// Cornering in which ride heights are those of the platform.
const STRAIGHT_LAT: f64 = 4.0;
/// Closer to the ground than this, the car touches it.
pub const TOUCH_MM: f64 = 2.0;
/// Higher than any ride height of a car on its wheels.
const MAX_RIDE_MM: f64 = 400.0;
/// Damper speeds above this come from bumps and kerbs, below it from the
/// driver's inputs.
pub const DAMPER_KNEE: f64 = 0.05;
/// Speeds that separate slow, medium and fast corners (120 and 180 km/h).
pub const SPEED_CLASSES: [f64; 2] = [33.4, 50.0];
/// Pointing this far from where it goes, the car is being caught, not driven.
const SLIDE_DEG: f64 = 8.0;
/// After a slide or leaving the track, so long is left out.
const UPSET_S: f64 = 3.0;
/// A lap counts if this much of it was driven without upsets.
const WHOLE: f64 = 0.8;
/// Before this share of the first lap the tyres are cold.
const WARM_LAPS: f64 = 1.0;
/// Laps before the numbers mean something.
pub const MIN_LAPS: usize = 2;

/// Which speed class a corner belongs to: slow, medium, fast.
pub(crate) fn speed_class(mps: f64) -> usize {
    SPEED_CLASSES.iter().filter(|s| mps >= **s).count()
}

/// One value per wheel.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Corners<T> {
    pub lf: T,
    pub rf: T,
    pub lr: T,
    pub rr: T,
}

impl<T> Corners<T> {
    pub fn all(&self) -> [(&'static str, &T); 4] {
        [
            ("LF", &self.lf),
            ("RF", &self.rf),
            ("LR", &self.lr),
            ("RR", &self.rr),
        ]
    }

    fn map<U>(&self, f: impl Fn(&T) -> U) -> Corners<U> {
        Corners {
            lf: f(&self.lf),
            rf: f(&self.rf),
            lr: f(&self.lr),
            rr: f(&self.rr),
        }
    }

    fn each(&mut self) -> [&mut T; 4] {
        [&mut self.lf, &mut self.rf, &mut self.lr, &mut self.rr]
    }
}

/// Time shares of a damper, and how fast it moves.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Damper {
    pub bump_low: f64,
    pub bump_high: f64,
    pub rebound_low: f64,
    pub rebound_high: f64,
}

/// Mean heights of the platform on straights.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ride {
    pub from_kmh: f64,
    pub to_kmh: Option<f64>,
    pub front_mm: f64,
    pub rear_mm: f64,
}

/// Seconds are those of a usual lap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chassis {
    /// Complete laps without upsets.
    pub laps: usize,
    pub lock_s: Corners<f64>,
    /// Deepest slip on the brakes, as a share of the speed.
    pub deepest_lock: Corners<f64>,
    pub spin_s: Corners<f64>,
    /// Share of the front in the brake pressure.
    pub front_brake: Option<f64>,
    /// While warm.
    pub pressure_kpa: Corners<Option<f64>>,
    /// Outer, middle, inner surface temperature while the tyre is on the
    /// loaded side of a corner.
    pub loaded_c: Corners<Option<[f64; 3]>>,
    pub ride: Vec<Ride>,
    /// Front and rear this close to the ground or closer.
    pub touch_front_s: f64,
    pub touch_rear_s: f64,
    pub dampers: Corners<Option<Damper>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Mean {
    sum: f64,
    n: f64,
}

impl Mean {
    fn add(&mut self, v: f64) {
        self.sum += v;
        self.n += 1.0;
    }

    fn get(&self) -> Option<f64> {
        (self.n > 0.0).then(|| self.sum / self.n)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct WheelSums {
    deepest: f64,
    pressure: Mean,
    loaded: [Mean; 3],
    damper: Damper,
}

/// Seconds of one lap.
#[derive(Debug, Clone, Copy, Default)]
struct LapSums {
    lock: [f64; 4],
    spin: [f64; 4],
    touch: [f64; 2],
    driven: f64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ChassisTracker {
    wheels: Corners<WheelSums>,
    front_brake: Mean,
    /// Front and rear, per speed class.
    ride: [[Mean; 2]; 3],
    lap: LapSums,
    done: Vec<LapSums>,
    number: Option<i32>,
    upset: Option<f64>,
    laps: f64,
    last_t: Option<f64>,
    last_pct: Option<f64>,
    seen: bool,
}

fn corners(w: &Wheels) -> [&Wheel; 4] {
    [&w.lf, &w.rf, &w.lr, &w.rr]
}

/// Mean height of an axle in mm; not while the car is on its roof or lifted.
fn axle(a: &Wheel, b: &Wheel) -> Option<f64> {
    let mm = (a.ride_height_m? + b.ride_height_m?) * 500.0;
    (0.0..MAX_RIDE_MM).contains(&mm).then_some(mm)
}

impl ChassisTracker {
    pub fn ingest(&mut self, f: &Frame) {
        let t = f.session.time_s;
        let dt = self.last_t.replace(t).map(|l| t - l);
        let (Some(p), Some(car)) = (&f.player, f.player_car()) else {
            return;
        };
        let Some(w) = &p.car.wheels else { return };
        self.seen = true;
        let pct = car.lap_dist_pct.unwrap_or(0.0);
        let speed = p.speed_mps.unwrap_or(0.0);
        let Some(dt) = dt.filter(|d| *d > 0.0 && *d <= MAX_DT_S) else {
            self.last_pct = None;
            return;
        };
        let moved = self.last_pct.replace(pct).map(|l| pct - l);
        if car.lap != self.number {
            let lap = std::mem::take(&mut self.lap);
            if lap.driven >= WHOLE {
                self.done.push(lap);
            }
            self.number = car.lap;
        }
        let sliding = match (p.car.velocity_x_mps, p.car.velocity_y_mps) {
            (Some(x), Some(y)) => y.atan2(x).to_degrees().abs() > SLIDE_DEG,
            _ => false,
        };
        let off = matches!(
            car.surface,
            TrackSurface::OffTrack | TrackSurface::NotInWorld
        );
        if speed > 1.0 && (sliding || off) {
            self.upset = Some(t);
        }
        if self
            .upset
            .is_some_and(|u| (0.0..UPSET_S).contains(&(t - u)))
            || !p.on_track
            || car.on_pit_road
            || speed < MIN_SPEED_MPS
        {
            return;
        }
        if let Some(d) = moved.filter(|d| (0.0..0.1).contains(d)) {
            self.laps += d;
            self.lap.driven += d;
        }
        let throttle = p.throttle.unwrap_or(0.0);
        let brake = p.brake.unwrap_or(0.0);
        // Positive to the left: the right side is loaded.
        let lat = p.car.lat_accel_mps2.unwrap_or(0.0);
        let warm = self.laps >= WARM_LAPS;
        for (i, (wheel, s)) in corners(w).into_iter().zip(self.wheels.each()).enumerate() {
            let left = i % 2 == 0;
            if let Some(v) = wheel.speed_mps {
                let slip = v / speed - 1.0;
                if brake > BRAKING {
                    s.deepest = s.deepest.min(slip);
                    if slip < -LOCK {
                        self.lap.lock[i] += dt;
                    }
                } else if throttle > ON_POWER && slip > SPIN {
                    self.lap.spin[i] += dt;
                }
            }
            if let Some(kpa) = wheel.pressure_kpa.filter(|_| warm) {
                s.pressure.add(kpa);
            }
            let outside = if left {
                lat < -LOADED_LAT
            } else {
                lat > LOADED_LAT
            };
            if outside && warm {
                for (m, c) in s.loaded.iter_mut().zip(wheel.surface_c) {
                    if let Some(c) = c {
                        m.add(c);
                    }
                }
            }
            if let Some(v) = wheel.shock_mps {
                let d = &mut s.damper;
                *match (v >= 0.0, v.abs() >= DAMPER_KNEE) {
                    (true, false) => &mut d.bump_low,
                    (true, true) => &mut d.bump_high,
                    (false, false) => &mut d.rebound_low,
                    (false, true) => &mut d.rebound_high,
                } += dt;
            }
        }
        if brake > BRAKING
            && let (Some(lf), Some(rf), Some(lr), Some(rr)) = (
                w.lf.brake_bar,
                w.rf.brake_bar,
                w.lr.brake_bar,
                w.rr.brake_bar,
            )
            && lf + rf + lr + rr > 1.0
        {
            self.front_brake.add((lf + rf) / (lf + rf + lr + rr));
        }
        for (i, h) in [axle(&w.lf, &w.rf), axle(&w.lr, &w.rr)]
            .into_iter()
            .enumerate()
        {
            let Some(h) = h else { continue };
            if h < TOUCH_MM {
                self.lap.touch[i] += dt;
            }
            if lat.abs() < STRAIGHT_LAT && brake < 0.02 {
                self.ride[speed_class(speed)][i].add(h);
            }
        }
    }

    /// `None` until there is something to tell.
    pub fn get(&self) -> Option<Chassis> {
        if !self.seen || self.done.is_empty() {
            return None;
        }
        // One bad lap does not make a tendency.
        let usual = |f: &dyn Fn(&LapSums) -> f64| {
            let v: Vec<f64> = self.done.iter().map(f).collect();
            crate::strategy::median(&v).unwrap_or_default()
        };
        let four = |f: &dyn Fn(&LapSums) -> [f64; 4]| Corners {
            lf: usual(&|l| f(l)[0]),
            rf: usual(&|l| f(l)[1]),
            lr: usual(&|l| f(l)[2]),
            rr: usual(&|l| f(l)[3]),
        };
        let edges = [0.0, SPEED_CLASSES[0], SPEED_CLASSES[1]];
        Some(Chassis {
            laps: self.done.len(),
            lock_s: four(&|l| l.lock),
            deepest_lock: self.wheels.map(|w| -w.deepest),
            spin_s: four(&|l| l.spin),
            front_brake: self.front_brake.get(),
            pressure_kpa: self.wheels.map(|w| w.pressure.get()),
            loaded_c: self.wheels.map(|w| {
                let [o, m, i] = w.loaded.map(|m| m.get());
                Some([o?, m?, i?])
            }),
            ride: self
                .ride
                .iter()
                .enumerate()
                .filter_map(|(i, [f, r])| {
                    Some(Ride {
                        from_kmh: (edges[i] * 3.6).round(),
                        to_kmh: edges.get(i + 1).map(|e| (e * 3.6).round()),
                        front_mm: f.get()?,
                        rear_mm: r.get()?,
                    })
                })
                .collect(),
            touch_front_s: usual(&|l| l.touch[0]),
            touch_rear_s: usual(&|l| l.touch[1]),
            dampers: self.wheels.map(|w| {
                let d = w.damper;
                let all = d.bump_low + d.bump_high + d.rebound_low + d.rebound_high;
                (all > 0.0).then(|| Damper {
                    bump_low: d.bump_low / all,
                    bump_high: d.bump_high / all,
                    rebound_low: d.rebound_low / all,
                    rebound_high: d.rebound_high / all,
                })
            }),
        })
    }
}
