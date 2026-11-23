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
    /// What drivers call it, on a known track (`tracks`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    /// Metres before the slowest point where braking begins: what a driver
    /// can aim at. This lap and the reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brake_before_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_brake_before_m: Option<f64>,
    /// Gear at the slowest point, this lap and the reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gear: Option<i8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_gear: Option<i8>,
    /// Speed where braking begins (where the corner begins without it), km/h:
    /// how fast the car comes in. This lap and the reference.
    #[serde(default)]
    pub entry_kmh: f64,
    #[serde(default)]
    pub reference_entry_kmh: f64,
    /// Least throttle in the corner, percent: 100 is flat out.
    #[serde(default)]
    pub min_throttle: u8,
    #[serde(default)]
    pub reference_min_throttle: u8,
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
    compare_on(a, b, None)
}

/// The places of a known track, each up to where the next one begins: what
/// a corner loses on the straight after it is lost in it.
fn places(bends: &[crate::tracks::Bend], b: &LapTrace) -> Vec<(usize, usize, usize)> {
    let at = |share: f64| ((share * POINTS as f64) as usize).min(POINTS);
    (0..bends.len())
        .map(|i| {
            let (from, end) = (at(bends[i].from), at(bends[i].to));
            let to = bends.get(i + 1).map_or(POINTS, |n| at(n.from));
            let apex = (from..end.max(from + 1)).min_by_key(|&k| b.speed[k.min(POINTS - 1)]);
            (from, apex.unwrap_or(from), to)
        })
        .collect()
}

/// `a` against `b`, by the named places of the track when there are some.
pub fn compare_on(
    a: &LapTrace,
    b: &LapTrace,
    bends: Option<&[crate::tracks::Bend]>,
) -> Vec<Corner> {
    if a.speed.len() != POINTS || b.speed.len() != POINTS {
        return Vec::new();
    }
    let time = |t: &LapTrace, i: usize| match t.t_ms.get(i) {
        Some(ms) => f64::from(*ms) / 1000.0,
        None => t.time_s,
    };
    let speed = |t: &LapTrace, i: usize| f64::from(t.speed[i.min(POINTS - 1)]) / 10.0;
    let metre = b.length_m / POINTS as f64;
    let found = match bends {
        Some(bends) => places(bends, b),
        None => corners(b),
    };
    let gear = |t: &LapTrace, k: usize| t.gear.get(k).copied().filter(|g| *g > 0);
    found
        .into_iter()
        .enumerate()
        .map(|(i, (from, apex, to))| {
            // Within the corner itself, not the straight after it.
            let end = bends.map_or(to, |b| {
                ((b[i].to * POINTS as f64) as usize).clamp(from + 1, to)
            });
            let slowest = |t: &LapTrace| (from..end).min_by_key(|&k| t.speed[k]).unwrap_or(apex);
            let brakes = |t: &LapTrace| (from..slowest(t)).find(|&k| t.brake[k] >= BRAKING);
            let opens = |t: &LapTrace| (slowest(t)..to).find(|&k| t.throttle[k] >= THROTTLE);
            let differ = |x: Option<usize>, y: Option<usize>| Some((x? as f64 - y? as f64) * metre);
            let gap = |k: usize| time(a, k) - time(b, k);
            let before = |t: &LapTrace| Some((slowest(t) - brakes(t)?) as f64 * metre);
            let entry = |t: &LapTrace| speed(t, brakes(t).unwrap_or(from));
            let lift = |t: &LapTrace| (from..end).map(|k| t.throttle[k]).min().unwrap_or(100);
            Corner {
                number: i + 1,
                name: bends.map(|b| b[i].name.to_string()),
                from: from as f64 / POINTS as f64,
                apex: apex as f64 / POINTS as f64,
                to: to as f64 / POINTS as f64,
                loss_s: gap(to) - gap(from),
                brake_m: differ(brakes(a), brakes(b)),
                min_kmh: speed(a, slowest(a)) - speed(b, slowest(b)),
                throttle_m: differ(opens(a), opens(b)),
                exit_kmh: speed(a, to) - speed(b, to),
                reference_min_kmh: speed(b, slowest(b)),
                brake_before_m: before(a),
                reference_brake_before_m: before(b),
                gear: gear(a, slowest(a)),
                reference_gear: gear(b, slowest(b)),
                entry_kmh: entry(a),
                reference_entry_kmh: entry(b),
                min_throttle: lift(a),
                reference_min_throttle: lift(b),
            }
        })
        .collect()
}

