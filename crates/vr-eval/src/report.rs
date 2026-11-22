//! Summaries and comparisons of graded runs.

use std::collections::BTreeMap;
use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::judge::{Grade, Verdict, consensus};
use crate::run::Record;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graded {
    #[serde(flatten)]
    pub record: Record,
    pub verdicts: Vec<Verdict>,
}

impl Graded {
    /// `None`: the judges differ, or the number check contradicts them.
    pub fn grade(&self) -> Option<Grade> {
        let g = consensus(&self.verdicts)?;
        let stated = self.record.facts_stated.iter().all(|s| *s);
        let checked = !self.record.facts_stated.is_empty();
        // A "good" answer that lacks a required number needs a human.
        (g != Grade::Good || !checked || stated || self.asked_back()).then_some(g)
    }

    /// A short question back contains none of the facts.
    fn asked_back(&self) -> bool {
        use crate::scenario::Behavior::AnswerOrAsk;
        self.record.behavior == AnswerOrAsk && !self.record.facts_stated.iter().any(|s| *s)
    }

    pub fn key(&self) -> (String, String) {
        (self.record.stint.clone(), self.record.call.clone())
    }
}

pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = p * (v.len() - 1) as f64;
    let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
    Some(v[lo] + (v[hi] - v[lo]) * rank.fract())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub calls: usize,
    pub good: usize,
    pub incomplete: usize,
    pub wrong: usize,
    pub disputed: usize,
    pub informed_p50: Option<f64>,
    pub informed_p90: Option<f64>,
    pub first_audio_p50: Option<f64>,
    pub transcript_p50: Option<f64>,
    /// Calls that should be answered and never were.
    pub unanswered: usize,
}

pub fn summarize(rows: &[&Graded]) -> Summary {
    let count = |g: Grade| rows.iter().filter(|r| r.grade() == Some(g)).count();
    let of = |f: &dyn Fn(&Record) -> Option<f64>| -> Vec<f64> {
        rows.iter().filter_map(|r| f(&r.record)).collect()
    };
    let answerable: Vec<&&Graded> = rows
        .iter()
        .filter(|r| r.record.behavior == crate::scenario::Behavior::Answer)
        .collect();
    let informed: Vec<f64> = answerable
        .iter()
        .filter_map(|r| r.record.informed_s)
        .collect();
    let first_audio: Vec<f64> = answerable
        .iter()
        .filter_map(|r| r.record.first_audio_s())
        .collect();
    Summary {
        calls: rows.len(),
        good: count(Grade::Good),
        incomplete: count(Grade::Incomplete),
        wrong: count(Grade::Wrong),
        disputed: rows.iter().filter(|r| r.grade().is_none()).count(),
        informed_p50: percentile(&informed, 0.5),
        informed_p90: percentile(&informed, 0.9),
        first_audio_p50: percentile(&first_audio, 0.5),
        transcript_p50: percentile(&of(&|r| r.transcript_s), 0.5),
        unanswered: answerable
            .iter()
            .filter(|r| r.record.heard.is_empty())
            .count(),
    }
}

fn secs(v: Option<f64>) -> String {
    v.map_or("--".into(), |v| format!("{v:.1}s"))
}

pub fn table(runs: &BTreeMap<String, Vec<Graded>>) -> String {
    let mut out = String::from(
        "| config | calls | good | incomplete | wrong | disputed | no answer | informed p50 | p90 | first audio p50 | heard text p50 |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for (name, rows) in runs {
        let s = summarize(&rows.iter().collect::<Vec<_>>());
        let _ = writeln!(
            out,
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.calls,
            s.good,
            s.incomplete,
            s.wrong,
            s.disputed,
            s.unanswered,
            secs(s.informed_p50),
            secs(s.informed_p90),
            secs(s.first_audio_p50),
            secs(s.transcript_p50),
        );
    }
    out
}

