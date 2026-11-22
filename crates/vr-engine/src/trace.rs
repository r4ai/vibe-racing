//! What the driver did and the car reached along a lap, at fixed places of
//! the track, so that laps can be laid over each other.
//!
//! The shape of the track comes from the car's own motion (games do not tell
//! it while driving): speed and yaw rate are integrated and the loop closed.

use serde::{Deserialize, Serialize};
use vr_core::Frame;

/// Places per lap: 4 to 7 m apart on most tracks.
pub const POINTS: usize = 1000;
/// Traces nobody took are dropped, oldest first.
const MAX_WAITING: usize = 200;
/// No car covers more of a lap than this per second, whatever the track; a
/// jump (tow, reset) does, and breaks the lap.
const MAX_RATE: f64 = 0.15;
/// So does a gap in the data of this many seconds.
const MAX_GAP_S: f64 = 1.0;

/// One lap. Every list has `POINTS` values, the first at the line.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LapTrace {
    pub lap: i32,
    pub time_s: f64,
    /// Drove through the pit lane.
    pub pit: bool,
    pub length_m: f64,
    /// Milliseconds since the line.
    pub t_ms: Vec<u32>,
    /// Tenths of km/h.
    pub speed: Vec<u16>,
    /// Percent.
    pub throttle: Vec<u8>,
    /// Percent.
    pub brake: Vec<u8>,
    pub gear: Vec<i8>,
    /// Steering wheel in tenths of a degree, positive to the left.
    pub steer: Vec<i16>,
    pub rpm: Vec<u16>,
    /// Lateral acceleration in hundredths of g, positive to the left.
    pub lat: Vec<i16>,
    /// The line driven, in decimetres.
    pub x: Vec<i32>,
    pub y: Vec<i32>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Row {
    t: f64,
    pct: f64,
    speed: f64,
    throttle: f64,
    brake: f64,
    gear: i32,
    steer: f64,
    rpm: f64,
    lat: f64,
    yaw: f64,
    x: f64,
    y: f64,
    dist: f64,
}