/// How fast the car was on the straights and in the corners, against the
/// reference. No game tells what the wings have lost: this does.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Speeds {
    pub top_kmh: f64,
    /// Against the reference, km/h.
    pub top: f64,
    /// Slowest speeds of the corners by their speed, mean difference in km/h;
    /// `None`: there is no such corner.
    pub slow: Option<f64>,
    pub medium: Option<f64>,
    pub fast: Option<f64>,
}

/// One lap against a reference, with what a lap usually loses to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CornersReport {
    pub lap: i32,
    pub time_s: f64,
    pub reference_lap: i32,
    pub reference_s: f64,
    pub length_m: f64,
    /// In the order of the lap.
    pub corners: Vec<Corner>,
    /// Median loss of the laps before and of this one, corner by corner.
    pub usual_s: Vec<f64>,
    pub usual_laps: usize,
    pub speeds: Speeds,
}

/// Corners slower than the first are slow, than the second medium.
const SLOW_KMH: f64 = 120.0;
const MEDIUM_KMH: f64 = 180.0;
/// Losses and gains worth a line.
const WORTH_S: f64 = 0.03;
const LISTED: usize = 5;

impl CornersReport {
    /// `laps` are the laps that count, the one looked at last.
    pub fn of(laps: &[LapTrace], reference: &LapTrace) -> Option<Self> {
        Self::on(laps, reference, None)
    }

    /// The same by the named places of a known track.
    pub fn on(
        laps: &[LapTrace],
        reference: &LapTrace,
        bends: Option<&[crate::tracks::Bend]>,
    ) -> Option<Self> {
        let a = laps.last()?;
        let corners = compare_on(a, reference, bends);
        if corners.is_empty() {
            return None;
        }
        let all: Vec<Vec<Corner>> = laps
            .iter()
            .filter(|l| l.lap != reference.lap)
            .map(|l| compare_on(l, reference, bends))
            .filter(|c| c.len() == corners.len())
            .collect();
        let usual_s = (0..corners.len())
            .filter(|_| !all.is_empty())
            .filter_map(|i| {
                // Not the mean: one spin is not a habit.
                let losses: Vec<f64> = all.iter().map(|c| c[i].loss_s).collect();
                crate::strategy::median(&losses)
            })
            .collect();
        let top = |t: &LapTrace| f64::from(t.speed.iter().copied().max().unwrap_or(0)) / 10.0;
        let mean = |range: std::ops::Range<f64>| {
            let d: Vec<f64> = corners
                .iter()
                .filter(|c| range.contains(&c.reference_min_kmh))
                .map(|c| c.min_kmh)
                .collect();
            (!d.is_empty()).then(|| d.iter().sum::<f64>() / d.len() as f64)
        };
        let speeds = Speeds {
            top_kmh: top(a),
            top: top(a) - top(reference),
            slow: mean(0.0..SLOW_KMH),
            medium: mean(SLOW_KMH..MEDIUM_KMH),
            fast: mean(MEDIUM_KMH..f64::INFINITY),
        };
        Some(Self {
            lap: a.lap,
            time_s: a.time_s,
            reference_lap: reference.lap,
            reference_s: reference.time_s,
            length_m: reference.length_m,
            usual_laps: all.len(),
            corners,
            usual_s,
            speeds,
        })
    }

