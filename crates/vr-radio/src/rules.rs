//! What the driver asked of the radio, in their own words: when to be quiet,
//! what to look out for, what to keep in mind, how often to call. The agent
//! turns the words into these rules; checking them costs nothing.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::expr::{Expr, Values};

/// More than anybody keeps track of.
pub const MAX_NOTES: usize = 20;
pub const MAX_WATCHES: usize = 20;
/// Between two calls of a watch that keeps coming true, unless told otherwise.
pub const EVERY_S: u32 = 30;

/// Which speech keeps quiet while a condition holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Answers too.
    All,
    /// What the engineer says by itself.
    Calls,
    /// The same, but dropped instead of held back.
    Quiet,
}

/// Something to say, or to do, when a condition comes true.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watch {
    pub when: String,
    /// What the engineer is to do about it.
    #[serde(rename = "do", skip_serializing_if = "Option::is_none")]
    pub act: Option<String>,
    /// Said as it is, without the engineer thinking about it: `{gap_behind}`
    /// is replaced by the value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub say: Option<String>,
    /// A topic of `vr say`, said without the engineer thinking about it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tell: Option<String>,
    /// Seconds.
    pub every: u32,
    /// Gone after the first time.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub once: bool,
    /// Set by the engineer's own planning: replaced by the next plan.
    #[serde(skip)]
    pub planned: bool,
}

impl std::fmt::Display for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = self.say.as_ref().or(self.tell.as_ref());
        let what = what.or(self.act.as_ref());
        write!(
            f,
            "{} → {}",
            self.when,
            what.map_or("知らせる", String::as_str)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    /// Seconds between the engineer's own calls that can wait.
    pub interval_s: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<String>,
    pub hold_calls: Option<String>,
    /// No calls that can wait while this holds; of this race only.
    #[serde(skip)]
    pub quiet: Option<String>,
    /// Speech is held back no longer than this.
    pub hold_max_s: u16,
    /// Standing orders, as text for the agent.
    pub notes: Vec<String>,
    pub watches: Vec<Watch>,
    /// Laps between the engineer's own looks at the strategy; 0: never.
    pub planning: u16,
    /// The usual questions are answered without the agent.
    pub quick: bool,
}

/// Quiet on the brakes, like a real engineer.
pub const HOLD_CALLS: &str = "brake > 10";
pub const INTERVAL_S: u16 = 8;
pub const HOLD_MAX_S: u16 = 8;
pub const PLANNING: u16 = 5;

impl Default for Rules {
    fn default() -> Self {
        Self {
            interval_s: INTERVAL_S,
            hold: None,
            hold_calls: Some(HOLD_CALLS.into()),
            quiet: None,
            hold_max_s: HOLD_MAX_S,
            notes: Vec::new(),
            watches: Vec::new(),
            planning: PLANNING,
            quick: true,
        }
    }
}

impl Rules {
    /// Without what the engineer set for this race.
    pub fn lasting(&self) -> Self {
        let mut r = self.clone();
        r.watches.retain(|w| !w.planned);
        r.quiet = None;
        r
    }

    pub fn hold(&self, scope: Scope) -> Option<&str> {
        match scope {
            Scope::All => self.hold.as_deref(),
            Scope::Calls => self.hold_calls.as_deref(),
            Scope::Quiet => self.quiet.as_deref(),
        }
    }

    pub fn set_hold(&mut self, scope: Scope, when: Option<String>) {
        match scope {
            Scope::All => self.hold = when,
            Scope::Calls => self.hold_calls = when,
            Scope::Quiet => self.quiet = when,
        }
    }

    /// The same order is not kept twice; the oldest go first.
    pub fn note(&mut self, text: &str) {
        let text = text.trim();
        self.notes.retain(|n| n != text);
        self.notes.push(text.into());
        let extra = self.notes.len().saturating_sub(MAX_NOTES);
        self.notes.drain(..extra);
    }

    pub fn watch(&mut self, w: Watch) {
        self.watches.retain(|x| x.when != w.when);
        self.watches.push(w);
        while self.watches.len() > MAX_WATCHES {
            // What the driver asked for outlives what the engineer set.
            let i = self.watches.iter().position(|w| w.planned).unwrap_or(0);
            self.watches.remove(i);
        }
    }

