//! When to stop, and what the stops of the others mean.
//!
//! Everything is measured in the race itself: how fast lap times fade within
//! a stint, what a stop costs, and what new tyres bring. The fade includes
//! the fuel that burns off, which is what counts for the plan. Nothing is
//! known about rules (mandatory stops, tyre sets): the engineer adds those.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use vr_core::{CarState, Flag, Frame, SessionKind};

use crate::report::*;
use crate::state::car_ref;
use crate::util::mean;
use crate::{RaceState, SessionRecord};

/// Laps a fade is measured over, at least.
const MIN_FADE_LAPS: usize = 4;
/// Slower than the stint's trend by this share: traffic, a mistake.
const OUTLIER: f64 = 0.01;
/// A car's own fade counts like the field's once it has this many laps.
const TRUST_LAPS: f64 = 6.0;
/// Fades beyond this are not tyres.
const MAX_FADE: f64 = 1.5;
/// Laps compared before and after a stop.
const AROUND_STOP: usize = 3;
/// A stop longer than this was a repair or a penalty.
const MAX_LOSS_S: f64 = 120.0;
/// Assumed where no stop was seen: which lap is best hardly depends on it.
const ASSUMED_LOSS_S: f64 = 30.0;
/// Passing in the pits needs this much more than the gap.
const MARGIN_S: f64 = 0.3;
/// Laps a rival is expected to answer within, and how far ahead a stop has to
/// be for the cars around to matter.
const ANSWER_LAPS: usize = 4;
/// Cars this close after a stop are traffic.
const TRAFFIC_S: f64 = 2.5;
/// New tyres differ from what the fade says by no more.
const MAX_FRESH: f64 = 2.0;
const MAX_STOPS: usize = 3;
/// Plans within this of the best one are listed.
const PLAN_SPREAD_S: f64 = 12.0;
/// Cars either side whose strategy is looked at.
const AROUND: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// From the player's own laps.
    Own,
    /// From the other cars of the class.
    Field,
    /// Own laps, steadied by the field's.
    Both,
    /// From earlier sessions on the track.
    Past,
    Unknown,
}

/// How lap times change over a stint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fade {
    /// Seconds per lap of tyre age; negative: the fuel burning off wins.
    pub own_s_per_lap: Option<f64>,
    /// Clean laps of the current stint it was measured over.
    pub own_laps: usize,
    /// Median of the class.
    pub field_s_per_lap: Option<f64>,
    /// What the plan uses.
    pub used_s_per_lap: f64,
    pub basis: Basis,
    /// Lap time on new tyres against the end of the stint before, as seen at
    /// the stops of this race (negative: faster).
    pub fresh_gain_s: Option<f64>,
    pub fresh_gain_stops: usize,
}

/// What a stop costs against driving on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PitLoss {
    pub seconds: Option<f64>,
    pub basis: Basis,
    pub stops: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// Laps at the end of which to stop; empty: no stop.
    pub stops: Vec<i32>,
    /// Slower than the best plan by this much.
    pub behind_best_s: f64,
}

/// Where the car comes out after a stop at the end of this lap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rejoin {
    pub position: u32,
    pub ahead: Option<CarRef>,
    pub ahead_s: Option<f64>,
    pub behind: Option<CarRef>,
    pub behind_s: Option<f64>,
    /// Someone within a few seconds.
    pub traffic: bool,
    /// Nobody stopped yet: the cost of the stop is the one the plans assume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumed_loss_s: Option<f64>,
}

/// A car near the player, and what stops could change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rival {
    pub car: CarRef,
    /// Positive: ahead of the player.
    pub gap_s: f64,
    /// The gap once both have made as many stops.
    pub net_gap_s: f64,
    pub stops: usize,
    /// Laps since the last stop (or the start).
    pub tyre_age: i32,
    pub fade_s_per_lap: Option<f64>,
    /// Average of the last clean laps against the player's (negative: faster).
    pub pace_delta_s: Option<f64>,
    pub in_pit: bool,
    /// Stopped within the last two laps.
    pub just_stopped: bool,
    /// Of a car ahead (net): gained by stopping now while it stays out 1, 2, ... laps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undercut_gain_s: Vec<f64>,
    /// Laps it would have to stay out for the undercut to work.
    pub undercut_laps: Option<usize>,
    /// Of a car behind (net): what it gains by stopping now while the player stays
    /// out 1, 2, ... laps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threat_gain_s: Vec<f64>,
    /// Laps after which the place is gone.
    pub threat_laps: Option<usize>,
    /// Of a car ahead (net) that just stopped: laps to stay out to come out ahead.
    pub overcut_laps: Option<usize>,
}

/// Something to act on, the most pressing first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum StrategyCall {
    /// The player must stop to have fuel.
    FuelStop { by_lap: i32 },
    /// Stopping at the end of this lap is the best plan.
    BoxThisLap { lap: i32 },
    /// The stop told for this lap is off: the next one, if any, is later.
    StayOut { next: Option<i32> },
    /// From now on one stop reaches the end.
    WindowOpen { until_lap: i32 },
    /// A car behind can pass by stopping now.
    UndercutThreat {
        car: CarRef,
        gap_s: f64,
        gain_s: f64,
    },
    /// A car behind has stopped: answer within so many laps or lose the place.
    Answer { car: CarRef, within_laps: usize },
    /// Stopping now passes a car ahead if it stays out one more lap.
    UndercutChance {
        car: CarRef,
        gap_s: f64,
        gain_s: f64,
    },
    /// A car ahead has stopped; staying out may pass it.
    OvercutChance { car: CarRef, laps: usize },
    /// Slow cars on track make a stop cheap.
    CheapStop,
    /// The best plan is a new one.
    Plan { stops: Vec<i32> },
}