    pub fn to_text(&self) -> String {
        use std::fmt::Write as _;
        let lap = crate::render::fmt_lap;
        let named = self.corners.iter().any(|c| c.name.is_some());
        let mut out = format!(
            "lap {} {} against lap {} {} ({:+.3}s), corner by corner{}:\n",
            self.lap,
            lap(self.time_s),
            self.reference_lap,
            lap(self.reference_s),
            self.time_s - self.reference_s,
            if named {
                " (each up to the next one's braking)"
            } else {
                " (numbers count from the line, not the official ones)"
            }
        );
        let label = |c: &Corner| match &c.name {
            Some(n) => n.clone(),
            None => format!("corner {}", c.number),
        };
        // Named corners are asked about one by one: all of them, in order.
        let listed: Vec<&Corner> = if named {
            self.corners.iter().collect()
        } else {
            let mut worst: Vec<&Corner> = self
                .corners
                .iter()
                .filter(|c| c.loss_s.abs() >= WORTH_S)
                .collect();
            worst.sort_by(|a, b| b.loss_s.abs().total_cmp(&a.loss_s.abs()));
            worst.truncate(LISTED);
            worst
        };
        for c in listed {
            let _ = write!(
                out,
                "  {}corner {} of {} at {:.0}m ({:.0} km/h): {:+.2}s",
                c.name.as_ref().map_or(String::new(), |n| format!("{n}: ")),
                c.number,
                self.corners.len(),
                c.apex * self.length_m,
                c.reference_min_kmh,
                c.loss_s
            );
            let side = |m: f64| if m < 0.0 { "earlier" } else { "later" };
            if let Some(m) = c.brake_m.filter(|m| m.abs() >= 1.0) {
                let _ = write!(out, " | brakes {:.0}m {}", m.abs(), side(m));
            }
            if let (Some(a), Some(b)) = (c.brake_before_m, c.reference_brake_before_m) {
                let _ = write!(
                    out,
                    " (braking {a:.0}m before the slowest point, reference {b:.0}m)"
                );
            }
            let _ = write!(
                out,
                " | entry {:.0} km/h (reference {:.0})",
                c.entry_kmh, c.reference_entry_kmh
            );
            if c.brake_before_m.is_none() && c.reference_brake_before_m.is_none() {
                let _ = write!(out, " | no braking");
            }
            let _ = write!(out, " | slowest {:+.1} km/h", c.min_kmh);
            if c.min_throttle == 100 && c.reference_min_throttle == 100 {
                let _ = write!(out, " | flat out");
            } else {
                let _ = write!(
                    out,
                    " | least throttle {}% (reference {}%)",
                    c.min_throttle, c.reference_min_throttle
                );
            }
            if let (Some(a), Some(b)) = (c.gear, c.reference_gear) {
                let _ = write!(out, " | gear {a} (reference {b})");
            }
            if let Some(m) = c.throttle_m.filter(|m| m.abs() >= 1.0) {
                let _ = write!(out, " | throttle {:.0}m {}", m.abs(), side(m));
            }
            let _ = writeln!(out, " | exit {:+.1} km/h", c.exit_kmh);
        }
        let mut usual: Vec<(String, f64)> = self
            .usual_s
            .iter()
            .zip(&self.corners)
            .map(|(l, c)| (label(c), *l))
            .filter(|(_, l)| *l >= WORTH_S)
            .collect();
        usual.sort_by(|a, b| b.1.total_cmp(&a.1));
        usual.truncate(LISTED);
        if !usual.is_empty() {
            let parts: Vec<String> = usual.iter().map(|(n, l)| format!("{n} {l:+.2}s")).collect();
            let _ = writeln!(
                out,
                "usually lost there (median of {} laps): {}",
                self.usual_laps,
                parts.join(", ")
            );
        }
        let s = &self.speeds;
        let part = |name: &str, v: Option<f64>| {
            v.map_or(String::new(), |v| format!(" | {name} corners {v:+.1} km/h"))
        };
        let _ = writeln!(
            out,
            "top speed {:.1} km/h ({:+.1}){}{}{}",
            s.top_kmh,
            s.top,
            part("slow", s.slow),
            part("medium", s.medium),
            part("fast", s.fast)
        );
        out
    }
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
    fn a_known_track_is_compared_by_its_places() {
        use crate::tracks::Bend;
        let bends = [
            Bend {
                name: "ヘアピン",
                from: 0.2,
                to: 0.4,
            },
            Bend {
                name: "スプーン",
                from: 0.6,
                to: 0.8,
            },
        ];
        let (mut a, b) = (lap(10.0, 220), lap(0.0, 240));
        a.gear = vec![3; POINTS];
        let mut b = b;
        b.gear = vec![4; POINTS];
        b.gear[299] = 0;
        let c = compare_on(&a, &b, Some(&bends));
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name.as_deref(), Some("ヘアピン"));
        assert_eq!((c[0].apex, c[0].to), (0.299, 0.6));
        // 79 and 59 places of 5 m before the slowest point.
        assert_eq!(c[0].brake_before_m, Some(395.0));
        assert_eq!(c[0].reference_brake_before_m, Some(295.0));
        assert_eq!(c[0].brake_m, Some(-100.0));
        // A gear of 0 is no gear.
        assert_eq!((c[0].gear, c[0].reference_gear), (Some(3), None));
        assert_eq!(c[1].name.as_deref(), Some("スプーン"));
        assert_eq!(c[1].to, 1.0);
        assert_eq!((c[1].brake_before_m, c[1].reference_gear), (None, Some(4)));
        assert!(c[0].loss_s > 0.3 && c[1].loss_s.abs() < 0.01, "{c:?}");
        let r = CornersReport::on(&[a], &b, Some(&bends)).unwrap();
        let text = r.to_text();
        assert!(
            text.contains("(each up to the next one's braking)"),
            "{text}"
        );
        assert!(
            text.contains("  ヘアピン: corner 1 of 2 at 1495m"),
            "{text}"
        );
        // Every named corner, with the gear only where both laps tell it.
        let line = |n: &str| text.lines().find(|l| l.contains(n)).unwrap().to_string();
        assert!(!line("ヘアピン:").contains(" | gear "), "{text}");
        assert!(
            line("スプーン:").contains(" | gear 3 (reference 4)"),
            "{text}"
        );
        // No brake and no lift on either lap: flat out.
        assert!(line("スプーン:").contains(" | no braking"), "{text}");
        assert!(line("スプーン:").contains(" | flat out"), "{text}");
        assert_eq!(
            (c[1].min_throttle, c[1].entry_kmh),
            (100, c[1].reference_entry_kmh)
        );
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

