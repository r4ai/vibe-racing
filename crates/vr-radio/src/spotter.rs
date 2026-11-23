//! The spotter: which side a car is on, when it is gone, and when one comes
//! from behind. Works on the live values of every frame, without a model: a
//! call that comes a second late is worse than none, and so is a long one.
//!
//! The sides are the game's own word, which is exact; how far the cars are is
//! measured along the track, to warn before they are beside and to be sure
//! when they are not any more.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::expr::Values;

/// Clear of a car only once it stays away: the game's word flickers.
const CLEAR: Duration = Duration::from_millis(250);
/// The same when no car is within its length.
const SURELY_CLEAR: Duration = Duration::from_millis(80);
/// Nose to tail, cars that do not overlap are further apart than this.
const CAR_M: f64 = 6.0;
/// A car that stays beside is told again this often.
const STILL: Duration = Duration::from_secs(3);
/// Without values for so long, nothing is known about the cars beside.
const STALE: Duration = Duration::from_millis(1500);
/// Slower cars stand on the grid or in the pits.
const MIN_KMH: f64 = 40.0;
/// A car comes when it is this near and quicker by this much, and will be
/// beside within this time.
const COMES_M: f64 = 25.0;
const COMES_MPS: f64 = 3.0;
const COMES_S: f64 = 1.0;
/// Told once, until it has been further away than that.
const GONE_M: f64 = 35.0;
/// How fast it comes is measured over this time, at least over that.
const PACE: Duration = Duration::from_millis(500);
const PACE_MIN: Duration = Duration::from_millis(150);
/// Nothing comes faster: it is another car.
const JUMP_MPS: f64 = 60.0;

/// Everything it says, to have the voice ready.
pub const PHRASES: [&str; 13] = [
    LEFT,
    RIGHT,
    TWO_LEFT,
    TWO_RIGHT,
    BOTH,
    CLEAR_ALL,
    CLEAR_LEFT,
    CLEAR_RIGHT,
    STILL_LEFT,
    STILL_RIGHT,
    STILL_BOTH,
    ALSO,
    COMES,
];
const LEFT: &str = "左。";
const RIGHT: &str = "右。";
const TWO_LEFT: &str = "左2台。";
const TWO_RIGHT: &str = "右2台。";
const BOTH: &str = "両側。";
const CLEAR_ALL: &str = "クリア。";
const CLEAR_LEFT: &str = "左クリア。";
const CLEAR_RIGHT: &str = "右クリア。";
const STILL_LEFT: &str = "まだ左。";
const STILL_RIGHT: &str = "まだ右。";
const STILL_BOTH: &str = "まだ両側。";
const ALSO: &str = "横。";
const COMES: &str = "後ろ、来る。";

/// Cars on the left and on the right.
type Beside = (u8, u8);

#[derive(Debug, Default)]
pub struct Spotter {
    /// What the driver was told.
    told: Beside,
    /// What the values say, and since when.
    seen: Option<(Beside, Instant)>,
    last_value: Option<Instant>,
    last_call: Option<Instant>,
    /// How far behind the next car was lately.
    behind: VecDeque<(Instant, f64)>,
    /// The driver knows that it comes.
    coming: bool,
}

