//! Blind judges: models of vendors other than the agents under test read the
//! question, the telemetry and what the driver heard, and nothing about which
//! configuration answered.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vr_radio::http;

use crate::run::Record;
use crate::scenario::Behavior;

/// Neither Anthropic nor OpenAI. Chosen by `vr-eval calibrate`.
pub const JUDGES: [&str; 2] = ["google/gemini-4-hydrogen", "x-ai/grok-4.7"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    /// Every point, nothing false.
    Good,
    /// Nothing false, but a point is missing.
    Incomplete,
    /// Says something false, or makes something up.
    Wrong,
}

/// What a judge is shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub said: String,
    pub history: Vec<(String, String)>,
    pub behavior: Behavior,
    pub points: Vec<String>,
    pub truth_at_question: String,
    pub truth_at_answer: String,
    pub truth_between: Vec<String>,
    pub reply: String,
}

impl From<&Record> for Case {
    fn from(r: &Record) -> Self {
        Self {
            said: r.said.clone(),
            history: r.history.clone(),
            behavior: r.behavior,
            points: r.points.clone(),
            truth_at_question: r.truth_at_question.clone(),
            truth_at_answer: r.truth_at_answer.clone(),
            truth_between: r.truth_between.clone(),
            reply: r.spoken(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub judge: String,
    pub grade: Grade,
    pub reason: String,
}

const SYSTEM: &str = "あなたはレース無線の回答を採点する中立な審査員。\
ドライバーの質問、レースの実データ（真実）、エンジニアが声で返した回答を読み、事実だけで採点する。\n\
採点規則:\n\
- 真実は質問が終わった時点から回答が始まった時点まで動いている。両時点の値か、その間の 1 秒ごとの推移に現れた値（同じ瞬間の組み合わせ）なら正しい\n\
- 声に出すための丸め（0.1 秒、0.1L、0.5 周、1 周単位の言い方）は正しい。「約」「ほど」「くらい」を付けた数は 5% までの違いを正しいとする。それを超える違いは誤り\n\
- 前と後ろ、縮む（closing）と広がる（growing）、誰の値かの取り違えは誤り\n\
- 真実から計算できる妥当な推論（例: 差 ÷ 毎周の縮まり = 追いつくまでの周回、残り周回との比較、給油量）は正しい。真実に無い数字や出来事は誤り\n\
- 判断や助言（「まだ余裕」「プッシュ」など）は、真実と矛盾しない限り誤りにしない\n\
- 「確認する」等の前置きや言い回し、長さ、敬語は採点しない\n\
- 音声の認識ミスで別の質問に答えた場合は、聞かれた points が欠ける（missing）\n\
出力は JSON だけ。points は与えられた順に、それぞれ correct / wrong / missing。\
false_claims は points 以外も含め、回答の中で真実に反する主張だけを短く引用する（無ければ空）。";

fn expectation(b: Behavior) -> &'static str {
    match b {
        Behavior::Answer => "points のすべてに答えること。",
        Behavior::AnswerOrAsk => {
            "points に答えるか、聞き取れなかったことを短く伝えて聞き返すこと。聞き返しなら points は correct とする。"
        }
        Behavior::Ack => {
            "質問ではない。短く受け答えすること。真実に反しない短い返答なら points は correct とする。"
        }
        Behavior::Silent => {
            "ドライバーは何も言っていない（雑音だけ）。何も言わないか、短い聞き返しだけなら points は correct。聞かれてもいないレース情報を話したら wrong。"
        }
        Behavior::AdmitUnknown => {
            "データは途切れていて古い（DISCONNECTED）。途切れていることを伝えれば correct。古い値を現在値として言ったら wrong。途切れる前の値だと断って言うのは可。"
        }
    }
}

pub fn prompt(c: &Case) -> String {
    let history: String = c
        .history
        .iter()
        .map(|(d, e)| format!("ドライバー: {d}\nエンジニア: {e}\n"))
        .collect();
    let points: String = c
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}. {p}\n", i + 1))
        .collect();
    format!(
        "# これまでの無線\n{}\n# ドライバーの今回の発言（本人の意図）\n「{}」\n\n# 期待\n{}\npoints:\n{points}\n\
         # 真実（質問が終わった時点）\n{}\n# 真実の推移（t=0 が質問の終わり）\n{}\n# 真実（回答が始まった時点）\n{}\n# エンジニアの回答（ドライバーに聞こえた全文）\n「{}」\n",
        if history.is_empty() {
            "（なし）\n"
        } else {
            &history
        },
        c.said,
        expectation(c.behavior),
        c.truth_at_question,
        c.truth_between.join("\n"),
        c.truth_at_answer,
        c.reply,
    )
}

fn schema() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "verdict",
            "strict": true,
            "schema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["reasoning", "points", "false_claims"],
                "properties": {
                    "reasoning": {"type": "string"},
                    "points": {
                        "type": "array",
                        "items": {"type": "string", "enum": ["correct", "wrong", "missing"]}
                    },
                    "false_claims": {"type": "array", "items": {"type": "string"}}
                }
            }
        }
    })
}

