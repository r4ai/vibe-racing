//! Command-line / environment settings.

use std::path::{Path, PathBuf};

use clap::Parser;

use crate::policy::Permissions;
use crate::stt::{self, SttBackend, SttConfig};

/// Default agent: Claude Code over ACP, pinned to a release older than 7 days.
pub const DEFAULT_AGENT: &str = "npx -y @agentclientprotocol/claude-agent-acp@0.107.0";

/// Codex over ACP, pinned likewise.
pub const CODEX_AGENT: &str = "npx -y @agentclientprotocol/codex-acp@2.6.0";

/// Most accurate on spoken Japanese racing terms at under a second per
/// question, among the OpenRouter models compared in docs/architecture.md.
pub const DEFAULT_STT_MODEL: &str = "openai/gpt-transcribe";

/// OpenRouter models offered in the settings.
pub const STT_MODELS: &[&str] = &[
    DEFAULT_STT_MODEL,
    "openai/gpt-4o-mini-transcribe",
    "elevenlabs/scribe-v2",
    "assemblyai/universal-3-6-pro",
    "x-ai/grok-stt-2.0",
    "qwen/qwen3-asr-0.6b",
];

/// Sent once when the agent connects, before the first question: the rules,
/// and time to read the skill. The last three rules (grounds for a call,
/// trends for catching up, steadying an upset driver) took the hard set of
/// vr-eval from 12 to 15 good answers of 16 in one trial each, about 0.5 s
/// slower (neither difference outside its 95 % interval yet).
pub const DEFAULT_PRIMER: &str = "\
これからシムレース走行中のドライバーの無線が 1 通ずつ届く。あなたはレースエンジニア。\n\
返答はそのまま音声で読み上げるので、次を守る。\n\
- 結論と数字から言い始める。日本語 1〜2 文。前置き、復唱、記号、マークダウン、箇条書きは無し\n\
- 聞かれたことにすべて答え、聞かれていないことは言わない。例外は新しい危険（イエロー、後ろの急接近）だけ。一度伝えた注意は繰り返さない\n\
- 無線には取得したての実データが付く。足りるときはコマンドを使わず即答し、足りないときだけ vr を実行する\n\
- 数字は推測しない。複数のデータで値が違うときは新しい方を使う。age が数秒以上か DISCONNECTED なら、値を言わずデータが途切れていると伝える\n\
- 無線は音声認識の文字なので、誤変換は文脈で補う（何週→何周、燃費・ボックス等）。疑問符は落ちるので、「イエロー出てる」「後ろは」のような状態や名詞だけの無線は質問とみなす。言い直しは最後の意図に従う\n\
- 冒頭や末尾が切れて二通りに取れるときは、推測で答えず「前と後ろ、どっち？」のように短く聞き返す。意味が取れない無線には「もう一度頼む」とだけ返す\n\
- お礼や了解、独り言だけの無線には「了解」など一言だけ返す\n\
- 判断（入るか・引っ張るか・守れるか・足りるか）を聞かれたら、結論と決め手の数字（燃料が何周分か、残り何周か、不足量、差と毎周の変化）を同じ答えで言う。2 文まで\n\
- 追いつく・追いつかれる見通しは、gap の毎周の変化（closing / growing）で計算する。1 周だけのラップ差で計算しない。growing なら追いつかれない\n\
- 動揺・怒り・焦りの無線には、最初に短く落ち着かせる一言、続けて事実と、次に集中すること一つ\n\
race-engineer スキルがあれば今読んで、データの読み方を把握しておくこと。コマンドはまだ実行しない。終わったら「了解」とだけ返す。";

pub const DEFAULT_PROMPT: &str = "\
無線:「{text}」\n\n\
{context}\n\n\
結論と数字から、日本語 1〜2 文で即答。";

/// Commands whose output goes with every question. Together they answer
/// nearly everything a driver asks while driving.
pub const DEFAULT_CONTEXT: &[&str] = &[
    "vr brief",
    "vr standings",
    "vr laps --last 5",
    "vr events --limit 10",
];

pub const DEFAULT_VOCABULARY: &str = "シムレースの無線。ギャップ、燃費、給油、ピット、ボックス、\
イエローフラッグ、アンダーカット、周回遅れ、ラップタイム、スティント。";