    /// Drops orders and watches: all, `n1`, `w2`, or what contains the words.
    /// Returns how many went.
    pub fn forget(&mut self, what: &str) -> usize {
        let what = what.trim();
        let before = self.notes.len() + self.watches.len();
        let number = |letter: char| {
            what.strip_prefix(letter)
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|n| n.checked_sub(1))
        };
        if what.is_empty() {
            return 0;
        } else if what == "all" || what == "全部" {
            self.notes.clear();
            self.watches.clear();
        } else if let Some(i) = number('n') {
            if i < self.notes.len() {
                self.notes.remove(i);
            }
        } else if let Some(i) = number('w') {
            if i < self.watches.len() {
                self.watches.remove(i);
            }
        } else {
            self.notes.retain(|n| !n.contains(what));
            self.watches.retain(|w| {
                let texts = [
                    Some(&w.when),
                    w.act.as_ref(),
                    w.say.as_ref(),
                    w.tell.as_ref(),
                ];
                !texts.into_iter().flatten().any(|t| t.contains(what))
            });
        }
        before - self.notes.len() - self.watches.len()
    }

    /// As the agent sees them.
    pub fn status(&self) -> String {
        let off = |s: Option<&str>| s.unwrap_or("off").to_string();
        let mut out = format!(
            "interval={} hold=\"{}\" hold_calls=\"{}\" quiet=\"{}\" hold_max={} quick={} planning={}",
            self.interval_s,
            off(self.hold.as_deref()),
            off(self.hold_calls.as_deref()),
            off(self.quiet.as_deref()),
            self.hold_max_s,
            if self.quick { "on" } else { "off" },
            if self.planning == 0 {
                "off".into()
            } else {
                self.planning.to_string()
            },
        );
        for (i, n) in self.notes.iter().enumerate() {
            out.push_str(&format!("\n指示 n{}: {n}", i + 1));
        }
        for (i, w) in self.watches.iter().enumerate() {
            out.push_str(&format!("\n見張り w{}: {w}", i + 1));
        }
        out
    }
}

