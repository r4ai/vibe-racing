//! The runs as one HTML page: self-contained except for web fonts, ready to
//! be published as it is.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::judge::Grade;
use crate::report::{Graded, paired, percentile, summarize, wilson};

const TEMPLATE: &str = include_str!("report.html");

/// How a run was made, kept next to its records.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    /// File stem of the scenario set.
    pub set: String,
    /// Shown instead of the name.
    pub label: String,
    pub note: String,
    /// Options of the radio.
    pub radio: Vec<String>,
    /// Typed questions: no voices, no transcription.
    #[serde(default)]
    pub text: bool,
}

/// Accuracy of a judge on the labelled replies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    pub model: String,
    pub cases: usize,
    pub right: usize,
    pub false_good: usize,
    pub false_bad: usize,
    pub used: bool,
}

pub struct Run {
    pub meta: Meta,
    pub rows: Vec<Graded>,
}

fn grade_name(g: Option<Grade>) -> &'static str {
    match g {
        Some(Grade::Good) => "good",
        Some(Grade::Incomplete) => "incomplete",
        Some(Grade::Wrong) => "wrong",
        None => "disputed",
    }
}

fn row(g: &Graded) -> Value {
    let r = &g.record;
    let verdicts: Vec<Value> = g
        .verdicts
        .iter()
        .map(|v| {
            json!({
                "judge": v.judge,
                "grade": grade_name(Some(v.grade)),
                "reason": v.reason.chars().take(400).collect::<String>(),
            })
        })
        .collect();
    json!({
        "stint": r.stint, "call": r.call, "trial": r.trial, "first": r.first,
        "said": r.said, "behavior": r.behavior, "snr_db": r.snr_db,
        "transcript": r.transcript, "spoken": r.spoken(), "tools": r.tools, "errors": r.errors,
        "speech_s": r.speech_s, "transcript_s": r.transcript_s, "first_token_s": r.first_token_s,
        "first_audio_s": r.first_audio_s(), "informed_s": r.informed_s,
        "grade": grade_name(g.grade()), "verdicts": verdicts,
        "tokens": r.tokens, "primer_tokens": r.primer_tokens,
    })
}

/// A value of a run with its 95 % interval, when it has one.
fn point(v: Option<f64>, ci: Option<(f64, f64)>, n: usize) -> Value {
    json!({"v": v, "lo": ci.map(|c| c.0), "hi": ci.map(|c| c.1), "n": n})
}

fn rate(k: usize, n: usize) -> Value {
    point((n > 0).then(|| k as f64 / n as f64), wilson(k, n), n)
}

fn mean(values: &[f64]) -> Value {
    let n = values.len();
    point(
        (n > 0).then(|| values.iter().sum::<f64>() / n as f64),
        None,
        n,
    )
}

/// What a call cost: the session total after it, less the one before it in
/// the same stint (the first call carries the primer).
fn call_costs(rows: &[Graded]) -> Vec<f64> {
    let mut last: Option<(usize, &str, f64)> = None;
    let mut out = Vec::new();
    for r in rows {
        let Some((total, _)) = &r.record.session_cost else {
            continue;
        };
        let before = match last {
            Some((t, s, c)) if t == r.record.trial && s == r.record.stint && !r.record.first => c,
            _ => 0.0,
        };
        out.push(total - before);
        last = Some((r.record.trial, &r.record.stint, *total));
    }
    out
}