#[derive(Debug, Clone, Parser)]
#[command(
    name = "vr-radio",
    version,
    about = "Push-to-talk radio to an ACP agent, read back with VOICEVOX"
)]
pub struct Config {
    /// ACP agent command (command line or JSON `{"command","args","env"}`).
    #[arg(long, env = "VR_RADIO_AGENT", default_value = DEFAULT_AGENT)]
    pub agent: String,

    /// Working directory of the agent session (where `vr` and the skill are).
    #[arg(long, env = "VR_RADIO_CWD")]
    pub cwd: Option<PathBuf>,

    /// How to answer the agent's permission requests.
    #[arg(long, value_enum, env = "VR_RADIO_PERMISSIONS", default_value = "safe")]
    pub permissions: Permissions,

    /// Option of the agent's session, e.g. `model=haiku` or `effort=low`
    /// (repeatable; the agent lists its own).
    #[arg(long = "agent-option", env = "VR_RADIO_AGENT_OPTIONS", value_delimiter = ';', value_parser = parse_option)]
    pub agent_options: Vec<(String, String)>,

    /// Sent once when the agent connects, so that the first question does
    /// not wait for it. Empty: nothing is sent.
    #[arg(long, env = "VR_RADIO_PRIMER", default_value = DEFAULT_PRIMER)]
    pub primer: String,

    /// Command whose output goes with every question, e.g. `vr brief`
    /// (repeatable). One empty value: none.
    #[arg(long, env = "VR_RADIO_CONTEXT", value_delimiter = ';', default_values = DEFAULT_CONTEXT)]
    pub context: Vec<String>,

    /// Prompt sent to the agent; `{text}` is what the driver said, `{context}`
    /// the data fetched for it.
    #[arg(long, env = "VR_RADIO_PROMPT", default_value = DEFAULT_PROMPT)]
    pub prompt: String,

    /// Who transcribes: an API, or the recognizer built into Windows (offline).
    #[arg(long, value_enum, env = "VR_RADIO_STT", default_value = "api")]
    pub stt: SttBackend,

    /// OpenAI-compatible speech-to-text base URL (OpenRouter, OpenAI, Groq, local whisper server).
    #[arg(
        long,
        env = "VR_RADIO_STT_URL",
        default_value = "https://openrouter.ai/api/v1"
    )]
    pub stt_url: String,

    /// API key for the transcriber. Falls back to OPENROUTER_API_KEY (OpenRouter)
    /// or OPENAI_API_KEY, from the environment or `.env` in the working directory.
    #[arg(long, env = "VR_RADIO_STT_API_KEY", hide_env_values = true)]
    pub stt_api_key: Option<String>,

    #[arg(long, env = "VR_RADIO_STT_MODEL", default_value = DEFAULT_STT_MODEL)]
    pub stt_model: String,

    /// Transcribe while the button is still held, at every pause.
    #[arg(long, env = "VR_RADIO_EARLY_STT", default_value_t = true, action = clap::ArgAction::Set)]
    pub early_stt: bool,

    #[arg(long, env = "VR_RADIO_LANGUAGE", default_value = "ja")]
    pub language: String,

    /// Vocabulary hint given to the transcriber.
    #[arg(long, env = "VR_RADIO_VOCABULARY", default_value = DEFAULT_VOCABULARY)]
    pub vocabulary: String,

    /// Chat model that fixes misheard racing terms (adds one LLM round trip).
    #[arg(long, env = "VR_RADIO_REFINE_MODEL")]
    pub refine_model: Option<String>,

    #[arg(long, env = "VOICEVOX_URL", default_value = "http://127.0.0.1:50021")]
    pub voicevox_url: String,

    /// VOICEVOX style id (3 = ずんだもん ノーマル).
    #[arg(long, env = "VR_RADIO_SPEAKER", default_value_t = 3)]
    pub speaker: u32,

    /// Speaking rate multiplier.
    #[arg(long, env = "VR_RADIO_SPEED", default_value_t = 1.2)]
    pub speed: f64,

    /// Global push-to-talk key, held while speaking (e.g. F9, Ctrl+Shift+Space).
    /// Map a wheel button to it to talk without leaving the game.
    #[arg(long, env = "VR_RADIO_HOTKEY", default_value = "F9")]
    pub hotkey: String,

    /// UI font family installed on the system (default: the bundled Noto Sans JP).
    #[arg(long, env = "VR_RADIO_FONT")]
    pub font: Option<String>,

    /// File of the choices made in the settings sheet.
    #[arg(long, env = "VR_RADIO_SETTINGS")]
    pub settings: Option<PathBuf>,

    /// Start in the tray, without showing the window (Windows).
    #[arg(long)]
    pub tray: bool,

    /// Ask one typed question without the window, print the events and exit
    /// once the answer has been spoken (for checking the setup).
    #[arg(long)]
    pub ask: Option<String>,
}