/// `{gap_behind}` in a text to say becomes the value.
pub fn fill(text: &str, v: &Values) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('}') else { break };
        match v.get(&rest[1..end]) {
            Some(n) => {
                let text = format!("{n:.1}");
                out.push_str(text.strip_suffix(".0").unwrap_or(&text));
            }
            None => out.push_str("不明"),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A watch while the race is on.
#[derive(Debug)]
struct Armed {
    watch: Watch,
    /// `None`: it cannot be read.
    when: Option<Expr>,
    held: bool,
    last: Option<Instant>,
}

/// Looks out for what comes true.
#[derive(Debug, Default)]
pub struct Lookout {
    armed: Vec<Armed>,
    /// The first values tell how things are, not what came true.
    seen: bool,
}

impl Lookout {
    /// Follows the rules; what is known about a watch that stays is kept.
    pub fn set(&mut self, watches: &[Watch]) {
        let mut old = std::mem::take(&mut self.armed);
        self.armed = watches
            .iter()
            .map(|w| match old.iter().position(|a| a.watch == *w) {
                Some(i) => old.swap_remove(i),
                None => Armed {
                    watch: w.clone(),
                    when: w.when.parse().ok(),
                    held: false,
                    last: None,
                },
            })
            .collect();
    }

    /// What came true with these values.
    pub fn check(&mut self, v: &Values, now: Instant) -> Vec<Watch> {
        // A watch set while its condition already holds tells so at once, but
        // not those kept from before when the radio starts.
        let first = !std::mem::replace(&mut self.seen, true);
        let mut out = Vec::new();
        for a in &mut self.armed {
            let holds = a.when.as_ref().is_some_and(|e| e.holds(v));
            let held = std::mem::replace(&mut a.held, holds);
            let came = holds && !held;
            let rested = a.last.is_none_or(|t| {
                now.saturating_duration_since(t) >= Duration::from_secs(a.watch.every.into())
            });
            if came && rested && !first {
                a.last = Some(now);
                out.push(a.watch.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(when: &str, every: u32) -> Watch {
        Watch {
            when: when.into(),
            every,
            ..Default::default()
        }
    }

    fn values(k: &str, v: f64) -> Values {
        [(k.to_string(), v)].into()
    }

    #[test]
    fn orders_are_kept_once_and_forgotten() {
        let mut r = Rules::default();
        r.note(" 順位は毎回言う ");
        r.note("タイヤの話はしない");
        r.note("順位は毎回言う");
        assert_eq!(r.notes, ["タイヤの話はしない", "順位は毎回言う"]);
        r.watch(watch("gap_behind < 1", 30));
        r.watch(Watch {
            say: Some("燃料2周".into()),
            ..watch("fuel_laps < 2", 60)
        });
        r.watch(watch("gap_behind < 1", 10));
        assert_eq!(r.watches.len(), 2);
        assert_eq!(r.watches[1].every, 10);
        assert_eq!(r.forget("タイヤ"), 1);
        assert_eq!(r.forget("燃料"), 1);
        assert_eq!(r.forget("ない"), 0);
        assert_eq!(r.forget(""), 0);
        assert_eq!(r.forget("w2"), 0);
        assert_eq!(r.forget("w0"), 0);
        assert_eq!(r.forget("w1"), 1);
        assert_eq!(r.forget("n1"), 1);
        r.note("a");
        r.watch(watch("lap > 1", 1));
        assert_eq!(r.forget("全部"), 2);
        assert_eq!(r, Rules::default());
    }

    #[test]
    fn there_is_an_end_to_what_is_kept() {
        let mut r = Rules::default();
        for i in 0..MAX_NOTES + 3 {
            r.note(&format!("{i}"));
        }
        assert_eq!((r.notes.len(), r.notes[0].as_str()), (MAX_NOTES, "3"));
        for i in 0..MAX_WATCHES {
            let mut w = watch(&format!("lap > {i}"), 1);
            w.planned = i == 5;
            r.watch(w);
        }
        r.watch(watch("lap > 99", 1));
        assert_eq!(r.watches.len(), MAX_WATCHES);
        assert!(r.watches.iter().all(|w| !w.planned));
        r.watch(watch("lap > 100", 1));
        assert_eq!(r.watches[0].when, "lap > 1");
    }

    #[test]
    fn the_agent_sees_them_with_their_numbers() {
        let mut r = Rules::default();
        assert_eq!(
            r.status(),
            "interval=8 hold=\"off\" hold_calls=\"brake > 10\" quiet=\"off\" hold_max=8 quick=on planning=5"
        );
        r.set_hold(Scope::All, Some("abs(steer) > 45".into()));
        r.set_hold(Scope::Calls, None);
        (r.interval_s, r.planning) = (60, 0);
        r.note("差は毎周");
        r.watch(watch("lap % 5 == 0", 1));
        assert_eq!(
            r.status(),
            "interval=60 hold=\"abs(steer) > 45\" hold_calls=\"off\" quiet=\"off\" hold_max=8 quick=on planning=off\n\
             指示 n1: 差は毎周\n見張り w1: lap % 5 == 0 → 知らせる"
        );
        assert_eq!(r.hold(Scope::All), Some("abs(steer) > 45"));
        assert_eq!(r.hold(Scope::Calls), None);
    }

    #[test]
    fn rules_survive_the_file() {
        let mut r = Rules::default();
        r.note("a");
        r.watch(Watch {
            act: Some("差を言う".into()),
            once: true,
            planned: true,
            ..watch("lap > 1", 5)
        });
        let text = serde_json::to_string(&r).unwrap();
        assert!(text.contains(r#""do":"差を言う""#), "{text}");
        let back: Rules = serde_json::from_str(&text).unwrap();
        assert!(!back.watches[0].planned);
        r.watches[0].planned = false;
        assert_eq!(back, r);
        // Switched off stays off.
        r.set_hold(Scope::Calls, None);
        let back: Rules = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.hold_calls, None);
        assert_eq!(
            serde_json::from_str::<Rules>("{}").unwrap(),
            Rules::default()
        );
    }

    #[test]
    fn values_are_filled_in() {
        let v: Values = [("gap_behind".into(), 0.84), ("lap".into(), 5.0)].into();
        assert_eq!(fill("後ろ{gap_behind}秒", &v), "後ろ0.8秒");
        assert_eq!(fill("{lap}周目、{x}", &v), "5周目、不明");
        assert_eq!(fill("a{lap", &v), "a{lap");
        assert_eq!(fill("そのまま", &v), "そのまま");
    }

    #[test]
    fn a_watch_tells_when_its_condition_comes_true() {
        let mut l = Lookout::default();
        l.set(&[watch("gap < 1", 30)]);
        let t = Instant::now();
        let at = |s: u64| t + Duration::from_secs(s);
        let mut check = |gap: f64, s: u64| l.check(&values("gap", gap), at(s)).len();
        assert_eq!(check(2.0, 0), 0);
        assert_eq!(check(0.9, 1), 1);
        assert_eq!(check(0.8, 2), 0);
        assert_eq!(check(1.5, 3), 0);
        // Again, but too soon.
        assert_eq!(check(0.9, 30), 0);
        assert_eq!(check(1.5, 31), 0);
        assert_eq!(check(0.9, 32), 1);
    }

    #[test]
    fn what_holds_at_the_start_is_no_news_but_a_new_watch_is() {
        let mut l = Lookout::default();
        l.set(&[watch("lap > 1", 1)]);
        let t = Instant::now();
        assert!(l.check(&values("lap", 5.0), t).is_empty());
        l.set(&[watch("lap > 1", 1), watch("lap > 2", 1)]);
        let told = l.check(&values("lap", 5.0), t);
        assert_eq!(told, [watch("lap > 2", 1)]);
        assert!(l.check(&values("lap", 5.0), t).is_empty());
        // Cannot be read: never.
        l.set(&[watch("lap >", 1)]);
        assert!(l.check(&values("lap", 5.0), t).is_empty());
    }
}
