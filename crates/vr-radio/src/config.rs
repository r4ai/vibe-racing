//! Command-line / environment settings.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;

use crate::decide::{Decide, Decisions};
use crate::policy::Permissions;
use crate::stt::{self, SttBackend, SttConfig};

/// Default agent: Claude Code over ACP, pinned to a release older than 7 days.
pub const DEFAULT_AGENT: &str = "npx -y @agentclientprotocol/claude-agent-acp@0.107.0";

/// Codex over ACP, pinned likewise.
pub const CODEX_AGENT: &str = "npx -y @agentclientprotocol/codex-acp@2.6.0";

/// Most accurate on spoken Japanese racing terms at under a second per
/// question, among the OpenRouter models compared in docs/architecture.md.
pub const DEFAULT_STT_MODEL: &str = "openai/gpt-transcribe";

/// Fastest of the decision models compared in docs/architecture.md, and
/// accurate enough: what it misses, the agent looks up.
pub const DEFAULT_DECIDE_MODEL: &str = "typesafe/jev-1.14";
pub const DECISIONS_URL: &str = "https://openrouter.ai/api/alpha";

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
- ラジオの設定を頼まれたら、返答の先頭に [[radio キー=値]] を書く。画面と音声には出ない。頼まれていない設定は変えない。現在値は無線に付く。例:「[[radio volume=-20]]音量下げた。」\n\
  キーは volume（0〜150、+20 や -20 も可。「うるさい」「聞こえない」は音量の依頼）、mute（on/off）、speed（0.5〜2.0、+0.1 も可）、speaker（キャラクター名やスタイル名）、proactive（on/off、自動連絡全体）、event.種類（on/off。flag position lap pit incident fuel weather session adjustment strategy）、interval（自動連絡の最短間隔、秒）、planning（裏で作戦を見直す間隔の周数、off で止める）\n\
- 話しかけ方・見張り・続く指示は、ドライバーの言葉どおりに自由に作る。できないと言わず、次のキーで組み立てる\n\
  hold=\"条件\": 成り立つ間は回答も含めて話さず、外れるのを待って話す。hold_calls=\"条件\": 自分からの連絡だけ同じ扱い。hold_max=秒: 待っても最長これで話す。quiet=\"条件\": 成り立つ間、急がない自動連絡をしない。どれも off で解除\n\
  when=\"条件\" do=\"その時することの指示\": 条件が成立した瞬間に、その指示があなたに届く。do の代わりに say=\"後ろ{gap_behind}秒\" と書くと、その文がすぐ読まれる（{値} は実際の値になる。決まった文ならこちらが速い）。every=秒 は次に知らせるまでの最短、once=on は一度だけ\n\
  note=\"…\": これからも守る指示（言い方、毎回言うこと、言わないこと）を覚える。forget=\"n1\" や \"w2\" や含まれる言葉や all: 指示と見張りを取り消す\n\
  条件には and or not ( ) < <= > >= == != + - * / % abs() min() max() と次の値が使える: {live}\n\
  例:「ブレーキ中は話すな」→ [[radio hold=\"brake > 5\"]]、「コーナー中は黙って」→ [[radio hold=\"brake > 5 or abs(lat_g) > 0.8\"]]、「あと3周静かに」（今10周目）→ [[radio quiet=\"lap < 13\"]]、「後ろが1秒切ったら教えて」→ [[radio when=\"gap_behind < 1\" say=\"後ろ{gap_behind}秒\" every=60]]、「5周ごとに燃料」→ [[radio when=\"lap % 5 == 0 and lap_pct > 5\" do=\"燃料の残り周数と過不足を言う\"]]、「順位は言わなくていい」→ [[radio note=\"順位は聞かれたときだけ言う\"]]、「無線を減らして」→ [[radio interval=60]]\n\
- 無線に付く「指示 n」は、以後のすべての返答と連絡で守る。「作戦メモ」は裏で立てた作戦。作戦を聞かれたら、付いている vr strategy の最新の値と合わせて答える\n\
- 作戦では、自分と他車のラップの落ち方（fade）、ピットの損失、燃料、戻る位置、前後の車が先に入ったときの得失（undercut / overcut / its stop gains）を根拠にする。net は同じ回数止まった後の差。数字の根拠が無いことは言わない。止まる周は plans と fuel window の中から選び、calls に無いアンダーカットを自分から勧めない\n\
- ときどき「出来事」がドライバーの無線の代わりに届く。そのときだけ、今知らせるべきことを 1 文で言う。不要なら - だけを返す。「見張り」の出来事はドライバーに頼まれたものなので、必ず指示のとおりに言う\n\
- ときどき「作戦会議」が届く。読み上げない裏の仕事で、書き方はそこにある\n\
race-engineer スキルがあれば今読んで、データの読み方を把握しておくこと。コマンドはまだ実行しない。終わったら「了解」とだけ返す。";

