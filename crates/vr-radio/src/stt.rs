//! Speech-to-text through an OpenAI-compatible API
//! (`/audio/transcriptions`), optionally followed by an LLM pass that fixes
//! misheard racing terms (`/chat/completions`).
//!
//! Works with OpenRouter, OpenAI, Groq and local servers (whisper.cpp,
//! faster-whisper, speaches) since they share the same endpoints.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::http::{self, Multipart};
use crate::sapi::WindowsTranscriber;

pub trait Transcriber: Send + Sync + 'static {
    /// Transcribes a WAV file. Blocking.
    fn transcribe(&self, wav: &[u8]) -> Result<String>;
}

/// Who turns speech into text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SttBackend {
    /// An OpenAI-compatible API.
    #[default]
    Api,
    /// The recognizer built into Windows, offline.
    Windows,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SttConfig {
    pub backend: SttBackend,
    /// Base URL ending before `/audio/transcriptions`, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    /// ISO-639-1 code, e.g. `ja`.
    pub language: Option<String>,
    /// Vocabulary hint (driver names, `ボックス`, `アンダーカット`...).
    pub prompt: Option<String>,
    /// Chat model that cleans up the transcript; `None` skips the pass.
    pub refine_model: Option<String>,
}

/// Without the API key: commands carrying it are logged.
impl std::fmt::Debug for SttConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SttConfig")
            .field("backend", &self.backend)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("has_key", &self.api_key.is_some())
            .finish_non_exhaustive()
    }
}

pub fn build(cfg: &SttConfig) -> Arc<dyn Transcriber> {
    match cfg.backend {
        SttBackend::Api => Arc::new(OpenAiTranscriber::new(cfg.clone())),
        SttBackend::Windows => Arc::new(WindowsTranscriber::new(cfg.language.as_deref())),
    }
}

/// Whether `url` is OpenRouter itself, not a look-alike host.
pub fn is_openrouter(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|u| u.split('/').next())
        .is_some_and(|host| host.eq_ignore_ascii_case("openrouter.ai"))
}

/// The key for `url`. A key is only sent to the service it belongs to: an
/// OpenAI key would be rejected (and leaked) by OpenRouter and vice versa.
pub fn api_key(
    url: &str,
    explicit: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
    dotenv: Option<&str>,
) -> Option<String> {
    let found = |k: Option<String>| k.filter(|k| !k.is_empty());
    let name = if is_openrouter(url) {
        "OPENROUTER_API_KEY"
    } else {
        "OPENAI_API_KEY"
    };
    found(explicit.map(str::to_string))
        .or_else(|| found(env(name)))
        .or_else(|| found(dotenv_key(dotenv?, name)))
}

/// `NAME=value` of a `.env` file. A bare `sk-or-…` line is an OpenRouter key.
fn dotenv_key(file: &str, name: &str) -> Option<String> {
    file.lines().map(str::trim).find_map(|line| {
        let line = line.strip_prefix("export ").unwrap_or(line);
        match line.split_once('=') {
            Some((key, value)) => {
                (key.trim() == name).then(|| value.trim().trim_matches(['"', '\'']).to_string())
            }
            None => (name == "OPENROUTER_API_KEY" && line.starts_with("sk-or-"))
                .then(|| line.to_string()),
        }
    })
}

pub struct OpenAiTranscriber {
    cfg: SttConfig,
    agent: ureq::Agent,
}

/// System prompt of the optional cleanup pass.
pub const REFINE_SYSTEM: &str = "あなたはシムレースの無線の音声認識結果を校正する。\
誤認識されたレース用語・ドライバー名・数字だけを直し、意味を変えず、校正後の文だけを返す。\
例: 「ネンピ」→「燃費」、「ボックス」はそのまま。";

impl OpenAiTranscriber {
    pub fn new(cfg: SttConfig) -> Self {
        Self {
            cfg,
            agent: http::agent(Duration::from_secs(30)),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.cfg.base_url.trim_end_matches('/'))
    }