impl StrategyCall {
    /// What tells one call from another: the same one is not made twice.
    pub(crate) fn key(&self) -> String {
        match self {
            Self::FuelStop { .. } => "fuel".into(),
            Self::BoxThisLap { .. } => "box".into(),
            Self::StayOut { .. } => "stay".into(),
            Self::WindowOpen { .. } => "window".into(),
            Self::UndercutThreat { car, .. } => format!("threat {}", car.number),
            Self::Answer { car, .. } => format!("answer {}", car.number),
            Self::UndercutChance { car, .. } => format!("undercut {}", car.number),
            Self::OvercutChance { car, .. } => format!("overcut {}", car.number),
            Self::CheapStop => "cheap".into(),
            Self::Plan { .. } => "plan".into(),
        }
    }

    pub fn text(&self) -> String {
        let who = |c: &CarRef| format!("#{} {}", c.number, c.driver);
        match self {
            Self::FuelStop { by_lap } => format!("fuel: stop by the end of lap {by_lap}"),
            Self::BoxThisLap { lap } => format!("best plan: stop at the end of this lap ({lap})"),
            Self::StayOut { next: Some(lap) } => {
                format!("stay out: no stop this lap, the next is at the end of lap {lap}")
            }
            Self::StayOut { next: None } => "stay out: no stop this lap".into(),
            Self::WindowOpen { until_lap } => {
                format!("pit window open: one stop now reaches the end (until lap {until_lap})")
            }
            Self::UndercutThreat { car, gap_s, gain_s } => format!(
                "undercut threat: {} is {gap_s:.1}s behind and gains {gain_s:.1}s by stopping first",
                who(car)
            ),
            Self::Answer { car, within_laps } => format!(
                "{} has stopped behind: answer within {within_laps} lap(s) or lose the place",
                who(car)
            ),
            Self::UndercutChance { car, gap_s, gain_s } => format!(
                "undercut chance: {} is {gap_s:.1}s ahead, stopping now gains {gain_s:.1}s",
                who(car)
            ),
            Self::OvercutChance { car, laps } => format!(
                "overcut chance: {} has stopped, staying out {laps} lap(s) comes out ahead",
                who(car)
            ),
            Self::CheapStop => "caution: a stop now costs less".into(),
            Self::Plan { stops } if stops.is_empty() => "plan: no stop".into(),
            Self::Plan { stops } => {
                let laps: Vec<String> = stops.iter().map(i32::to_string).collect();
                format!("plan: stop at the end of lap {}", laps.join(" and "))
            }
        }
    }
}

/// A plan that moved by less is the same one.
const PLAN_MOVES: i32 = 2;
/// A plan is kept as long as it loses less than this.
const PLAN_KEPT_S: f64 = 1.5;

/// Whether a stop at the end of `lap` still holds: for the fuel, or as a plan
/// about as good as the best. Without plans nothing is known to have changed.
pub(crate) fn still_boxing(r: &StrategyReport, lap: i32) -> bool {
    let fuel = r
        .calls
        .iter()
        .any(|c| matches!(c, StrategyCall::FuelStop { by_lap } if *by_lap <= lap));
    let planned = r
        .plans
        .iter()
        .any(|p| p.behind_best_s < PLAN_KEPT_S && p.stops.first() == Some(&lap));
    fuel || planned || r.plans.is_empty()
}

/// The plan to tell about, if the one told before no longer holds.
pub(crate) fn new_plan(told: Option<&[i32]>, plans: &[Plan]) -> Option<Vec<i32>> {
    let best = plans.first()?;
    let holds = told.is_some_and(|t| {
        plans
            .iter()
            .any(|p| p.behind_best_s < PLAN_KEPT_S && !changed(t, &p.stops))
    });
    (!holds).then(|| best.stops.clone())
}

/// Whether the plan is another one worth telling.
pub(crate) fn changed(from: &[i32], to: &[i32]) -> bool {
    from.len() != to.len()
        || from
            .iter()
            .zip(to)
            .any(|(a, b)| (a - b).abs() >= PLAN_MOVES)
}

/// What the stops of a kept session cost.
pub fn pit_loss_of(r: &SessionRecord) -> Option<f64> {
    let own = Stints::of(&r.laps);
    let others = r.others.iter().map(|o| Stints::of(&o.laps));
    let losses: Vec<f64> = std::iter::once(own)
        .chain(others)
        .flat_map(|s| s.stops)
        .map(|s| s.loss)
        .collect();
    median(&losses)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyReport {
    pub meta: Meta,
    /// Why there is no plan.
    pub missing: Vec<String>,
    /// Lap the player is on.
    pub lap: Option<i32>,
    pub laps_total: Option<i32>,
    pub stops_done: usize,
    pub tyre_age: i32,
    pub fade: Option<Fade>,
    pub pit_loss: PitLoss,
    /// Laps at the end of which a single stop for fuel is possible.
    pub fuel_window: Option<(i32, i32)>,
    /// The fuel lasts to the end.
    pub fuel_to_end: Option<bool>,
    pub plans: Vec<Plan>,
    pub rejoin_now: Option<Rejoin>,
    pub rivals: Vec<Rival>,
    pub calls: Vec<StrategyCall>,
    /// Faster tyres others are on, and whether changing to them pays before the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tyre_call: Option<TyreCall>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TyreCall {
    pub compound: String,
    /// Faster on them than on yours, per lap.
    pub gain_s_per_lap: f64,
    /// Laps the stop takes to pay back.
    pub payback_laps: Option<f64>,
    /// The lap in progress counted.
    pub laps_left: Option<i32>,
    /// It pays before the end.
    pub worth: Option<bool>,
}

/// Gains less a lap are the track, not the tyres.
const TYRE_GAIN_S: f64 = 0.5;

/// The fastest other tyres of the class against the driver's own.
pub(crate) fn tyre_call(
    groups: &[crate::report::TyreGroup],
    loss: Option<f64>,
    laps_left: Option<i32>,
) -> Option<TyreCall> {
    let mine = groups.iter().find(|g| g.you)?.pace_s?;
    let other = groups
        .iter()
        .filter(|g| !g.you)
        .filter_map(|g| Some((g, g.pace_s?)))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    let gain = mine - other.1;
    if gain < TYRE_GAIN_S {
        return None;
    }
    let payback = loss.map(|l| l / gain);
    Some(TyreCall {
        compound: other.0.compound.clone(),
        gain_s_per_lap: gain,
        payback_laps: payback,
        laps_left,
        // Paid back only on the last lap is no gain (and no float noise decides it).
        worth: payback.zip(laps_left).map(|(p, l)| p < f64::from(l) - 1e-6),
    })
}

/// Clean laps of a stint with the age of the tyres.
#[derive(Debug, Clone, Default)]
struct Stint {
    laps: Vec<(i32, f64)>,
    /// Lap the stint began with.
    first_lap: i32,
}

/// A complete stop.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stop {
    loss: f64,
    /// Pace after against before.
    gain: f64,
    /// Laps of age between the tyres those paces were driven on.
    ages: f64,
}

