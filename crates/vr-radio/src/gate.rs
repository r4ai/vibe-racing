//! Holds speech back while the driver is busy: on the brakes, in a corner,
//! whatever they asked for. The lap before tells where the next quiet stretch
//! is, so that a sentence does not run into a corner.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::expr::{Expr, Values};
use crate::rules::{Rules, Scope};

/// Older values say nothing about now.
const STALE: Duration = Duration::from_secs(2);
/// What cannot wait waits no longer than this.
const URGENT: Duration = Duration::from_millis(1500);
/// Shorter pauses between two holds are no chance to speak.
const CHECK: Duration = Duration::from_millis(40);
/// A lap with fewer samples was not driven through.
const MIN_SAMPLES: usize = 100;

/// A moment of a lap: how far around, when, and whether speech was held.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    pct: f64,
    t: f64,
    held: bool,
}

/// One condition and what the last lap was like under it.
#[derive(Debug, Default)]
struct Hold {
    source: Option<String>,
    when: Option<Expr>,
    lap: Vec<Sample>,
    last: Vec<Sample>,
}

impl Hold {
    fn set(&mut self, source: Option<&str>) {
        if self.source.as_deref() == source {
            return;
        }
        *self = Self {
            source: source.map(str::to_string),
            when: source.and_then(|s| s.parse().ok()),
            ..Self::default()
        };
    }

    fn held(&self, v: &Values) -> bool {
        self.when.as_ref().is_some_and(|e| e.holds(v))
    }

    fn learn(&mut self, v: &Values, t: f64) {
        let Some(pct) = v.get("lap_pct").copied() else {
            return;
        };
        if let Some(prev) = self.lap.last() {
            if prev.pct - pct > 50.0 {
                let lap = std::mem::take(&mut self.lap);
                let whole = lap.first().is_some_and(|s| s.pct < 5.0);
                if whole && lap.len() >= MIN_SAMPLES {
                    self.last = lap;
                }
            } else if pct < prev.pct - 1.0 || pct > prev.pct + 5.0 {
                // Towed, or reset.
                self.lap.clear();
            }
        }
        self.lap.push(Sample {
            pct,
            t,
            held: self.held(v),
        });
    }

    /// Seconds until a sentence of `len` seconds fits, 0 for now. When it
    /// fits nowhere within `most` seconds, until the longest pause.
    fn wait(&self, pct: f64, len: f64, most: f64) -> f64 {
        let n = self.last.len();
        if n < 2 {
            return 0.0;
        }
        let lap_s = self.last[n - 1].t - self.last[0].t;
        let from = self.last.iter().position(|s| s.pct >= pct).unwrap_or(0);
        let start = self.last[from].t;
        // Pauses as (seconds from now, length).
        let mut pauses: Vec<(f64, f64)> = Vec::new();
        let mut open: Option<f64> = None;
        for k in 0..n {
            let i = from + k;
            let s = self.last[i % n];
            let at = s.t - start + if i >= n { lap_s } else { 0.0 };
            match (s.held, open) {
                (false, None) if at > most => break,
                (false, None) => open = Some(at),
                (true, Some(o)) => {
                    pauses.push((o, at - o));
                    open = None;
                }
                _ => {}
            }
        }
        if let Some(o) = open {
            // No end in sight.
            pauses.push((o, f64::INFINITY));
        }
        let fits = pauses.iter().find(|(_, l)| *l >= len);
        let longest = || pauses.iter().max_by(|a, b| a.1.total_cmp(&b.1));
        fits.or_else(longest).map_or(0.0, |p| p.0)
    }
}

#[derive(Debug)]
struct Inner {
    all: Hold,
    calls: Hold,
    max: Duration,
    values: Option<(Instant, Values)>,
    began: Instant,
}

#[derive(Debug)]
pub struct Gate(Mutex<Inner>);

impl Default for Gate {
    fn default() -> Self {
        Self(Mutex::new(Inner {
            all: Hold::default(),
            calls: Hold::default(),
            max: Duration::ZERO,
            values: None,
            began: Instant::now(),
        }))
    }
}