    #[test]
    fn the_report_tells_what_a_lap_usually_loses() {
        let numbered = |n: i32, l: LapTrace| LapTrace { lap: n, ..l };
        let best = numbered(3, lap(0.0, 240));
        let laps = [
            numbered(2, lap(20.0, 240)),
            best.clone(),
            numbered(4, lap(10.0, 220)),
        ];
        let r = CornersReport::of(&laps, &best).unwrap();
        assert_eq!((r.lap, r.reference_lap, r.usual_laps), (4, 3, 2));
        let first = [&laps[0], &laps[2]].map(|l| compare(l, &best)[0].loss_s);
        assert!((r.usual_s[0] - (first[0] + first[1]) / 2.0).abs() < 1e-9);
        assert!(r.usual_s[0] > r.corners[0].loss_s);
        // The first corner is slow (100 km/h), the second medium (140 km/h).
        assert!(
            (r.speeds.slow.unwrap() + 10.0).abs() < 0.2,
            "{:?}",
            r.speeds
        );
        assert!(r.speeds.medium.unwrap().abs() < 0.2);
        assert_eq!((r.speeds.fast, r.speeds.top), (None, 0.0));
        let text = r.to_text();
        assert!(text.starts_with("lap 4 "), "{text}");
        assert!(text.contains(" against lap 3 "), "{text}");
        assert!(
            text.contains("corner 1 of 2 at 1495m (100 km/h): +"),
            "{text}"
        );
        assert!(
            text.contains(" | brakes 100m earlier (braking 395m before the slowest point, reference 295m) | entry 191 km/h (reference 176) | slowest -10.0 km/h | least throttle 0% (reference 0%) | exit "),
            "{text}"
        );
        assert!(
            text.contains("usually lost there (median of 2 laps): corner 1 +"),
            "{text}"
        );
        assert!(
            text.contains(
                "top speed 199.9 km/h (+0.0) | slow corners -10.0 km/h | medium corners "
            ),
            "{text}"
        );
        // The reference itself: nothing to compare the others with.
        let own = CornersReport::of(std::slice::from_ref(&best), &best).unwrap();
        assert_eq!((own.usual_laps, own.usual_s.len()), (0, 0));
        assert_eq!(CornersReport::of(&[], &best), None);
        assert_eq!(CornersReport::of(&laps, &LapTrace::default()), None);
    }
}
