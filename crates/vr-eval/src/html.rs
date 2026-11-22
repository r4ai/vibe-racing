//! The runs as one HTML page: self-contained except for web fonts, ready to
//! be published as it is.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::judge::Grade;
use crate::report::{Graded, paired, percentile, summarize};

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
    })
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