/// Difference `b - a` of a per-call value over the calls both answered, with
/// a 95 % interval from resampling whole stints (calls of one stint are not
/// independent).
pub fn paired(
    a: &[Graded],
    b: &[Graded],
    value: impl Fn(&Graded) -> Option<f64>,
) -> Option<(f64, f64, f64, usize)> {
    let mean = |rows: &[Graded]| -> BTreeMap<(String, String), f64> {
        let mut by: BTreeMap<_, Vec<f64>> = BTreeMap::new();
        for r in rows {
            if let Some(v) = value(r) {
                by.entry(r.key()).or_default().push(v);
            }
        }
        by.into_iter()
            .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
            .collect()
    };
    let (ma, mb) = (mean(a), mean(b));
    let mut stints: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (k, va) in &ma {
        if let Some(vb) = mb.get(k) {
            stints.entry(k.0.clone()).or_default().push(vb - va);
        }
    }
    let groups: Vec<Vec<f64>> = stints.into_values().collect();
    let all: Vec<f64> = groups.iter().flatten().copied().collect();
    if all.is_empty() {
        return None;
    }
    let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    let mut draws: Vec<f64> = (0..2000)
        .map(|_| {
            let picked: Vec<f64> = (0..groups.len())
                .flat_map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    groups[(rng % groups.len() as u64) as usize].clone()
                })
                .collect();
            avg(&picked)
        })
        .collect();
    draws.sort_by(f64::total_cmp);
    Some((
        avg(&all),
        percentile(&draws, 0.025)?,
        percentile(&draws, 0.975)?,
        all.len(),
    ))
}

pub fn compare(a: &[Graded], b: &[Graded]) -> String {
    let mut out = String::new();
    let line = |what: &str, d: Option<(f64, f64, f64, usize)>, unit: &str| match d {
        Some((m, lo, hi, n)) => {
            format!("{what}: {m:+.2}{unit} [95% {lo:+.2}, {hi:+.2}] over {n} calls\n")
        }
        None => format!("{what}: no calls in common\n"),
    };
    let is = |g: Grade| move |r: &Graded| Some(f64::from(u8::from(r.grade() == Some(g))));
    out += &line("informed", paired(a, b, |r| r.record.informed_s), "s");
    out += &line(
        "first audio",
        paired(a, b, |r| r.record.first_audio_s()),
        "s",
    );
    out += &line("good", paired(a, b, is(Grade::Good)), "");
    out += &line("wrong", paired(a, b, is(Grade::Wrong)), "");
    out
}