/// A sentence that wants to be spoken.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wish {
    pub scope: Scope,
    /// How long it takes.
    pub len: Duration,
    pub urgent: bool,
    /// Since when it waits.
    pub since: Instant,
}

impl Gate {
    pub fn set(&self, rules: &Rules) {
        let mut g = self.0.lock().unwrap();
        g.all.set(rules.hold(Scope::All));
        g.calls.set(rules.hold(Scope::Calls));
        g.max = Duration::from_secs(rules.hold_max_s.into());
    }

    /// Whether anything is ever held back.
    pub fn watches(&self, scope: Scope) -> bool {
        let g = self.0.lock().unwrap();
        g.values.is_some()
            && (g.all.when.is_some() || (scope == Scope::Calls && g.calls.when.is_some()))
    }

    pub fn update(&self, v: Values, now: Instant) {
        let mut g = self.0.lock().unwrap();
        let t = now.saturating_duration_since(g.began).as_secs_f64();
        g.all.learn(&v, t);
        g.calls.learn(&v, t);
        g.values = Some((now, v));
    }

    /// Whether the sentence may begin now.
    pub fn open(&self, w: Wish, now: Instant) -> bool {
        let g = self.0.lock().unwrap();
        let Some((at, v)) = &g.values else {
            return true;
        };
        let waited = now.saturating_duration_since(w.since);
        let max = if w.urgent { g.max.min(URGENT) } else { g.max };
        if now.saturating_duration_since(*at) > STALE || waited >= max {
            return true;
        }
        let holds: &[&Hold] = match w.scope {
            Scope::All => &[&g.all],
            Scope::Calls | Scope::Quiet => &[&g.all, &g.calls],
        };
        if holds.iter().any(|h| h.held(v)) {
            return false;
        }
        if w.urgent {
            return true;
        }
        let Some(pct) = v.get("lap_pct").copied() else {
            return true;
        };
        let left = (max - waited).as_secs_f64();
        holds
            .iter()
            .filter(|h| h.when.is_some())
            .all(|h| h.wait(pct, w.len.as_secs_f64(), left) <= 0.0)
    }

    /// Returns once the sentence may begin, or `still` is no longer true.
    pub async fn wait(&self, w: Wish, still: impl Fn() -> bool) {
        while still() && !self.open(w, Instant::now()) {
            tokio::time::sleep(CHECK).await;
        }
    }
}