/// The laps of a car, cut at its stops.
#[derive(Debug, Clone, Default)]
struct Stints {
    stints: Vec<Stint>,
    stops: Vec<Stop>,
    stop_count: usize,
    last_stop_lap: Option<i32>,
}

/// How a car's lap times change per lap of its longest stint.
pub(crate) fn fade_of(laps: &[LapRecord]) -> Option<f64> {
    Stints::of(laps).fade()
}

/// Stops made, and how many laps old the tyres are on `lap`.
pub(crate) fn stint(laps: &[LapRecord], lap: i32) -> (usize, i32) {
    let s = Stints::of(laps);
    (s.stop_count, s.age(lap))
}

pub(crate) fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// Median age and time of some laps.
fn middle(laps: &[(i32, f64)]) -> Option<(f64, f64)> {
    median(&laps.iter().map(|l| f64::from(l.0)).collect::<Vec<_>>())
        .zip(median(&laps.iter().map(|l| l.1).collect::<Vec<_>>()))
}

/// Laps not held up by traffic or a mistake, seen against their own trend.
fn undisturbed(laps: &[(i32, f64)]) -> Vec<(i32, f64)> {
    let trend = if laps.len() < MIN_FADE_LAPS {
        0.0
    } else {
        trend(laps).unwrap_or(0.0)
    };
    let level = |l: &(i32, f64)| l.1 - trend * f64::from(l.0);
    let levels: Vec<f64> = laps.iter().map(level).collect();
    let Some(m) = median(&levels) else {
        return Vec::new();
    };
    laps.iter()
        .copied()
        .filter(|l| level(l) <= m * (1.0 + OUTLIER))
        .collect()
}

fn slope(laps: &[(i32, f64)]) -> Option<f64> {
    let laps = undisturbed(laps);
    if laps.len() < MIN_FADE_LAPS {
        return None;
    }
    trend(&laps).map(|s| s.clamp(-MAX_FADE, MAX_FADE))
}

/// Median of the slopes between all pairs (Theil-Sen): a few odd laps do not
/// move it.
fn trend(laps: &[(i32, f64)]) -> Option<f64> {
    let mut slopes = Vec::new();
    for (i, a) in laps.iter().enumerate() {
        for b in &laps[i + 1..] {
            if b.0 != a.0 {
                slopes.push((b.1 - a.1) / f64::from(b.0 - a.0));
            }
        }
    }
    median(&slopes)
}

impl Stints {
    fn of(laps: &[LapRecord]) -> Self {
        let mut out = Self::default();
        let mut stint = Stint {
            laps: Vec::new(),
            first_lap: 1,
        };
        let mut i = 0;
        while i < laps.len() {
            let l = &laps[i];
            if !l.pit {
                // The first lap starts standing.
                if l.is_clean() && l.lap > 1 {
                    let t = l.time_s.expect("clean laps have a time");
                    stint.laps.push((l.lap - stint.first_lap + 1, t));
                }
                i += 1;
                continue;
            }
            // In and out lap; a stop by the line may touch one lap only.
            let from = i;
            while i < laps.len() && laps[i].pit && i - from < 2 {
                i += 1;
            }
            let group = &laps[from..i];
            let came_out = i < laps.len() || group.len() == 2;
            out.stop_count += 1;
            out.last_stop_lap = Some(group[0].lap);
            let before = std::mem::replace(
                &mut stint,
                Stint {
                    laps: Vec::new(),
                    first_lap: group[0].lap + 1,
                },
            );
            let after: Vec<(i32, f64)> = laps[i..]
                .iter()
                .take_while(|l| !l.pit)
                .filter(|l| l.is_clean())
                .filter_map(|l| Some((l.lap - group[0].lap, l.time_s?)))
                .take(AROUND_STOP)
                .collect();
            let last: Vec<(i32, f64)> = undisturbed(&before.laps)
                .into_iter()
                .rev()
                .take(AROUND_STOP)
                .collect();
            let caution = group.iter().any(|l| l.caution);
            let times: Vec<f64> = group.iter().filter_map(|l| l.time_s).collect();
            if came_out
                && !caution
                && times.len() == group.len()
                && let (Some((b_age, b)), Some((a_age, a))) = (middle(&last), middle(&after))
            {
                let loss = times.iter().sum::<f64>() - times.len() as f64 * (a + b) / 2.0;
                if (0.0..MAX_LOSS_S).contains(&loss) {
                    out.stops.push(Stop {
                        loss,
                        gain: a - b,
                        ages: b_age - a_age,
                    });
                }
            }
            out.stints.push(before);
        }
        out.stints.push(stint);
        out
    }

