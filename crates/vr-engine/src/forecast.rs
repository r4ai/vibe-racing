//! Where the race ends if everyone goes on as they do now: the gap to every
//! car at the flag, and the lap on which two cars meet.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use vr_core::{CarState, Frame, SessionKind};

use crate::RaceState;
use crate::render::{car_label, fmt_lap, meta_line, pos_label};
use crate::report::{CarRef, GapMode, LapRecord, Meta};
use crate::strategy::median;

/// Laps that make the pace of a car, and how many it takes at least.
const PACE_LAPS: usize = 4;
const MIN_LAPS: usize = 2;
/// A lap slower than the car's pace by more was a mistake or traffic.
const ODD_S: f64 = 2.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForecastReport {
    pub meta: Meta,
    pub laps_left: Option<f64>,
    /// In class, as the cars run now.
    pub position: Option<u32>,
    /// In class, at the flag. Stops still to be made are not counted.
    pub finish: Option<u32>,
    pub pace_s: Option<f64>,
    /// The cars of the class by how they run, leader first.
    pub cars: Vec<ForecastCar>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForecastCar {
    pub car: CarRef,
    /// Positive: ahead of you.
    pub gap_s: f64,
    /// Median of the last clean laps.
    pub pace_s: Option<f64>,
    /// Their pace minus yours; positive: they are slower.
    pub slower_s: Option<f64>,
    /// The gap at the flag; positive: ahead of you.
    pub at_flag_s: Option<f64>,
    /// Laps until you meet, when that is before the flag.
    pub meets_in_laps: Option<f64>,
}

impl ForecastReport {
    /// The next car ahead that can be caught, or behind that will come.
    pub fn next(&self, ahead: bool) -> Option<&ForecastCar> {
        let mut side: Vec<&ForecastCar> = self
            .cars
            .iter()
            .filter(|c| (c.gap_s > 0.0) == ahead)
            .collect();
        side.sort_by(|a, b| a.gap_s.abs().total_cmp(&b.gap_s.abs()));
        side.first().copied()
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        let (Some(left), Some(now)) = (self.laps_left, self.position) else {
            out.push_str(
                "forecast: unavailable (needs a race with laps to go and two clean laps)\n",
            );
            return out;
        };
        let finish = self.finish.map_or("unknown".into(), |p| format!("P{p}"));
        let _ = writeln!(
            out,
            "forecast (pace of the last clean laps, stops not counted): now P{now}, {left:.1} laps left, finish {finish} | your pace {}",
            self.pace_s.map_or("--".into(), fmt_lap)
        );
        for c in &self.cars {
            let _ = write!(
                out,
                "{} {} gap {:+.1}s",
                pos_label(&c.car, true),
                car_label(&c.car),
                c.gap_s
            );
            if let (Some(p), Some(s)) = (c.pace_s, c.slower_s) {
                let _ = write!(out, " | pace {} ({s:+.2}s/lap)", fmt_lap(p));
            }
            if let Some(f) = c.at_flag_s {
                let _ = write!(out, " | at the flag {f:+.1}s");
            }
            if let Some(l) = c.meets_in_laps {
                let _ = write!(out, " | meets in {l:.1} laps");
            }
            out.push('\n');
        }
        out.push_str("(gap: positive = ahead of you; s/lap: positive = slower than you)\n");
        out
    }
}

/// The pace of the laps driven of late.
fn pace(laps: &[LapRecord]) -> Option<f64> {
    let mut times: Vec<f64> = laps
        .iter()
        .rev()
        .filter(|l| l.is_clean())
        .filter_map(|l| l.time_s)
        .take(PACE_LAPS)
        .collect();
    let fastest = times.iter().copied().fold(f64::INFINITY, f64::min);
    times.retain(|t| t - fastest <= ODD_S);
    if times.len() < MIN_LAPS {
        return None;
    }
    median(&times)
}

pub(crate) fn forecast(state: &RaceState) -> ForecastReport {
    let mut out = ForecastReport {
        meta: state.meta(),
        laps_left: None,
        position: None,
        finish: None,
        pace_s: None,
        cars: Vec::new(),
    };
    let Some((f, me)) = state.frame().and_then(|f| Some((f, f.player_car()?))) else {
        return out;
    };
    if f.session.kind != SessionKind::Race {
        return out;
    }
    out.laps_left = state.fuel_info(f).and_then(|i| i.laps_to_finish);
    out.pace_s = pace(&state.car_laps(me.idx));
    out.cars = cars(state, f, me, out.laps_left.zip(out.pace_s));
    let ahead = out.cars.iter().filter(|c| c.gap_s > 0.0).count() as u32;
    out.position = Some(ahead + 1);
    out.finish = out.laps_left.zip(out.pace_s).map(|_| {
        // Whoever cannot be judged stays where it is.
        let there = out
            .cars
            .iter()
            .filter(|c| c.at_flag_s.unwrap_or(c.gap_s) > 0.0);
        there.count() as u32 + 1
    });
    out
}

fn cars(state: &RaceState, f: &Frame, me: &CarState, mine: Option<(f64, f64)>) -> Vec<ForecastCar> {
    let progress = |c: &CarState| {
        c.laps_completed
            .map(|l| f64::from(l) + c.lap_dist_pct.unwrap_or(0.0))
    };
    let mut out: Vec<ForecastCar> = f
        .cars
        .iter()
        .filter(|c| c.idx != me.idx && c.class_id == me.class_id)
        .filter_map(|c| {
            let g = state.gap_between(f, me, c);
            if g.mode == GapMode::BestLap {
                return None;
            }
            let ahead = match (g.live_ahead, progress(c), progress(me)) {
                (Some(a), ..) => a,
                (None, Some(c), Some(me)) => c > me,
                _ => return None,
            };
            let gap = g.live_s.or(g.gap_s)?;
            let gap_s = if ahead { gap } else { -gap };
            let pace_s = pace(&state.car_laps(c.idx));
            let slower_s = pace_s.zip(mine).map(|(p, (_, own))| p - own);
            let at_flag_s = slower_s.zip(mine).map(|(s, (left, _))| gap_s - s * left);
            let meets_in_laps = slower_s.zip(mine).and_then(|(s, (left, _))| {
                let laps = gap_s / s;
                (laps.is_finite() && laps > 0.0 && laps <= left).then_some(laps)
            });
            Some(ForecastCar {
                car: crate::state::car_ref(c),
                gap_s,
                pace_s,
                slower_s,
                at_flag_s,
                meets_in_laps,
            })
        })
        .collect();
    out.sort_by(|a, b| b.gap_s.total_cmp(&a.gap_s));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lap(n: i32, time: f64, pit: bool) -> LapRecord {
        LapRecord {
            lap: n,
            time_s: Some(time),
            fuel_used_l: None,
            pit,
            caution: false,
            position: None,
            session_time_s: 0.0,
        }
    }

    #[test]
    fn the_pace_is_that_of_the_last_clean_laps() {
        assert_eq!(pace(&[]), None);
        assert_eq!(pace(&[lap(1, 90.0, false)]), None);
        let laps = [
            lap(1, 80.0, false),
            lap(2, 91.0, false),
            lap(3, 90.0, false),
            lap(4, 120.0, true),
            lap(5, 90.4, false),
            // Exactly as slow as still counts.
            lap(6, 92.0, false),
        ];
        assert_eq!(pace(&laps), Some((90.4 + 91.0) / 2.0));
        let mut slow = laps.to_vec();
        slow[5].time_s = Some(92.01);
        assert_eq!(pace(&slow), Some(90.4));
        // One lap says nothing, nor does it with an odd one.
        assert_eq!(pace(&[lap(1, 90.0, false), lap(2, 95.0, false)]), None);
    }
}