impl Spotter {
    /// Forgets the cars beside: nothing is said about them leaving.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// What to say with these values.
    pub fn check(&mut self, v: &Values, now: Instant) -> Option<&'static str> {
        let since = |t: Option<Instant>| t.map(|t| now.saturating_duration_since(t));
        if since(self.last_value).is_some_and(|d| d > STALE) {
            self.reset();
        }
        self.last_value = Some(now);
        let get = |k: &str| v.get(k).copied().unwrap_or(0.0);
        let (left, right) = (v.get("left"), v.get("right"));
        let (Some(left), Some(right)) = (left, right) else {
            return None;
        };
        if get("on_track") <= 0.0 || get("on_pit") > 0.0 || get("speed") < MIN_KMH {
            *self = Self {
                last_value: self.last_value,
                ..Self::default()
            };
            return None;
        }
        let beside = (*left as u8, *right as u8);
        let began = match self.seen {
            Some((b, t)) if b == beside => t,
            _ => now,
        };
        self.seen = Some((beside, began));
        if beside == self.told {
            let there = beside != (0, 0);
            if there && since(self.last_call).is_none_or(|d| d >= STILL) {
                return self.say(beside, still(beside), now);
            }
            return self.comes(v, beside, now);
        }
        // Coming is told at once, leaving once it is certain.
        let fewer = beside.0 <= self.told.0 && beside.1 <= self.told.1;
        let apart = |k: &str| v.get(k).map(|m| *m > CAR_M);
        let wait = match (apart("near_ahead_m"), apart("near_behind_m")) {
            (None, None) => CLEAR,
            (a, b) if a != Some(false) && b != Some(false) => SURELY_CLEAR,
            _ => CLEAR,
        };
        if fewer && now.saturating_duration_since(began) < wait {
            return None;
        }
        let text = change(self.told, beside);
        self.say(beside, text, now)
    }

    /// A car that will be beside in a moment: there is still time to leave it
    /// the room, or not to.
    fn comes(&mut self, v: &Values, beside: Beside, now: Instant) -> Option<&'static str> {
        let Some(m) = v.get("near_behind_m").copied().filter(|_| beside == (0, 0)) else {
            self.behind.clear();
            return None;
        };
        if m > GONE_M {
            self.coming = false;
        }
        while self
            .behind
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) > PACE)
        {
            self.behind.pop_front();
        }
        let pace = self.behind.front().and_then(|(t, was)| {
            let d = now.saturating_duration_since(*t);
            (d >= PACE_MIN).then(|| (was - m) / d.as_secs_f64())
        });
        if pace.is_some_and(|p| p.abs() > JUMP_MPS) {
            self.behind.clear();
        }
        self.behind.push_back((now, m));
        let pace = pace.filter(|p| (COMES_MPS..=JUMP_MPS).contains(p))?;
        let soon = (m - CAR_M) / pace <= COMES_S;
        if self.coming || m > COMES_M || !soon {
            return None;
        }
        self.coming = true;
        Some(COMES)
    }

    fn say(&mut self, b: Beside, text: &'static str, now: Instant) -> Option<&'static str> {
        self.told = b;
        self.last_call = Some(now);
        self.behind.clear();
        Some(text)
    }
}

fn still(b: Beside) -> &'static str {
    match b {
        (_, 0) => STILL_LEFT,
        (0, _) => STILL_RIGHT,
        _ => STILL_BOTH,
    }
}

