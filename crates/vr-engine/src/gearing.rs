//! When the driver changes up, against when the car pulls hardest.
//!
//! No game tells the power curve. The laps do: at full throttle the car
//! accelerates at some rate in each gear at each speed. Where the next gear
//! begins to pull harder than this one is the change up to try.
//! Drag is the same for both gears at the same speed. Slope, draft and the
//! way out of a corner are not, where the gears are used at different places:
//! the laps are pooled, so the crossing is an estimate to try, and the time a
//! change up takes is not in it.

use serde::{Deserialize, Serialize};

use crate::trace::{LapTrace, POINTS};

/// Throttle and brake of a sample that tells how hard the car pulls.
const FULL: u8 = 98;
/// Throttle at which a change up counts as one under power.
const UNDER_POWER: u8 = 90;
/// Speed bins, km/h.
const BIN_KMH: f64 = 5.0;
/// Lateral acceleration of a straight, hundredths of g.
const STRAIGHT_LAT: i32 = 30;
/// Samples a bin needs, and laps they come from.
const SAMPLES: usize = 3;
const BIN_LAPS: usize = 2;
/// The median absolute deviation of a normal spread, to its standard deviation.
const MAD_SD: f64 = 1.4826;
/// Standard errors the pull must turn by to be a crossing.
const NOISE: f64 = 2.0;
/// Within this of the limiter, on it.
const LIMITER: f64 = 0.99;
/// Points the revs stay flat for to be a cut, and the pull before it, m/s².
const CLIP_POINTS: usize = 3;
const PULLING: f64 = 1.0;
/// Laps with cuts that make a limiter.
const CUTS: usize = 2;
/// Of the rise the pull before would give, what a cut still rises.
const CUT_SHARE: f64 = 0.25;
/// Points after a change down in which its revs show.
const AFTER_DOWNSHIFT: usize = 3;

/// A change down that took the revs past the limiter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverRev {
    /// Into this gear.
    pub gear: i8,
    /// Part of the lap, 0-1.
    pub at: f64,
    pub kmh: f64,
    /// Highest revs just after it.
    pub rpm: f64,
}

/// The change up from one gear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shift {
    /// The gear changed up from.
    pub gear: i8,
    /// This lap, median of its changes up from that gear.
    pub rpm: f64,
    pub kmh: f64,
    pub reference_rpm: Option<f64>,
    pub reference_kmh: Option<f64>,
    /// Where the next gear begins to pull harder, from the laps of the
    /// session; `None` while it is not known.
    pub best_kmh: Option<f64>,
    pub best_rpm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gearing {
    pub shifts: Vec<Shift>,
    /// The highest revs of all the laps.
    pub max_rpm: f64,
    /// Where the revs stop under power in a gear held: the limiter, as far as
    /// the laps tell. `None`: never reached.
    pub limiter_rpm: Option<f64>,
    /// Seconds on the limiter, this lap and the reference.
    pub limiter_s: f64,
    pub reference_limiter_s: f64,
    /// Changes down that took the revs past the limiter, this lap and the reference.
    #[serde(default)]
    pub over_revs: usize,
    /// Those of this lap: where, and how far past.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub over_rev_at: Vec<OverRev>,
    #[serde(default)]
    pub reference_over_revs: usize,
}

fn secs(t: &LapTrace, k: usize) -> Option<f64> {
    t.t_ms.get(k).map(|ms| f64::from(*ms) / 1000.0)
}