impl Config {
    pub fn stt(&self) -> SttConfig {
        let dotenv = std::fs::read_to_string(self.agent_cwd().join(".env")).ok();
        SttConfig {
            backend: self.stt,
            base_url: self.stt_url.clone(),
            api_key: stt::api_key(
                &self.stt_url,
                self.stt_api_key.as_deref(),
                |name| std::env::var(name).ok(),
                dotenv.as_deref(),
            ),
            model: self.stt_model.clone(),
            language: Some(self.language.clone()).filter(|l| !l.is_empty()),
            prompt: Some(self.vocabulary.clone()).filter(|v| !v.is_empty()),
            refine_model: self.refine_model.clone().filter(|m| !m.is_empty()),
        }
    }

    pub fn session(&self) -> crate::agent::SessionConfig {
        crate::agent::SessionConfig {
            cwd: self.agent_cwd(),
            permissions: self.permissions,
            options: self.agent_options.clone(),
            primer: Some(self.primer.clone()).filter(|p| !p.trim().is_empty()),
        }
    }

    /// The commands that fetch data for a question.
    pub fn context_commands(&self) -> Vec<String> {
        self.context
            .iter()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect()
    }

    pub fn agent_cwd(&self) -> PathBuf {
        let cwd = self
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        std::path::absolute(&cwd).unwrap_or(cwd)
    }
}

fn parse_option(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((id, value)) if !id.is_empty() && !value.is_empty() => Ok((id.into(), value.into())),
        _ => Err("expected id=value".into()),
    }
}