    fn current(&self) -> &Stint {
        self.stints.last().expect("there is always one")
    }

    /// Of the stint in progress, or of the longest one before.
    fn fade(&self) -> Option<f64> {
        slope(&self.current().laps).or_else(|| {
            self.stints
                .iter()
                .max_by_key(|s| s.laps.len())
                .and_then(|s| slope(&s.laps))
        })
    }

    fn age(&self, lap: i32) -> i32 {
        (lap - self.current().first_lap + 1).max(1)
    }

    /// What the car laps in now, without odd laps, and at what tyre age.
    fn pace(&self, n: usize) -> Option<(f64, f64)> {
        self.current().pace(n)
    }

    /// Of the stint before, for a car that has just come out.
    fn pace_before(&self, n: usize) -> Option<(f64, f64)> {
        let before = self.stints.len().checked_sub(2)?;
        self.stints[before].pace(n)
    }
}

impl Stint {
    fn pace(&self, n: usize) -> Option<(f64, f64)> {
        let laps = undisturbed(&self.laps);
        let last: Vec<(i32, f64)> = laps.into_iter().rev().take(n).collect();
        Some((
            mean(&last.iter().map(|l| f64::from(l.0)).collect::<Vec<_>>())?,
            mean(&last.iter().map(|l| l.1).collect::<Vec<_>>())?,
        ))
    }
}

/// Lap time by the age of the tyres.
#[derive(Debug, Clone, Copy)]
struct Pace {
    /// On tyres of age zero, in the current stint.
    base: f64,
    fade: f64,
    /// New tyres against what the fade alone says (fuel load, track).
    fresh: f64,
}

impl Pace {
    fn old(&self, age: i32) -> f64 {
        self.base + self.fade * f64::from(age)
    }

    fn new_tyres(&self, age: i32) -> f64 {
        self.base + self.fresh + self.fade * f64::from(age)
    }
}

struct Field {
    fade: Option<f64>,
    loss: Option<f64>,
    loss_stops: usize,
    /// Pace change at a stop.
    gain: Option<f64>,
    /// What of it the age of the tyres does not explain.
    fresh: f64,
    gain_stops: usize,
}

fn field(all: &[(usize, Stints)], except: usize) -> Field {
    let others: Vec<&Stints> = all
        .iter()
        .filter(|(i, _)| *i != except)
        .map(|(_, s)| s)
        .collect();
    let fades: Vec<f64> = others.iter().filter_map(|s| s.fade()).collect();
    let fade = (fades.len() >= 2).then(|| median(&fades)).flatten();
    let losses: Vec<f64> = others
        .iter()
        .flat_map(|s| &s.stops)
        .map(|s| s.loss)
        .collect();
    // New tyres mean the same to all: the player's own stops count too.
    let mut gains = Vec::new();
    let mut unexplained = Vec::new();
    for (_, s) in all {
        for stop in &s.stops {
            gains.push(stop.gain);
            unexplained.extend(s.fade().or(fade).map(|f| stop.gain + f * stop.ages));
        }
    }
    Field {
        fade,
        loss: median(&losses),
        loss_stops: losses.len(),
        gain: median(&gains),
        fresh: median(&unexplained).map_or(0.0, |f| f.clamp(-MAX_FRESH, MAX_FRESH)),
        gain_stops: gains.len(),
    }
}

/// What one lap costs less by answering a lap later, summed up.
fn gains(mover: &Pace, stayer: &Pace, stayer_age: i32) -> Vec<f64> {
    let mut sum = 0.0;
    (1..=ANSWER_LAPS as i32)
        .map(|k| {
            sum += stayer.old(stayer_age + k) - mover.new_tyres(k);
            sum
        })
        .collect()
}

fn enough(gains: &[f64], gap: f64) -> Option<usize> {
    gains
        .iter()
        .position(|g| *g >= gap + MARGIN_S)
        .map(|i| i + 1)
}

/// What it costs to get somewhere, and the stops on the way.
type Best = Option<(f64, Vec<usize>)>;