fn kmh(t: &LapTrace, k: usize) -> f64 {
    f64::from(t.speed[k.min(POINTS - 1)]) / 10.0
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

fn usable(t: &LapTrace) -> bool {
    let n = t.speed.len();
    n == POINTS && t.gear.len() == n && t.rpm.len() == n && t.throttle.len() == n
}

/// Changes up under power: gear, revs and speed just before.
fn upshifts(t: &LapTrace) -> Vec<(i8, f64, f64)> {
    (0..POINTS - 1)
        .filter(|&k| {
            t.gear[k] > 0 && t.gear[k + 1] == t.gear[k] + 1 && t.throttle[k] >= UNDER_POWER
        })
        .map(|k| (t.gear[k], f64::from(t.rpm[k]), kmh(t, k)))
        .collect()
}

fn of_gear(shifts: &[(i8, f64, f64)], gear: i8) -> Option<(f64, f64)> {
    let mut rpm: Vec<f64> = shifts.iter().filter(|s| s.0 == gear).map(|s| s.1).collect();
    let mut speed: Vec<f64> = shifts.iter().filter(|s| s.0 == gear).map(|s| s.2).collect();
    Some((median(&mut rpm)?, median(&mut speed)?))
}

/// Speed and pull, m/s², between each two points of a lap at full throttle
/// in `gear`, straight on: in a corner the tyres take some of the pull.
fn samples(t: &LapTrace, gear: i8) -> Vec<(f64, f64)> {
    if t.t_ms.len() != POINTS {
        return Vec::new();
    }
    let straight = |i: usize| {
        t.lat
            .get(i)
            .is_none_or(|g| i32::from(*g).abs() <= STRAIGHT_LAT)
    };
    let full =
        |i: usize| t.throttle[i] >= FULL && t.brake[i] == 0 && t.gear[i] == gear && straight(i);
    t.t_ms
        .windows(2)
        .enumerate()
        .filter(|(k, w)| full(*k) && full(k + 1) && w[1] > w[0])
        .map(|(k, w)| {
            let dt = f64::from(w[1] - w[0]) / 1000.0;
            let (a, b) = (kmh(t, k), kmh(t, k + 1));
            ((a + b) / 2.0, (b - a) / 3.6 / dt)
        })
        .collect()
}

/// The middle of a speed bin, km/h.
fn centre(bin: i64) -> f64 {
    (bin as f64 + 0.5) * BIN_KMH
}

/// How hard the car pulls in a gear, m/s², by speed bin, where more than one
/// lap tells: one pass may be up a slope or out of a corner.
/// With each bin, the standard error of its median (from the spread).
fn pull(laps: &[&LapTrace], gear: i8) -> Vec<(i64, f64, f64)> {
    let mut bins: std::collections::BTreeMap<i64, Vec<(usize, f64)>> = Default::default();
    for (n, t) in laps.iter().enumerate() {
        for (mid, accel) in samples(t, gear) {
            bins.entry((mid / BIN_KMH).floor() as i64)
                .or_default()
                .push((n, accel));
        }
    }
    bins.into_iter()
        .filter_map(|(bin, v)| {
            // Points next to each other on one lap are one look, not many:
            // each lap gives one value, and the spread is between the laps.
            let mut laps: Vec<usize> = v.iter().map(|s| s.0).collect();
            laps.dedup();
            if v.len() < SAMPLES || laps.len() < BIN_LAPS {
                return None;
            }
            let mut per_lap: Vec<f64> = laps
                .into_iter()
                .filter_map(|n| {
                    let mut a: Vec<f64> = v.iter().filter(|s| s.0 == n).map(|s| s.1).collect();
                    median(&mut a)
                })
                .collect();
            let m = median(&mut per_lap)?;
            let mut off: Vec<f64> = per_lap.iter().map(|x| (x - m).abs()).collect();
            let se = median(&mut off)? * MAD_SD / (per_lap.len() as f64).sqrt();
            Some((bin, m, se))
        })
        .collect()
}

/// The speed from which `gear + 1` pulls harder than `gear`.
fn crossing(laps: &[&LapTrace], gear: i8) -> Option<f64> {
    let high = pull(laps, gear + 1);
    // Bin, the pull of each gear, and the error of their difference.
    let both: Vec<(i64, f64, f64, f64)> = pull(laps, gear)
        .into_iter()
        .filter_map(|(bin, a, ea)| {
            let (_, b, eb) = high.iter().find(|h| h.0 == bin)?;
            Some((bin, a, *b, ea.hypot(*eb)))
        })
        .collect();
    cross(&both)
}

/// Where the higher gear begins to pull harder, from the pull of both by
/// bin: only where the lower gear is seen to pull harder first, so that it is
/// a crossing.
fn cross(both: &[(i64, f64, f64, f64)]) -> Option<f64> {
    let first = both.first()?;
    if first.1 <= first.2 {
        return None;
    }
    let d = |i: usize| both[i].2 - both[i].1;
    // Pooled over places of the track: a turn smaller than the noise of the
    // laps is not told.
    let clear = |i: usize, j: usize| d(j) - d(i) >= NOISE * both[i].3.hypot(both[j].3);
    // Where the difference turns from the lower gear to the higher one, in
    // bins next to each other, and stays there in the next: one noisy bin or
    // a jump over missing speeds is no crossing. Between the two bins, where
    // the difference is zero.
    let next = |i: usize| both[i].0 == both[i - 1].0 + 1;
    (1..both.len())
        .find(|&i| {
            next(i)
                && clear(i - 1, i)
                && d(i - 1) < 0.0
                && d(i) >= 0.0
                && both
                    .get(i + 1)
                    .is_none_or(|_| next(i + 1) && d(i + 1) >= 0.0)
        })
        .map(|i| centre(both[i - 1].0) + BIN_KMH * -d(i - 1) / (d(i) - d(i - 1)))
}

/// Revs per km/h in a gear, from the laps.
fn ratio(laps: &[&LapTrace], gear: i8) -> Option<f64> {
    let mut r: Vec<f64> = laps
        .iter()
        .flat_map(|t| {
            (0..POINTS)
                .filter(move |&k| t.gear[k] == gear && t.speed[k] > 300 && t.throttle[k] >= FULL)
                .map(move |k| f64::from(t.rpm[k]) / kmh(t, k))
        })
        .collect();
    median(&mut r)
}

/// The same gear for a few points around `k`.
fn held(t: &LapTrace, k: usize) -> bool {
    k >= 2 && k + 1 < POINTS && (k - 2..=k + 1).all(|i| t.gear[i] == t.gear[k])
}

/// Runs where the revs stay flat under power in a gear held, right after the
/// car was pulling hard: the engine cut in. Drag flattens the revs slowly, so
/// a top speed is no such run. `(from, to)` points, and the revs.
fn clips(t: &LapTrace) -> Vec<(usize, usize, f64)> {
    let flat = |k: usize, rpm: f64| {
        t.throttle[k] >= FULL
            && t.gear[k] > 0
            && held(t, k)
            && (f64::from(t.rpm[k]) - rpm).abs() <= rpm * (1.0 - LIMITER)
    };
    // Pulling hard before `k`, and the revs over `k..end` rising by less than
    // a quarter of what that pull would give: cut, not slowly fading.
    let cut = |k: usize, end: usize| {
        let before = k.checked_sub(CLIP_POINTS)?;
        let dt = secs(t, k)? - secs(t, before)?;
        let gained = (kmh(t, k) - kmh(t, before)) / kmh(t, k).max(1.0);
        let expected = gained * (end - 1 - k) as f64 / CLIP_POINTS as f64;
        let rose = (f64::from(t.rpm[end - 1]) - f64::from(t.rpm[k])) / f64::from(t.rpm[k]).max(1.0);
        Some(
            dt > 0.0
                && t.gear[before] == t.gear[k]
                && (kmh(t, k) - kmh(t, before)) / 3.6 / dt >= PULLING
                && rose <= expected * CUT_SHARE,
        )
    };
    let mut out = Vec::new();
    // A point in a cut is in no other. A run that is no cut may hold the
    // start of one: the revs climbing up to it.
    let mut from = 0;
    for k in 0..POINTS {
        let rpm = f64::from(t.rpm[k]);
        if k < from || rpm <= 0.0 {
            continue;
        }
        // The last point is never held: a run ends by then.
        let Some(end) = (k..POINTS).find(|&e| !flat(e, rpm)) else {
            continue;
        };
        if end - k >= CLIP_POINTS && cut(k, end) == Some(true) {
            let mut at: Vec<f64> = (k..end).map(|i| f64::from(t.rpm[i])).collect();
            out.push((k, end, median(&mut at).unwrap_or(rpm)));
            from = end;
        }
    }
    out
}

/// The revs where the engine cuts, from cuts seen more than once: one is a
/// missed shift, or noise.
fn limiter(laps: &[&LapTrace]) -> Option<f64> {
    let cut: Vec<Vec<f64>> = laps
        .iter()
        .map(|t| clips(t).into_iter().map(|c| c.2).collect::<Vec<f64>>())
        .filter(|c| !c.is_empty())
        .collect();
    // On more than one lap: one lap may hold one missed shift twice.
    if cut.len() < CUTS {
        return None;
    }
    median(&mut cut.concat())
}

/// Changes down that sent the revs past the limiter.
fn over_revs(t: &LapTrace, limit: f64) -> Vec<OverRev> {
    (0..POINTS - 1)
        .filter(|&k| t.gear[k] > 1 && t.gear[k + 1] == t.gear[k] - 1)
        .filter_map(|k| {
            let after = k + 1..(k + 1 + AFTER_DOWNSHIFT).min(POINTS);
            let rpm = after.map(|i| f64::from(t.rpm[i])).fold(0.0, f64::max);
            (rpm > limit * (2.0 - LIMITER)).then(|| OverRev {
                gear: t.gear[k + 1],
                at: k as f64 / POINTS as f64,
                kmh: kmh(t, k),
                rpm,
            })
        })
        .collect()
}

/// Seconds in the cuts at the limiter.
fn on_limiter(t: &LapTrace, limit: f64) -> f64 {
    clips(t)
        .into_iter()
        .filter(|c| c.2 >= limit * LIMITER)
        .filter_map(|(from, to, _)| Some(secs(t, to)? - secs(t, from)?))
        .sum()
}

/// The changes up of lap `a` against the reference, with the best moment
/// from all the laps.
pub fn gearing(laps: &[LapTrace], a: &LapTrace, reference: &LapTrace) -> Option<Gearing> {
    if !usable(a) || !usable(reference) {
        return None;
    }
    // Each lap once: the reference is often one of the laps too.
    let mut all: Vec<&LapTrace> = laps.iter().filter(|t| usable(t)).collect();
    if !all.iter().any(|t| t.lap == reference.lap) {
        all.push(reference);
    }
    let max_rpm = all
        .iter()
        .flat_map(|t| t.rpm.iter())
        .copied()
        .max()
        .map(f64::from)
        .filter(|r| *r > 0.0)?;
    let mine = upshifts(a);
    let theirs = upshifts(reference);
    let mut gears: Vec<i8> = mine.iter().map(|s| s.0).collect();
    gears.sort_unstable();
    gears.dedup();
    let shifts = gears
        .into_iter()
        .filter_map(|g| {
            let (rpm, kmh) = of_gear(&mine, g)?;
            let reference = of_gear(&theirs, g);
            let best_kmh = crossing(&all, g);
            Some(Shift {
                gear: g,
                rpm,
                kmh,
                reference_rpm: reference.map(|r| r.0),
                reference_kmh: reference.map(|r| r.1),
                best_kmh,
                best_rpm: best_kmh.zip(ratio(&all, g)).map(|(v, r)| v * r),
            })
        })
        .collect();
    let limiter_rpm = limiter(&all);
    let of = |t: &LapTrace| limiter_rpm.map_or(0.0, |l| on_limiter(t, l));
    let revs = |t: &LapTrace| limiter_rpm.map_or_else(Vec::new, |l| over_revs(t, l));
    let over_rev_at = revs(a);
    Some(Gearing {
        shifts,
        max_rpm,
        limiter_rpm,
        limiter_s: of(a),
        reference_limiter_s: of(reference),
        over_revs: over_rev_at.len(),
        reference_over_revs: revs(reference).len(),
        over_rev_at,
    })
}

impl Gearing {
    pub fn to_text(&self) -> String {
        let parts: Vec<String> = self
            .shifts
            .iter()
            .map(|s| {
                let reference = match (s.reference_rpm, s.reference_kmh) {
                    (Some(r), Some(v)) => format!(" (reference {r:.0} rpm, {v:.0} km/h)"),
                    _ => String::new(),
                };
                let best = match (s.best_kmh, s.best_rpm) {
                    (Some(v), Some(r)) => {
                        format!(", pull crossover {v:.0} km/h ({r:.0} rpm, estimated from the laps, shift time not counted)")
                    }
                    (Some(v), None) => format!(", pull crossover {v:.0} km/h"),
                    _ => String::new(),
                };
                format!(
                    "{}->{} at {:.0} rpm {:.0} km/h{reference}{best}",
                    s.gear,
                    s.gear + 1,
                    s.rpm,
                    s.kmh
                )
            })
            .collect();
        let limiter = match self.limiter_rpm {
            Some(l) => format!(
                "limiter about {l:.0} rpm (estimated from the laps), on it {:.1}s (reference {:.1}s), changes down past it {} ({})",
                self.limiter_s, self.reference_limiter_s, self.over_revs, self.reference_over_revs
            ),
            None => format!("no limiter reached, highest {:.0} rpm", self.max_rpm),
        };
        let past: Vec<String> = self
            .over_rev_at
            .iter()
            .map(|o| {
                format!(
                    "into {} at {:.0}% of the lap, {:.0} km/h, {:.0} rpm",
                    o.gear,
                    o.at * 100.0,
                    o.kmh,
                    o.rpm
                )
            })
            .collect();
        let limiter = if past.is_empty() {
            limiter
        } else {
            format!("{limiter}: {}", past.join("; "))
        };
        format!(
            "changes up: {} | {limiter}\n",
            if parts.is_empty() {
                "none under power".to_string()
            } else {
                parts.join(" | ")
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lap at full throttle through 3 gears: gear 2 pulls `low` m/s² up to
    /// 150 km/h then less, gear 3 pulls a steady 6 m/s²; the driver changes
    /// up at `shift_kmh`.
    fn lap(n: i32, shift_kmh: f64) -> LapTrace {
        let mut t = LapTrace {
            lap: n,
            time_s: 0.0,
            pit: false,
            length_m: 5000.0,
            t_ms: Vec::new(),
            speed: Vec::new(),
            throttle: Vec::new(),
            brake: Vec::new(),
            gear: Vec::new(),
            steer: Vec::new(),
            rpm: Vec::new(),
            lat: Vec::new(),
            x: Vec::new(),
            y: Vec::new(),
        };
        let (mut v, mut s) = (100.0_f64 / 3.6, 0.0_f64);
        let step = 5.0;
        for _ in 0..POINTS {
            let k = v * 3.6;
            let gear = if k < shift_kmh { 2 } else { 3 };
            let accel = match gear {
                2 if k < 150.0 => 8.0,
                2 => 4.0,
                _ => 6.0,
            };
            t.t_ms.push((s * 1000.0) as u32);
            t.speed.push((k * 10.0) as u16);
            t.throttle.push(100);
            t.brake.push(0);
            t.gear.push(gear);
            t.steer.push(0);
            t.rpm.push((k * if gear == 2 { 55.0 } else { 42.0 }) as u16);
            t.lat.push(0);
            t.x.push(0);
            t.y.push(0);
            let dt = step / v;
            s += dt;
            // Drag: the pull fades towards 70 m/s, never stops at once.
            v += accel * (1.0 - (v / 70.0).powi(3)) * dt;
        }
        t
    }

    #[test]
    fn the_best_change_up_is_where_the_next_gear_pulls_harder() {
        // Some laps change up late, some early: both gears are seen at speed.
        let laps = vec![lap(1, 175.0), lap(2, 130.0), lap(3, 175.0), lap(4, 130.0)];
        let g = gearing(&laps, &laps[2], &laps[1]).unwrap();
        let s = &g.shifts[0];
        assert_eq!(s.gear, 2);
        assert!((s.kmh - 175.0).abs() < 1.5, "{}", s.kmh);
        assert!(
            s.reference_kmh.is_some_and(|v| (v - 130.0).abs() < 2.5),
            "{s:?}"
        );
        // Gear 2 pulls 8 below 150 km/h and 4 above; gear 3 pulls 6.
        // Between the 147.5 bin (-2) and the 152.5 bin (+2): 150.
        let best = s.best_kmh.unwrap();
        assert!((best - 150.0).abs() < 0.2, "{best}");
        let rpm = s.best_rpm.unwrap();
        assert!((rpm - 150.0 * 55.0).abs() < 60.0, "{rpm}");
        assert!(g.to_text().contains("2->3 at"), "{}", g.to_text());
        // Cornering all the way: no pull to compare.
        let mut bent: Vec<LapTrace> = laps.clone();
        for t in &mut bent {
            t.lat = vec![31; POINTS];
        }
        let g = gearing(&bent, &bent[2], &bent[1]).unwrap();
        assert_eq!(g.shifts[0].best_kmh, None);
        // Not seen pulling harder first: no crossing to tell.
        let early = vec![lap(1, 130.0), lap(2, 130.0)];
        let g = gearing(&early, &early[1], &early[0]).unwrap();
        assert_eq!(g.shifts[0].best_kmh, None);
        // Revs that only climb: no limiter, nothing on it.
        assert_eq!(g.limiter_rpm, None);
        assert_eq!((g.limiter_s, g.over_revs), (0.0, 0));
        assert!(
            g.to_text().contains("no limiter reached"),
            "{}",
            g.to_text()
        );
    }

    #[test]
    fn the_limiter_is_where_the_revs_stop_and_a_change_down_past_it_shows() {
        // On the limiter in 2nd: the revs and the speed stop together.
        let mut held = lap(1, 175.0);
        let mut stopped = None;
        for k in 0..POINTS {
            if held.gear[k] == 2 && held.rpm[k] >= 9000 {
                let v = *stopped.get_or_insert(held.speed[k]);
                held.rpm[k] = 9000;
                held.speed[k] = v;
            }
        }
        // One spike is no limiter.
        let mut spike = lap(2, 130.0);
        spike.rpm[500] = 9900;
        let g = gearing(&[spike.clone()], &spike, &spike).unwrap();
        assert_eq!(g.limiter_rpm, None);
        // One cut could be a missed shift.
        let g = gearing(&[], &held, &held).unwrap();
        assert_eq!(g.limiter_rpm, None);
        // The reference again is the same lap.
        let g = gearing(&[held.clone()], &held, &held).unwrap();
        assert_eq!(g.limiter_rpm, None);
        let mut other = held.clone();
        other.lap = 2;
        let two = [held.clone(), other];
        let g = gearing(&two, &held, &held).unwrap();
        assert_eq!(g.limiter_rpm, Some(9000.0));
        assert!(g.limiter_s > 0.5, "{}", g.limiter_s);
        assert_eq!(g.over_revs, 0);
        // Down to 2nd at the end of the lap: past the limiter.
        let mut down = held.clone();
        for k in 900..904 {
            down.gear[k] = 2;
            down.throttle[k] = 0;
            down.rpm[k] = 8000;
        }
        down.rpm[901] = 9200;
        let g = gearing(&two, &down, &held).unwrap();
        assert_eq!((g.over_revs, g.reference_over_revs), (1, 0));
        let o = &g.over_rev_at[0];
        assert_eq!((o.gear, o.at, o.rpm), (2, 0.899, 9200.0));
        assert!(
            g.to_text().contains(": into 2 at 90% of the lap, "),
            "{}",
            g.to_text()
        );
        // Just at it is no over-rev.
        down.rpm[901] = 9090;
        let g = gearing(&two, &down, &held).unwrap();
        assert_eq!(g.over_revs, 0);
        assert!(
            g.to_text()
                .contains("limiter about 9000 rpm (estimated from the laps)"),
            "{}",
            g.to_text()
        );
    }

    /// A lap at full throttle, straight on, a point every `step_ms`: speed,
    /// gear and revs of each point from `at`.
    fn built(step_ms: u32, at: impl Fn(usize) -> (f64, i8, f64)) -> LapTrace {
        let mut t = lap(1, 0.0);
        for k in 0..POINTS {
            let (v, g, r) = at(k);
            t.t_ms[k] = k as u32 * step_ms;
            t.speed[k] = (v * 10.0).round() as u16;
            t.gear[k] = g;
            t.rpm[k] = r.round() as u16;
        }
        t
    }

    #[test]
    fn the_small_parts() {
        let t = built(100, |k| (100.0 + k as f64 * 0.1, 2, 5000.0));
        assert_eq!(kmh(&t, POINTS), kmh(&t, POINTS - 1));
        assert_eq!(kmh(&t, POINTS), 199.9);
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut []), None);
        assert!(usable(&t));
        let short = |f: fn(&mut LapTrace)| {
            let mut s = t.clone();
            f(&mut s);
            usable(&s)
        };
        assert!(!short(|s| s.speed.truncate(10)));
        assert!(!short(|s| s.gear.truncate(10)));
        assert!(!short(|s| s.rpm.truncate(10)));
        assert!(!short(|s| s.throttle.truncate(10)));
        // The same gear around a point.
        let mut g = t.clone();
        g.gear[10] = 3;
        let held_at: Vec<usize> = (0..16).filter(|&k| held(&g, k)).collect();
        assert_eq!(held_at, [2, 3, 4, 5, 6, 7, 8, 13, 14, 15]);
        assert!(!held(&g, POINTS - 1));
        // Out of neutral is no change up.
        let n = built(100, |k| {
            let gear = match k {
                0..10 => 0,
                10..20 => 1,
                _ => 2,
            };
            (100.0, gear, 5000.0)
        });
        assert_eq!(upshifts(&n), [(1, 5000.0, 100.0)]);
        // Revs per km/h, not from the slowest points.
        let r = built(100, |k| {
            if k < 600 {
                (30.0, 2, 9000.0)
            } else {
                (100.0, 2, 5000.0)
            }
        });
        assert_eq!(ratio(&[&r], 2), Some(50.0));
    }

    #[test]
    fn the_pull_of_each_lap_and_its_spread() {
        // 1, 2 and 3 km/h a tenth of a second from 100 km/h.
        let by = |kmh: f64| built(100, move |k| (100.0 + kmh * k as f64, 2, 5000.0));
        let (a, b, c) = (by(1.0), by(2.0), by(3.0));
        let pull_of = |kmh: f64| kmh / 3.6 / 0.1;
        let s = samples(&a, 2);
        assert_eq!(s.len(), POINTS - 1);
        assert_eq!(s[0].0, 100.5);
        assert!((s[0].1 - pull_of(1.0)).abs() < 1e-9, "{}", s[0].1);
        // A point off the throttle takes the samples on either side of it.
        let mut off = a.clone();
        off.throttle[1] = 50;
        assert_eq!(samples(&off, 2)[0].0, 102.5);
        // Without the times of the points, nothing.
        let mut timeless = a.clone();
        timeless.t_ms.clear();
        assert!(samples(&timeless, 2).is_empty());
        // 100-105 km/h: one value a lap, the middle one, and its error.
        let p = pull(&[&a, &b, &c], 2);
        let (bin, m, se) = p[0];
        assert_eq!(bin, 20);
        assert!((m - pull_of(2.0)).abs() < 1e-9, "{m}");
        let spread = pull_of(1.0) * MAD_SD / 3f64.sqrt();
        assert!((se - spread).abs() < 1e-9, "{se}");
        // One lap alone tells nothing.
        assert!(pull(&[&a], 2).is_empty());
        // Two laps, but one sample each in 100-105.
        let p = pull(&[&by(5.0), &by(6.0)], 2);
        assert!(p.iter().all(|b| b.0 != 20), "{p:?}");
    }

    #[test]
    fn a_crossing_is_a_clear_turn_in_bins_next_to_each_other() {
        // Bin, pull of the lower gear, of the higher, error of the difference.
        let low = (20, 7.0, 6.0, 0.6);
        let high = (21, 5.0, 6.0, 0.8);
        // -1 then +1: a turn of 2, twice the error; zero halfway.
        assert_eq!(cross(&[low, high]), Some(105.0));
        assert_eq!(cross(&[low, high, (22, 4.0, 6.0, 0.1)]), Some(105.0));
        // From -2 to 0.
        assert_eq!(
            cross(&[(20, 8.0, 6.0, 0.6), (21, 6.0, 6.0, 0.8)]),
            Some(107.5)
        );
        // A turn of 1.9: within the noise.
        assert_eq!(cross(&[low, (21, 5.1, 6.0, 0.8)]), None);
        // Across a missing bin.
        assert_eq!(cross(&[low, (22, 5.0, 6.0, 0.8)]), None);
        // Turning back in the next bin, or a gap before it.
        assert_eq!(cross(&[low, high, (22, 7.0, 6.0, 0.1)]), None);
        assert_eq!(cross(&[low, high, (23, 4.0, 6.0, 0.1)]), None);
        // The higher gear pulling harder from the first bin.
        assert_eq!(cross(&[high]), None);
        assert_eq!(cross(&[]), None);
    }

    /// Up to 300 km/h by `gain` km/h a point (a quarter of a second) in 2nd at
    /// 30 rpm a km/h, held from point 40 with the revs rising by `rise` to
    /// point 49; 3rd from 51.
    fn cut_lap(gain: f64, rise: f64) -> LapTrace {
        built(250, move |k| match k {
            0..40 => (
                300.0 - gain * (40 - k) as f64,
                2,
                (300.0 - gain * (40 - k) as f64) * 30.0,
            ),
            40..51 => (300.0, 2, 9000.0 + rise * (k - 40).min(9) as f64 / 9.0),
            _ => (300.0 + 0.01 * (k - 50) as f64, 3, 6000.0),
        })
    }

    #[test]
    fn a_cut_is_flat_revs_right_after_a_hard_pull() {
        // 3.6 km/h in 0.75 s: 1.33 m/s². 1.2% faster over the 3 points, so
        // the revs of 10 points could rise by 3.6%; a cut rises by under a
        // quarter of it, 81 rpm.
        assert_eq!(
            clips(&cut_lap(1.2, 80.0)),
            [(40, 50, 9000.0 + 80.0 * 4.5 / 9.0)]
        );
        assert!(clips(&cut_lap(1.2, 82.0)).is_empty());
        // 1.8 km/h in 0.75 s: 0.67 m/s², no pull to cut.
        assert!(clips(&cut_lap(0.6, 0.0)).is_empty());
        // Flat at once: the last 2 points of the climb are within 1% of it,
        // from point 38 at 9.5 s to 50 at 12.5 s.
        let t = cut_lap(1.2, 0.0);
        assert_eq!(clips(&t), [(38, 50, 9000.0)]);
        assert_eq!(on_limiter(&t, 9000.0), 3.0);
        // No time between the points: nothing told of the pull.
        let mut still = t.clone();
        still.t_ms[35..=50].fill(10_000);
        assert!(clips(&still).is_empty(), "{:?}", clips(&still));
        // Without revs, below full throttle or in neutral: no cut.
        let mut none = t.clone();
        none.rpm = vec![0; POINTS];
        assert!(clips(&none).is_empty());
        let mut part = t.clone();
        part.throttle = vec![FULL - 1; POINTS];
        assert!(clips(&part).is_empty());
        let mut neutral = t.clone();
        neutral.gear = vec![0; POINTS];
        assert!(clips(&neutral).is_empty());
    }

    #[test]
    fn past_the_limiter_just_after_a_change_down() {
        // Down to 2nd at point 500, the revs high at one point after it.
        let down = |peak_at: usize, rpm: f64| {
            built(100, move |k| {
                let gear = if k <= 500 { 3 } else { 2 };
                (100.0, gear, if k == peak_at { rpm } else { 80.0 })
            })
        };
        // 1% over 100 rpm: 101, past it only above.
        assert!(over_revs(&down(501, 101.0), 100.0).is_empty());
        assert_eq!(
            over_revs(&down(503, 102.0), 100.0),
            [OverRev {
                gear: 2,
                at: 0.5,
                kmh: 100.0,
                rpm: 102.0
            }]
        );
        // Three points after it, no later.
        assert!(over_revs(&down(504, 102.0), 100.0).is_empty());
    }

    #[test]
    fn nothing_without_what_it_takes() {
        let t = lap(1, 175.0);
        let mut bad = t.clone();
        bad.rpm.clear();
        assert_eq!(gearing(&[], &bad, &t), None);
        assert_eq!(gearing(&[], &t, &bad), None);
        let mut dead = t.clone();
        dead.rpm = vec![0; POINTS];
        assert_eq!(gearing(&[], &dead, &dead), None);
    }

    #[test]
    fn the_text_tells_what_is_known() {
        let shift = |gear: i8, reference: bool, best: (Option<f64>, Option<f64>)| Shift {
            gear,
            rpm: 8000.0,
            kmh: 140.0,
            reference_rpm: reference.then_some(8200.0),
            reference_kmh: reference.then_some(145.0),
            best_kmh: best.0,
            best_rpm: best.1,
        };
        let g = Gearing {
            shifts: vec![
                shift(2, true, (Some(150.0), Some(8250.0))),
                shift(3, false, (Some(190.0), None)),
                shift(4, false, (None, None)),
            ],
            max_rpm: 8500.0,
            limiter_rpm: None,
            limiter_s: 0.0,
            reference_limiter_s: 0.0,
            over_revs: 0,
            over_rev_at: Vec::new(),
            reference_over_revs: 0,
        };
        assert_eq!(
            g.to_text(),
            "changes up: 2->3 at 8000 rpm 140 km/h (reference 8200 rpm, 145 km/h), pull crossover 150 km/h (8250 rpm, estimated from the laps, shift time not counted) | 3->4 at 8000 rpm 140 km/h, pull crossover 190 km/h | 4->5 at 8000 rpm 140 km/h | no limiter reached, highest 8500 rpm\n"
        );
    }
}