/// On Windows, `npx` is `npx.cmd`, which `CreateProcess` does not find by
/// itself: resolves a bare program name through `PATH` and `PATHEXT`.
pub fn resolve_program(
    program: &str,
    path: Option<&str>,
    pathext: Option<&str>,
) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.components().count() > 1 || p.extension().is_some() {
        return None;
    }
    let exts: Vec<String> = pathext
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_lowercase())
        .collect();
    for dir in std::env::split_paths(path?) {
        for ext in &exts {
            let candidate = dir.join(format!("{program}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Rewrites the program of a plain command line for Windows; JSON configs and
/// other platforms are returned unchanged.
pub fn agent_command(command: &str) -> String {
    let path = std::env::var("PATH").ok();
    let pathext = std::env::var("PATHEXT").ok();
    agent_command_with(command, cfg!(windows), path.as_deref(), pathext.as_deref())
}

fn agent_command_with(
    command: &str,
    windows: bool,
    path: Option<&str>,
    pathext: Option<&str>,
) -> String {
    let trimmed = command.trim_start();
    if !windows || trimmed.starts_with('{') {
        return command.to_string();
    }
    let (program, rest) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
    match resolve_program(program, path, pathext) {
        Some(resolved) => {
            let resolved = resolved.to_string_lossy().replace('\\', "/");
            format!("\"{resolved}\" {rest}").trim_end().to_string()
        }
        None => command.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::parse_from(["vr-radio"]);
        assert_eq!(c.agent, DEFAULT_AGENT);
        assert_eq!(c.permissions, Permissions::Safe);
        assert_eq!(c.speaker, 3);
        assert_eq!(c.speed, 1.2);
        assert_eq!(c.hotkey, "F9");
        assert!(c.prompt.contains("{text}") && c.prompt.contains("{context}"));
        assert_eq!(c.context_commands()[0], "vr brief");
        assert_eq!(c.context_commands().len(), 4);
        assert!(c.agent_options.is_empty());
        let stt = c.stt();
        assert_eq!(stt.model, "openai/gpt-transcribe");
        assert_eq!(stt.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(stt.backend, SttBackend::Api);
        assert!(!c.tray);
        assert_eq!(stt.language.as_deref(), Some("ja"));
        assert_eq!(stt.refine_model, None);
    }

    #[test]
    fn empty_values_disable_options() {
        let c = Config::parse_from([
            "vr-radio",
            "--language",
            "",
            "--vocabulary",
            "",
            "--refine-model",
            "",
            "--stt-api-key",
            "k",
        ]);
        let stt = c.stt();
        assert_eq!(stt.language, None);
        assert_eq!(stt.prompt, None);
        assert_eq!(stt.refine_model, None);
        assert_eq!(stt.api_key.as_deref(), Some("k"));
    }

    #[test]
    fn key_comes_from_dotenv_in_the_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "VR_TEST=1\nOPENAI_API_KEY=k\n").unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let c = Config::parse_from(["vr-radio", "--cwd", &cwd, "--stt-api-key", "given"]);
        assert_eq!(c.stt().api_key.as_deref(), Some("given"));
        if std::env::var("OPENAI_API_KEY").is_err() {
            let mut c = Config::parse_from(["vr-radio", "--cwd", &cwd]);
            c.stt_api_key = None;
            c.stt_url = "https://api.openai.com/v1".into();
            assert_eq!(c.stt().api_key.as_deref(), Some("k"));
        }
    }

    #[test]
    fn context_and_agent_options() {
        let c = Config::parse_from([
            "vr-radio",
            "--context",
            "vr fuel",
            "--context",
            "vr gap",
            "--agent-option",
            "model=haiku",
            "--agent-option",
            "effort=low",
        ]);
        assert_eq!(c.context_commands(), ["vr fuel", "vr gap"]);
        assert_eq!(
            c.agent_options,
            [
                ("model".to_string(), "haiku".to_string()),
                ("effort".to_string(), "low".to_string())
            ]
        );
        let none = Config::parse_from(["vr-radio", "--context", ""]);
        assert!(none.context_commands().is_empty());
        for bad in ["model", "=x", "model="] {
            assert!(Config::try_parse_from(["vr-radio", "--agent-option", bad]).is_err());
        }
    }

    #[test]
    fn agent_cwd_is_absolute() {
        let c = Config::parse_from(["vr-radio", "--cwd", "."]);
        assert!(c.agent_cwd().is_absolute());
    }

    #[test]
    fn resolves_through_pathext() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("npx.cmd"), "").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        assert_eq!(
            resolve_program("npx", Some(&path), Some(".EXE;.CMD")),
            Some(dir.path().join("npx.cmd"))
        );
        assert_eq!(resolve_program("npx", Some(&path), Some(".EXE")), None);
        assert_eq!(resolve_program("npx.cmd", Some(&path), None), None);
        assert_eq!(resolve_program("bin/npx", Some(&path), None), None);
        // Paths and names with an extension are used as given, even when a
        // PATHEXT match would exist.
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin").join("npx.cmd"), "").unwrap();
        std::fs::write(dir.path().join("npx.cmd.cmd"), "").unwrap();
        assert_eq!(resolve_program("bin/npx", Some(&path), Some(".CMD")), None);
        assert_eq!(resolve_program("npx.cmd", Some(&path), Some(".CMD")), None);
        assert_eq!(resolve_program("npx", None, None), None);
    }

    #[test]
    fn agent_command_resolves_only_plain_commands_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("npx.cmd"), "").unwrap();
        // A program token "{" would resolve too: JSON must be left alone.
        std::fs::write(dir.path().join("{.cmd"), "").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let resolved = agent_command_with("npx -y pkg", true, Some(&path), Some(".CMD"));
        let expected = dir
            .path()
            .join("npx.cmd")
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(resolved, format!("\"{expected}\" -y pkg"));
        assert_eq!(
            agent_command_with("npx -y pkg", false, Some(&path), Some(".CMD")),
            "npx -y pkg"
        );
        let json = r#"{ "command": "npx" }"#;
        assert_eq!(
            agent_command_with(json, true, Some(&path), Some(".CMD")),
            json
        );
        assert_eq!(
            agent_command_with("missing x", true, Some(&path), Some(".CMD")),
            "missing x"
        );
        assert_eq!(agent_command(json), json);
    }
}