/// The fastest plans for each number of stops, best first.
fn plans(
    pace: &Pace,
    age: i32,
    lap: i32,
    last: i32,
    loss: f64,
    now_laps: Option<f64>,
    full_laps: Option<f64>,
) -> Vec<Plan> {
    let n = (last - lap + 1).max(0) as usize;
    if n == 0 {
        return Vec::new();
    }
    // Laps of the first stint with what is in the tank; the lap in progress
    // is the first one.
    let first_max = now_laps.map_or(n, |l| (l.floor().max(0.0) as usize).min(n));
    let full = full_laps.map_or(n, |l| (l.floor().max(1.0) as usize).min(n));
    let run = |from: usize, to: usize, fresh: bool| -> f64 {
        (from..to)
            .map(|k| {
                let a = (k - from) as i32 + 1;
                if fresh {
                    pace.new_tyres(a)
                } else {
                    pace.old(age + a - 1)
                }
            })
            .sum()
    };
    // best[s][i]: i laps done, the last stop (if any) just before lap i.
    let mut best: Vec<Vec<Best>> = vec![vec![None; n + 1]; MAX_STOPS + 1];
    best[0][0] = Some((0.0, Vec::new()));
    for s in 1..=MAX_STOPS {
        for to in 1..n {
            let mut found: Best = None;
            for (from, before) in best[s - 1].iter().enumerate().take(to) {
                let Some((cost, stops)) = before else {
                    continue;
                };
                let fresh = s > 1;
                if to - from > if fresh { full } else { first_max } {
                    continue;
                }
                let total = cost + run(from, to, fresh) + loss;
                if found.as_ref().is_none_or(|(b, _)| total < *b) {
                    let mut all = stops.clone();
                    all.push(to);
                    found = Some((total, all));
                }
            }
            best[s][to] = found;
        }
    }
    let mut out: Vec<(f64, Vec<usize>)> = Vec::new();
    for (s, row) in best.iter().enumerate() {
        for (from, entry) in row.iter().enumerate().take(n) {
            let Some((cost, stops)) = entry else { continue };
            if n - from > if s > 0 { full } else { first_max } {
                continue;
            }
            out.push((cost + run(from, n, s > 0), stops.clone()));
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
    let Some(fastest) = out.first().map(|p| p.0) else {
        return Vec::new();
    };
    let mut kept: Vec<Plan> = Vec::new();
    for (cost, stops) in out {
        let behind = cost - fastest;
        // Of every number of stops the best one and what is about as good.
        let same = kept.iter().filter(|p| p.stops.len() == stops.len()).count();
        if behind > PLAN_SPREAD_S && !kept.is_empty() || same >= 4 {
            continue;
        }
        kept.push(Plan {
            stops: stops.iter().map(|k| lap + *k as i32 - 1).collect(),
            behind_best_s: behind,
        });
    }
    kept.truncate(8);
    kept
}

fn rejoin(frame: &Frame, player: &CarState, loss: f64) -> Option<Rejoin> {
    let mine = player.gap_to_leader_s? + loss;
    let mut others: Vec<(f64, &CarState)> = frame
        .cars
        .iter()
        .filter(|c| c.idx != player.idx && c.class_id == player.class_id)
        .filter_map(|c| Some((c.gap_to_leader_s?, c)))
        .collect();
    others.sort_by(|a, b| a.0.total_cmp(&b.0));
    let ahead = others.iter().rev().find(|(g, _)| *g <= mine);
    let behind = others.iter().find(|(g, _)| *g > mine);
    let position = others.iter().filter(|(g, _)| *g <= mine).count() as u32 + 1;
    let (ahead_s, behind_s) = (ahead.map(|a| mine - a.0), behind.map(|b| b.0 - mine));
    Some(Rejoin {
        position,
        ahead: ahead.map(|a| car_ref(a.1)),
        ahead_s,
        behind: behind.map(|b| car_ref(b.1)),
        behind_s,
        traffic: [ahead_s, behind_s].iter().flatten().any(|s| *s < TRAFFIC_S),
        assumed_loss_s: None,
    })
}

/// `prior`: pit loss of earlier sessions on the track.
pub(crate) fn strategy(state: &RaceState, prior_loss: Option<f64>) -> StrategyReport {
    let mut report = StrategyReport {
        meta: state.meta(),
        missing: Vec::new(),
        lap: None,
        laps_total: None,
        stops_done: 0,
        tyre_age: 0,
        fade: None,
        pit_loss: PitLoss {
            seconds: None,
            basis: Basis::Unknown,
            stops: 0,
        },
        fuel_window: None,
        fuel_to_end: None,
        plans: Vec::new(),
        rejoin_now: None,
        rivals: Vec::new(),
        calls: Vec::new(),
        tyre_call: None,
    };
    let Some(frame) = state.frame() else {
        report.missing.push("no data yet".into());
        return report;
    };
    let Some(player) = frame.player_car() else {
        report.missing.push("no car".into());
        return report;
    };
    if frame.session.kind != SessionKind::Race {
        report.missing.push("not a race".into());
        return report;
    }
    let done = player.laps_completed.unwrap_or(0).max(0);
    let lap = player.lap.unwrap_or(done + 1).max(1);
    report.lap = Some(lap);

    let all: Vec<(usize, Stints)> = frame
        .cars
        .iter()
        .filter(|c| c.class_id == player.class_id)
        .map(|c| (c.idx, Stints::of(&state.car_laps(c.idx))))
        .collect();
    let own = &all
        .iter()
        .find(|(i, _)| *i == player.idx)
        .expect("the player is in its class")
        .1;
    let field = field(&all, player.idx);
    report.stops_done = own.stop_count;
    report.tyre_age = own.age(lap);

    // The fade: the player's own, steadied by the field's while it is young.
    let own_fade = slope(&own.current().laps);
    let own_laps = undisturbed(&own.current().laps).len();
    let (used, basis) = match (own_fade.or(own.fade()), field.fade) {
        (Some(o), Some(f)) => {
            let w = own_laps as f64 / (own_laps as f64 + TRUST_LAPS);
            (o * w + f * (1.0 - w), Basis::Both)
        }
        (Some(o), None) => (o, Basis::Own),
        (None, Some(f)) => (f, Basis::Field),
        (None, None) => (0.0, Basis::Unknown),
    };
    report.fade = Some(Fade {
        own_s_per_lap: own_fade,
        own_laps,
        field_s_per_lap: field.fade,
        used_s_per_lap: used,
        basis,
        fresh_gain_s: field.gain,
        fresh_gain_stops: field.gain_stops,
    });

    let own_losses: Vec<f64> = own.stops.iter().map(|s| s.loss).collect();
    let (loss, loss_basis, stops) = match (median(&own_losses), field.loss, prior_loss) {
        (Some(l), _, _) => (Some(l), Basis::Own, own_losses.len()),
        (None, Some(l), _) => (Some(l), Basis::Field, field.loss_stops),
        (None, None, Some(l)) => (Some(l), Basis::Past, 0),
        (None, None, None) => (None, Basis::Unknown, 0),
    };
    report.pit_loss = PitLoss {
        seconds: loss,
        basis: loss_basis,
        stops,
    };

    let fuel = state.fuel_info(frame);
    let to_finish = fuel.as_ref().and_then(|f| f.laps_to_finish);
    // The lap in progress counts as the first one left.
    let last = to_finish.map(|l| {
        let pct = player.lap_dist_pct.unwrap_or(0.0);
        lap + (l + pct - 1e-6).ceil().max(1.0) as i32 - 1
    });
    report.laps_total = frame.session.laps_total.or(last);
    let pct = player.lap_dist_pct.unwrap_or(0.0);
    // Counted from the beginning of the lap in progress.
    let now_laps = fuel.as_ref().and_then(|f| f.laps_of_fuel).map(|l| l + pct);
    let full_laps = fuel
        .as_ref()
        .and_then(|f| Some(f.capacity_l? / f.per_lap_l.filter(|p| *p > 0.0)?));
    if let (Some(last), Some(now)) = (last, now_laps) {
        let n = last - lap + 1;
        report.fuel_to_end = Some(now >= f64::from(n));
        let latest = lap + (now.floor() as i32).clamp(0, n) - 1;
        if let Some(full) = full_laps
            && now < f64::from(n)
        {
            let earliest = (last - full.floor() as i32).max(lap);
            if earliest <= latest {
                report.fuel_window = Some((earliest, latest));
            }
        }
    }

    let pace_of = |s: &Stints| -> Option<Pace> {
        let ((age, now), shift) = match s.pace(3) {
            Some(p) => (p, 0.0),
            None => (s.pace_before(3)?, field.fresh),
        };
        // Cars fade alike unless their own laps say otherwise.
        let fade = match (slope(&s.current().laps), field.fade) {
            (Some(o), Some(f)) => {
                let w = s.current().laps.len() as f64;
                o * w / (w + TRUST_LAPS) + f * TRUST_LAPS / (w + TRUST_LAPS)
            }
            (Some(o), None) => o,
            (None, Some(f)) => f,
            (None, None) => used,
        };
        Some(Pace {
            base: now - fade * age + shift,
            fade,
            fresh: field.fresh,
        })
    };
    let mine = pace_of(own);
    // A stop that has to be made can be planned without knowing its cost.
    let forced = report.fuel_to_end == Some(false);
    let assumed = loss.or(forced.then_some(ASSUMED_LOSS_S));
    if loss.is_none() && forced {
        let what = format!(
            "the cost of a stop (nobody stopped yet; the plans assume {ASSUMED_LOSS_S:.0}s)"
        );
        report.missing.push(what);
    }
    match (mine, last, assumed) {
        (Some(p), Some(last), Some(loss)) => {
            report.plans = plans(&p, report.tyre_age, lap, last, loss, now_laps, full_laps);
            if report.plans.is_empty() {
                report
                    .missing
                    .push("no plan reaches the finish on fuel".into());
            }
        }
        (p, l, s) => {
            for (there, what) in [
                (p.is_some(), "clean laps of the player"),
                (l.is_some(), "the length of the race"),
                (s.is_some(), "the cost of a stop (nobody stopped yet)"),
            ] {
                if !there {
                    report.missing.push(what.into());
                }
            }
        }
    }
    // A stop to be made: where it comes out, at the cost the plans assume.
    if let Some(cost) = assumed {
        report.rejoin_now = rejoin(frame, player, cost).map(|r| Rejoin {
            assumed_loss_s: loss.is_none().then_some(cost),
            ..r
        });
    }
    report.rivals = rivals(frame, player, &all, mine, &pace_of, &report);
    report.calls = calls(&report, frame, lap);
    report.tyre_call = tyre_call(
        &crate::tyres::field_tyres(state, frame),
        loss,
        report.laps_total.map(|t| t - lap + 1),
    );
    report
}

fn rivals(
    frame: &Frame,
    player: &CarState,
    all: &[(usize, Stints)],
    mine: Option<Pace>,
    pace_of: &dyn Fn(&Stints) -> Option<Pace>,
    report: &StrategyReport,
) -> Vec<Rival> {
    let age = report.tyre_age;
    let Some(my_gap) = player.gap_to_leader_s else {
        return Vec::new();
    };
    let stops_of = |c: &CarState| {
        let s = all.iter().find(|(i, _)| *i == c.idx);
        s.map_or(0, |(_, s)| s.stop_count) as f64
    };
    let mine_done = report.stops_done as f64;
    // A car that has stopped while the player has not is nearer than it seems.
    let net = |c: &CarState, gap: f64| {
        gap + report.pit_loss.seconds.unwrap_or(0.0) * (stops_of(c) - mine_done)
    };
    let mut near: Vec<(&CarState, f64, f64)> = frame
        .cars
        .iter()
        .filter(|c| c.idx != player.idx && c.class_id == player.class_id)
        .filter_map(|c| {
            let gap = my_gap - c.gap_to_leader_s?;
            Some((c, gap, net(c, gap)))
        })
        .collect();
    near.sort_by(|a, b| b.2.total_cmp(&a.2));
    let ahead = near.iter().take_while(|c| c.2 > 0.0).count();
    near = near
        .into_iter()
        .skip(ahead.saturating_sub(AROUND))
        .take(AROUND + ahead.min(AROUND))
        .collect();
    let own_avg = all
        .iter()
        .find(|(i, _)| *i == player.idx)
        .and_then(|(_, s)| s.pace(3));
    near.into_iter()
        .filter_map(|(c, gap, net)| {
            let s = &all.iter().find(|(i, _)| *i == c.idx)?.1;
            let their_lap = c.lap.unwrap_or(1);
            let their_age = s.age(their_lap);
            let just = s.last_stop_lap.is_some_and(|l| their_lap - l <= 2);
            let theirs = pace_of(s);
            let mut r = Rival {
                car: car_ref(c),
                gap_s: gap,
                net_gap_s: net,
                stops: s.stop_count,
                tyre_age: their_age,
                fade_s_per_lap: slope(&s.current().laps),
                pace_delta_s: s.pace(3).zip(own_avg).map(|(t, o)| t.1 - o.1),
                in_pit: c.on_pit_road,
                just_stopped: just,
                undercut_gain_s: Vec::new(),
                undercut_laps: None,
                threat_gain_s: Vec::new(),
                threat_laps: None,
                overcut_laps: None,
            };
            if let (Some(me), Some(them)) = (mine, theirs) {
                if net > 0.0 && just {
                    // It is on new tyres: does staying out gain anything?
                    let mut sum = 0.0;
                    r.overcut_laps = (1..=ANSWER_LAPS as i32)
                        .find(|k| {
                            sum += them.old(their_age + k) - me.old(age + k);
                            sum >= net + MARGIN_S
                        })
                        .map(|k| k as usize);
                } else if net > 0.0 {
                    r.undercut_gain_s = gains(&me, &them, their_age);
                    r.undercut_laps = enough(&r.undercut_gain_s, net);
                } else {
                    let gain = if just {
                        // Already on the new ones.
                        let mut sum = 0.0;
                        (1..=ANSWER_LAPS as i32)
                            .map(|k| {
                                sum += me.old(age + k) - them.old(their_age + k);
                                sum
                            })
                            .collect()
                    } else {
                        gains(&them, &me, age)
                    };
                    r.threat_laps = enough(&gain, -net - 2.0 * MARGIN_S);
                    r.threat_gain_s = gain;
                }
            }
            Some(r)
        })
        .collect()
}

fn calls(r: &StrategyReport, frame: &Frame, lap: i32) -> Vec<StrategyCall> {
    let mut out = Vec::new();
    let stop_planned = r.plans.first().is_some_and(|p| !p.stops.is_empty());
    if let Some((_, latest)) = r.fuel_window
        && latest <= lap + 1
    {
        out.push(StrategyCall::FuelStop { by_lap: latest });
    }
    let slow = frame
        .session
        .flags
        .iter()
        .any(|f| matches!(f, Flag::Caution | Flag::Red));
    if slow && stop_planned {
        out.push(StrategyCall::CheapStop);
    }
    if let Some(p) = r.plans.first()
        && p.stops.first() == Some(&lap)
    {
        out.push(StrategyCall::BoxThisLap { lap });
    }
    // The cars next to the player decide; the others are a matter of the plan.
    let behind = r.rivals.iter().find(|c| c.net_gap_s < 0.0);
    let ahead = r.rivals.iter().rev().find(|c| c.net_gap_s > 0.0);
    // Only with a stop to come soon: on young tyres nobody undercuts.
    let first = r.plans.first().and_then(|p| p.stops.first());
    let soon = first.is_some_and(|s| s - lap <= ANSWER_LAPS as i32)
        || r.fuel_window.is_some_and(|w| w.0 <= lap);
    if let Some(b) = behind
        && soon
        && let Some(within) = b.threat_laps
    {
        if b.in_pit {
            // Known once it is out again.
        } else if b.just_stopped {
            out.push(StrategyCall::Answer {
                car: b.car.clone(),
                within_laps: within,
            });
        } else if within == 1 {
            out.push(StrategyCall::UndercutThreat {
                car: b.car.clone(),
                gap_s: -b.net_gap_s,
                gain_s: b.threat_gain_s[0],
            });
        }
    }
    if let Some(a) = ahead
        && soon
    {
        if a.undercut_laps == Some(1) && !a.in_pit {
            out.push(StrategyCall::UndercutChance {
                car: a.car.clone(),
                gap_s: a.net_gap_s,
                gain_s: a.undercut_gain_s[0],
            });
        }
        if let Some(laps) = a.overcut_laps {
            out.push(StrategyCall::OvercutChance {
                car: a.car.clone(),
                laps,
            });
        }
    }
    if let Some((earliest, latest)) = r.fuel_window
        && earliest == lap
    {
        out.push(StrategyCall::WindowOpen { until_lap: latest });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_call_holds_while_a_plan_as_good_or_the_fuel_wants_it() {
        let st = RaceState::new(
            "t",
            crate::SourceKind::Synthetic,
            vr_core::Capabilities::ALL,
        );
        let mut r = strategy(&st, None);
        // Nothing known: nothing changed.
        assert!(still_boxing(&r, 8));
        let plan = |stops: Vec<i32>, behind_best_s: f64| Plan {
            stops,
            behind_best_s,
        };
        r.plans = vec![plan(vec![10], 0.0), plan(vec![8], 1.49)];
        assert!(still_boxing(&r, 8));
        r.plans[1].behind_best_s = 1.5;
        assert!(!still_boxing(&r, 8));
        r.calls = vec![StrategyCall::FuelStop { by_lap: 9 }];
        assert!(!still_boxing(&r, 8));
        r.calls = vec![StrategyCall::FuelStop { by_lap: 8 }];
        assert!(still_boxing(&r, 8));
    }

    fn lap(lap: i32, time: f64, pit: bool) -> LapRecord {
        LapRecord {
            lap,
            time_s: Some(time),
            fuel_used_l: None,
            pit,
            caution: false,
            position: None,
            session_time_s: 0.0,
        }
    }

    #[test]
    fn the_fade_survives_odd_laps() {
        let mut laps: Vec<(i32, f64)> = (2..12).map(|i| (i, 90.0 + 0.1 * f64::from(i))).collect();
        assert!((slope(&laps).unwrap() - 0.1).abs() < 1e-9);
        laps[4].1 += 8.0;
        laps[7].1 += 1.0;
        assert!((slope(&laps).unwrap() - 0.1).abs() < 0.02);
        assert_eq!(slope(&laps[..MIN_FADE_LAPS - 1]), None);
        assert!(slope(&laps[..MIN_FADE_LAPS]).is_some());
        let wild: Vec<(i32, f64)> = (1..6)
            .map(|i| (i, 90.0 + 1.9 * f64::from(i) / 5.0))
            .collect();
        assert!(slope(&wild).is_some());
    }

    #[test]
    fn stops_are_measured() {
        // 6 laps at 90, in 100, out 105, 4 laps at 89.
        let mut laps = vec![lap(1, 95.0, false)];
        laps.extend((2..=7).map(|i| lap(i, 90.0, false)));
        laps.push(lap(8, 100.0, true));
        laps.push(lap(9, 105.0, true));
        laps.extend((10..=13).map(|i| lap(i, 89.0, false)));
        let s = Stints::of(&laps);
        assert_eq!((s.stop_count, s.last_stop_lap), (1, Some(8)));
        // 205 s for two laps that take 89.5 s.
        assert_eq!(
            s.stops,
            [Stop {
                loss: 26.0,
                gain: -1.0,
                // Laps 5 to 7 against the second to fourth on new tyres.
                ages: 3.0,
            }]
        );
        // The first lap starts standing and does not count.
        assert_eq!(s.stints[0].laps.len(), 6);
        assert_eq!(s.current().laps[0], (2, 89.0));
        assert_eq!(s.age(14), 6);
        // Still in the pits: not measured yet.
        let s = Stints::of(&laps[..8]);
        assert_eq!((s.stop_count, s.stops.len()), (1, 0));
    }

    fn pace(fade: f64, fresh: f64) -> Pace {
        Pace {
            base: 90.0,
            fade,
            fresh,
        }
    }

    #[test]
    fn one_stop_splits_the_race() {
        // 20 laps to go on new tyres, fuel is no matter: equal stints.
        let p = plans(&pace(0.2, 0.0), 1, 1, 20, 10.0, None, None);
        assert_eq!(p[0].stops, [10]);
        // Without fade nobody stops.
        let p = plans(&pace(0.0, 0.0), 1, 1, 20, 20.0, None, None);
        assert_eq!(p[0].stops, Vec::<i32>::new());
        assert_eq!(p.len(), 1);
        // Worn tyres come off sooner.
        let p = plans(&pace(0.2, 0.0), 8, 8, 20, 5.0, None, None);
        assert!(p[0].stops[0] < 14, "{p:?}");
    }

    #[test]
    fn fuel_limits_the_stints() {
        // 8.4 laps in the tank, 15 in a full one, 20 to go.
        let p = plans(&pace(0.0, 0.0), 1, 1, 20, 20.0, Some(8.4), Some(15.0));
        assert!(p.iter().all(|p| !p.stops.is_empty()));
        assert!(
            p[0].stops.len() == 1 && (5..=8).contains(&p[0].stops[0]),
            "{p:?}"
        );
        // A faded tyre wants the stop late, the fuel allows lap 8.
        let p = plans(&pace(-0.05, 0.0), 1, 1, 20, 20.0, Some(8.4), Some(15.0));
        assert_eq!(p[0].stops.len(), 1);
        // Two stops when a tank does not reach.
        let p = plans(&pace(0.0, 0.0), 1, 1, 20, 20.0, Some(8.4), Some(7.0));
        assert_eq!(p[0].stops.len(), 2);
        // Not even three stops.
        assert!(plans(&pace(0.0, 0.0), 1, 1, 60, 20.0, Some(8.4), Some(7.0)).is_empty());
    }

    #[test]
    fn plans_match_trying_everything() {
        let p = pace(0.13, 0.4);
        let cost = |stop: i32| -> f64 {
            let first: f64 = (0..stop).map(|k| p.old(5 + k)).sum();
            let second: f64 = (1..=(25 - stop)).map(|k| p.new_tyres(k)).sum();
            first + second + 18.0
        };
        let best = (1..25)
            .min_by(|a, b| cost(*a).total_cmp(&cost(*b)))
            .unwrap();
        let got = plans(&p, 5, 6, 30, 18.0, None, None);
        let one = got.iter().find(|p| p.stops.len() == 1).unwrap();
        assert_eq!(one.stops, [5 + best]);
    }

    #[test]
    fn a_plan_has_to_move_to_be_told_again() {
        assert!(!changed(&[12], &[13]));
        assert!(changed(&[12], &[14]));
        assert!(changed(&[12], &[]));
        assert!(changed(&[], &[12]));
        assert!(!changed(&[10, 20], &[11, 19]));
        assert!(changed(&[10, 20], &[10, 22]));
    }

    #[test]
    fn a_plan_is_kept_while_it_is_about_as_good() {
        let plan = |stops: &[i32], behind_best_s| Plan {
            stops: stops.to_vec(),
            behind_best_s,
        };
        let plans = [plan(&[17], 0.0), plan(&[], 1.4), plan(&[20], 1.5)];
        assert_eq!(new_plan(None, &plans), Some(vec![17]));
        assert_eq!(new_plan(Some(&[17]), &plans), None);
        assert_eq!(new_plan(Some(&[16]), &plans), None);
        assert_eq!(new_plan(Some(&[]), &plans), None);
        assert_eq!(new_plan(Some(&[20]), &plans), Some(vec![17]));
        assert_eq!(new_plan(Some(&[20]), &[]), None);
    }

    #[test]
    fn an_undercut_needs_more_than_the_gap() {
        // The stayer is on 15 laps old tyres with 0.1 s per lap.
        let g = gains(&pace(0.1, 0.0), &pace(0.1, 0.0), 15);
        assert!(
            (g[0] - 1.5).abs() < 1e-9 && (g[1] - 3.0).abs() < 1e-9,
            "{g:?}"
        );
        assert_eq!(enough(&g, 1.2), Some(1));
        assert_eq!(enough(&g, 1.21), Some(2));
        assert_eq!(enough(&g, 5.71), None);
    }
}
