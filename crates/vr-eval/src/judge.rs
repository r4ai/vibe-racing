//! Blind judges: models of vendors other than the agents under test read the
//! question, the telemetry and what the driver heard, and nothing about which
//! configuration answered.

use std::time::Duration;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vr_radio::http;

use crate::run::Record;
use crate::scenario::Behavior;

/// Two models through the Codex CLI (`codex:<model>[@<effort>]`): it runs on
/// the ChatGPT plan, no per-token bill. Not blind to the vendor when Codex is
/// also under test: reports say so. `<vendor>/<model>` goes to OpenRouter.
/// One cheap model three times: its verdicts vary from run to run, the
/// majority is steadier.
pub const JUDGES: [&str; 3] = [LUNA, LUNA, LUNA];
const LUNA: &str = "codex:gpt-6-luna@high";

/// Replies judged in one Codex call: the CLI spends ~14k tokens on itself
/// per call.
pub const BATCH: usize = 8;

/// `codex:gpt-6.2-sol@low` is `("gpt-6.2-sol", Some("low"))`.
pub fn codex_model(judge: &str) -> Option<(&str, Option<&str>)> {
    let spec = judge.strip_prefix("codex:")?;
    Some(match spec.split_once('@') {
        Some((model, effort)) => (model, Some(effort)),
        None => (spec, None),
    })
}

/// Whether any of the judges needs the OpenRouter key.
pub fn needs_key(judges: &[&str]) -> bool {
    judges.iter().any(|j| codex_model(j).is_none())
}

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
        Behavior::Calm => {
            "ドライバーは動揺している。短い一言で受け止めて落ち着かせ、points に答え、次に集中すべきことを実データに基づいて伝えること。説教、長話、真実に反する励まし（「差は広がっている」等の嘘）は wrong か missing。"
        }
        Behavior::AdmitUnknown => {
            "データは途切れていて古い（DISCONNECTED）。途切れていることを伝えれば correct。古い値を現在値として言ったら wrong。途切れる前の値だと断って言うのは可。"
        }
    }
}

/// Every reply of a batch, numbered from 1, judged on its own.
pub fn batch_prompt(cases: &[Case]) -> String {
    let mut out = format!(
        "{SYSTEM}\nファイルやコマンドは使わず、下の文章だけで採点する。\n\
         下の {} 件は別々の無線。1 件ずつ独立に採点し、他の件の内容を持ち込まない。\
         verdicts には全件を case の番号付きで入れる。\n",
        cases.len()
    );
    for (i, c) in cases.iter().enumerate() {
        out.push_str(&format!(
            "\n======== case {} ========\n{}",
            i + 1,
            prompt(c)
        ));
    }
    out
}

fn batch_schema() -> Value {
    let mut one = schema()["json_schema"]["schema"].clone();
    one["required"]
        .as_array_mut()
        .expect("required")
        .insert(0, json!("case"));
    one["properties"]["case"] = json!({"type": "integer"});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verdicts"],
        "properties": {"verdicts": {"type": "array", "items": one}}
    })
}

/// The CLI has no system role: the rules come first, and no tools.
pub fn codex_prompt(c: &Case) -> String {
    format!(
        "{SYSTEM}\nファイルやコマンドは使わず、下の文章だけで採点する。\n\n{}",
        prompt(c)
    )
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

/// The grade more than half of at least two verdicts give; `None` otherwise
/// (two judges must agree, three need two alike).
pub fn consensus(verdicts: &[Verdict]) -> Option<Grade> {
    if verdicts.len() < 2 {
        return None;
    }
    verdicts
        .iter()
        .map(|v| v.grade)
        .find(|g| 2 * verdicts.iter().filter(|v| v.grade == *g).count() > verdicts.len())
}

/// A fixed order of `0..n` in which neighbours come from far apart: steps
/// of a stride coprime with `n`, near the golden ratio of it.
pub fn spread(n: usize) -> Vec<usize> {
    if n < 3 {
        return (0..n).collect();
    }
    let gcd = |mut a: usize, mut b: usize| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let mut stride = (n as f64 * 0.618) as usize;
    while gcd(stride, n) != 1 {
        stride += 1;
    }
    (0..n).map(|i| i * stride % n).collect()
}

/// Groups of at most `size` cases, in `spread` order, where no group has
/// two replies to the same words: a judge would mix them up.
pub fn batches(said: &[&str], size: usize) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    for i in spread(said.len()) {
        let free = out
            .iter_mut()
            .find(|b| b.len() < size && b.iter().all(|&j| said[j] != said[i]));
        match free {
            Some(b) => b.push(i),
            None => out.push(vec![i]),
        }
    }
    out
}