/// The axes of the trade-off chart, per configuration.
pub fn metrics(rows: &[Graded]) -> Value {
    use crate::scenario::Behavior::Answer;
    let n = rows.len();
    let count = |g: Grade| rows.iter().filter(|r| r.grade() == Some(g)).count();
    let answer: Vec<&Graded> = rows
        .iter()
        .filter(|r| r.record.behavior == Answer)
        .collect();
    let checked: Vec<&&Graded> = answer
        .iter()
        .filter(|r| !r.record.facts_stated.is_empty())
        .collect();
    let stated = checked
        .iter()
        .filter(|r| r.record.facts_stated.iter().all(|s| *s))
        .count();
    let judged: Vec<bool> = rows.iter().filter_map(Graded::unanimous).collect();
    let time = |f: &dyn Fn(&Graded) -> Option<f64>, p: f64| {
        let v: Vec<f64> = answer.iter().filter_map(|r| f(r)).collect();
        point(percentile(&v, p), None, v.len())
    };
    let tokens = |f: &dyn Fn(&crate::run::Tokens) -> u64| {
        let v: Vec<f64> = rows
            .iter()
            .filter_map(|r| r.record.tokens.as_ref().map(|t| f(t) as f64))
            .collect();
        mean(&v)
    };
    let primer: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.record.primer_tokens.map(|t| t.total as f64))
        .collect();
    json!({
        "good": rate(count(Grade::Good), n),
        "wrong": rate(count(Grade::Wrong), n),
        "facts": rate(stated, checked.len()),
        "unanimous": rate(judged.iter().filter(|u| **u).count(), judged.len()),
        "informed_p50": time(&|r| r.record.informed_s, 0.5),
        "informed_p90": time(&|r| r.record.informed_s, 0.9),
        "first_audio_p50": time(&|r| r.record.first_audio_s(), 0.5),
        "first_token_p50": time(&|r| r.record.first_token_s, 0.5),
        "tokens": tokens(&|t| t.total),
        "fresh_tokens": tokens(&|t| t.input + t.cached_write + t.output),
        "output_tokens": tokens(&|t| t.output),
        "primer_tokens": mean(&primer),
        "cost": mean(&call_costs(rows)),
        "currency": rows.iter().find_map(|r| r.record.session_cost.as_ref().map(|c| c.1.clone())),
    })
}

/// `model` and `effort` of the agent, from the radio options.
fn agent_setup(radio: &[String]) -> Value {
    let mut out = serde_json::Map::new();
    let mut it = radio.iter();
    while let Some(a) = it.next() {
        let opt = match a.strip_prefix("--agent-option=") {
            Some(v) => Some(v.to_string()),
            None if a == "--agent-option" => it.next().cloned(),
            None => None,
        };
        for o in opt.iter().flat_map(|o| o.split(';')) {
            if let Some((k, v)) = o.split_once('=') {
                let k = if k == "reasoning_effort" { "effort" } else { k };
                out.insert(k.into(), v.into());
            }
        }
    }
    Value::Object(out)
}

fn median(rows: &[Graded], f: impl Fn(&Graded) -> Option<f64>) -> Option<f64> {
    let values: Vec<f64> = rows.iter().filter_map(f).collect();
    percentile(&values, 0.5)
}

fn run_json(run: &Run, base: Option<&Run>) -> Value {
    let s = summarize(&run.rows.iter().collect::<Vec<_>>());
    let versus = base.map(|b| {
        let d = |f: &dyn Fn(&Graded) -> Option<f64>| {
            paired(&b.rows, &run.rows, f)
                .map(|(mean, lo, hi, n)| json!({"mean": mean, "lo": lo, "hi": hi, "n": n}))
        };
        let is = |g: Grade| move |r: &Graded| Some(f64::from(u8::from(r.grade() == Some(g))));
        json!({
            "informed": d(&|r| r.record.informed_s),
            "first_audio": d(&|r| r.record.first_audio_s()),
            "good": d(&is(Grade::Good)),
            "wrong": d(&is(Grade::Wrong)),
        })
    });
    // Where the time before the first sound goes.
    let breakdown = json!({
        "stt": median(&run.rows, |r| r.record.transcript_s),
        "agent": median(&run.rows, |r| Some(r.record.first_token_s? - r.record.transcript_s?)),
        "voice": median(&run.rows, |r| Some(r.record.first_audio_s()? - r.record.first_token_s?)),
    });
    json!({
        "label": run.meta.label, "note": run.meta.note, "set": run.meta.set,
        "text": run.meta.text, "agent": agent_setup(&run.meta.radio),
        "metrics": metrics(&run.rows),
        "summary": {
            "calls": s.calls, "good": s.good, "incomplete": s.incomplete, "wrong": s.wrong,
            "disputed": s.disputed, "informed_p50": s.informed_p50, "informed_p90": s.informed_p90,
            "first_audio_p50": s.first_audio_p50, "transcript_p50": s.transcript_p50,
        },
        "breakdown": breakdown,
        "versus": versus,
        "rows": run.rows.iter().map(row).collect::<Vec<_>>(),
    })
}