pub const DEFAULT_PROMPT: &str = "\
無線:「{text}」\n\n\
{context}\n\n\
{radio}\n\n\
結論と数字から、日本語 1〜2 文で即答。";

/// Sent when something happened that the driver may have to know.
pub const CALL_PROMPT: &str = "\
出来事（ドライバーの無線ではない）: {text}\n\n\
{context}\n\n\
{radio}\n\n\
ドライバーが今知るべきことだけを、日本語の短い 1 文で言う。前に伝えたこと、本人に見えていること、\
行動が変わらないことなら - だけを返す。「見張り」は頼まれたことなので、指示のとおりに必ず言う（「作戦で決めた見張り」は自分の覚え書きで、新しく言うことがあるときだけ言う）。\
作戦の出来事（plan・undercut・overcut・box・answer・fuel）は計算で確かめた結果。期限のあるもの（best plan: stop at the end of this lap、answer within、fuel: stop by）は、何をするかと決め手の数字を必ず言う。ほかは作戦が変わるときだけ言う。\
「指示 n」を守る。ラジオの設定は変えない。コマンドは使わない。";

/// Sent from time to time, so that the engineer has a plan before it is asked.
pub const PLAN_PROMPT: &str = "\
作戦会議（裏の仕事。ドライバーの無線ではない）。{text}\n\n\
{context}\n\n\
{radio}\n\n\
このデータで作戦を立て直し、次のとおりに返す。コマンドは使わない。\n\
1. [[radio plan=\"…\"]] に作戦メモを 120 字以内で書く: いつ止まるか（第一案と、外れたときの第二案）、決め手の数字（落ち方、ピットの損失、燃料の窓、戻る位置）、警戒する車と狙う車（番号・名前・net の差・何周以内にどうするか）。データに無いことは書かない\n\
2. 見張りが要るなら 4 つまで [[radio when=\"条件\" do=\"…\"]] を書く。前回の作戦の見張りは消える。ドライバーの見張りと重ねない\n\
3. ドライバーに今言うべきこと（初めての作戦、作戦の変更、次の 2 周の間に決めること、アンダーカットの危険や好機）があれば、その後に短い 1 文で言う。前に伝えたこと、まだ先のことなら - だけを書く";

/// Sent when the agent wrote a setting that cannot be read.
pub const FIX_PROMPT: &str = "\
直前の返答の [[radio …]] が使えなかった: {text}\n\
使えなかったものだけを、ドライバーの頼みに合う正しい [[radio …]] に書き直して返す。使えたものは繰り返さない。言葉は付けない。作れないときは - だけを返す。";

/// What conditions may read (`vr live`), as told to the agent.
pub const LIVE: &str = "\
speed(km/h) throttle brake(0〜100) steer(度、左が正) lat_g(左が正) long_g(減速が負) gear rpm abs(0/1) \
lap lap_pct(周の何%) lap_m(m) lap_time last_lap best_lap delta_best(秒、負が速い) \
position cars laps_left time_left(秒) session_time(秒) \
gap_ahead gap_behind(秒、順位が前後の車) trend_ahead trend_behind(秒/周、負は縮む) \
near_ahead near_behind(秒、コース上で前後の車) lapping_behind(秒、周回上の後ろの車) alongside(横に車 0/1) \
fuel(L) fuel_laps(周) fuel_spare(完走時に余るL、負は不足) stops tyre_age(周) stop_in(予定のピットまでの周、0はこの周) \
on_pit in_stall on_track off_track(0/1) incidents water oil(℃) track_temp rain(0〜100) p2p \
yellow caution blue white racing(0/1)";

