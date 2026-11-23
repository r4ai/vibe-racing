//! When every car passed every hundredth of the track: sector times, the gap
//! to another car as it is now, and where on the lap it is won or lost.
//!
//! Games tell lap times of all cars, but sector times of none. The positions
//! around the lap give both, to about the time between two samples.

use std::collections::{HashMap, VecDeque};

use vr_core::{CarState, Frame, TrackSurface};

use crate::util::wrap_half;

/// Places per lap besides the sector lines.
const GRID: usize = 100;
/// Laps kept per car, the one being driven included.
const LAPS: usize = 6;
/// No car covers more of a lap than this per second; a tow or a reset does.
const MAX_RATE: f64 = 0.15;
/// Longer without a sample, nobody knows what happened meanwhile.
const MAX_GAP_S: f64 = 1.0;
/// Positions jitter: going back by less is standing still.
const JITTER: f64 = 0.01;
/// Closer to the line than this, two places are the same.
const SAME: f64 = 1e-6;
/// Where nobody tells the sectors: thirds of the lap.
const THIRDS: [f64; 3] = [0.0, 1.0 / 3.0, 2.0 / 3.0];
/// Lengths compared to find where time goes, in places of the grid.
const STRETCH: usize = 5;

/// One lap of one car.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Passing {
    /// As the game counts.
    pub lap: Option<i32>,
    /// Drove through the pit lane.
    pub pit: bool,
    /// Session time at each gate; `None`: not passed.
    at: Vec<Option<f64>>,
}

#[derive(Debug, Clone, Default)]
struct Track {
    last: Option<(f64, f64)>,
    /// Oldest first; the last one is being driven.
    laps: VecDeque<Passing>,
    best: Vec<Option<f64>>,
}

/// A stretch of the lap and what it costs against another car.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Stretch {
    pub from: f64,
    pub to: f64,
    /// Positive: slower than the other car.
    pub lost_s: f64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Timing {
    /// Places on the lap, ascending, the first at the line.
    gates: Vec<f64>,
    /// Which of them begin a sector.
    sectors: Vec<usize>,
    /// The game told the sectors.
    told: bool,
    cars: HashMap<usize, Track>,
}

/// The sector starts that make sense, the first at the line.
fn starts(told: &[f64]) -> Option<Vec<f64>> {
    let mut s: Vec<f64> = told
        .iter()
        .copied()
        .filter(|p| p.is_finite() && (0.0..1.0).contains(p))
        .collect();
    s.sort_by(f64::total_cmp);
    s.dedup_by(|a, b| (*a - *b).abs() < SAME);
    (s.len() >= 2 && s[0] < SAME).then_some(s)
}

impl Timing {
    pub fn restart(&mut self) {
        self.cars.clear();
    }

    fn lay_out(&mut self, told: &[f64]) {
        let known = starts(told);
        let sectors = known.clone().unwrap_or_else(|| THIRDS.to_vec());
        let same = !self.gates.is_empty()
            && self.told == known.is_some()
            && self.sectors.len() == sectors.len()
            && self
                .sectors
                .iter()
                .zip(&sectors)
                .all(|(i, s)| (self.gates[*i] - s).abs() < SAME);
        if same {
            return;
        }
        let mut gates: Vec<f64> = (0..GRID).map(|i| i as f64 / GRID as f64).collect();
        gates.extend(&sectors);
        gates.sort_by(f64::total_cmp);
        gates.dedup_by(|a, b| (*a - *b).abs() < SAME);
        self.sectors = sectors
            .iter()
            .filter_map(|s| gates.iter().position(|g| (g - s).abs() < SAME))
            .collect();
        self.gates = gates;
        self.told = known.is_some();
        self.cars.clear();
    }

    pub fn ingest(&mut self, f: &Frame) {
        let told = f.session.track.as_ref().map_or(&[][..], |t| &t.sectors);
        self.lay_out(told);
        for car in &f.cars {
            let t = f.session.time_s;
            let tr = self.cars.entry(car.idx).or_default();
            match car.lap_dist_pct.filter(|p| (0.0..=1.0).contains(p)) {
                Some(pct) if car.surface != TrackSurface::NotInWorld => {
                    tr.drive(&self.gates, &self.sectors, t, pct, car);
                }
                _ => tr.leave(),
            }
        }
    }

    /// Whether the game told where the sectors begin.
    pub fn told(&self) -> bool {
        self.told
    }

    pub fn sector_starts(&self) -> Vec<f64> {
        self.sectors.iter().map(|i| self.gates[*i]).collect()
    }

    pub fn best(&self, idx: usize) -> Vec<Option<f64>> {
        let n = self.sectors.len();
        match self.cars.get(&idx) {
            Some(t) if t.best.len() == n => t.best.clone(),
            _ => vec![None; n],
        }
    }

    /// The last lap driven from line to line.
    pub fn last(&self, idx: usize) -> Option<(&Passing, f64)> {
        let laps = &self.cars.get(&idx)?.laps;
        (0..laps.len().saturating_sub(1)).rev().find_map(|i| {
            let end = laps[i + 1].at[0]?;
            laps[i]
                .at
                .iter()
                .all(Option::is_some)
                .then(|| (&laps[i], end))
        })
    }

    /// Sector times of the last lap driven from line to line.
    pub fn last_sectors(&self, idx: usize) -> Vec<Option<f64>> {
        match self.last(idx) {
            Some((lap, end)) => sector_times(lap, &self.sectors, Some(end)),
            None => vec![None; self.sectors.len()],
        }
    }