impl Row {
    /// Between `self` (k = 0) and `o`; the gear is that of `self`.
    fn towards(&self, o: &Row, k: f64) -> Row {
        let m = |a: f64, b: f64| a + (b - a) * k;
        Row {
            t: m(self.t, o.t),
            pct: m(self.pct, o.pct),
            speed: m(self.speed, o.speed),
            throttle: m(self.throttle, o.throttle),
            brake: m(self.brake, o.brake),
            gear: self.gear,
            steer: m(self.steer, o.steer),
            rpm: m(self.rpm, o.rpm),
            lat: m(self.lat, o.lat),
            yaw: m(self.yaw, o.yaw),
            x: m(self.x, o.x),
            y: m(self.y, o.y),
            dist: m(self.dist, o.dist),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TraceRecorder {
    last: Option<Row>,
    lap: i32,
    /// Since the line; empty while the lap has no beginning.
    rows: Vec<Row>,
    started: bool,
    pit: bool,
    done: Vec<LapTrace>,
}

impl TraceRecorder {
    pub fn take(&mut self) -> Vec<LapTrace> {
        std::mem::take(&mut self.done)
    }

    /// Another session; laps already done stay.
    pub fn restart(&mut self) {
        self.last = None;
        self.break_off();
    }

    /// The lap in progress is no good.
    fn break_off(&mut self) {
        self.rows.clear();
        self.started = false;
        self.pit = false;
    }

    pub fn ingest(&mut self, f: &Frame) {
        let (Some(p), Some(car)) = (&f.player, f.player_car()) else {
            return;
        };
        let (Some(pct), Some(speed)) = (car.lap_dist_pct, p.speed_mps) else {
            return;
        };
        if !p.on_track || !(0.0..=1.0).contains(&pct) {
            self.last = None;
            self.break_off();
            return;
        }
        let t = f.session.time_s;
        let c = &p.car;
        let mut row = Row {
            t,
            pct,
            speed,
            throttle: p.throttle.unwrap_or(0.0),
            brake: p.brake.unwrap_or(0.0),
            gear: p.gear.unwrap_or(0),
            steer: c.steering_rad.unwrap_or(0.0),
            rpm: p.rpm.unwrap_or(0.0),
            lat: c.lat_accel_mps2.unwrap_or(0.0),
            ..Row::default()
        };
        let Some(last) = self.last else {
            self.last = Some(row);
            return;
        };
        let dt = t - last.t;
        if dt <= 0.0 {
            // The clock went back: another session.
            if dt < 0.0 {
                self.last = Some(row);
                self.break_off();
            }
            return;
        }
        let rate = c.yaw_rate_rps.unwrap_or(0.0);
        let (vx, vy) = match (c.velocity_x_mps, c.velocity_y_mps) {
            (Some(x), Some(y)) => (x, y),
            _ => (speed, 0.0),
        };
        row.yaw = last.yaw + rate * dt;
        let heading = last.yaw + rate * dt / 2.0;
        row.x = last.x + (vx * heading.cos() - vy * heading.sin()) * dt;
        row.y = last.y + (vx * heading.sin() + vy * heading.cos()) * dt;
        row.dist = last.dist + speed * dt;

        let step = pct - last.pct;
        let wrapped = step < -0.5;
        let forward = if wrapped { step + 1.0 } else { step };
        if dt > MAX_GAP_S || forward.abs() > MAX_RATE * dt {
            self.last = Some(row);
            self.break_off();
            return;
        }
        self.pit |= car.on_pit_road;
        if forward <= 0.0 {
            // Rolling back: the place counts again once it is passed.
            let kept = Row {
                pct: last.pct,
                ..row
            };
            self.last = Some(kept);
            return;
        }
        if wrapped {
            let k = (1.0 - last.pct) / forward;
            let ahead = Row {
                pct: pct + 1.0,
                ..row
            };
            self.fill(&last, &ahead);
            let line = Row {
                pct: 0.0,
                ..last.towards(&ahead, k)
            };
            self.finish(&line);
            self.break_off();
            self.started = true;
            self.pit = car.on_pit_road;
            self.lap += 1;
            self.rows.push(line);
            self.fill(&line, &row);
        } else {
            // Around the line the number may change a moment early or late.
            if (0.1..0.9).contains(&pct)
                && let Some(lap) = car.lap
            {
                self.lap = lap;
            }
            self.fill(&last, &row);
        }
        self.last = Some(row);
    }

    /// Adds the places between two rows.
    fn fill(&mut self, a: &Row, b: &Row) {
        if !self.started {
            return;
        }
        while self.rows.len() < POINTS {
            let at = self.rows.len() as f64 / POINTS as f64;
            if at > b.pct {
                break;
            }
            let k = ((at - a.pct) / (b.pct - a.pct)).clamp(0.0, 1.0);
            self.rows.push(a.towards(b, k));
        }
    }

    fn finish(&mut self, line: &Row) {
        if !self.started || self.rows.len() != POINTS {
            return;
        }
        let first = self.rows[0];
        // After a lap the car points the same way again...
        let turned = line.yaw - first.yaw;
        let turns = (turned / std::f64::consts::TAU).round() * std::f64::consts::TAU;
        let drift = turned - turns;
        // ...and is where it began. The drift is spread over the lap.
        let length = line.dist - first.dist;
        let mut xy = Vec::with_capacity(POINTS + 1);
        let (mut x, mut y) = (0.0, 0.0);
        let mut before = first;
        xy.push((0.0, 0.0));
        for r in self.rows.iter().skip(1).chain(std::iter::once(line)) {
            let share = (before.dist - first.dist) / length.max(1.0);
            let turn = -(first.yaw + drift * share);
            let (dx, dy) = (r.x - before.x, r.y - before.y);
            x += dx * turn.cos() - dy * turn.sin();
            y += dx * turn.sin() + dy * turn.cos();
            xy.push((x, y));
            before = *r;
        }
        let (ex, ey) = (x, y);
        let closed = |i: usize, v: f64, end: f64| {
            ((v - end * i as f64 / POINTS as f64) * 10.0).round() as i32
        };
        let rows = &self.rows;
        let percent = |v: f64| (v * 100.0).round().clamp(0.0, 100.0) as u8;
        let trace = LapTrace {
            lap: self.lap,
            time_s: line.t - first.t,
            pit: self.pit,
            length_m: length,
            t_ms: rows
                .iter()
                .map(|r| ((r.t - first.t) * 1000.0).round() as u32)
                .collect(),
            speed: rows
                .iter()
                .map(|r| (r.speed * 36.0).round().max(0.0) as u16)
                .collect(),
            throttle: rows.iter().map(|r| percent(r.throttle)).collect(),
            brake: rows.iter().map(|r| percent(r.brake)).collect(),
            gear: rows.iter().map(|r| r.gear as i8).collect(),
            steer: rows
                .iter()
                .map(|r| (r.steer.to_degrees() * 10.0).round() as i16)
                .collect(),
            rpm: rows.iter().map(|r| r.rpm.round().max(0.0) as u16).collect(),
            lat: rows
                .iter()
                .map(|r| (r.lat / 9.806_65 * 100.0).round() as i16)
                .collect(),
            x: xy
                .iter()
                .take(POINTS)
                .enumerate()
                .map(|(i, p)| closed(i, p.0, ex))
                .collect(),
            y: xy
                .iter()
                .take(POINTS)
                .enumerate()
                .map(|(i, p)| closed(i, p.1, ey))
                .collect(),
        };
        if self.done.len() == MAX_WAITING {
            self.done.remove(0);
        }
        self.done.push(trace);
    }
}

/// One corner and the way to it, two laps against each other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Corner {
    /// Counted from the line; not the official number.
    pub number: usize,
    /// Shares of the lap.
    pub from: f64,
    pub apex: f64,
    pub to: f64,
    /// Time lost against the reference (negative: gained).
    pub loss_s: f64,
    /// Where braking begins, in metres: positive is later.
    pub brake_m: Option<f64>,
    /// Slowest speed, km/h: positive is faster.
    pub min_kmh: f64,
    /// Where the throttle is half open again, in metres: positive is later.
    pub throttle_m: Option<f64>,
    /// Speed at the end, km/h.
    pub exit_kmh: f64,
    /// Slowest speed of the reference.
    pub reference_min_kmh: f64,
}

/// A corner slows the car down by this much, in km/h.
const PROMINENCE: f64 = 12.0;
const BRAKING: u8 = 10;
const THROTTLE: u8 = 50;

/// Where the car is slowest, with the fastest places before and after it.
fn corners(t: &LapTrace) -> Vec<(usize, usize, usize)> {
    let n = t.speed.len();
    let v = |i: usize| f64::from(t.speed[i]) / 10.0;
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    let (mut top, mut low, mut falling) = (0usize, 0usize, true);
    let mut from = 0usize;
    for i in 0..n {
        if falling {
            if v(i) < v(low) {
                low = i;
            }
            if v(i) > v(top) {
                (top, low) = (i, i);
            }
            if v(top) - v(low) >= PROMINENCE && v(i) - v(low) >= PROMINENCE {
                // Through the corner, and on to the next straight.
                falling = false;
                out.push((from, low, i));
                top = i;
            }
        } else {
            if v(i) > v(top) {
                top = i;
            }
            if v(top) - v(i) >= PROMINENCE {
                out.last_mut().expect("a corner was found").2 = top;
                from = top;
                falling = true;
                low = i;
            }
        }
    }
    // A corner the line ends in: not faster again yet.
    if falling && v(top) - v(low) >= PROMINENCE && low > from {
        out.push((from, low, n));
    }
    if let Some(last) = out.last_mut() {
        last.2 = n;
    }
    out
}

/// `a` against the reference `b`, corner by corner.
pub fn compare(a: &LapTrace, b: &LapTrace) -> Vec<Corner> {
    if a.speed.len() != POINTS || b.speed.len() != POINTS {
        return Vec::new();
    }
    let time = |t: &LapTrace, i: usize| match t.t_ms.get(i) {
        Some(ms) => f64::from(*ms) / 1000.0,
        None => t.time_s,
    };
    let speed = |t: &LapTrace, i: usize| f64::from(t.speed[i.min(POINTS - 1)]) / 10.0;
    let metre = b.length_m / POINTS as f64;
    corners(b)
        .into_iter()
        .enumerate()
        .map(|(i, (from, apex, to))| {
            let slowest = |t: &LapTrace| (from..to).min_by_key(|&k| t.speed[k]).unwrap_or(apex);
            let brakes = |t: &LapTrace| (from..slowest(t)).find(|&k| t.brake[k] >= BRAKING);
            let opens = |t: &LapTrace| (slowest(t)..to).find(|&k| t.throttle[k] >= THROTTLE);
            let differ = |x: Option<usize>, y: Option<usize>| Some((x? as f64 - y? as f64) * metre);
            let gap = |k: usize| time(a, k) - time(b, k);
            Corner {
                number: i + 1,
                from: from as f64 / POINTS as f64,
                apex: apex as f64 / POINTS as f64,
                to: to as f64 / POINTS as f64,
                loss_s: gap(to) - gap(from),
                brake_m: differ(brakes(a), brakes(b)),
                min_kmh: speed(a, slowest(a)) - speed(b, slowest(b)),
                throttle_m: differ(opens(a), opens(b)),
                exit_kmh: speed(a, to) - speed(b, to),
                reference_min_kmh: speed(b, slowest(b)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two corners: slow at 30 % and 70 %.
    fn lap(slow: f64, brake_at: usize) -> LapTrace {
        let v = |i: usize| {
            let dip = |c: f64, w: f64, d: f64| {
                let x = (i as f64 / POINTS as f64 - c) / w;
                d * (-x * x).exp()
            };
            200.0 - dip(0.3, 0.05, 100.0 + slow) - dip(0.7, 0.05, 60.0)
        };
        let mut t = 0.0;
        let mut out = LapTrace {
            lap: 1,
            length_m: 5000.0,
            ..Default::default()
        };
        for i in 0..POINTS {
            out.t_ms.push((t * 1000.0) as u32);
            out.speed.push((v(i) * 10.0) as u16);
            out.brake
                .push(if (brake_at..300).contains(&i) { 80 } else { 0 });
            out.throttle
                .push(if (brake_at..310).contains(&i) { 0 } else { 100 });
            t += 5.0 / (v(i) / 3.6);
        }
        out.time_s = t;
        out
    }

    #[test]
    fn corners_are_where_it_is_slow() {
        let c = corners(&lap(0.0, 240));
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!((c[0].0, c[1].2), (0, POINTS));
        assert_eq!(c[0].2, c[1].0);
        assert!(
            (295..=305).contains(&c[0].1) && (695..=705).contains(&c[1].1),
            "{c:?}"
        );
        assert!((400..=520).contains(&c[0].2), "{c:?}");
    }

    #[test]
    fn a_flat_lap_has_no_corners() {
        let flat = LapTrace {
            speed: vec![2000; POINTS],
            ..lap(0.0, 240)
        };
        assert!(corners(&flat).is_empty());
        let mut kink = flat.clone();
        // 11.9 km/h less is no corner, 12 is one.
        kink.speed[500] = 1881;
        assert!(corners(&kink).is_empty());
        kink.speed[500] = 1880;
        assert_eq!(corners(&kink), [(0, 500, POINTS)]);
    }

    #[test]
    fn losses_add_up_and_have_reasons() {
        let (a, b) = (lap(10.0, 220), lap(0.0, 240));
        let c = compare(&a, &b);
        assert_eq!(c.len(), 2);
        let total: f64 = c.iter().map(|c| c.loss_s).sum();
        assert!((total - (a.time_s - b.time_s)).abs() < 0.002, "{total}");
        assert!(c[0].loss_s > 0.1 && c[1].loss_s.abs() < 0.002, "{c:?}");
        // 20 places of 5 m earlier on the brakes.
        assert_eq!(c[0].brake_m, Some(-100.0));
        assert_eq!(c[0].throttle_m, Some(0.0));
        assert!((c[0].min_kmh + 10.0).abs() < 0.2);
        assert!((c[0].reference_min_kmh - 100.0).abs() < 0.2);
        assert_eq!(c[1].brake_m, None);
        assert!(compare(&LapTrace::default(), &b).is_empty());
    }
}
