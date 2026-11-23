//! Questions about kept sessions whose answers SQL knows: the right answer is
//! worked out from the data each time, and an answer is checked for its
//! numbers and names without a judge.
//!
//! A set is YAML, a list of [`Question`]s. `{race}`, `{before}` and `{warmup}`
//! in the SQL are the newest race, the race before it on the same track and
//! the warm-up before the newest race.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vr_server::lake;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// As the driver says it.
    pub ask: String,
    /// Its answer from the data.
    pub sql: String,
    /// The columns the answer has to say; all of them by default.
    #[serde(default)]
    pub cols: Vec<usize>,
    /// The rows it has to say, from the first.
    #[serde(default = "one")]
    pub rows: usize,
    /// How far a number may be off; by default a little for rounding.
    #[serde(default)]
    pub tol: Option<f64>,
}

fn one() -> usize {
    1
}

pub fn load(path: &Path) -> Result<Vec<Question>> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    Ok(serde_yaml::from_str(&text)?)
}

/// The sessions the placeholders stand for.
pub fn places(sessions: &Path) -> Result<Vec<(&'static str, String)>> {
    let race = "(SELECT max(id) FROM sessions WHERE kind = 'race' AND laps_completed > 2)";
    let r = lake::query(
        sessions,
        &format!(
            "SELECT {race}, (SELECT max(id) FROM sessions WHERE kind = 'race' AND laps_completed > 2 AND id < {race}
               AND track = (SELECT track FROM sessions WHERE id = {race})),
             (SELECT max(id) FROM sessions WHERE kind = 'warmup' AND id < {race})"
        ),
    )?;
    let row = r.rows.first().context("no sessions")?;
    Ok(["{race}", "{before}", "{warmup}"]
        .into_iter()
        .zip(row)
        .map(|(k, v)| (k, v.as_str().unwrap_or_default().to_string()))
        .collect())
}

/// What the answer has to say.
#[derive(Debug, Clone, PartialEq)]
pub enum Fact {
    Number(f64),
    Name(String),
}

impl std::fmt::Display for Fact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fact::Number(n) => write!(f, "{}", (n * 1000.0).round() / 1000.0),
            Fact::Name(s) => f.write_str(s),
        }
    }
}

/// The right answer to a question, from the data.
pub fn gold(sessions: &Path, q: &Question, places: &[(&str, String)]) -> Result<Vec<Fact>> {
    let mut sql = q.sql.clone();
    for (k, id) in places {
        sql = sql.replace(k, &format!("'{}'", id.replace('\'', "''")));
    }
    let r = lake::query(sessions, &sql).with_context(|| sql.clone())?;
    let cols: Vec<usize> = if q.cols.is_empty() {
        (0..r.columns.len()).collect()
    } else {
        q.cols.clone()
    };
    let mut out = Vec::new();
    for row in r.rows.iter().take(q.rows) {
        for &c in &cols {
            match row.get(c) {
                Some(Value::Number(n)) => out.push(Fact::Number(n.as_f64().unwrap_or_default())),
                Some(Value::String(s)) => out.push(match s.trim().parse::<f64>() {
                    Ok(n) => Fact::Number(n),
                    Err(_) => Fact::Name(s.clone()),
                }),
                Some(Value::Bool(b)) => out.push(Fact::Name(b.to_string())),
                _ => {}
            }
        }
    }
    Ok(out)
}

/// The numbers said, as values: `1.4秒`, `-0.3`, `マイナス0.3`, `コンマ3`,
/// `1分51秒7` and `1:51.7` (seconds), full-width digits too.
pub fn numbers(text: &str) -> Vec<f64> {
    let s: Vec<char> = text
        .chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            '．' => '.',
            '−' | '－' | 'ー' => '-',
            _ => c,
        })
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    let read = |i: &mut usize| -> Option<String> {
        let start = *i;
        while *i < s.len() && (s[*i].is_ascii_digit() || (s[*i] == '.' && *i > start)) {
            *i += 1;
        }
        (*i > start).then(|| s[start..*i].iter().collect())
    };
    while i < s.len() {
        let rest: String = s[i..].iter().take(4).collect();
        if rest.starts_with("コンマ") {
            i += 3;
            if let Some(d) = read(&mut i) {
                out.push(format!("0.{d}").parse().unwrap_or_default());
            }
            continue;
        }
        let minus = rest.starts_with("マイナス") || s[i] == '-';
        let at = if rest.starts_with("マイナス") {
            i + 4
        } else if s[i] == '-' {
            i + 1
        } else {
            i
        };
        if at < s.len() && s[at].is_ascii_digit() {
            i = at;
            let Some(n) = read(&mut i) else { continue };
            let mut v: f64 = n.trim_end_matches('.').parse().unwrap_or_default();
            // Minutes and seconds: one number.
            if i + 1 < s.len() && matches!(s[i], '分' | ':') && s[i + 1].is_ascii_digit() {
                i += 1;
                let sec = read(&mut i).unwrap_or_default();
                let mut secs: f64 = sec.parse().unwrap_or_default();
                if i + 1 < s.len() && s[i] == '秒' && s[i + 1].is_ascii_digit() {
                    i += 1;
                    let frac = read(&mut i).unwrap_or_default();
                    secs += format!("0.{frac}").parse::<f64>().unwrap_or_default();
                }
                v = v * 60.0 + secs;
            }
            out.push(if minus { -v } else { v });
        } else {
            i += 1;
        }
    }
    out
}