    /// Sectors already done on the lap being driven.
    pub fn current_sectors(&self, idx: usize) -> Vec<Option<f64>> {
        let lap = self.cars.get(&idx).and_then(|t| t.laps.back());
        match lap {
            Some(lap) => sector_times(lap, &self.sectors, None),
            None => vec![None; self.sectors.len()],
        }
    }

    /// Seconds from the car ahead on track to the one behind, where the one
    /// behind is now, and how that changed since its lap before.
    pub fn gap(&self, ahead: usize, behind: usize) -> Option<(f64, Option<f64>)> {
        let (a, b) = (self.cars.get(&ahead)?, self.cars.get(&behind)?);
        let lap = b.laps.back()?;
        let gate = lap.at.iter().rposition(Option::is_some)?;
        let now = lap.at[gate]?;
        let i = a.before(gate, now)?;
        let gap = now - a.laps[i].at[gate]?;
        let was = || {
            let then = b.laps.get(b.laps.len().checked_sub(2)?)?.at[gate]?;
            Some(then - a.laps.get(i.checked_sub(1)?)?.at[gate]?)
        };
        Some((gap, was().map(|w| gap - w)))
    }

    /// Where the last lap of `me` lost or won most against that of `other`:
    /// stretches of the lap, the costliest first.
    pub fn compare(&self, me: usize, other: usize) -> Vec<Stretch> {
        let (Some(a), Some(b)) = (self.last(me), self.last(other)) else {
            return Vec::new();
        };
        let n = self.gates.len();
        let at = |(lap, end): &(&Passing, f64), i: usize| -> Option<f64> {
            let start = lap.at[0]?;
            Some(if i >= n { *end } else { lap.at[i]? } - start)
        };
        let mut out: Vec<Stretch> = (0..n)
            .step_by(STRETCH)
            .filter_map(|i| {
                let j = (i + STRETCH).min(n);
                let mine = at(&a, j)? - at(&a, i)?;
                let theirs = at(&b, j)? - at(&b, i)?;
                Some(Stretch {
                    from: self.gates[i],
                    to: self.gates.get(j).copied().unwrap_or(1.0),
                    lost_s: mine - theirs,
                })
            })
            .collect();
        out.sort_by(|x, y| y.lost_s.total_cmp(&x.lost_s));
        out
    }
}

fn sector_times(lap: &Passing, sectors: &[usize], end: Option<f64>) -> Vec<Option<f64>> {
    (0..sectors.len())
        .map(|k| {
            let from = lap.at[sectors[k]]?;
            let to = match sectors.get(k + 1) {
                Some(i) => lap.at[*i],
                None => end,
            };
            Some(to? - from)
        })
        .collect()
}

impl Track {
    /// The lap being driven cannot be told any more.
    fn leave(&mut self) {
        self.last = None;
        if let Some(lap) = self.laps.back_mut() {
            lap.at.fill(None);
        }
    }

    fn drive(&mut self, gates: &[f64], sectors: &[usize], t: f64, pct: f64, car: &CarState) {
        // The line is at 0, not at 1.
        let pct = if pct >= 1.0 { 0.0 } else { pct };
        let Some((t0, p0)) = self.last else {
            self.last = Some((t, pct));
            return;
        };
        let (dt, d) = (t - t0, wrap_half(pct - p0));
        if !(0.0..=MAX_GAP_S).contains(&dt) || d < -JITTER || d > MAX_RATE * dt + JITTER {
            self.leave();
            self.last = Some((t, pct));
            return;
        }
        if d <= 0.0 {
            self.last = Some((t, p0));
            return;
        }
        self.last = Some((t, pct));
        // Places from the last one on, over the line if need be.
        let mut passed: Vec<(f64, usize)> = gates
            .iter()
            .enumerate()
            .map(|(i, g)| ((g - p0).rem_euclid(1.0), i))
            .filter(|(ahead, _)| *ahead > 0.0 && *ahead <= d)
            .collect();
        passed.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (ahead, i) in passed {
            self.pass(i, t0 + ahead / d * dt, gates.len(), sectors);
        }
        if let Some(lap) = self.laps.back_mut() {
            lap.pit |= car.on_pit_road;
            // The game counts the lap a little later.
            if (0.05..0.95).contains(&pct) {
                lap.lap = car.lap;
            }
        }
    }

    fn pass(&mut self, gate: usize, t: f64, gates: usize, sectors: &[usize]) {
        if gate == 0 {
            self.finish(t, sectors);
            let mut at = vec![None; gates];
            at[0] = Some(t);
            self.laps.push_back(Passing {
                lap: None,
                pit: false,
                at,
            });
            if self.laps.len() > LAPS {
                self.laps.pop_front();
            }
        } else if let Some(lap) = self.laps.back_mut() {
            lap.at[gate] = Some(t);
        }
    }

    /// The lap is done: its sectors may be the best so far.
    fn finish(&mut self, end: f64, sectors: &[usize]) {
        let Some(lap) = self.laps.back().filter(|l| !l.pit) else {
            return;
        };
        let times = sector_times(lap, sectors, Some(end));
        self.best.resize(sectors.len(), None);
        for (best, t) in self.best.iter_mut().zip(times) {
            if let Some(t) = t
                && best.is_none_or(|b| t < b)
            {
                *best = Some(t);
            }
        }
    }

    /// The last lap on which the gate was passed no later than `t`.
    fn before(&self, gate: usize, t: f64) -> Option<usize> {
        self.laps
            .iter()
            .rposition(|l| l.at[gate].is_some_and(|x| x <= t))
    }
}