    fn post(&self, path: &str) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let req = self.agent.post(self.url(path));
        match &self.cfg.api_key {
            Some(key) => req.header("Authorization", format!("Bearer {key}")),
            None => req,
        }
    }

    fn whisper(&self, wav: &[u8]) -> Result<String> {
        let mut form = Multipart::new("vr-radio-boundary-7d1f3a")
            .text("model", &self.cfg.model)
            .text("response_format", "json")
            .text("temperature", "0");
        if let Some(lang) = &self.cfg.language {
            form = form.text("language", lang);
        }
        if let Some(prompt) = &self.cfg.prompt {
            form = form.text("prompt", prompt);
        }
        let form = form.file("file", "speech.wav", "audio/wav", wav);
        let content_type = form.content_type();
        let resp = self
            .post("/audio/transcriptions")
            .content_type(content_type)
            .send(&form.finish()[..])
            .with_context(|| format!("transcriber unreachable at {}", self.cfg.base_url))?;
        let mut resp = http::check(resp, "transcription")?;
        let body: Value = resp.body_mut().read_json()?;
        Ok(body["text"].as_str().unwrap_or_default().trim().to_string())
    }

    fn refine(&self, model: &str, text: &str) -> Result<String> {
        let resp = self
            .post("/chat/completions")
            .send_json(json!({
                "model": model,
                "temperature": 0,
                "messages": [
                    {"role": "system", "content": REFINE_SYSTEM},
                    {"role": "user", "content": text},
                ],
            }))
            .context("refine model unreachable")?;
        let mut resp = http::check(resp, "refine")?;
        let body: Value = resp.body_mut().read_json()?;
        let out = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .trim();
        // An empty answer would lose what the driver said.
        Ok(if out.is_empty() { text } else { out }.to_string())
    }
}