/// How far a number may be off: what rounding it to a few figures gives.
fn tolerance(x: f64) -> f64 {
    (x.abs() * 0.01).max(0.051)
}

/// What the answer did not say right.
pub fn missing(answer: &str, facts: &[Fact], tol: Option<f64>) -> Vec<Fact> {
    let said = numbers(answer);
    let text: String = answer.split_whitespace().collect::<String>().to_lowercase();
    facts
        .iter()
        .filter(|f| match f {
            // A sign may be said in words ("速い", "遅い").
            Fact::Number(x) => !said
                .iter()
                .any(|n| (n.abs() - x.abs()).abs() <= tol.unwrap_or_else(|| tolerance(*x))),
            // Either of the ways to say it (`a|b`).
            Fact::Name(n) => !n.split('|').any(|n| {
                let n: String = n.split_whitespace().collect::<String>().to_lowercase();
                !n.is_empty() && text.contains(&n)
            }),
        })
        .cloned()
        .collect()
}

/// An answer to check: from an eval run (`q`, `a`) or the radio's transcript
/// (`asked`, `said`).
#[derive(Debug, Clone, Deserialize)]
pub struct Answer {
    #[serde(alias = "asked")]
    pub q: String,
    #[serde(alias = "said")]
    pub a: String,
}

pub fn answers(path: &Path) -> Result<Vec<Answer>> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    Ok(text
        .lines()
        .filter(|l| l.starts_with('{'))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

/// Questions the radio was asked that the set does not have yet, newest
/// last, once each: what to write SQL for next.
pub fn unasked<'a>(set: &[Question], said: &'a [Answer]) -> Vec<&'a Answer> {
    let mut seen: std::collections::BTreeSet<&str> = set.iter().map(|q| q.ask.as_str()).collect();
    said.iter()
        .filter(|a| !a.q.trim().is_empty() && seen.insert(a.q.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_read_as_said_on_the_radio() {
        assert_eq!(numbers("スプーンで合計1.4秒失った。"), vec![1.4]);
        assert_eq!(numbers("マイナス0.3秒、-2周"), vec![-0.3, -2.0]);
        assert_eq!(numbers("コンマ3秒速い"), vec![0.3]);
        assert_eq!(numbers("ベストは1分51秒7"), vec![111.7]);
        assert_eq!(numbers("1分52秒"), vec![112.0]);
        assert_eq!(numbers("1:51.726"), vec![111.726]);
        assert_eq!(numbers("５号車、１２０キロ。"), vec![5.0, 120.0]);
        assert_eq!(numbers("3周目。"), vec![3.0]);
        assert_eq!(numbers("なし"), Vec::<f64>::new());
    }

    #[test]
    fn an_answer_has_to_say_each_fact() {
        let facts = [Fact::Name("スプーン".into()), Fact::Number(1.43)];
        assert!(missing("スプーンで1.4秒", &facts, None).is_empty());
        assert_eq!(
            missing("スプーンで1.3秒", &facts, None),
            vec![Fact::Number(1.43)]
        );
        assert_eq!(
            missing("ヘアピンで1.4秒", &facts, None),
            vec![Fact::Name("スプーン".into())]
        );
        // A little for rounding: 1% or 0.051, whichever is more.
        assert!(missing("133キロ", &[Fact::Number(132.5)], None).is_empty());
        assert!(!missing("134キロ", &[Fact::Number(132.5)], None).is_empty());
        assert!(missing("0.1秒遅い", &[Fact::Number(-0.095)], None).is_empty());
        assert!(!missing("0.5", &[Fact::Number(0.3)], Some(0.1)).is_empty());
        assert!(missing("0.35", &[Fact::Number(0.3)], Some(0.1)).is_empty());
        let who = [Fact::Name("Tony Gardner|4号車".into())];
        assert!(missing("4号車のガードナー", &who, None).is_empty());
        assert!(missing("tony gardnerだ", &who, None).is_empty());
        assert!(!missing("ガードナーだ", &who, None).is_empty());
    }

    #[test]
    fn only_new_questions_are_harvested() {
        let set = [Question {
            ask: "前との差は？".into(),
            sql: String::new(),
            cols: vec![],
            rows: 1,
            tol: None,
        }];
        let a = |q: &str| Answer {
            q: q.into(),
            a: "x".into(),
        };
        let said = [
            a("前との差は？"),
            a(""),
            a("燃料は？"),
            a("燃料は？"),
            a("タイヤは？"),
        ];
        let new: Vec<&str> = unasked(&set, &said).iter().map(|a| a.q.as_str()).collect();
        assert_eq!(new, ["燃料は？", "タイヤは？"]);
    }
}