/// How long a WAV clip plays.
pub fn length(wav: &[u8]) -> Duration {
    let u32_at = |i: usize| {
        wav.get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let rate = u32_at(28).filter(|r| *r > 0 && wav.starts_with(b"RIFF"));
    rate.map_or(Duration::ZERO, |r| {
        Duration::from_secs_f64(wav.len().saturating_sub(44) as f64 / f64::from(r))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pct: f64, brake: f64) -> Values {
        [("lap_pct".into(), pct), ("brake".into(), brake)].into()
    }

    fn gate() -> Gate {
        let g = Gate::default();
        g.set(&Rules::default());
        g
    }

    fn wish(scope: Scope, len: f64, since: Instant) -> Wish {
        Wish {
            scope,
            len: Duration::from_secs_f64(len),
            urgent: false,
            since,
        }
    }

    /// A 100 s lap: on the brakes from 20 to 24 and from 30 to 34 s, where
    /// seconds are percents too.
    fn drive(g: &Gate, t0: Instant, laps: usize) -> Instant {
        let mut t = t0;
        for i in 0..laps * 1000 {
            let pct = (i % 1000) as f64 / 10.0;
            let on = (20.0..24.0).contains(&pct) || (30.0..34.0).contains(&pct);
            t = t0 + Duration::from_millis(100 * i as u64);
            g.update(values(pct, if on { 80.0 } else { 0.0 }), t);
        }
        t
    }

    #[test]
    fn calls_wait_for_the_brakes_and_answers_do_not() {
        let g = gate();
        let t = Instant::now();
        assert!(g.open(wish(Scope::Calls, 1.0, t), t), "no values");
        g.update(values(10.0, 80.0), t);
        assert!(!g.open(wish(Scope::Calls, 1.0, t), t));
        assert!(g.open(wish(Scope::All, 1.0, t), t));
        g.update(values(10.1, 10.0), t);
        assert!(g.open(wish(Scope::Calls, 1.0, t), t));

        g.set(&Rules {
            hold: Some("brake > 5".into()),
            ..Rules::default()
        });
        assert!(!g.open(wish(Scope::All, 1.0, t), t));
        assert!(!g.open(wish(Scope::Calls, 1.0, t), t));
    }

    #[test]
    fn no_speech_is_held_for_ever() {
        let g = gate();
        let t = Instant::now();
        let at = |ms: u64| t + Duration::from_millis(ms);
        g.update(values(10.0, 80.0), at(7999));
        assert!(!g.open(wish(Scope::Calls, 1.0, t), at(7999)));
        assert!(g.open(wish(Scope::Calls, 1.0, t), at(8000)));
        let urgent = Wish {
            urgent: true,
            ..wish(Scope::Calls, 1.0, t)
        };
        g.update(values(10.0, 80.0), at(1400));
        assert!(!g.open(urgent, at(1499)));
        assert!(g.open(urgent, at(1500)));
        // The values stopped coming.
        assert!(!g.open(wish(Scope::Calls, 1.0, at(3000)), at(3400)));
        assert!(g.open(wish(Scope::Calls, 1.0, at(3000)), at(3401)));
    }

    #[test]
    fn a_sentence_does_not_run_into_the_next_corner() {
        let g = gate();
        let t = drive(&g, Instant::now(), 2);
        let may = |pct: f64, len: f64, waited: f64| {
            g.update(values(99.0, 0.0), t);
            let now = t + Duration::from_millis(500);
            g.update(values(pct, 0.0), now);
            let w = wish(Scope::Calls, len, now - Duration::from_secs_f64(waited));
            let urgent = Wish { urgent: true, ..w };
            assert!(g.open(urgent, now));
            g.open(w, now)
        };
        // The corner begins in 3 s: two fit, four do not.
        assert!(may(17.0, 2.0, 0.0));
        assert!(!may(17.0, 4.0, 0.0));
        // 5.5 s later there is room: worth waiting for, unless it has waited.
        assert!(may(19.5, 4.0, 5.0));
        // Between the corners 6 s; 7 s fit only behind both.
        assert!(may(24.0, 5.5, 0.0));
        assert!(!may(28.0, 7.0, 0.0));
        assert!(may(34.0, 7.0, 0.0));
        // Nothing is longer than this: the longest pause in reach.
        assert!(may(34.0, 200.0, 0.0));
    }

    #[test]
    fn only_whole_laps_teach() {
        let g = gate();
        let t = Instant::now();
        // Joined in the middle of a lap, then towed.
        for i in 500..1000 {
            g.update(values(f64::from(i) / 10.0, 80.0), t);
        }
        g.update(values(0.0, 0.0), t);
        assert!(g.0.lock().unwrap().calls.last.is_empty());
        g.update(values(30.0, 0.0), t);
        assert_eq!(g.0.lock().unwrap().calls.lap.len(), 1);
        let end = drive(&g, t, 2);
        assert!(g.0.lock().unwrap().calls.last.len() >= 999);
        // Another condition, another lap.
        g.set(&Rules {
            hold_calls: Some("brake > 50".into()),
            ..Rules::default()
        });
        assert!(g.0.lock().unwrap().calls.last.is_empty());
        let now = end + Duration::from_millis(1);
        assert!(g.open(wish(Scope::Calls, 60.0, now), now));
    }

    #[test]
    fn the_length_of_a_clip() {
        let wav = crate::wav::encode_pcm16(&vec![0.0; 24_000], 16_000);
        assert_eq!(length(&wav), Duration::from_millis(1500));
        assert_eq!(length(b"RIFF"), Duration::ZERO);
        assert_eq!(length(&[1; 100]), Duration::ZERO);
    }

    #[tokio::test]
    async fn waiting_ends_when_nobody_wants_it_any_more() {
        let g = gate();
        g.update(values(10.0, 80.0), Instant::now());
        g.wait(wish(Scope::Calls, 1.0, Instant::now()), || false)
            .await;
    }
}