impl Transcriber for OpenAiTranscriber {
    fn transcribe(&self, wav: &[u8]) -> Result<String> {
        let text = self.whisper(wav)?;
        match &self.cfg.refine_model {
            Some(model) if !text.is_empty() => self.refine(model, &text),
            _ => Ok(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake;

    fn cfg(url: &str) -> SttConfig {
        SttConfig {
            backend: SttBackend::Api,
            base_url: format!("{url}/v1/"),
            api_key: Some("sk-test".into()),
            model: "gpt-4o-mini-transcribe".into(),
            language: Some("ja".into()),
            prompt: Some("ボックス".into()),
            refine_model: None,
        }
    }

    #[test]
    fn posts_multipart_and_reads_text() {
        let server = fake::serve(|_| fake::Response::json(r#"{"text":" 前との差は？ "}"#));
        let t = OpenAiTranscriber::new(cfg(&server.url));
        assert_eq!(t.transcribe(b"RIFFdata").unwrap(), "前との差は？");

        let reqs = server.requests.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        let r = &reqs[0];
        assert_eq!(r.method, "POST");
        assert_eq!(r.target, "/v1/audio/transcriptions");
        assert_eq!(r.header("authorization"), Some("Bearer sk-test"));
        assert_eq!(
            r.header("content-type"),
            Some("multipart/form-data; boundary=vr-radio-boundary-7d1f3a")
        );
        let body = String::from_utf8_lossy(&r.body);
        assert!(body.contains("name=\"model\"\r\n\r\ngpt-4o-mini-transcribe\r\n"));
        assert!(body.contains("name=\"language\"\r\n\r\nja\r\n"));
        assert!(body.contains("name=\"prompt\"\r\n\r\nボックス\r\n"));
        assert!(
            body.contains("filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFFdata\r\n")
        );
    }

    #[test]
    fn omits_optional_fields_and_auth() {
        let server = fake::serve(|_| fake::Response::json(r#"{"text":"ok"}"#));
        let mut c = cfg(&server.url);
        c.api_key = None;
        c.language = None;
        c.prompt = None;
        OpenAiTranscriber::new(c).transcribe(b"x").unwrap();
        let reqs = server.requests.lock().unwrap();
        assert_eq!(reqs[0].header("authorization"), None);
        let body = String::from_utf8_lossy(&reqs[0].body);
        assert!(!body.contains("name=\"language\""));
        assert!(!body.contains("name=\"prompt\""));
    }

    #[test]
    fn refine_pass_replaces_text() {
        let server = fake::serve(|r| {
            if r.target.ends_with("/chat/completions") {
                fake::Response::json(r#"{"choices":[{"message":{"content":"燃費は？"}}]}"#)
            } else {
                fake::Response::json(r#"{"text":"ネンピは？"}"#)
            }
        });
        let mut c = cfg(&server.url);
        c.refine_model = Some("gpt-4.1-nano".into());
        assert_eq!(
            OpenAiTranscriber::new(c).transcribe(b"x").unwrap(),
            "燃費は？"
        );
        let reqs = server.requests.lock().unwrap();
        assert_eq!(reqs[1].target, "/v1/chat/completions");
        let body: Value = serde_json::from_slice(&reqs[1].body).unwrap();
        assert_eq!(body["model"], "gpt-4.1-nano");
        assert_eq!(body["messages"][1]["content"], "ネンピは？");
    }

    #[test]
    fn refine_keeps_original_on_empty_answer_and_skips_silence() {
        let server = fake::serve(|r| {
            if r.target.ends_with("/chat/completions") {
                fake::Response::json(r#"{"choices":[{"message":{"content":"  "}}]}"#)
            } else {
                fake::Response::json(r#"{"text":"ボックス"}"#)
            }
        });
        let mut c = cfg(&server.url);
        c.refine_model = Some("m".into());
        assert_eq!(
            OpenAiTranscriber::new(c.clone()).transcribe(b"x").unwrap(),
            "ボックス"
        );

        let silent = fake::serve(|_| fake::Response::json(r#"{"text":""}"#));
        c.base_url = silent.url.clone();
        assert_eq!(OpenAiTranscriber::new(c).transcribe(b"x").unwrap(), "");
        assert_eq!(silent.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn keys_go_to_their_own_service() {
        let or = "https://openrouter.ai/api/v1";
        let oa = "https://api.openai.com/v1";
        let env = |n: &str| Some(format!("env-{n}"));
        let none = |_: &str| None;
        let key = |k: &str| Some(k.to_string());
        assert_eq!(api_key(or, None, env, None), key("env-OPENROUTER_API_KEY"));
        assert_eq!(api_key(oa, None, env, None), key("env-OPENAI_API_KEY"));
        assert_eq!(api_key(or, Some("x"), env, None), key("x"));
        assert_eq!(
            api_key(or, Some(""), env, None),
            key("env-OPENROUTER_API_KEY")
        );
        assert_eq!(api_key(or, None, none, None), None);
        assert_eq!(api_key(or, None, |_| key(""), None), None);

        let file = "# keys\nexport OPENAI_API_KEY='oa'\nOPENROUTER_API_KEY = \"or=1\"\n";
        assert_eq!(api_key(or, None, none, Some(file)), key("or=1"));
        assert_eq!(api_key(oa, None, none, Some(file)), key("oa"));
        assert_eq!(
            api_key(oa, None, env, Some(file)),
            key("env-OPENAI_API_KEY")
        );
        let bare = "sk-or-v1-abc\r\n";
        assert_eq!(api_key(or, None, none, Some(bare)), key("sk-or-v1-abc"));
        assert_eq!(api_key(oa, None, none, Some(bare)), None);
        assert_eq!(api_key(or, None, none, Some("hello\nA=1")), None);
    }

    #[test]
    fn openrouter_is_matched_by_host() {
        assert!(is_openrouter("https://openrouter.ai/api/v1"));
        assert!(is_openrouter("https://OpenRouter.ai"));
        assert!(!is_openrouter("https://openrouter.ai.evil.example/api"));
        assert!(!is_openrouter("http://openrouter.ai/api/v1"));
        assert!(!is_openrouter("https://api.openai.com/openrouter.ai"));
    }

    #[test]
    fn debug_hides_the_key() {
        let shown = format!("{:?}", cfg("http://x"));
        assert!(!shown.contains("sk-test"), "{shown}");
        assert!(shown.contains("has_key: true"), "{shown}");
    }

    #[test]
    fn build_follows_the_backend() {
        let server = fake::serve(|_| fake::Response::json(r#"{"text":"ok"}"#));
        assert_eq!(build(&cfg(&server.url)).transcribe(b"x").unwrap(), "ok");
        let mut c = cfg(&server.url);
        c.backend = SttBackend::Windows;
        build(&c);
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn http_error_is_reported() {
        let server = fake::serve(|_| fake::Response {
            status: 500,
            content_type: "text/plain",
            body: b"boom".to_vec(),
        });
        let err = OpenAiTranscriber::new(cfg(&server.url))
            .transcribe(b"x")
            .unwrap_err()
            .to_string();
        assert_eq!(err, "transcription: HTTP 500 Internal Server Error: boom");
    }
}