/// The first run of each set is what the others are compared with.
pub fn render(runs: &[Run], calibration: &[Calibration], generated: &str) -> String {
    let list: Vec<Value> = runs
        .iter()
        .map(|r| {
            let base = runs.iter().find(|b| b.meta.set == r.meta.set);
            run_json(r, base.filter(|b| !std::ptr::eq(*b, r)))
        })
        .collect();
    let data = json!({ "generated": generated, "runs": list, "calibration": calibration });
    // Inside a script element only `</` could end it.
    TEMPLATE.replace("__DATA__", &data.to_string().replace("</", "<\\/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::tests::graded;

    #[test]
    fn a_call_costs_what_the_session_grew_by() {
        let g = |stint: &str, first: bool, total: Option<f64>| {
            let mut r = graded(stint, "c", Some(1.0), &[Grade::Good, Grade::Good]);
            r.record.first = first;
            r.record.session_cost = total.map(|t| (t, "USD".to_string()));
            r
        };
        let rows = [
            g("a", true, Some(0.05)),
            g("a", false, Some(0.08)),
            g("a", false, None),
            g("a", false, Some(0.10)),
            g("b", true, Some(0.04)),
        ];
        let costs = call_costs(&rows);
        let want = [0.05, 0.03, 0.02, 0.04];
        assert_eq!(costs.len(), want.len());
        for (c, w) in costs.iter().zip(want) {
            assert!((c - w).abs() < 1e-12, "{costs:?}");
        }
        assert_eq!(metrics(&rows)["currency"], "USD");
    }

    #[test]
    fn metrics_of_a_run() {
        use crate::run::Tokens;
        let mut a = graded(
            "s",
            "a",
            Some(4.0),
            &[Grade::Good, Grade::Good, Grade::Good],
        );
        a.record.tokens = Some(Tokens {
            turns: 1,
            total: 1000,
            input: 100,
            cached_read: 850,
            cached_write: 10,
            output: 40,
            thought: 5,
        });
        a.record.primer_tokens = Some(Tokens {
            total: 9000,
            ..Tokens::default()
        });
        let b = graded(
            "s",
            "b",
            Some(6.0),
            &[Grade::Wrong, Grade::Good, Grade::Wrong],
        );
        let m = metrics(&[a, b]);
        assert_eq!(m["good"]["v"], 0.5);
        assert_eq!(m["wrong"]["v"], 0.5);
        assert_eq!(m["good"]["n"], 2);
        assert_eq!(m["unanimous"]["v"], 0.5);
        assert_eq!(m["informed_p50"]["v"], 5.0);
        assert_eq!(m["tokens"]["v"], 1000.0);
        assert_eq!(m["tokens"]["n"], 1);
        assert_eq!(m["fresh_tokens"]["v"], 150.0);
        assert_eq!(m["output_tokens"]["v"], 40.0);
        assert_eq!(m["primer_tokens"]["v"], 9000.0);
        assert_eq!(m["cost"]["v"], Value::Null);
        assert_eq!(m["facts"]["n"], 0);
    }

    #[test]
    fn the_agent_setup_is_read_from_the_options() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            agent_setup(&args(&[
                "--agent",
                "x",
                "--agent-option",
                "model=gpt-6.1-terra",
                "--agent-option=reasoning_effort=medium;fast-mode=on",
            ])),
            json!({"model": "gpt-6.1-terra", "effort": "medium", "fast-mode": "on"})
        );
        assert_eq!(agent_setup(&args(&["--agent-option"])), json!({}));
    }

    #[test]
    fn the_page_carries_its_data() {
        let good = &[Grade::Good, Grade::Good];
        let run = |label: &str, set: &str, t: f64| Run {
            meta: Meta {
                set: set.into(),
                label: label.into(),
                ..Meta::default()
            },
            rows: vec![graded("s", "</script>", Some(t), good)],
        };
        let runs = [
            run("base", "dev", 9.0),
            run("fast", "dev", 4.0),
            run("other", "heldout", 5.0),
        ];
        let page = render(&runs, &[], "2026-11-22");
        assert!(page.starts_with("<title>"));
        assert!(!page.contains("__DATA__"));
        let start = page.find("id=\"data\">").unwrap() + "id=\"data\">".len();
        let end = start + page[start..].find("</script>").unwrap();
        let data: Value = serde_json::from_str(&page[start..end]).unwrap();
        assert_eq!(data["runs"][0]["versus"], Value::Null);
        assert_eq!(data["runs"][1]["versus"]["informed"]["mean"], -5.0);
        assert_eq!(data["runs"][2]["versus"], Value::Null);
        assert_eq!(data["runs"][0]["rows"][0]["grade"], "good");
        assert_eq!(data["runs"][0]["breakdown"]["stt"], 0.5);
    }
}