/// Whether every verdict is the same.
pub fn unanimous(verdicts: &[Verdict]) -> bool {
    verdicts.windows(2).all(|w| w[0].grade == w[1].grade)
}

pub struct Judge {
    /// For OpenRouter judges.
    pub key: Option<String>,
    pub model: String,
    /// An empty folder for the Codex CLI: no AGENTS.md, no repository.
    pub cwd: PathBuf,
}

/// A Codex judge that does not answer in time is retried.
const CODEX_TIMEOUT: Duration = Duration::from_secs(300);

/// `codex`, or `codex.cmd` on Windows.
fn codex_program() -> PathBuf {
    let program = std::env::var("VR_EVAL_CODEX").unwrap_or_else(|_| "codex".into());
    cfg!(windows)
        .then(|| {
            vr_radio::config::resolve_program(
                &program,
                std::env::var("PATH").ok().as_deref(),
                std::env::var("PATHEXT").ok().as_deref(),
            )
        })
        .flatten()
        .unwrap_or_else(|| program.into())
}

/// The CLI arguments of a Codex judge.
pub fn codex_args(model: &str, effort: Option<&str>, schema: &Path, out: &Path) -> Vec<String> {
    let mut args: Vec<String> = [
        "exec",
        "--ephemeral",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "--ignore-rules",
        "--sandbox",
        "read-only",
        "--model",
        model,
    ]
    .map(String::from)
    .to_vec();
    if let Some(e) = effort {
        args.extend(["-c".into(), format!("model_reasoning_effort={e}")]);
    }
    args.extend([
        "--output-schema".into(),
        schema.to_string_lossy().into(),
        "--output-last-message".into(),
        out.to_string_lossy().into(),
        // The prompt comes on stdin.
        "-".into(),
    ]);
    args
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

    /// Several replies; in one call for Codex, one by one otherwise.
    pub fn judge_many(&self, cases: &[Case]) -> Result<Vec<Verdict>> {
        let Some((model, effort)) = codex_model(&self.model) else {
            return cases.iter().map(|c| self.judge(c)).collect();
        };
        let mut last = None;
        for wait in [0, 5, 15] {
            std::thread::sleep(Duration::from_secs(wait));
            match self.ask_batch(model, effort, cases) {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
        }
        Err(last.expect("tried at least once"))
    }

    fn ask_batch(&self, model: &str, effort: Option<&str>, cases: &[Case]) -> Result<Vec<Verdict>> {
        let text = self.run_codex(model, effort, &batch_schema(), batch_prompt(cases))?;
        let v: Value = serde_json::from_str(text.trim())
            .with_context(|| format!("verdicts are not JSON: {text}"))?;
        let list = v["verdicts"].as_array().context("no verdicts")?;
        (0..cases.len())
            .map(|i| {
                let found: Vec<&Value> = list
                    .iter()
                    .filter(|x| x["case"].as_u64() == Some(i as u64 + 1))
                    .collect();
                ensure!(
                    found.len() == 1,
                    "case {} judged {} times",
                    i + 1,
                    found.len()
                );
                let points = found[0]["points"].as_array().map_or(0, Vec::len);
                ensure!(
                    points == cases[i].points.len(),
                    "case {}: {points} points for {}",
                    i + 1,
                    cases[i].points.len()
                );
                self.verdict(found[0])
            })
            .collect()
    }

    fn ask(&self, case: &Case) -> Result<Verdict> {
        let text = match codex_model(&self.model) {
            Some((model, effort)) => self.run_codex(
                model,
                effort,
                &schema()["json_schema"]["schema"],
                codex_prompt(case),
            )?,
            None => self.ask_openrouter(case)?,
        };
        let v: Value = serde_json::from_str(text.trim())
            .with_context(|| format!("verdict is not JSON: {text}"))?;
        self.verdict(&v)
    }

    fn verdict(&self, v: &Value) -> Result<Verdict> {
        Ok(Verdict {
            judge: self.model.clone(),
            grade: grade(v)?,
            reason: format!(
                "{} {}",
                v["reasoning"].as_str().unwrap_or_default(),
                v["false_claims"]
            ),
        })
    }

    fn run_codex(
        &self,
        model: &str,
        effort: Option<&str>,
        schema: &Value,
        input: String,
    ) -> Result<String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        std::fs::create_dir_all(&self.cwd)?;
        let schema_file = self.cwd.join(format!("schema-{id}.json"));
        let out = self.cwd.join(format!("verdict-{id}.json"));
        std::fs::write(&schema_file, schema.to_string())?;
        let mut child = std::process::Command::new(codex_program())
            .args(codex_args(model, effort, &schema_file, &out))
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("codex")?;
        let mut stdin = child.stdin.take().context("stdin")?;
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
        let mut stderr = child.stderr.take().context("stderr")?;
        let err = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            s
        });
        let began = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            if began.elapsed() > CODEX_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                bail!("codex judge timed out");
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        let _ = std::fs::remove_file(&schema_file);
        let _ = std::fs::remove_file(&out);
        let err = err.join().unwrap_or_default();
        ensure!(
            status.success() && !text.trim().is_empty(),
            "codex judge failed ({status}): {}",
            err.lines().rev().take(3).collect::<Vec<_>>().join(" | ")
        );
        Ok(text)
    }

    fn ask_openrouter(&self, case: &Case) -> Result<String> {
        let key = self
            .key
            .as_deref()
            .context("this judge needs OPENROUTER_API_KEY")?;
        let resp = http::agent(Duration::from_secs(120))
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {key}"))
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
        Ok(text.into())
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
    fn codex_judges_are_named_with_an_optional_effort() {
        assert_eq!(
            codex_model("codex:gpt-6.2-sol"),
            Some(("gpt-6.2-sol", None))
        );
        assert_eq!(
            codex_model("codex:gpt-6.2-sol@low"),
            Some(("gpt-6.2-sol", Some("low")))
        );
        assert_eq!(codex_model("x-ai/grok-4.7"), None);
        assert_eq!(codex_model("codexx:a"), None);
        assert!(!needs_key(&JUDGES));
        assert!(needs_key(&["codex:a", "x-ai/grok-4.7"]));
        assert!(!needs_key(&[]));
    }

    #[test]
    fn codex_runs_without_the_user_setup_and_reads_stdin() {
        let a = codex_args("m", Some("low"), Path::new("s.json"), Path::new("o.json"));
        assert_eq!(
            a,
            [
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "--ignore-user-config",
                "--ignore-rules",
                "--sandbox",
                "read-only",
                "--model",
                "m",
                "-c",
                "model_reasoning_effort=low",
                "--output-schema",
                "s.json",
                "--output-last-message",
                "o.json",
                "-",
            ]
        );
        let b = codex_args("m", None, Path::new("s.json"), Path::new("o.json"));
        assert_eq!(b.len(), a.len() - 2);
        assert!(!b.iter().any(|x| x.starts_with("model_reasoning")));
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_judge_round_trip() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        // Writes the verdict where -o points, and keeps the prompt it got.
        let fake = d.path().join("codex");
        std::fs::write(
            &fake,
            "#!/bin/sh\nwhile [ \"$1\" != \"--output-last-message\" ]; do shift; done\n\
             cat > prompt.txt\n\
             echo '{\"reasoning\":\"ok\",\"points\":[\"correct\"],\"false_claims\":[]}' > \"$2\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: the only test that reads the variable.
        unsafe { std::env::set_var("VR_EVAL_CODEX", &fake) };
        let cwd = d.path().join("judge");
        let judge = Judge {
            key: None,
            model: "codex:m@low".into(),
            cwd: cwd.clone(),
        };
        let v = judge.ask(&case()).unwrap();
        assert_eq!(v.grade, Grade::Good);
        assert_eq!(v.judge, "codex:m@low");
        let prompt = std::fs::read_to_string(cwd.join("prompt.txt")).unwrap();
        assert!(prompt.starts_with(SYSTEM));
        assert!(prompt.contains("「0.6。」"));
        // Nothing is left behind but what the fake wrote.
        let left: Vec<_> = std::fs::read_dir(&cwd).unwrap().collect();
        assert_eq!(left.len(), 1);
        // A batch: every case once, with as many points as asked.
        let batch = |verdicts: &str| {
            format!(
                "#!/bin/sh\nwhile [ \"$1\" != \"--output-last-message\" ]; do shift; done\n\
                 cat > /dev/null\necho '{{\"verdicts\":[{verdicts}]}}' > \"$2\"\n"
            )
        };
        let one = |n: u32, p: &str| {
            format!("{{\"case\":{n},\"reasoning\":\"r\",\"points\":[\"{p}\"],\"false_claims\":[]}}")
        };
        std::fs::write(
            &fake,
            batch(&format!("{},{}", one(2, "missing"), one(1, "correct"))),
        )
        .unwrap();
        let v = judge.ask_batch("m", None, &[case(), case()]).unwrap();
        assert_eq!(
            v.iter().map(|v| v.grade).collect::<Vec<_>>(),
            [Grade::Good, Grade::Incomplete]
        );
        std::fs::write(&fake, batch(&one(1, "correct"))).unwrap();
        let e = format!(
            "{:#}",
            judge.ask_batch("m", None, &[case(), case()]).unwrap_err()
        );
        assert!(e.contains("case 2 judged 0 times"), "{e}");
        std::fs::write(&fake, "#!/bin/sh\necho broken >&2\nexit 3\n").unwrap();
        let e = format!("{:#}", judge.ask(&case()).unwrap_err());
        assert!(
            e.contains("codex judge failed") && e.contains("broken"),
            "{e}"
        );
    }

    #[test]
    fn an_openrouter_judge_without_a_key_fails_at_once() {
        let judge = Judge {
            key: None,
            model: "x-ai/grok-4.7".into(),
            cwd: PathBuf::new(),
        };
        let e = format!("{:#}", judge.ask(&case()).unwrap_err());
        assert!(e.contains("OPENROUTER_API_KEY"), "{e}");
    }

    fn case() -> Case {
        Case {
            said: "前との差は？".into(),
            history: vec![("後ろは？".into(), "1.0".into())],
            behavior: Behavior::Answer,
            points: vec!["前との差".into()],
            truth_at_question: "T1".into(),
            truth_at_answer: "T2".into(),
            truth_between: vec!["T3".into()],
            reply: "0.6。".into(),
        }
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
        // Three: two alike decide, three different do not.
        let (g, i, w) = (v(Grade::Good), v(Grade::Incomplete), v(Grade::Wrong));
        assert_eq!(
            consensus(&[w.clone(), g.clone(), g.clone()]),
            Some(Grade::Good)
        );
        assert_eq!(consensus(&[g.clone(), i.clone(), w.clone()]), None);
        assert!(unanimous(&[g.clone(), g.clone(), g.clone()]));
        assert!(!unanimous(&[g.clone(), g, w]));
        assert!(unanimous(&[i]));
    }

    #[test]
    fn neighbours_are_spread_apart() {
        for n in [0, 1, 2, 3, 8, 24, 63] {
            let mut o = spread(n);
            if n >= 8 {
                // Adjacent cases of the input never meet in the output.
                assert!(o.windows(2).all(|w| w[0].abs_diff(w[1]) > 1), "{n}");
            }
            o.sort();
            assert_eq!(o, (0..n).collect::<Vec<_>>());
        }
        // 8 × 0.618 → 4, not coprime with 8 → 5.
        assert_eq!(spread(8), [0, 5, 2, 7, 4, 1, 6, 3]);
        assert_eq!(spread(2), [0, 1]);
    }

    #[test]
    fn a_batch_never_holds_the_same_words_twice() {
        let said = ["a", "a", "a", "b", "b", "c", "d", "e", "f", "g"];
        let b = batches(&said, 4);
        let mut all: Vec<usize> = b.concat();
        all.sort();
        assert_eq!(all, (0..said.len()).collect::<Vec<_>>());
        for g in &b {
            assert!(g.len() <= 4);
            let mut words: Vec<&str> = g.iter().map(|&i| said[i]).collect();
            words.sort();
            words.dedup();
            assert_eq!(words.len(), g.len(), "{b:?}");
        }
        // Three "a" need three batches; ten cases fit in three of four.
        assert_eq!(b.len(), 3);
        assert_eq!(batches(&[], 8), Vec::<Vec<usize>>::new());
        assert_eq!(batches(&["x"], 1), [[0]]);
    }

    #[test]
    fn a_batch_numbers_every_case() {
        let p = batch_prompt(&[case(), case()]);
        assert!(p.starts_with(SYSTEM));
        assert!(p.contains("下の 2 件は別々の無線"));
        assert!(p.contains("======== case 1 ========") && p.contains("======== case 2 ========"));
        assert!(!p.contains("case 3"));
        let s = batch_schema();
        let item = &s["properties"]["verdicts"]["items"];
        assert_eq!(
            item["required"],
            json!(["case", "reasoning", "points", "false_claims"])
        );
        assert_eq!(item["properties"]["case"]["type"], "integer");
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
        assert!(codex_prompt(&c).ends_with(&p));
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