/// The grade follows from the findings, not from the judge's overall feeling.
pub fn grade(v: &Value) -> Result<Grade> {
    let points: Vec<&str> = v["points"]
        .as_array()
        .context("no points")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let claims = v["false_claims"].as_array().map_or(0, Vec::len);
    Ok(if claims > 0 || points.contains(&"wrong") {
        Grade::Wrong
    } else if points.is_empty() || points.iter().any(|p| *p != "correct") {
        Grade::Incomplete
    } else {
        Grade::Good
    })
}

/// Two opinions as one; `None` when they differ or one is missing.
pub fn consensus(verdicts: &[Verdict]) -> Option<Grade> {
    let first = verdicts.first()?.grade;
    (verdicts.len() >= 2 && verdicts.iter().all(|v| v.grade == first)).then_some(first)
}

pub struct Judge {
    pub key: String,
    pub model: String,
}

impl Judge {
    pub fn judge(&self, case: &Case) -> Result<Verdict> {
        let mut last = None;
        for wait in [0, 2, 5, 10] {
            std::thread::sleep(Duration::from_secs(wait));
            match self.ask(case) {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
        }
        Err(last.expect("tried at least once"))
    }

    fn ask(&self, case: &Case) -> Result<Verdict> {
        let resp = http::agent(Duration::from_secs(120))
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.key))
            .send_json(json!({
                "model": self.model,
                "temperature": 0,
                "max_tokens": 20000,
                "response_format": schema(),
                "messages": [
                    {"role": "system", "content": SYSTEM},
                    {"role": "user", "content": prompt(case)},
                ],
            }))?;
        let mut resp = http::check(resp, "judge")?;
        let body: Value = resp.body_mut().read_json()?;
        let text = body["choices"][0]["message"]["content"]
            .as_str()
            .context("no verdict")?;
        let v: Value = serde_json::from_str(text.trim())
            .with_context(|| format!("verdict is not JSON: {text}"))?;
        Ok(Verdict {
            judge: self.model.clone(),
            grade: grade(&v)?,
            reason: format!(
                "{} {}",
                v["reasoning"].as_str().unwrap_or_default(),
                v["false_claims"]
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_follow_the_findings() {
        let g = |points: &[&str], claims: &[&str]| {
            grade(&json!({"points": points, "false_claims": claims})).unwrap()
        };
        assert_eq!(g(&["correct", "correct"], &[]), Grade::Good);
        assert_eq!(g(&["correct", "missing"], &[]), Grade::Incomplete);
        assert_eq!(g(&["correct", "wrong"], &[]), Grade::Wrong);
        assert_eq!(g(&["correct"], &["P3"]), Grade::Wrong);
        assert_eq!(g(&[], &[]), Grade::Incomplete);
        assert!(grade(&json!({})).is_err());
    }

    #[test]
    fn consensus_needs_agreement() {
        let v = |grade| Verdict {
            judge: "j".into(),
            grade,
            reason: String::new(),
        };
        assert_eq!(
            consensus(&[v(Grade::Good), v(Grade::Good)]),
            Some(Grade::Good)
        );
        assert_eq!(consensus(&[v(Grade::Good), v(Grade::Wrong)]), None);
        assert_eq!(consensus(&[v(Grade::Good)]), None);
        assert_eq!(consensus(&[]), None);
    }

    #[test]
    fn the_prompt_is_blind() {
        let c = Case {
            said: "前との差は？".into(),
            history: vec![("後ろは？".into(), "1.0".into())],
            behavior: Behavior::Answer,
            points: vec!["前との差".into()],
            truth_at_question: "T1".into(),
            truth_at_answer: "T2".into(),
            truth_between: vec!["T3".into()],
            reply: "0.6。".into(),
        };
        let p = prompt(&c);
        for part in [
            "前との差は？",
            "後ろは？",
            "1. 前との差",
            "T1",
            "T2",
            "T3",
            "「0.6。」",
        ] {
            assert!(p.contains(part), "{part}");
        }
        for name in ["claude", "codex", "config"] {
            assert!(!p.to_lowercase().contains(name));
        }
    }
}