fn change(was: Beside, b: Beside) -> &'static str {
    match (was, b) {
        (_, (0, 0)) => CLEAR_ALL,
        // From one side to the other.
        ((l, 0), (0, 1)) if l > 0 => RIGHT,
        ((0, r), (1, 0)) if r > 0 => LEFT,
        ((_, r), (_, 0)) if r > 0 => CLEAR_RIGHT,
        ((l, _), (0, _)) if l > 0 => CLEAR_LEFT,
        (_, (1, 0)) => LEFT,
        (_, (0, 1)) => RIGHT,
        (_, (2, 0)) => TWO_LEFT,
        (_, (0, 2)) => TWO_RIGHT,
        (_, (l, r)) if l > 0 && r > 0 => BOTH,
        _ => ALSO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(left: f64, right: f64) -> Values {
        [
            ("left", left),
            ("right", right),
            ("on_track", 1.0),
            ("speed", 150.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    /// Values every 50 ms: what is said, and when (ms).
    fn run(steps: &[(u64, f64, f64)]) -> Vec<(u64, &'static str)> {
        let mut s = Spotter::default();
        let t0 = Instant::now();
        let mut out = Vec::new();
        let mut ms = 0;
        for (until, l, r) in steps {
            while ms < *until {
                if let Some(text) = s.check(&at(*l, *r), t0 + Duration::from_millis(ms)) {
                    out.push((ms, text));
                }
                ms += 50;
            }
        }
        out
    }

    #[test]
    fn a_car_comes_and_goes() {
        let got = run(&[(1000, 0.0, 0.0), (2000, 1.0, 0.0), (4000, 0.0, 0.0)]);
        // At once when it comes, 0.25 s later when it goes.
        assert_eq!(got, [(1000, LEFT), (2250, CLEAR_ALL)]);
        let got = run(&[(500, 0.0, 0.0), (1500, 0.0, 1.0), (3000, 0.0, 0.0)]);
        assert_eq!(got, [(500, RIGHT), (1750, CLEAR_ALL)]);
    }

    #[test]
    fn a_flicker_is_not_clear() {
        let got = run(&[(1000, 1.0, 0.0), (1250, 0.0, 0.0), (2950, 1.0, 0.0)]);
        assert_eq!(got, [(0, LEFT)]);
    }

    #[test]
    fn a_car_that_stays_is_told_again() {
        let got = run(&[(6500, 0.0, 1.0)]);
        assert_eq!(got, [(0, RIGHT), (3000, STILL_RIGHT), (6000, STILL_RIGHT)]);
        assert_eq!(
            run(&[(3500, 2.0, 0.0)]),
            [(0, TWO_LEFT), (3000, STILL_LEFT)]
        );
        assert_eq!(run(&[(3500, 1.0, 1.0)]), [(0, BOTH), (3000, STILL_BOTH)]);
    }

    #[test]
    fn several_cars_are_told_by_side() {
        let got = run(&[
            (1000, 1.0, 0.0),
            (2000, 1.0, 1.0),
            (3000, 0.0, 1.0),
            (4000, 0.0, 2.0),
            (5000, 0.0, 0.0),
        ]);
        assert_eq!(
            got,
            [
                (0, LEFT),
                (1000, BOTH),
                (2250, CLEAR_LEFT),
                (3000, TWO_RIGHT),
                (4250, CLEAR_ALL)
            ]
        );
        let got = run(&[(1000, 1.0, 1.0), (2000, 1.0, 0.0)]);
        assert_eq!(got, [(0, BOTH), (1250, CLEAR_RIGHT)]);
        // From one side to the other.
        let got = run(&[(1000, 1.0, 0.0), (2000, 0.0, 1.0)]);
        assert_eq!(got, [(0, LEFT), (1000, RIGHT)]);
        let got = run(&[(1000, 0.0, 1.0), (2000, 1.0, 0.0)]);
        assert_eq!(got, [(0, RIGHT), (1000, LEFT)]);
        for p in [LEFT, BOTH, CLEAR_ALL, ALSO, STILL_BOTH, COMES] {
            assert!(PHRASES.contains(&p));
        }
    }

    #[test]
    fn nothing_is_said_on_the_grid_or_in_the_pits() {
        let t = Instant::now();
        for (key, x) in [("speed", 39.9), ("on_track", 0.0), ("on_pit", 1.0)] {
            let mut s = Spotter::default();
            let mut v = at(1.0, 0.0);
            v.insert(key.into(), x);
            assert_eq!(s.check(&v, t), None, "{key}");
        }
        let mut v = at(1.0, 0.0);
        v.insert("speed".into(), 40.0);
        assert_eq!(Spotter::default().check(&v, t), Some(LEFT));
        // Into the pits with a car beside: it is not told to leave.
        let mut s = Spotter::default();
        assert_eq!(s.check(&at(1.0, 0.0), t), Some(LEFT));
        let mut pit = at(1.0, 0.0);
        pit.insert("on_pit".into(), 1.0);
        assert_eq!(s.check(&pit, t), None);
        assert_eq!(s.check(&at(0.0, 0.0), t + Duration::from_secs(1)), None);
        // A game that tells no sides.
        let mut v = at(1.0, 0.0);
        v.remove("right");
        assert_eq!(Spotter::default().check(&v, t), None);
    }

    /// With the distances along the track, in metres.
    fn near(beside: f64, ahead: f64, behind: f64) -> Values {
        let mut v = at(beside, 0.0);
        v.insert("near_ahead_m".into(), ahead);
        v.insert("near_behind_m".into(), behind);
        v
    }

    #[test]
    fn clear_is_told_sooner_when_nobody_is_near() {
        let t = Instant::now();
        let after = |v: Values| {
            let mut s = Spotter::default();
            assert_eq!(s.check(&near(1.0, 2.0, 50.0), t), Some(LEFT));
            (1..40u64)
                .map(|k| k * 10)
                .find(|ms| s.check(&v, t + Duration::from_millis(*ms)).is_some())
        };
        // Counted from the first value that said so, at 10 ms.
        assert_eq!(after(near(0.0, 6.1, 6.1)), Some(90));
        assert_eq!(after(near(0.0, 6.0, 50.0)), Some(260));
        assert_eq!(after(near(0.0, 50.0, 6.0)), Some(260));
        let mut one = near(0.0, 50.0, 6.1);
        one.remove("near_ahead_m");
        assert_eq!(after(one), Some(90));
        assert_eq!(after(at(0.0, 0.0)), Some(260));
    }

    /// The car behind comes nearer by so much a second, from so far.
    fn approach(from: f64, mps: f64) -> Vec<(f64, &'static str)> {
        let mut s = Spotter::default();
        let t = Instant::now();
        let mut out = Vec::new();
        for k in 0..600u64 {
            let ms = k * 16;
            let m = from - mps * ms as f64 / 1000.0;
            if m < CAR_M {
                break;
            }
            let at = t + Duration::from_millis(ms);
            if let Some(text) = s.check(&near(0.0, 99.0, m), at) {
                out.push((m, text));
            }
        }
        out
    }

    #[test]
    fn a_car_from_behind_is_told_before_it_is_beside() {
        // At 10 m/s: one second before it is there, 16 m away.
        let got = approach(40.0, 10.0);
        assert_eq!(got.len(), 1);
        assert!((15.8..=16.0).contains(&got[0].0), "{got:?}");
        assert_eq!(got[0].1, COMES);
        // Very fast: as soon as it is near.
        let got = approach(60.0, 30.0);
        assert!((24.0..=25.0).contains(&got[0].0), "{got:?}");
        // Following is not coming.
        assert_eq!(approach(20.0, 2.9), []);
        assert_eq!(approach(20.0, 0.0), []);
        assert_eq!(approach(20.0, 3.1).len(), 1);
    }

    #[test]
    fn a_car_that_comes_is_told_once() {
        let mut s = Spotter::default();
        let t = Instant::now();
        let mut ms = 0;
        let mut go = |s: &mut Spotter, from: f64, to: f64| {
            let mut said = Vec::new();
            for k in 0..=10 {
                let m = from + (to - from) * f64::from(k) / 10.0;
                let at = t + Duration::from_millis(ms);
                said.extend(s.check(&near(0.0, 99.0, m), at));
                ms += 50;
            }
            said
        };
        assert_eq!(go(&mut s, 20.0, 12.0), [COMES]);
        // Back a little and near again: known.
        assert_eq!(go(&mut s, 12.0, 34.9), [] as [&str; 0]);
        assert_eq!(go(&mut s, 34.9, 12.0), [] as [&str; 0]);
        // It was gone.
        assert_eq!(go(&mut s, 12.0, 35.1), [] as [&str; 0]);
        assert_eq!(go(&mut s, 35.1, 12.0), [COMES]);
        // Another car: no pace to be told from.
        let mut s = Spotter::default();
        assert_eq!(s.check(&near(0.0, 99.0, 90.0), t), None);
        let at = t + Duration::from_millis(200);
        assert_eq!(s.check(&near(0.0, 99.0, 20.0), at), None);
        // Beside: not any more a matter of coming.
        let mut s = Spotter::default();
        assert_eq!(s.check(&near(1.0, 99.0, 8.0), t), Some(LEFT));
        assert_eq!(s.check(&near(1.0, 99.0, 5.0), at), None);
    }

    #[test]
    fn old_values_are_forgotten() {
        let t = Instant::now();
        let mut s = Spotter::default();
        assert_eq!(s.check(&at(1.0, 0.0), t), Some(LEFT));
        let gap = |ms| t + Duration::from_millis(ms);
        // After a hole in the data nobody knows whether it ever left.
        assert_eq!(s.check(&at(0.0, 0.0), gap(1501)), None);
        assert_eq!(s.check(&at(0.0, 0.0), gap(2500)), None);
        let mut s = Spotter::default();
        assert_eq!(s.check(&at(1.0, 0.0), t), Some(LEFT));
        assert_eq!(s.check(&at(0.0, 0.0), gap(1500)), None);
        assert_eq!(s.check(&at(0.0, 0.0), gap(1750)), Some(CLEAR_ALL));
        s.check(&at(1.0, 0.0), gap(2000));
        s.reset();
        assert_eq!(s.check(&at(0.0, 0.0), gap(2100)), None);
    }
}