/// One line per call that is not plainly good.
pub fn problems(rows: &[Graded]) -> String {
    let mut out = String::new();
    for r in rows.iter().filter(|r| r.grade() != Some(Grade::Good)) {
        let x = &r.record;
        let grades: Vec<String> = r
            .verdicts
            .iter()
            .map(|v| format!("{:?}", v.grade))
            .collect();
        let _ = writeln!(
            out,
            "{}/{} #{} {:?} facts={:?}\n  said:  {}\n  heard: {:?}\n  reply: {}\n  tools: {:?} errors: {:?}",
            x.stint,
            x.call,
            x.trial,
            grades,
            x.facts_stated,
            x.said,
            x.transcript,
            x.spoken(),
            x.tools,
            x.errors,
        );
        for v in &r.verdicts {
            let reason: String = v.reason.chars().take(220).collect();
            let _ = writeln!(out, "  {}: {reason}", v.judge);
        }
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::run::Heard;
    use crate::scenario::Behavior;

    pub fn graded(stint: &str, call: &str, informed: Option<f64>, grades: &[Grade]) -> Graded {
        Graded {
            record: Record {
                config: "c".into(),
                trial: 0,
                stint: stint.into(),
                call: call.into(),
                first: false,
                said: "q".into(),
                voice: "vv:1".into(),
                snr_db: None,
                behavior: Behavior::Answer,
                facts: vec![],
                points: vec!["p".into()],
                speech_s: 1.0,
                history: vec![],
                transcript: Some("q".into()),
                transcript_s: Some(0.5),
                first_token_s: None,
                tools: vec![],
                heard: informed
                    .map(|i| Heard {
                        text: "r".into(),
                        start_s: i - 1.0,
                        end_s: i,
                    })
                    .into_iter()
                    .collect(),
                reply: "r".into(),
                outcome: String::new(),
                errors: vec![],
                facts_stated: vec![],
                informed_s: informed,
                truth_at_question: String::new(),
                truth_at_answer: String::new(),
                truth_between: vec![],
            },
            verdicts: grades
                .iter()
                .map(|g| Verdict {
                    judge: "j".into(),
                    grade: *g,
                    reason: String::new(),
                })
                .collect(),
        }
    }

    const GOOD: &[Grade] = &[Grade::Good, Grade::Good];

    #[test]
    fn percentiles() {
        let v = [4.0, 1.0, 2.0, 3.0];
        assert_eq!(percentile(&v, 0.5), Some(2.5));
        assert_eq!(percentile(&v, 0.0), Some(1.0));
        assert_eq!(percentile(&v, 1.0), Some(4.0));
        assert_eq!(percentile(&[], 0.5), None);
    }

    #[test]
    fn disagreement_is_not_hidden() {
        let mut r = graded("s", "a", Some(5.0), GOOD);
        assert_eq!(r.grade(), Some(Grade::Good));
        r.record.facts_stated = vec![true, false];
        assert_eq!(r.grade(), None);
        r.record.facts_stated = vec![false];
        r.record.behavior = Behavior::AnswerOrAsk;
        assert_eq!(r.grade(), Some(Grade::Good));
        let split = graded("s", "a", Some(5.0), &[Grade::Good, Grade::Wrong]);
        assert_eq!(split.grade(), None);
    }

    #[test]
    fn summary_counts() {
        let rows = [
            graded("s", "a", Some(4.0), GOOD),
            graded("s", "b", Some(8.0), &[Grade::Wrong, Grade::Wrong]),
            graded("s", "c", None, &[Grade::Incomplete, Grade::Incomplete]),
            graded("s", "d", Some(6.0), &[Grade::Good, Grade::Wrong]),
        ];
        let s = summarize(&rows.iter().collect::<Vec<_>>());
        assert_eq!(
            (s.good, s.wrong, s.incomplete, s.disputed, s.unanswered),
            (1, 1, 1, 1, 1)
        );
        assert_eq!(s.informed_p50, Some(6.0));
        assert_eq!(s.first_audio_p50, Some(5.0));
        assert_eq!(s.transcript_p50, Some(0.5));
        let runs = BTreeMap::from([("x".to_string(), rows.to_vec())]);
        assert!(table(&runs).contains("| x | 4 | 1 | 1 | 1 | 1 | 1 | 6.0s |"));
        let p = problems(&rows);
        assert!(p.contains("s/b") && !p.contains("s/a "), "{p}");
    }

    #[test]
    fn paired_difference_uses_the_same_calls() {
        let a = [
            graded("s1", "a", Some(10.0), GOOD),
            graded("s1", "b", Some(8.0), GOOD),
            graded("s2", "c", Some(6.0), GOOD),
            graded("s2", "only-a", Some(99.0), GOOD),
        ];
        let b = [
            graded("s1", "a", Some(7.0), GOOD),
            graded("s1", "b", Some(5.0), GOOD),
            // Two trials are averaged.
            graded("s2", "c", Some(2.0), GOOD),
            graded("s2", "c", Some(4.0), GOOD),
        ];
        let (m, lo, hi, n) = paired(&a, &b, |r| r.record.informed_s).unwrap();
        assert_eq!((m, lo, hi, n), (-3.0, -3.0, -3.0, 3));
        assert!(paired(&a, &[], |r| r.record.informed_s).is_none());
        assert!(compare(&a, &b).contains("informed: -3.00s [95% -3.00, -3.00] over 3 calls"));
    }
}