/// Whether `vr live` tells a value of that name.
pub fn live_key(name: &str) -> bool {
    LIVE.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|k| !k.is_empty() && k == name)
}

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

    /// Also fetch data for what the question is about (setup, tyres, past
    /// sessions, a car number), told from its words.
    #[arg(long, env = "VR_RADIO_TOPICS", default_value_t = true, action = clap::ArgAction::Set)]
    pub topics: bool,

    /// OpenRouter decision model that tells what a question is about when its
    /// words do not. Empty, without an OpenRouter key or with the recognizer
    /// of Windows: only the words count.
    #[arg(long, env = "VR_RADIO_DECIDE_MODEL", default_value = DEFAULT_DECIDE_MODEL)]
    pub decide_model: String,

    /// Seconds without an answer after which the driver is told to wait; told
    /// at once when the agent looks something up. 0: never told.
    #[arg(long, env = "VR_RADIO_STANDBY", default_value_t = 4.0, value_parser = seconds)]
    pub standby: f64,

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

    /// Loudness of the voice in percent (over 100: amplified).
    #[arg(long, env = "VR_RADIO_VOLUME", default_value_t = 100, value_parser = clap::value_parser!(u8).range(0..=150))]
    pub volume: u8,

    /// Starts the VOICEVOX engine when it does not answer at a local
    /// `VOICEVOX_URL`: its command, empty to leave it alone. Default: the
    /// installed one.
    #[arg(long, env = "VR_RADIO_VOICEVOX")]
    pub voicevox: Option<String>,

    /// Kept running with the window: collects the race. Empty: nothing.
    #[arg(long, env = "VR_RADIO_SERVE", default_value = "vr serve --if-needed")]
    pub serve: String,

    /// Tells the race events the engineer calls about. Empty: no calls.
    #[arg(long, env = "VR_RADIO_WATCH", default_value = "vr watch -f json")]
    pub watch: String,

    /// Tells the values that conditions are checked against. Empty: none are.
    #[arg(long, env = "VR_RADIO_LIVE", default_value = "vr live --hz 10")]
    pub live: String,

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

    /// Who tells what a question is about. Nobody when the driver keeps the
    /// speech on this computer.
    pub fn decide(&self) -> Option<Arc<dyn Decide>> {
        if self.decide_model.is_empty() || self.stt == SttBackend::Windows {
            return None;
        }
        let key = self
            .stt()
            .api_key
            .filter(|_| stt::is_openrouter(&self.stt_url))?;
        Some(Arc::new(Decisions::new(
            DECISIONS_URL,
            key,
            &self.decide_model,
        )))
    }

    pub fn session(&self) -> crate::agent::SessionConfig {
        crate::agent::SessionConfig {
            cwd: self.agent_cwd(),
            permissions: self.permissions,
            options: self.agent_options.clone(),
            primer: Some(self.primer.replace("{live}", LIVE)).filter(|p| !p.trim().is_empty()),
        }
    }

    /// The speech rate in percent.
    pub fn speed_percent(&self) -> u16 {
        (self.speed * 100.0).round().clamp(0.0, 1000.0) as u16
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

fn seconds(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if (0.0..=600.0).contains(&v) => Ok(v),
        _ => Err("expected seconds, 0 to 600".into()),
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

/// Codex without its plugins: a third fewer tokens in every turn, and no tools
/// the radio has no use for. The user's own configuration stays as it is.
const CODEX_CONFIG: &str = r#"{"features":{"apps":false,"plugins":false}}"#;

/// Adds `CODEX_CONFIG` to a plain Codex command, unless the user has their own.
fn with_codex_config(command: String, own: bool) -> String {
    if own || command.starts_with('{') || !command.contains("codex-acp") {
        return command;
    }
    format!("CODEX_CONFIG='{CODEX_CONFIG}' {command}")
}

/// Rewrites the program of a plain command line for Windows; JSON configs and
/// other platforms are returned unchanged.
pub fn agent_command(command: &str) -> String {
    let path = std::env::var("PATH").ok();
    let pathext = std::env::var("PATHEXT").ok();
    let resolved = agent_command_with(command, cfg!(windows), path.as_deref(), pathext.as_deref());
    let own = command.contains("CODEX_CONFIG") || std::env::var_os("CODEX_CONFIG").is_some();
    with_codex_config(resolved, own)
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
        assert_eq!(c.speed_percent(), 120);
        assert_eq!(c.volume, 100);
        assert_eq!(c.serve, "vr serve --if-needed");
        assert_eq!(c.watch, "vr watch -f json");
        assert_eq!(c.live, "vr live --hz 10");
        let primer = c.session().primer.unwrap();
        assert!(primer.contains("gap_behind(秒") && !primer.contains("{live}"));
        assert!(live_key("gap_behind") && live_key("lap_pct") && live_key("racing"));
        assert!(!live_key("gap") && !live_key("秒") && !live_key(""));
        for p in [PLAN_PROMPT, CALL_PROMPT] {
            assert!(p.contains("{text}") && p.contains("{context}") && p.contains("{radio}"));
        }
        assert_eq!(c.voicevox, None);
        assert!(c.prompt.contains("{radio}") && c.primer.contains("[[radio volume=-20]]"));
        assert!(CALL_PROMPT.contains("{text}") && CALL_PROMPT.contains("{context}"));
        assert_eq!(c.hotkey, "F9");
        assert!(c.prompt.contains("{text}") && c.prompt.contains("{context}"));
        assert_eq!(c.context_commands()[0], "vr brief");
        assert_eq!(c.context_commands().len(), 4);
        assert!(c.agent_options.is_empty());
        assert!(c.topics);
        assert_eq!(c.decide_model, "typesafe/jev-1.14");
        assert_eq!(c.standby, 4.0);
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
            "--serve",
            "",
            "--watch",
            "",
            "--voicevox",
            "",
        ]);
        assert_eq!((c.serve.as_str(), c.watch.as_str()), ("", ""));
        assert_eq!(c.voicevox.as_deref(), Some(""));
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
    fn decisions_need_a_key_of_openrouter() {
        let with = |args: &[&str]| {
            let all = ["vr-radio", "--stt-api-key", "k"];
            Config::parse_from(all.iter().chain(args))
                .decide()
                .is_some()
        };
        assert!(with(&[]));
        assert!(!with(&["--decide-model", ""]));
        assert!(!with(&["--stt", "windows"]));
        assert!(!with(&["--stt-url", "https://api.openai.com/v1"]));
    }

    #[test]
    fn standby_is_in_seconds() {
        let standby = |v| Config::try_parse_from(["vr-radio", "--standby", v]).map(|c| c.standby);
        assert_eq!(standby("0").unwrap(), 0.0);
        assert_eq!(standby("600").unwrap(), 600.0);
        for bad in ["600.1", "-1", "NaN", "x"] {
            assert!(standby(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn volume_is_limited() {
        let volume = |v| Config::try_parse_from(["vr-radio", "--volume", v]).map(|c| c.volume);
        assert_eq!(volume("0").unwrap(), 0);
        assert_eq!(volume("150").unwrap(), 150);
        assert!(volume("151").is_err());
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
    fn codex_runs_without_plugins() {
        let codex = with_codex_config("npx codex-acp".into(), false);
        assert_eq!(
            codex,
            r#"CODEX_CONFIG='{"features":{"apps":false,"plugins":false}}' npx codex-acp"#
        );
        let acp: agent_client_protocol::AcpAgent = codex.parse().unwrap();
        assert!(format!("{acp:?}").contains("CODEX_CONFIG"));
        for (command, own) in [
            ("npx codex-acp", true),
            ("npx claude-agent-acp", false),
            (r#"{"command":"codex-acp"}"#, false),
        ] {
            assert_eq!(with_codex_config(command.into(), own), command);
        }
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
