//! The window (iced + shadcn-iced): conversation, latency, settings.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use agent_client_protocol::AcpAgent;
use clap::Parser;
use iced::futures::{SinkExt, Stream};
use iced::{Subscription, Task, window};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::agent::{AgentLink, AgentOption, OptionKind, ToolState, Turn, merged};
use crate::audio::{LateMic, LateSpeakers, Output, Recorder};
use crate::child::{Retry, Supervised};
use crate::config::{self, AgentOptions, Config};
use crate::context::{Commands, ContextSource};
use crate::directive::Setting;
use crate::engine::{self, Command, Deps, Event, Outcome};
use crate::hotkey::{self, Captured};
use crate::live::Feed;
use crate::resident::{self, ResidentEvent};
use crate::rules::{Rules, Scope};
use crate::settings::Settings;
use crate::stt::{self, SttBackend};
use crate::voicevox::{self, Synth, Voice, Voicevox};
use crate::watch::{Follow, Group, Timing};
use crate::wheel::{self, ButtonEvent};

/// Settings for the engine subscription (`Subscription::run` takes a `fn`).
pub static CONFIG: OnceLock<Config> = OnceLock::new();
/// Result of registering the global push-to-talk key.
pub static HOTKEY: OnceLock<Result<String, String>> = OnceLock::new();
/// Saved choices and the file they are kept in.
pub static SETTINGS: OnceLock<(Settings, Option<PathBuf>)> = OnceLock::new();

mod view;

const SCROLLER: &str = "conversation";

#[derive(Debug, Clone)]
pub enum Message {
    EngineReady(UnboundedSender<Command>),
    Engine(Event),
    Voices(Result<Vec<Voice>, String>),
    Hotkey(bool),
    TogglePtt,
    Draft(String),
    Send,
    Cancel,
    Speaker(Voice),
    /// A character was picked: use its first style.
    Character(String),
    Muted(bool),
    /// Percent.
    Volume(u8),
    /// Speech rate in percent.
    Speed(u16),
    /// A slider was let go.
    SaveSettings,
    /// Whether the engineer calls by itself.
    Proactive(bool),
    ProactiveGroup(Group, bool),
    /// Seconds between calls that can wait.
    Interval(u16),
    Hold(Scope, HoldChoice),
    /// Edits a condition of the driver's own.
    HoldDraft(Scope, String),
    /// Laps between looks at the strategy.
    Planning(PlanChoice),
    NoteDraft(String),
    AddNote,
    /// Drops a standing order or a watch, by its number.
    DropNote(usize),
    DropWatch(usize),
    Scrolled(bool),
    ScrollToEnd,
    Settings(bool),
    /// Shows one page of the settings.
    SettingsPage(SettingsPage),
    /// Plays a sample of the selected voice.
    Preview,
    AgentChoice(AgentChoice),
    /// Model, effort or speed of the agent.
    AgentOption(OptionChoice),
    AgentDraft(String),
    /// Starts the chosen agent in place of the current one.
    Connect,
    Stt(SttChoice),
    /// Waits for the key or the button to assign to push-to-talk.
    Capture(Capture),
    Captured(Captured),
    Wheel(ButtonEvent),
    ClearWheel,
    CloseToTray(bool),
    StartWithWindows(bool),
    Resident(ResidentEvent),
    DismissError,
    /// Caption buttons drawn by the app (Windows). Maximize is native
    /// (`HTMAXBUTTON`), see `chrome`.
    Minimize,
    Close,
    Chrome(crate::chrome::ChromeEvent),
}

/// When to keep quiet, for those who do not write conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldChoice {
    Never,
    Brakes,
    Corners,
    Straights,
    Custom,
}

impl HoldChoice {
    const ALL: [Self; 5] = [
        Self::Never,
        Self::Brakes,
        Self::Corners,
        Self::Straights,
        Self::Custom,
    ];

    /// The condition; `None` where there is none to choose.
    fn condition(self) -> Option<&'static str> {
        match self {
            Self::Never | Self::Custom => None,
            Self::Brakes => Some(crate::rules::HOLD_CALLS),
            Self::Corners => Some("brake > 10 or abs(lat_g) > 0.8"),
            Self::Straights => Some("throttle < 95 or abs(lat_g) > 0.5"),
        }
    }

    fn of(condition: Option<&str>) -> Self {
        let Some(c) = condition else {
            return Self::Never;
        };
        Self::ALL
            .into_iter()
            .find(|h| h.condition() == Some(c))
            .unwrap_or(Self::Custom)
    }
}

impl std::fmt::Display for HoldChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Never => "いつでも話す",
            Self::Brakes => "ブレーキ中は待つ",
            Self::Corners => "ブレーキ中とコーナーは待つ",
            Self::Straights => "ストレートだけ",
            Self::Custom => "条件を書く",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanChoice(pub u16);

impl PlanChoice {
    const ALL: [u16; 6] = [0, 1, 2, 3, 5, 10];
}

impl std::fmt::Display for PlanChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            0 => f.write_str("しない"),
            n => write!(f, "{n}周ごと"),
        }
    }
}

/// Agents that can be picked without knowing their command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentChoice {
    Claude,
    Codex,
    Custom,
}

impl AgentChoice {
    const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Custom];

    fn command(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some(config::DEFAULT_AGENT),
            Self::Codex => Some(config::CODEX_AGENT),
            Self::Custom => None,
        }
    }

    fn of(command: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|c| c.command() == Some(command))
            .unwrap_or(Self::Custom)
    }
}

impl std::fmt::Display for AgentChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Custom => "カスタム",
        })
    }
}

/// A value of an option of the agent, as the settings list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionChoice {
    pub id: String,
    pub value: String,
    label: String,
}

impl OptionChoice {
    fn all(o: &AgentOption) -> Vec<Self> {
        o.choices
            .iter()
            .map(|(value, name)| Self {
                id: o.id.clone(),
                value: value.clone(),
                label: match (o.kind, value.as_str()) {
                    (OptionKind::Speed, "on") => "高速（消費が増える）".into(),
                    (OptionKind::Speed, "off") => "標準".into(),
                    _ => name.clone(),
                },
            })
            .collect()
    }
}

impl std::fmt::Display for OptionChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// Who transcribes, as one list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttChoice {
    Windows,
    Model(String),
}

impl std::fmt::Display for SttChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Windows => f.write_str("Windows 標準（オフライン）"),
            Self::Model(m) => f.write_str(m),
        }
    }
}

/// A page of the settings, picked in their sidebar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsPage {
    #[default]
    Talk,
    Voice,
    Calls,
    Answers,
    Orders,
    Agent,
    App,
}

impl SettingsPage {
    pub const ALL: [Self; 7] = [
        Self::Talk,
        Self::Voice,
        Self::Calls,
        Self::Answers,
        Self::Orders,
        Self::Agent,
        Self::App,
    ];
}

/// What the settings wait for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Capture {
    #[default]
    None,
    Key,
    Wheel,
}

/// What the radio is doing right now, for the status at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Recording,
    Transcribing,
    Researching,
    Answering,
    Speaking,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Phase::Idle => "待機中",
            Phase::Recording => "録音中",
            Phase::Transcribing => "聞き取り中",
            Phase::Researching => "調べています",
            Phase::Answering => "回答中",
            Phase::Speaking => "読み上げ中",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Phase::Idle => "ボタンを押して話しかけてください",
            Phase::Recording => "離すと送信します",
            Phase::Transcribing => "音声を文字にしています",
            Phase::Researching => "エージェントがデータを確認中",
            Phase::Answering | Phase::Speaking => "話し始めると中断します",
        }
    }

    /// Something is running that 止める would stop.
    fn busy(self) -> bool {
        !matches!(self, Phase::Idle | Phase::Recording)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentStatus {
    Starting,
    Ready(String),
    Closed(String),
}

/// Whether a part of the radio can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Readiness {
    Ready,
    Starting,
    Failed,
    Off,
}

/// The voices of VOICEVOX.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum VoiceState {
    #[default]
    Starting,
    Ready,
    Failed,
}

#[derive(Debug, Default)]
struct TurnView {
    id: Turn,
    /// The engineer called by itself: `heard` is what it is about.
    call: bool,
    heard: Option<String>,
    reply: String,
    tools: Vec<(String, String, ToolState)>,
    stt_ms: Option<u64>,
    token_ms: Option<u64>,
    audio_ms: Option<u64>,
    done: Option<(Outcome, u64)>,
}

pub struct App {
    engine: Option<UnboundedSender<Command>>,
    agent: AgentStatus,
    recording: bool,
    speaking: bool,
    level: f32,
    turns: Vec<TurnView>,
    errors: Vec<String>,
    draft: String,
    voices: Vec<Voice>,
    voice: Option<Voice>,
    muted: bool,
    at_end: bool,
    settings_open: bool,
    settings_page: SettingsPage,
    chrome: crate::chrome::ChromeState,
    /// Current settings, starting from the command line and the saved file.
    config: Config,
    settings: Settings,
    settings_path: Option<PathBuf>,
    agent_choice: AgentChoice,
    agent_draft: String,
    /// What can be chosen about the agent.
    offered: Vec<AgentOption>,
    hotkey: Option<Result<String, String>>,
    capture: Capture,
    capture_hint: Option<&'static str>,
    /// What is being spoken is a voice sample.
    previewing: bool,
    /// The tray icon is there, so the window may be hidden.
    tray: bool,
    starts_with_windows: bool,
    has_stt_key: bool,
    voice_state: VoiceState,
    microphone: Option<bool>,
    speakers: Option<bool>,
    telemetry: Option<bool>,
    note_draft: String,
    /// Conditions being written, where they cannot be read yet too.
    hold_drafts: [Option<String>; 2],
    /// The engineer's notes on the strategy.
    plan: Option<String>,
}

impl App {
    pub fn new() -> Self {
        let config = CONFIG
            .get()
            .cloned()
            .unwrap_or_else(|| Config::parse_from(["vr-radio"]));
        let (settings, settings_path) = SETTINGS.get().cloned().unwrap_or_default();
        Self {
            agent_choice: AgentChoice::of(&config.agent),
            agent_draft: config.agent.clone(),
            offered: Vec::new(),
            hotkey: HOTKEY.get().cloned(),
            capture: Capture::None,
            capture_hint: None,
            previewing: false,
            tray: false,
            starts_with_windows: false,
            has_stt_key: config.stt().api_key.is_some(),
            voice_state: VoiceState::Starting,
            microphone: None,
            speakers: None,
            telemetry: None,
            note_draft: String::new(),
            hold_drafts: [None, None],
            plan: None,
            config,
            settings,
            settings_path,
            engine: None,
            agent: AgentStatus::Starting,
            recording: false,
            speaking: false,
            level: 0.0,
            turns: Vec::new(),
            errors: Vec::new(),
            draft: String::new(),
            voices: Vec::new(),
            voice: None,
            muted: false,
            at_end: true,
            settings_open: false,
            settings_page: SettingsPage::default(),
            chrome: Default::default(),
        }
    }

    /// The app at startup: also installs the native window chrome.
    pub fn boot() -> (Self, Task<Message>) {
        let install = window::latest()
            .and_then(|id| window::run(id, |w| crate::chrome::install_native_chrome(w)))
            .discard();
        let mut app = Self::new();
        app.tray = resident::install_tray();
        app.starts_with_windows = resident::starts_with_windows();
        // Never leave the radio running with no way to reach it.
        let show = if app.config.tray && !app.tray {
            show_window()
        } else {
            Task::none()
        };
        (app, Task::batch([install, show]))
    }

    /// A conversation in progress, for snapshots and screenshots.
    pub fn sample() -> Self {
        let mut app = Self::new();
        app.agent = AgentStatus::Ready("Codex".into());
        let option = |id: &str, kind, value: &str, choices: &[(&str, &str)]| AgentOption {
            id: id.into(),
            kind,
            value: value.into(),
            choices: choices
                .iter()
                .map(|(v, name)| (v.to_string(), name.to_string()))
                .collect(),
        };
        app.offered = vec![
            option(
                "model",
                OptionKind::Model,
                "terra",
                &[("sol", "6.2 Sol"), ("terra", "6.1 Terra")],
            ),
            option(
                "reasoning_effort",
                OptionKind::Effort,
                "low",
                &[("low", "Low"), ("medium", "Medium")],
            ),
            option(
                "fast-mode",
                OptionKind::Speed,
                "off",
                &[("off", "Off"), ("on", "On")],
            ),
        ];
        app.voices = ["ノーマル", "あまあま", "ツンツン", "セクシー"]
            .iter()
            .enumerate()
            .map(|(i, style)| Voice {
                id: i as u32 + 1,
                speaker: "ずんだもん".into(),
                style: (*style).into(),
            })
            .chain(
                ["ノーマル", "あまあま"]
                    .iter()
                    .enumerate()
                    .map(|(i, style)| Voice {
                        id: i as u32 + 10,
                        speaker: "四国めたん".into(),
                        style: (*style).into(),
                    }),
            )
            .collect();
        app.voice = app.voices.first().cloned();
        app.voice_state = VoiceState::Ready;
        app.microphone = Some(true);
        app.speakers = Some(true);
        app.telemetry = Some(true);
        for e in [
            Event::TurnStarted { turn: 1 },
            Event::Transcript {
                turn: 1,
                text: "前との差は？".into(),
                ms: 420,
            },
            Event::Tool {
                turn: 1,
                id: "a".into(),
                title: Some("vr brief".into()),
                state: ToolState::Done,
            },
            Event::FirstToken { turn: 1, ms: 1800 },
            Event::Chunk {
                turn: 1,
                text: "前の Ito まで 0.6。毎周 0.4 詰めてる、2 周で射程内。".into(),
            },
            Event::FirstAudio { turn: 1, ms: 2100 },
            Event::TurnDone {
                turn: 1,
                outcome: Outcome::Answered,
                ms: 3200,
            },
            Event::CallStarted {
                turn: 3,
                about: "yellow flag".into(),
            },
            Event::Chunk {
                turn: 3,
                text: "第2セクター、ローカルイエロー。前の車に気をつけて。".into(),
            },
            Event::TurnDone {
                turn: 3,
                outcome: Outcome::Answered,
                ms: 1900,
            },
            Event::Plan(Some(
                "15周目に入る。9号車 Hart が2.0秒後ろ、先に入られたら2周以内に応じる。".into(),
            )),
            Event::TurnStarted { turn: 2 },
            Event::Transcript {
                turn: 2,
                text: "燃料あと何周？".into(),
                ms: 380,
            },
            Event::Tool {
                turn: 2,
                id: "b".into(),
                title: Some("vr fuel".into()),
                state: ToolState::Running,
            },
        ] {
            app.apply(e);
        }
        app
    }

    /// A key and a wheel button, for snapshots.
    pub fn assign_sample(&mut self) {
        self.hotkey = Some(Ok("F9".into()));
        self.settings.wheel = Some(wheel::Binding {
            device: "VID_346E&PID_0006".into(),
            name: "MOZA R9".into(),
            button: 40,
        });
    }

    /// Feeds an engine event, as the running app does.
    pub fn handle(&mut self, event: Event) {
        self.apply(event);
    }

    fn phase(&self) -> Phase {
        if self.recording {
            return Phase::Recording;
        }
        if self.speaking {
            return Phase::Speaking;
        }
        match self.turns.last() {
            Some(t) if t.done.is_none() && t.heard.is_none() => Phase::Transcribing,
            Some(t) if t.done.is_none() && t.reply.is_empty() => Phase::Researching,
            Some(t) if t.done.is_none() => Phase::Answering,
            _ => Phase::Idle,
        }
    }

    /// What can be used, in the order of the list in the window.
    fn readiness(&self) -> [(&'static str, Readiness); 4] {
        let device = |d: Option<bool>| match d {
            Some(true) => Readiness::Ready,
            Some(false) => Readiness::Failed,
            None => Readiness::Starting,
        };
        let agent = match self.agent {
            AgentStatus::Starting => Readiness::Starting,
            AgentStatus::Ready(_) => Readiness::Ready,
            AgentStatus::Closed(_) => Readiness::Failed,
        };
        let voice = match (self.voice_state, device(self.speakers)) {
            (VoiceState::Failed, _) | (_, Readiness::Failed) => Readiness::Failed,
            (VoiceState::Ready, Readiness::Ready) => Readiness::Ready,
            _ => Readiness::Starting,
        };
        let telemetry = match self.telemetry {
            _ if self.config.watch.trim().is_empty() => Readiness::Off,
            Some(true) => Readiness::Ready,
            _ => Readiness::Starting,
        };
        [
            ("エージェント", agent),
            ("音声", voice),
            ("テレメトリ", telemetry),
            ("マイク", device(self.microphone)),
        ]
    }

    fn set_voice(&mut self, v: Voice) {
        self.send(Command::SetSpeaker(v.id));
        self.settings.speaker = Some(v.id);
        self.config.speaker = v.id;
        self.voice = Some(v);
        self.save();
    }

    /// Follows a setting the driver had changed by voice.
    fn changed(&mut self, s: Setting) {
        if let Some(t) = self.turns.last_mut() {
            t.tools
                .push((format!("setting:{s}"), s.to_string(), ToolState::Done));
        }
        match s {
            Setting::Volume(v) => {
                self.config.volume = v;
                self.settings.volume = Some(v);
            }
            Setting::Speed(v) => self.speed(v),
            // Not kept: the radio must not start silent.
            Setting::Muted(m) => self.muted = m,
            Setting::Speaker(name) => {
                match find_voice(&self.voices, self.voice.as_ref(), &name).cloned() {
                    Some(v) => self.set_voice(v),
                    None => self.error(format!("話者が見つかりません: {name}")),
                }
            }
            Setting::Proactive(on) => self.settings.proactive.enabled = on,
            Setting::Event(group, on) => self.settings.proactive.set(group, on),
            // The rules come as a whole.
            Setting::Interval(_)
            | Setting::Hold(..)
            | Setting::HoldMax(_)
            | Setting::Note(_)
            | Setting::Forgot(_)
            | Setting::Watch(_)
            | Setting::Planning(_) => return,
        }
        self.save();
    }

    /// Changes the rules from the settings sheet.
    fn rules(&mut self, change: impl FnOnce(&mut Rules)) {
        change(&mut self.settings.rules);
        self.send(Command::SetRules(self.settings.rules.clone()));
    }

    fn speed(&mut self, percent: u16) {
        self.config.speed = f64::from(percent) / 100.0;
        self.settings.speed = Some(self.config.speed);
    }

    fn hotkey(&self) -> Option<&str> {
        self.hotkey.as_ref()?.as_deref().ok()
    }

    fn error(&mut self, e: String) {
        self.errors.push(e);
        let excess = self.errors.len().saturating_sub(3);
        self.errors.drain(..excess);
    }

    fn save(&mut self) {
        if let Some(path) = &self.settings_path
            && let Err(e) = (Settings {
                // What the engineer set for this race is not a setting.
                rules: self.settings.rules.lasting(),
                ..self.settings.clone()
            })
            .save(path)
        {
            self.error(format!("設定を保存できません: {e:#}"));
        }
    }

    /// Push-to-talk, unless the key or button is being assigned.
    fn ptt(&self, down: bool) {
        if self.capture == Capture::None {
            self.send(if down {
                Command::PttDown
            } else {
                Command::PttUp
            });
        }
    }

    fn set_capture(&mut self, capture: Capture) {
        if self.capture == capture {
            return;
        }
        // A registered key never reaches the window: released while waiting
        // for one, so that the same key can be pressed again.
        if capture == Capture::Key {
            hotkey::unbind();
        } else if self.capture == Capture::Key
            && let Some(Ok(key)) = &self.hotkey
        {
            self.hotkey = Some(hotkey::bind(key));
        }
        self.capture = capture;
        self.capture_hint = None;
    }

    fn assign_key(&mut self, key: String) {
        match hotkey::bind(&key) {
            Ok(key) => {
                self.hotkey = Some(Ok(key.clone()));
                self.settings.hotkey = Some(key);
                self.capture = Capture::None;
                self.save();
            }
            Err(e) => {
                // Back to the previous key.
                self.set_capture(Capture::None);
                self.error(format!("キーを割り当てられません: {e}"));
            }
        }
    }

    /// Hides to the tray when asked to stay resident, else quits.
    fn close(&self) -> Task<Message> {
        if self.settings.close_to_tray && self.tray {
            window::latest().and_then(|id| window::set_mode(id, window::Mode::Hidden))
        } else {
            iced::exit()
        }
    }

    fn stt_choice(&self) -> SttChoice {
        match self.config.stt {
            SttBackend::Windows => SttChoice::Windows,
            SttBackend::Api => SttChoice::Model(self.config.stt_model.clone()),
        }
    }

    /// Windows (where it exists), the current model and, on OpenRouter, the
    /// models compared for this use.
    fn stt_choices(&self) -> Vec<SttChoice> {
        let mut models = vec![];
        if stt::is_openrouter(&self.config.stt_url) {
            models.extend(config::STT_MODELS.iter().map(|m| m.to_string()));
        }
        if !models.contains(&self.config.stt_model) {
            models.insert(0, self.config.stt_model.clone());
        }
        let windows =
            (cfg!(windows) || self.config.stt == SttBackend::Windows).then_some(SttChoice::Windows);
        models
            .into_iter()
            .map(SttChoice::Model)
            .chain(windows)
            .collect()
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.engine {
            let _ = tx.send(cmd);
        }
    }

    fn turn(&mut self, id: Turn) -> Option<&mut TurnView> {
        self.turns.iter_mut().rev().find(|t| t.id == id)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::EngineReady(tx) => self.engine = Some(tx),
            Message::Engine(event) => {
                self.apply(event);
                if self.at_end {
                    return shadcn_iced::message_scroller::scroll_to_end(SCROLLER);
                }
            }
            Message::Voices(Ok(voices)) => {
                let id = self.config.speaker;
                self.voice = voices.iter().find(|v| v.id == id).cloned();
                self.voices = voices;
                self.voice_state = VoiceState::Ready;
                // It was not there yet to be warmed up.
                self.send(Command::SetSpeaker(id));
            }
            Message::Voices(Err(e)) => {
                self.voice_state = VoiceState::Failed;
                self.error(e);
            }
            Message::Hotkey(down) => self.ptt(down),
            Message::TogglePtt => self.send(if self.recording {
                Command::PttUp
            } else {
                Command::PttDown
            }),
            Message::Draft(d) => self.draft = d,
            Message::Send => {
                let text = std::mem::take(&mut self.draft);
                self.send(Command::Ask(text));
            }
            Message::Cancel => self.send(Command::Cancel),
            Message::Character(name) => {
                if let Some(v) = self.voices.iter().find(|v| v.speaker == name).cloned() {
                    return self.update(Message::Speaker(v));
                }
            }
            Message::Speaker(v) => {
                let id = v.id;
                self.set_voice(v);
                // Hear it at once, but not over an answer.
                if self.phase() == Phase::Idle || self.previewing {
                    self.previewing = true;
                    self.send(Command::Preview(id));
                }
            }
            Message::Muted(m) => {
                self.muted = m;
                self.send(Command::SetMuted(m));
            }
            Message::Volume(v) => {
                self.config.volume = v;
                self.settings.volume = Some(v);
                self.send(Command::SetVolume(v));
            }
            Message::Speed(v) => {
                self.speed(v);
                self.send(Command::SetSpeed(v));
            }
            Message::SaveSettings => self.save(),
            Message::Proactive(on) => {
                self.settings.proactive.enabled = on;
                self.send(Command::SetProactive(self.settings.proactive));
                self.save();
            }
            Message::ProactiveGroup(group, on) => {
                self.settings.proactive.set(group, on);
                self.send(Command::SetProactive(self.settings.proactive));
                self.save();
            }
            Message::Interval(s) => self.rules(|r| r.interval_s = s),
            Message::Hold(scope, choice) => {
                let draft = &mut self.hold_drafts[usize::from(scope == Scope::Calls)];
                *draft = None;
                if choice == HoldChoice::Custom {
                    let now = self.settings.rules.hold(scope);
                    *draft = Some(now.unwrap_or_default().to_string());
                } else {
                    let when = choice.condition().map(str::to_string);
                    self.rules(|r| r.set_hold(scope, when));
                    self.save();
                }
            }
            Message::HoldDraft(scope, text) => {
                if text.parse::<crate::expr::Expr>().is_ok() {
                    let when = Some(text.clone());
                    self.rules(|r| r.set_hold(scope, when));
                    self.save();
                }
                self.hold_drafts[usize::from(scope == Scope::Calls)] = Some(text);
            }
            Message::Planning(n) => {
                self.rules(|r| r.planning = n.0);
                self.save();
            }
            Message::NoteDraft(text) => self.note_draft = text,
            Message::AddNote => {
                let note = std::mem::take(&mut self.note_draft);
                if !note.trim().is_empty() {
                    self.rules(|r| r.note(&note));
                    self.save();
                }
            }
            Message::DropNote(i) => {
                self.rules(|r| {
                    r.forget(&format!("n{}", i + 1));
                });
                self.save();
            }
            Message::DropWatch(i) => {
                self.rules(|r| {
                    r.forget(&format!("w{}", i + 1));
                });
                self.save();
            }
            Message::Settings(open) => {
                self.hold_drafts = [None, None];
                self.settings_open = open;
                self.set_capture(Capture::None);
            }
            Message::SettingsPage(page) => {
                self.hold_drafts = [None, None];
                self.settings_page = page;
                self.set_capture(Capture::None);
            }
            Message::Preview => {
                if let Some(v) = &self.voice {
                    self.send(Command::Preview(v.id));
                    self.previewing = true;
                }
            }
            Message::AgentChoice(choice) => {
                self.agent_choice = choice;
                if let Some(command) = choice.command() {
                    self.agent_draft = command.into();
                }
            }
            Message::AgentOption(c) => {
                let agent = self.config.agent.clone();
                let saved = self
                    .settings
                    .agent_options
                    .entry(agent.clone())
                    .or_default();
                saved.radio = merged(&saved.radio, &[(c.id.clone(), c.value.clone())]);
                self.config
                    .saved_options
                    .clone_from(&self.settings.agent_options);
                if let Some(o) = self.offered.iter_mut().find(|o| o.id == c.id) {
                    // Until the agent says so itself.
                    o.value = c.value;
                }
                self.save();
                let options = self.config.options_of(&agent);
                self.send(Command::SetAgentOptions(options));
            }
            Message::AgentDraft(d) => self.agent_draft = d,
            Message::Connect => {
                let command = self.agent_draft.trim().to_string();
                if !command.is_empty() && self.engine.is_some() {
                    self.agent = AgentStatus::Starting;
                    self.offered.clear();
                    self.config.agent = command.clone();
                    self.settings.agent = Some(command.clone());
                    self.save();
                    let options = self.config.options_of(&command);
                    self.send(Command::SetAgent(command, options));
                }
            }
            Message::Stt(choice) => {
                self.config.stt = match choice {
                    SttChoice::Windows => SttBackend::Windows,
                    SttChoice::Model(m) => {
                        self.settings.stt_model = Some(m.clone());
                        self.config.stt_model = m;
                        SttBackend::Api
                    }
                };
                self.settings.stt = Some(self.config.stt);
                self.save();
                let stt = self.config.stt();
                self.has_stt_key = stt.api_key.is_some();
                self.send(Command::SetStt(stt));
            }
            Message::Capture(c) => self.set_capture(c),
            Message::Captured(c) => match c {
                _ if self.capture != Capture::Key => {}
                Captured::Key(key) => self.assign_key(key),
                Captured::Cancel => self.set_capture(Capture::None),
                Captured::NeedsModifier => {
                    self.capture_hint = Some("文字キーはCtrl・Alt・Winと組み合わせてください");
                }
            },
            Message::Wheel(e) => {
                if self.capture == Capture::Wheel {
                    if e.pressed {
                        self.settings.wheel = Some(e.button);
                        self.set_capture(Capture::None);
                        self.save();
                    }
                } else if self
                    .settings
                    .wheel
                    .as_ref()
                    .is_some_and(|b| b.same_button(&e.button))
                {
                    self.ptt(e.pressed);
                }
            }
            Message::ClearWheel => {
                self.settings.wheel = None;
                self.save();
            }
            Message::CloseToTray(on) => {
                self.settings.close_to_tray = on;
                self.save();
            }
            Message::StartWithWindows(on) => {
                match resident::set_start_with_windows(on, &self.config.agent_cwd()) {
                    Ok(()) => self.starts_with_windows = on,
                    Err(e) => self.error(format!("自動起動を変更できません: {e}")),
                }
            }
            Message::Resident(ResidentEvent::Show) => return show_window(),
            Message::Resident(ResidentEvent::Quit) => return iced::exit(),
            Message::Minimize => {
                return window::latest().and_then(|id| window::minimize(id, true));
            }
            Message::Close => return self.close(),
            Message::Chrome(e) => self.chrome.apply(e),
            Message::DismissError => {
                self.errors.pop();
            }
            Message::Scrolled(at_end) => self.at_end = at_end,
            Message::ScrollToEnd => {
                self.at_end = true;
                return shadcn_iced::message_scroller::scroll_to_end(SCROLLER);
            }
        }
        Task::none()
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::AgentReady { name } => self.agent = AgentStatus::Ready(name),
            Event::AgentStarting => self.agent = AgentStatus::Starting,
            Event::Microphone(ok) => self.microphone = Some(ok),
            Event::Speakers(ok) => self.speakers = Some(ok),
            Event::Telemetry(ok) => self.telemetry = Some(ok),
            Event::Setting(s) => self.changed(s),
            Event::Rules(r) => {
                let keep = r.lasting() != self.settings.rules.lasting();
                self.settings.rules = r;
                if keep {
                    self.save();
                }
            }
            Event::Plan(p) => self.plan = p,
            Event::CallStarted { turn, about } => {
                self.previewing = false;
                self.turns.push(TurnView {
                    id: turn,
                    call: true,
                    heard: Some(about),
                    ..Default::default()
                });
            }
            Event::AgentClosed { error } => {
                self.agent =
                    AgentStatus::Closed(error.unwrap_or_else(|| "connection closed".into()))
            }
            Event::Recording(r) => self.recording = r,
            Event::Level(l) => self.level = l,
            Event::Speaking(s) => {
                self.speaking = s;
                self.previewing &= s;
            }
            Event::Error(e) => self.error(e),
            // Measured by vr-eval; the window does not show them.
            Event::Usage { .. } | Event::Cost { .. } => {}
            Event::AgentOptions(o) => self.offered = o,
            Event::TurnStarted { turn } => {
                self.previewing = false;
                self.turns.push(TurnView {
                    id: turn,
                    ..Default::default()
                });
            }
            Event::Transcript { turn, text, ms } => {
                if let Some(t) = self.turn(turn) {
                    t.heard = Some(text);
                    t.stt_ms = (ms > 0).then_some(ms);
                }
            }
            Event::FirstToken { turn, ms } => {
                if let Some(t) = self.turn(turn) {
                    t.token_ms = Some(ms);
                }
            }
            Event::Chunk { turn, text } => {
                if let Some(t) = self.turn(turn) {
                    t.reply.push_str(&text);
                }
            }
            Event::Tool {
                turn,
                id,
                title,
                state,
            } => {
                if let Some(t) = self.turn(turn) {
                    match t.tools.iter_mut().find(|(i, ..)| *i == id) {
                        Some(tool) => {
                            if let Some(title) = title {
                                tool.1 = title;
                            }
                            tool.2 = state;
                        }
                        None => t
                            .tools
                            .push((id, title.unwrap_or_else(|| "tool".into()), state)),
                    }
                }
            }
            Event::Permission {
                turn,
                title,
                allowed,
            } => {
                if !allowed && let Some(t) = self.turn(turn) {
                    t.tools.push((
                        format!("denied:{title}"),
                        format!("拒否: {title}"),
                        ToolState::Failed,
                    ));
                }
            }
            Event::FirstAudio { turn, ms } => {
                if let Some(t) = self.turn(turn) {
                    t.audio_ms = Some(ms);
                }
            }
            Event::TurnDone { turn, outcome, ms } => {
                if let Some(t) = self.turn(turn) {
                    t.done = Some((outcome, ms));
                }
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn latency(t: &TurnView) -> String {
    let fmt =
        |label: &str, ms: Option<u64>| ms.map(|m| format!("{label} {:.1}s", m as f64 / 1000.0));
    let mut parts: Vec<String> = [
        fmt("認識", t.stt_ms),
        fmt("初回応答", t.token_ms),
        fmt("初回音声", t.audio_ms),
    ]
    .into_iter()
    .flatten()
    .collect();
    match &t.done {
        Some((Outcome::Answered, ms)) => parts.push(format!("完了 {:.1}s", *ms as f64 / 1000.0)),
        Some((Outcome::Cancelled, _)) => parts.push("中断".into()),
        Some((Outcome::NoSpeech, _)) => parts.push("聞き取れず".into()),
        Some((Outcome::Silent, _)) => {}
        Some((Outcome::Failed(e), _)) => parts.push(format!("失敗: {e}")),
        None => {}
    }
    parts.join(" · ")
}

impl TurnView {
    /// The driver's words as shown, `None` while transcribing.
    fn heard_label(&self) -> Option<&str> {
        match (self.heard.as_deref(), &self.done) {
            (Some(""), _) => Some("（無音）"),
            (Some(h), _) => Some(h),
            // The transcriber failed (e.g. HTTP 401): the turn is over.
            (None, Some(_)) => Some("（聞き取れませんでした）"),
            (None, None) => None,
        }
    }

    /// Something was asked, so the engineer answers.
    fn asked(&self) -> bool {
        self.heard.as_deref().is_some_and(|h| !h.is_empty())
    }

    /// Waiting for the first words of the reply.
    fn thinking(&self) -> bool {
        self.reply.is_empty() && self.done.is_none()
    }
}

/// A voice by its character, its style or both: `ずんだもん`, `あまあま`,
/// `四国めたん ノーマル`. A style alone is one of the current character.
fn find_voice<'a>(voices: &'a [Voice], current: Option<&Voice>, name: &str) -> Option<&'a Voice> {
    let plain = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && !"（）()・".contains(*c))
            .collect()
    };
    let name = plain(name);
    let of = |f: &dyn Fn(&Voice) -> bool| voices.iter().find(|v| f(v));
    if name.is_empty() {
        return None;
    }
    of(&|v| plain(&v.to_string()) == name)
        .or_else(|| {
            of(&|v| current.is_some_and(|c| c.speaker == v.speaker) && plain(&v.style) == name)
        })
        .or_else(|| of(&|v| plain(&v.speaker) == name && v.style == "ノーマル"))
        .or_else(|| of(&|v| plain(&v.speaker) == name))
        .or_else(|| of(&|v| plain(&v.style) == name))
}

pub fn subscription(app: &App) -> Subscription<Message> {
    let keys = if app.capture == Capture::Key {
        iced::event::listen_with(captured_key)
    } else {
        Subscription::none()
    };
    Subscription::batch([
        Subscription::run(engine_stream),
        Subscription::run(hotkey_stream),
        Subscription::run(chrome_stream),
        Subscription::run(wheel_stream),
        Subscription::run(resident_stream),
        window::close_requests().map(|_| Message::Close),
        keys,
    ])
}

fn captured_key(event: iced::Event, _: iced::event::Status, _: window::Id) -> Option<Message> {
    use iced::keyboard::{Event, key::Physical};
    match event {
        iced::Event::Keyboard(Event::KeyPressed {
            physical_key: Physical::Code(code),
            modifiers,
            ..
        }) => hotkey::capture(code, modifiers).map(Message::Captured),
        _ => None,
    }
}

fn show_window() -> Task<Message> {
    window::latest().and_then(|id| {
        Task::batch([
            window::set_mode(id, window::Mode::Windowed),
            window::minimize(id, false),
            window::gain_focus(id),
        ])
    })
}

fn context(cfg: &Config) -> Option<Arc<dyn ContextSource>> {
    let commands = cfg.context_commands();
    let say = cfg.say.trim().to_string();
    (!commands.is_empty() || cfg.topics || !say.is_empty()).then(|| {
        Arc::new(Commands {
            commands,
            say,
            topics: cfg.topics,
            decide: cfg.decide(),
            cwd: cfg.agent_cwd(),
        }) as Arc<dyn ContextSource>
    })
}

/// Starts an agent from its command line.
fn connect(cfg: &Config, command: &str, options: &AgentOptions) -> AgentLink {
    match AcpAgent::from_str(&config::agent_command(command)) {
        Ok(acp) => AgentLink::spawn(acp, cfg.session(options)),
        Err(e) => AgentLink::failed(format!("bad agent command: {e}")),
    }
}

/// The pipeline between the given devices: transcriber, agent and speech as
/// configured. Must be called on a tokio runtime.
pub fn deps(
    cfg: &Config,
    recorder: Arc<dyn Recorder>,
    output: Arc<dyn Output>,
    synth: Arc<dyn Synth>,
) -> Deps {
    let connect_cfg = cfg.clone();
    let options = cfg.options_of(&cfg.agent);
    Deps {
        recorder,
        transcriber: stt::build(&cfg.stt()),
        make_transcriber: Box::new(stt::build),
        synth,
        output,
        agent: connect(cfg, &cfg.agent, &options),
        agent_command: cfg.agent.clone(),
        agent_options: options,
        connect: Box::new(move |command, options| connect(&connect_cfg, command, options)),
        context: context(cfg),
        prompt_template: cfg.prompt.clone(),
        speaker: cfg.speaker,
        volume: cfg.volume,
        speed: cfg.speed_percent(),
        early_stt: cfg.early_stt,
        language: Some(cfg.language.clone()).filter(|l| !l.is_empty()),
        watch: None,
        proactive: SETTINGS.get().map(|(s, _)| s.proactive).unwrap_or_default(),
        timing: Timing::default(),
        rules: SETTINGS
            .get()
            .map(|(s, _)| s.rules.clone())
            .unwrap_or_default(),
        live: None,
        standby: (cfg.standby > 0.0).then(|| Duration::from_secs_f64(cfg.standby)),
        transcript: Some(PathBuf::from(&cfg.transcript)).filter(|p| !p.as_os_str().is_empty()),
    }
}

/// Opens the devices, starts the agent session and the pipeline.
/// Returns the VOICEVOX voices.
pub async fn start_engine(
    cfg: &Config,
    cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
    ev_tx: UnboundedSender<Event>,
) -> Result<Vec<Voice>, String> {
    let tx = ev_tx.clone();
    let recorder: Arc<dyn Recorder> = Arc::new(LateMic::open(move |opened| {
        let _ = tx.send(Event::Microphone(opened.is_ok()));
        if let Err(e) = opened {
            let _ = tx.send(Event::Error(format!("{e:#}")));
        }
    }));
    let tx = ev_tx.clone();
    let output: Arc<dyn Output> = Arc::new(LateSpeakers::open(move |opened| {
        let _ = tx.send(Event::Speakers(opened.is_ok()));
        if let Err(e) = opened {
            let _ = tx.send(Event::Error(format!("{e:#}")));
        }
    }));
    let voicevox = Arc::new(Voicevox::new(cfg.voicevox_url.clone(), cfg.speed));
    let vv = voicevox.clone();
    let voices = tokio::task::spawn_blocking(move || vv.voices().map_err(|e| format!("{e:#}")))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));

    let mut deps = deps(cfg, recorder, output, voicevox);
    deps.watch = Follow::start(&cfg.watch, cfg.agent_cwd());
    deps.live = Feed::start(&cfg.live, cfg.agent_cwd());
    tokio::spawn(engine::run(deps, cmd_rx, ev_tx));
    voices
}

/// How long VOICEVOX may take to start before that is an error, and how often
/// it is asked until then and later.
#[derive(Debug, Clone, Copy)]
struct Patience {
    wait: Duration,
    poll: Duration,
    slow: Duration,
}

const PATIENCE: Patience = Patience {
    wait: Duration::from_secs(60),
    poll: Duration::from_secs(2),
    slow: Duration::from_secs(10),
};

/// Asks for the voices until there are some. Their absence is told once,
/// when it takes too long. Blocking.
fn wait_for_voices(
    ask: impl Fn() -> Result<Vec<Voice>, String>,
    patience: Patience,
    tell: impl Fn(Result<Vec<Voice>, String>) -> bool,
) {
    let started = std::time::Instant::now();
    let mut told = false;
    loop {
        std::thread::sleep(if told { patience.slow } else { patience.poll });
        match ask() {
            Err(_) if told || started.elapsed() < patience.wait => {}
            answer => {
                told = true;
                let found = answer.is_ok();
                if !tell(answer) || found {
                    return;
                }
            }
        }
    }
}

fn engine_stream() -> impl Stream<Item = Message> {
    iced::stream::channel(256, async |mut out| {
        let cfg = CONFIG
            .get()
            .cloned()
            .expect("config is set before the app starts");
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (ev_tx, mut ev_rx) = unbounded_channel();
        let _ = out.send(Message::EngineReady(cmd_tx)).await;
        let cwd = cfg.agent_cwd();
        let _serve =
            Supervised::spawn(&cfg.serve, cwd, Retry::every(Duration::from_secs(15)), None);
        let voices = start_engine(&cfg, cmd_rx, ev_tx).await;
        let (voice_tx, mut voice_rx) = unbounded_channel();
        let mut _voicevox = None;
        if let Err(e) = &voices {
            tracing::info!("{e}");
            _voicevox = voicevox::start_engine(&cfg.voicevox_url, cfg.voicevox.as_deref());
            let vv = Voicevox::new(cfg.voicevox_url.clone(), cfg.speed);
            let _ = std::thread::Builder::new()
                .name("vr-radio-voices".into())
                .spawn(move || {
                    wait_for_voices(
                        || vv.voices().map_err(|e| format!("{e:#}")),
                        PATIENCE,
                        |v| voice_tx.send(v).is_ok(),
                    )
                });
        } else {
            let _ = voice_tx.send(voices);
        }
        loop {
            let message = tokio::select! {
                Some(v) = voice_rx.recv() => Message::Voices(v),
                ev = ev_rx.recv() => match ev {
                    Some(ev) => Message::Engine(ev),
                    None => break,
                },
            };
            if out.send(message).await.is_err() {
                break;
            }
        }
    })
}

fn hotkey_stream() -> impl Stream<Item = Message> {
    iced::stream::channel(16, async |mut out| {
        use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
        let rx = GlobalHotKeyEvent::receiver();
        loop {
            while let Ok(e) = rx.try_recv() {
                let down = e.state == HotKeyState::Pressed;
                if out.send(Message::Hotkey(down)).await.is_err() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
}

fn wheel_stream() -> impl Stream<Item = Message> {
    iced::stream::channel(64, async |mut out| {
        let (tx, mut rx) = unbounded_channel();
        wheel::listen(tx);
        while let Some(e) = rx.recv().await {
            if out.send(Message::Wheel(e)).await.is_err() {
                return;
            }
        }
    })
}

fn resident_stream() -> impl Stream<Item = Message> {
    iced::stream::channel(4, async |mut out| {
        loop {
            while let Some(e) = resident::poll() {
                if out.send(Message::Resident(e)).await.is_err() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
}

fn chrome_stream() -> impl Stream<Item = Message> {
    iced::stream::channel(16, async |mut out| {
        loop {
            for e in crate::chrome::drain() {
                if out.send(Message::Chrome(e)).await.is_err() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(16)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_turn() -> App {
        let mut app = App::new();
        app.apply(Event::TurnStarted { turn: 1 });
        app
    }

    #[test]
    fn events_fill_the_turn() {
        let mut app = app_with_turn();
        app.apply(Event::Transcript {
            turn: 1,
            text: "差は？".into(),
            ms: 420,
        });
        app.apply(Event::Tool {
            turn: 1,
            id: "t".into(),
            title: Some("vr brief".into()),
            state: ToolState::Running,
        });
        app.apply(Event::Tool {
            turn: 1,
            id: "t".into(),
            title: None,
            state: ToolState::Done,
        });
        app.apply(Event::FirstToken { turn: 1, ms: 1200 });
        app.apply(Event::Chunk {
            turn: 1,
            text: "0.6".into(),
        });
        app.apply(Event::Chunk {
            turn: 1,
            text: "秒。".into(),
        });
        app.apply(Event::FirstAudio { turn: 1, ms: 1650 });
        app.apply(Event::TurnDone {
            turn: 1,
            outcome: Outcome::Answered,
            ms: 2500,
        });
        let t = &app.turns[0];
        assert_eq!(t.heard.as_deref(), Some("差は？"));
        assert_eq!(t.reply, "0.6秒。");
        assert_eq!(
            t.tools,
            vec![("t".into(), "vr brief".into(), ToolState::Done)]
        );
        assert_eq!(
            latency(t),
            "認識 0.4s · 初回応答 1.2s · 初回音声 1.6s · 完了 2.5s"
        );
    }

    #[test]
    fn typed_question_has_no_stt_latency_and_outcomes_are_labelled() {
        let mut app = app_with_turn();
        app.apply(Event::Transcript {
            turn: 1,
            text: "q".into(),
            ms: 0,
        });
        app.apply(Event::TurnDone {
            turn: 1,
            outcome: Outcome::Cancelled,
            ms: 10,
        });
        assert_eq!(latency(&app.turns[0]), "中断");
        app.turns[0].done = Some((Outcome::NoSpeech, 0));
        assert_eq!(latency(&app.turns[0]), "聞き取れず");
        app.turns[0].done = Some((Outcome::Failed("x".into()), 0));
        assert_eq!(latency(&app.turns[0]), "失敗: x");
    }

    #[test]
    fn denied_permission_is_shown_and_errors_are_capped() {
        let mut app = app_with_turn();
        app.apply(Event::Permission {
            turn: 1,
            title: "git push".into(),
            allowed: false,
        });
        app.apply(Event::Permission {
            turn: 1,
            title: "vr brief".into(),
            allowed: true,
        });
        assert_eq!(app.turns[0].tools.len(), 1);
        assert_eq!(app.turns[0].tools[0].1, "拒否: git push");
        for i in 0..5 {
            app.apply(Event::Error(format!("e{i}")));
        }
        assert_eq!(app.errors, vec!["e2", "e3", "e4"]);
    }

    #[test]
    fn agent_status_and_view_build() {
        let mut app = app_with_turn();
        app.apply(Event::AgentReady {
            name: "Claude".into(),
        });
        assert_eq!(app.agent, AgentStatus::Ready("Claude".into()));
        app.apply(Event::AgentClosed { error: None });
        assert_eq!(app.agent, AgentStatus::Closed("connection closed".into()));
        app.apply(Event::Transcript {
            turn: 1,
            text: "q".into(),
            ms: 5,
        });
        let _ = app.view();
    }

    #[test]
    fn turn_states() {
        let mut t = TurnView::default();
        assert_eq!(
            (t.heard_label(), t.asked(), t.thinking()),
            (None, false, true)
        );
        t.heard = Some(String::new());
        assert_eq!((t.heard_label(), t.asked()), (Some("（無音）"), false));
        t.heard = Some("差は？".into());
        t.done = None;
        assert_eq!(
            (t.heard_label(), t.asked(), t.thinking()),
            (Some("差は？"), true, true)
        );
        t.reply = "0.6".into();
        assert!(!t.thinking());
        let failed = TurnView {
            done: Some((Outcome::Failed("HTTP 401".into()), 1)),
            ..Default::default()
        };
        assert_eq!(
            (failed.heard_label(), failed.asked()),
            (Some("（聞き取れませんでした）"), false)
        );
        t.reply.clear();
        t.done = Some((Outcome::Answered, 1));
        assert!(!t.thinking());
    }

    #[test]
    fn phase_follows_the_turn() {
        let mut app = App::new();
        assert_eq!(app.phase(), Phase::Idle);
        app.apply(Event::Recording(true));
        assert_eq!(app.phase(), Phase::Recording);
        app.apply(Event::Recording(false));
        app.apply(Event::TurnStarted { turn: 1 });
        assert_eq!(app.phase(), Phase::Transcribing);
        app.apply(Event::Transcript {
            turn: 1,
            text: "差は？".into(),
            ms: 1,
        });
        assert_eq!(app.phase(), Phase::Researching);
        app.apply(Event::Chunk {
            turn: 1,
            text: "0.6".into(),
        });
        assert_eq!(app.phase(), Phase::Answering);
        app.apply(Event::Speaking(true));
        assert_eq!(app.phase(), Phase::Speaking);
        app.apply(Event::TurnDone {
            turn: 1,
            outcome: Outcome::Answered,
            ms: 1,
        });
        assert_eq!(app.phase(), Phase::Speaking);
        app.apply(Event::Speaking(false));
        assert_eq!(app.phase(), Phase::Idle);
        app.apply(Event::Recording(true));
        app.apply(Event::Speaking(true));
        assert_eq!(app.phase(), Phase::Recording);
    }

    #[test]
    fn phase_hints() {
        let hints: Vec<_> = [
            Phase::Idle,
            Phase::Recording,
            Phase::Transcribing,
            Phase::Researching,
            Phase::Answering,
            Phase::Speaking,
        ]
        .into_iter()
        .map(Phase::hint)
        .collect();
        assert_eq!(
            hints,
            vec![
                "ボタンを押して話しかけてください",
                "離すと送信します",
                "音声を文字にしています",
                "エージェントがデータを確認中",
                "話し始めると中断します",
                "話し始めると中断します",
            ]
        );
    }

    #[test]
    fn sample_is_a_conversation_in_progress() {
        let mut app = App::sample();
        let ids: Vec<u32> = app.voices.iter().map(|v| v.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 10, 11]);
        assert_eq!(app.voice.as_ref().map(|v| v.id), Some(1));
        assert_eq!(app.turns.len(), 3);
        assert!(app.turns[1].call);
        assert!(app.readiness().iter().all(|(_, r)| *r == Readiness::Ready));
        assert_eq!(app.phase(), Phase::Researching);
        app.handle(Event::Recording(true));
        assert_eq!(app.phase(), Phase::Recording);
    }

    #[test]
    fn hotkey_comes_from_registration() {
        let _ = HOTKEY.set(Ok("F9".into()));
        assert_eq!(App::new().hotkey(), Some("F9"));
    }

    #[test]
    fn stop_is_offered_only_while_something_runs() {
        let busy: Vec<_> = [
            Phase::Idle,
            Phase::Recording,
            Phase::Transcribing,
            Phase::Researching,
            Phase::Answering,
            Phase::Speaking,
        ]
        .into_iter()
        .map(|p| (p.label(), p.busy()))
        .collect();
        assert_eq!(
            busy,
            vec![
                ("待機中", false),
                ("録音中", false),
                ("聞き取り中", true),
                ("調べています", true),
                ("回答中", true),
                ("読み上げ中", true),
            ]
        );
    }

    #[test]
    fn settings_sheet_and_errors() {
        let mut app = App::new();
        let _ = app.update(Message::Settings(true));
        assert!(app.settings_open);
        let _ = app.view();
        let _ = app.update(Message::Settings(false));
        assert!(!app.settings_open);
        app.apply(Event::Error("a".into()));
        app.apply(Event::Error("b".into()));
        let _ = app.update(Message::DismissError);
        assert_eq!(app.errors, vec!["a"]);
    }

    #[test]
    fn voices_select_the_configured_speaker() {
        let mut app = App::new();
        let voices = vec![
            Voice {
                id: 1,
                speaker: "a".into(),
                style: "s".into(),
            },
            Voice {
                id: 3,
                speaker: "b".into(),
                style: "s".into(),
            },
        ];
        let _ = app.update(Message::Voices(Ok(voices.clone())));
        // CONFIG is unset in tests: the default speaker is 3.
        assert_eq!(app.voice, Some(voices[1].clone()));
        assert_eq!(app.voices, voices);
        let _ = app.update(Message::Character("a".into()));
        assert_eq!(app.voice, Some(voices[0].clone()));
        let _ = app.update(Message::Voices(Err("down".into())));
        assert_eq!(app.errors, vec!["down"]);
    }

    fn wheel_event(button: u16, pressed: bool) -> Message {
        Message::Wheel(ButtonEvent {
            button: wheel::Binding {
                device: "VID".into(),
                name: "R9".into(),
                button,
            },
            pressed,
        })
    }

    /// An app wired to a fake engine, keeping its settings in a folder.
    fn wired() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.settings_path = Some(dir.path().join("settings.json"));
        let (tx, rx) = unbounded_channel();
        let _ = app.update(Message::EngineReady(tx));
        (app, rx, dir)
    }

    fn saved(app: &App) -> Settings {
        Settings::load(app.settings_path.as_ref().unwrap()).unwrap()
    }

    #[test]
    fn agent_is_switched_and_remembered() {
        let (mut app, mut rx, _dir) = wired();
        app.apply(Event::AgentReady { name: "A".into() });
        let _ = app.update(Message::AgentChoice(AgentChoice::Codex));
        assert_eq!(app.agent_draft, config::CODEX_AGENT);
        assert_eq!(app.agent, AgentStatus::Ready("A".into()));
        let _ = app.update(Message::Connect);
        assert_eq!(app.agent, AgentStatus::Starting);
        assert_eq!(
            rx.try_recv(),
            Ok(Command::SetAgent(
                config::CODEX_AGENT.into(),
                app.config.options_of(config::CODEX_AGENT)
            ))
        );
        assert_eq!(saved(&app).agent.as_deref(), Some(config::CODEX_AGENT));
        // Custom keeps the command to edit it.
        let _ = app.update(Message::AgentChoice(AgentChoice::Custom));
        assert_eq!(app.agent_draft, config::CODEX_AGENT);
        let _ = app.update(Message::AgentDraft("  ".into()));
        let _ = app.update(Message::Connect);
        assert!(rx.try_recv().is_err());
        assert_eq!(AgentChoice::of(config::DEFAULT_AGENT), AgentChoice::Claude);
        assert_eq!(AgentChoice::of("other"), AgentChoice::Custom);
        let labels = AgentChoice::ALL.map(|c| c.to_string());
        assert_eq!(labels, ["Claude Code", "Codex", "カスタム"]);
    }

    #[test]
    fn options_of_the_agent_are_chosen_and_remembered() {
        let (mut app, mut cmds, _dir) = wired();
        app.config.agent = config::CODEX_AGENT.into();
        app.offered = App::sample().offered;
        let pick = |app: &App, i: usize, value: &str| {
            let all = OptionChoice::all(&app.offered[i]);
            all.into_iter().find(|c| c.value == value).unwrap()
        };
        let fast = pick(&app, 2, "on");
        assert_eq!(fast.to_string(), "高速（消費が増える）");
        assert_eq!(pick(&app, 2, "off").to_string(), "標準");
        assert_eq!(pick(&app, 0, "sol").to_string(), "6.2 Sol");
        let _ = app.update(Message::AgentOption(fast));
        assert_eq!(app.offered[2].value, "on");
        let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
        let saved_options = saved(&app).agent_options[config::CODEX_AGENT].clone();
        assert_eq!(saved_options.radio, [pair("fast-mode", "on")]);
        // The whole set goes to the engine: what was not chosen stays.
        let Ok(Command::SetAgentOptions(o)) = cmds.try_recv() else {
            panic!("options expected")
        };
        assert!(o.radio.contains(&pair("fast-mode", "on")));
        assert!(o.radio.contains(&pair("model", "gpt-6.1-terra")));
        assert!(o.radio.contains(&pair("reasoning_effort", "medium")));
        // Another agent has options of its own.
        assert_eq!(
            app.config.options_of(config::DEFAULT_AGENT),
            AgentOptions::default()
        );
        app.apply(Event::AgentOptions(Vec::new()));
        assert!(app.offered.is_empty());
    }

    #[test]
    fn transcriber_is_switched_and_remembered() {
        let (mut app, mut rx, _dir) = wired();
        let model = SttChoice::Model("openai/gpt-transcribe".into());
        assert_eq!(app.stt_choice(), model);
        let _ = app.update(Message::Stt(SttChoice::Windows));
        assert_eq!(app.stt_choice(), SttChoice::Windows);
        let Ok(Command::SetStt(cfg)) = rx.try_recv() else {
            panic!("SetStt expected")
        };
        assert_eq!(cfg.backend, SttBackend::Windows);
        assert_eq!(saved(&app).stt, Some(SttBackend::Windows));
        let _ = app.update(Message::Stt(SttChoice::Model("m".into())));
        assert_eq!(app.stt_choice(), SttChoice::Model("m".into()));
        assert_eq!(saved(&app).stt_model.as_deref(), Some("m"));
        assert_eq!(saved(&app).stt, Some(SttBackend::Api));
        // The current model is listed even when it is not a known one.
        let choices = app.stt_choices();
        assert_eq!(choices[0], SttChoice::Model("m".into()));
        assert_eq!(choices[1], model);
        assert_eq!(choices.contains(&SttChoice::Windows), cfg!(windows));
        app.config.stt_url = "http://127.0.0.1:9000/v1".into();
        let others: Vec<_> = app
            .stt_choices()
            .into_iter()
            .filter(|c| *c != SttChoice::Windows)
            .collect();
        assert_eq!(others, [SttChoice::Model("m".into())]);
        assert_eq!(SttChoice::Windows.to_string(), "Windows 標準（オフライン）");
        assert_eq!(model.to_string(), "openai/gpt-transcribe");
    }

    #[test]
    fn wheel_button_is_assigned_then_talks() {
        let (mut app, mut rx, _dir) = wired();
        let _ = app.update(wheel_event(5, true));
        assert!(rx.try_recv().is_err());
        let _ = app.update(Message::Capture(Capture::Wheel));
        // Letting go of another button does not assign it; talking waits.
        let _ = app.update(wheel_event(4, false));
        let _ = app.update(Message::Hotkey(true));
        assert_eq!(app.capture, Capture::Wheel);
        assert!(rx.try_recv().is_err());
        let _ = app.update(wheel_event(5, true));
        assert_eq!(app.capture, Capture::None);
        assert_eq!(saved(&app).wheel.unwrap().button, 5);
        let _ = app.update(wheel_event(5, false));
        let _ = app.update(wheel_event(6, true));
        let _ = app.update(wheel_event(5, true));
        assert_eq!(rx.try_recv(), Ok(Command::PttUp));
        assert_eq!(rx.try_recv(), Ok(Command::PttDown));
        let _ = app.update(Message::ClearWheel);
        let _ = app.update(wheel_event(5, true));
        assert!(rx.try_recv().is_err());
        assert_eq!(saved(&app).wheel, None);
    }

    #[test]
    fn capturing_a_key_can_be_cancelled() {
        let (mut app, _rx, _dir) = wired();
        app.hotkey = None;
        let _ = app.update(Message::Captured(Captured::Key("F13".into())));
        assert_eq!(app.settings.hotkey, None);
        let _ = app.update(Message::Capture(Capture::Key));
        let _ = app.update(Message::Captured(Captured::NeedsModifier));
        assert!(app.capture_hint.is_some());
        let _ = app.view();
        let _ = app.update(Message::Captured(Captured::Cancel));
        assert_eq!((app.capture, app.capture_hint), (Capture::None, None));
        // A key that cannot be registered keeps the old one and says so.
        let _ = app.update(Message::Capture(Capture::Key));
        let _ = app.update(Message::Captured(Captured::Key("Nope".into())));
        assert_eq!(app.capture, Capture::None);
        assert_eq!(app.settings.hotkey, None);
        assert!(app.errors[0].starts_with("キーを割り当てられません: Nope"));
        // The key is given back while waiting for one, and only then: an
        // unregistrable one shows that it was registered again.
        app.hotkey = Some(Ok("Nope".into()));
        let _ = app.update(Message::Capture(Capture::Wheel));
        let _ = app.update(Message::Capture(Capture::None));
        assert_eq!(app.hotkey, Some(Ok("Nope".into())));
        let _ = app.update(Message::Capture(Capture::Key));
        assert_eq!(app.hotkey, Some(Ok("Nope".into())));
        let _ = app.update(Message::Capture(Capture::None));
        assert!(matches!(app.hotkey, Some(Err(_))));
        // Closing the settings stops waiting.
        let _ = app.update(Message::Capture(Capture::Wheel));
        let _ = app.update(Message::Settings(false));
        assert_eq!(app.capture, Capture::None);
    }

    #[test]
    fn settings_pages() {
        let mut app = App::sample();
        let _ = app.update(Message::Settings(true));
        assert_eq!(app.settings_page, SettingsPage::Talk);
        for page in SettingsPage::ALL {
            let _ = app.update(Message::SettingsPage(page));
            assert_eq!(app.settings_page, page);
            let _ = app.view();
        }
        // Leaving the page stops waiting for a key and drops unfinished
        // conditions; closing keeps the page for the next time.
        let _ = app.update(Message::SettingsPage(SettingsPage::Talk));
        let _ = app.update(Message::Capture(Capture::Wheel));
        app.hold_drafts = [Some("brake >".into()), None];
        let _ = app.update(Message::SettingsPage(SettingsPage::Calls));
        assert_eq!(app.capture, Capture::None);
        assert_eq!(app.hold_drafts, [None, None]);
        let _ = app.update(Message::Settings(false));
        let _ = app.update(Message::Settings(true));
        assert_eq!(app.settings_page, SettingsPage::Calls);
    }

    #[test]
    fn settings_build_in_every_state() {
        let mut app = App::sample();
        let (tx, _rx) = unbounded_channel();
        let _ = app.update(Message::EngineReady(tx));
        let _ = app.update(Message::Settings(true));
        let _ = app.view();
        for choice in AgentChoice::ALL {
            let _ = app.update(Message::AgentChoice(choice));
            let _ = app.view();
        }
        let _ = app.update(Message::Connect);
        let _ = app.view();
        app.apply(Event::AgentClosed { error: None });
        for (backend, key) in [
            (SttBackend::Api, true),
            (SttBackend::Api, false),
            (SttBackend::Windows, false),
        ] {
            app.config.stt = backend;
            app.has_stt_key = key;
            let _ = app.view();
        }
        for hotkey in [Ok("F13".into()), Err("使用中".into())] {
            app.hotkey = Some(hotkey);
            let _ = app.view();
        }
        for capture in [Capture::Key, Capture::Wheel] {
            app.capture = capture;
            let _ = app.view();
        }
        let _ = app.update(wheel_event(5, true));
        app.voices.clear();
        let _ = app.view();
    }

    #[test]
    fn voices_are_heard_when_nothing_else_is() {
        let (mut app, mut rx, _dir) = wired();
        let v = |id| Voice {
            id,
            speaker: "v".into(),
            style: "s".into(),
        };
        let _ = app.update(Message::Preview);
        assert!(rx.try_recv().is_err());
        let _ = app.update(Message::Speaker(v(8)));
        assert_eq!(rx.try_recv(), Ok(Command::SetSpeaker(8)));
        assert_eq!(rx.try_recv(), Ok(Command::Preview(8)));
        assert_eq!(saved(&app).speaker, Some(8));
        // The next voice replaces the sample still playing.
        app.apply(Event::Speaking(true));
        let _ = app.update(Message::Speaker(v(9)));
        assert_eq!(rx.try_recv(), Ok(Command::SetSpeaker(9)));
        assert_eq!(rx.try_recv(), Ok(Command::Preview(9)));
        // An answer is not interrupted by picking a voice...
        app.apply(Event::TurnStarted { turn: 1 });
        let _ = app.update(Message::Speaker(v(10)));
        assert_eq!(rx.try_recv(), Ok(Command::SetSpeaker(10)));
        assert!(rx.try_recv().is_err());
        // ...only by asking for the sample.
        let _ = app.update(Message::Preview);
        assert_eq!(rx.try_recv(), Ok(Command::Preview(10)));
        app.apply(Event::Speaking(false));
        assert!(!app.previewing);
    }

    #[test]
    fn volume_and_speed_are_set_and_remembered() {
        let (mut app, mut rx, _dir) = wired();
        let _ = app.update(Message::Volume(60));
        assert_eq!(rx.try_recv(), Ok(Command::SetVolume(60)));
        assert_eq!(app.config.volume, 60);
        // While the slider moves nothing is written.
        assert_eq!(saved(&app).volume, None);
        let _ = app.update(Message::SaveSettings);
        assert_eq!(saved(&app).volume, Some(60));
        let _ = app.update(Message::Speed(130));
        assert_eq!(rx.try_recv(), Ok(Command::SetSpeed(130)));
        assert_eq!(app.config.speed, 1.3);
        let _ = app.update(Message::SaveSettings);
        assert_eq!(saved(&app).speed, Some(1.3));
    }

    #[test]
    fn calls_are_switched_and_remembered() {
        let (mut app, mut rx, _dir) = wired();
        let _ = app.update(Message::Proactive(false));
        let mut want = crate::watch::Proactive {
            enabled: false,
            ..Default::default()
        };
        assert_eq!(rx.try_recv(), Ok(Command::SetProactive(want)));
        let _ = app.update(Message::ProactiveGroup(Group::Lap, true));
        want.lap = true;
        assert_eq!(rx.try_recv(), Ok(Command::SetProactive(want)));
        assert_eq!(saved(&app).proactive, want);
    }

    /// Settings changed by voice are in effect already: the engine is not
    /// told again, except of the voice, which only the window can look up.
    #[test]
    fn settings_changed_by_voice_are_followed() {
        let (mut app, mut rx, _dir) = wired();
        let v = |id, speaker: &str, style: &str| Voice {
            id,
            speaker: speaker.into(),
            style: style.into(),
        };
        let voices = vec![
            v(3, "ずんだもん", "ノーマル"),
            v(1, "ずんだもん", "あまあま"),
            v(7, "四国めたん", "ツンツン"),
            v(2, "四国めたん", "ノーマル"),
        ];
        let _ = app.update(Message::Voices(Ok(voices)));
        assert_eq!(rx.try_recv(), Ok(Command::SetSpeaker(3)));
        app.apply(Event::TurnStarted { turn: 1 });
        for s in [
            Setting::Volume(40),
            Setting::Speed(150),
            Setting::Muted(true),
            Setting::Proactive(false),
            Setting::Event(Group::Fuel, false),
        ] {
            app.apply(Event::Setting(s));
        }
        assert!(rx.try_recv().is_err());
        assert!(app.muted);
        assert_eq!((app.config.volume, app.config.speed), (40, 1.5));
        let s = saved(&app);
        assert_eq!((s.volume, s.speed), (Some(40), Some(1.5)));
        assert!(!s.proactive.enabled && !s.proactive.fuel && s.proactive.flag);
        let shown: Vec<&str> = app.turns[0].tools.iter().map(|t| t.1.as_str()).collect();
        assert_eq!(
            shown,
            [
                "音量 40%",
                "話速 1.50",
                "読み上げ オフ",
                "自動連絡 オフ",
                "自動連絡（燃料）オフ"
            ]
        );
        for (name, id) in [
            ("四国めたん", 2),
            ("ツンツン", 7),
            ("ずんだもん（あまあま）", 1),
            ("ずんだもん ノーマル", 3),
            ("あまあま", 1),
        ] {
            app.apply(Event::Setting(Setting::Speaker(name.into())));
            assert_eq!(rx.try_recv(), Ok(Command::SetSpeaker(id)), "{name}");
            assert_eq!(saved(&app).speaker, Some(id));
        }
        for name in ["だれか", " "] {
            app.apply(Event::Setting(Setting::Speaker(name.into())));
            assert!(rx.try_recv().is_err());
        }
        assert_eq!(app.errors[0], "話者が見つかりません: だれか");
    }

    #[test]
    fn a_call_is_shown_with_what_it_is_about() {
        let mut app = App::new();
        app.apply(Event::CallStarted {
            turn: 4,
            about: "yellow flag".into(),
        });
        app.apply(Event::Chunk {
            turn: 4,
            text: "イエロー。".into(),
        });
        let t = &app.turns[0];
        assert!(t.call && t.asked());
        assert_eq!(
            (t.heard.as_deref(), t.reply.as_str()),
            (Some("yellow flag"), "イエロー。")
        );
        // A call that says nothing leaves no trace.
        app.apply(Event::TurnDone {
            turn: 5,
            outcome: Outcome::Silent,
            ms: 1,
        });
        assert_eq!(app.turns.len(), 1);
        let _ = app.view();
    }

    #[test]
    fn readiness_says_what_is_still_starting() {
        use Readiness::{Failed, Off, Ready, Starting};
        let mut app = App::new();
        let states = |app: &App| app.readiness().map(|(_, r)| r);
        assert_eq!(
            app.readiness().map(|(l, _)| l),
            ["エージェント", "音声", "テレメトリ", "マイク"]
        );
        assert_eq!(states(&app), [Starting; 4]);
        app.apply(Event::AgentReady { name: "A".into() });
        app.apply(Event::Microphone(true));
        app.apply(Event::Telemetry(true));
        app.apply(Event::Speakers(true));
        assert_eq!(states(&app), [Ready, Starting, Ready, Ready]);
        let _ = app.update(Message::Voices(Ok(vec![])));
        assert_eq!(states(&app), [Ready; 4]);
        app.apply(Event::Speakers(false));
        app.apply(Event::Microphone(false));
        app.apply(Event::Telemetry(false));
        app.apply(Event::AgentClosed { error: None });
        assert_eq!(states(&app), [Failed, Failed, Starting, Failed]);
        app.apply(Event::AgentStarting);
        app.apply(Event::Speakers(true));
        let _ = app.update(Message::Voices(Err("down".into())));
        app.config.watch = " ".into();
        assert_eq!(states(&app), [Starting, Failed, Off, Failed]);
        let _ = app.view();
    }

    #[test]
    fn voices_are_awaited_and_their_absence_is_told_once() {
        use std::cell::{Cell, RefCell};
        let patience = Patience {
            wait: Duration::from_millis(50),
            poll: Duration::from_millis(1),
            slow: Duration::from_millis(1),
        };
        let tries = Cell::new(0);
        let late = Cell::new(0);
        let told = RefCell::new(Vec::new());
        let started = std::time::Instant::now();
        wait_for_voices(
            || {
                tries.set(tries.get() + 1);
                if late.get() == 0 {
                    return Err("down".into());
                }
                // Told: five more tries, then it is there.
                late.set(late.get() + 1);
                if late.get() == 6 {
                    Ok(vec![])
                } else {
                    Err("down".into())
                }
            },
            patience,
            |v| {
                late.set(late.get().max(1));
                told.borrow_mut().push(v);
                true
            },
        );
        assert_eq!(*told.borrow(), [Err("down".to_string()), Ok(vec![])]);
        assert!(started.elapsed() >= patience.wait && tries.get() > 6);
        assert_eq!((PATIENCE.wait.as_secs(), PATIENCE.poll.as_secs()), (60, 2));
        // Nobody listens any more.
        wait_for_voices(
            || Err("x".into()),
            Patience {
                wait: Duration::ZERO,
                ..patience
            },
            |_| false,
        );
    }

    #[test]
    fn residency_is_remembered_and_save_errors_are_shown() {
        let (mut app, _rx, dir) = wired();
        let _ = app.update(Message::CloseToTray(true));
        assert!(saved(&app).close_to_tray);
        let _ = app.update(Message::CloseToTray(false));
        assert!(!saved(&app).close_to_tray);
        // A file where the folder should be.
        std::fs::write(dir.path().join("f"), "").unwrap();
        app.settings_path = Some(dir.path().join("f").join("settings.json"));
        let _ = app.update(Message::CloseToTray(true));
        assert!(app.errors[0].starts_with("設定を保存できません"));
    }

    #[test]
    fn commands_reach_the_engine() {
        let mut app = App::new();
        let (tx, mut rx) = unbounded_channel();
        let _ = app.update(Message::EngineReady(tx));
        let _ = app.update(Message::TogglePtt);
        app.apply(Event::Recording(true));
        let _ = app.update(Message::TogglePtt);
        let _ = app.update(Message::Hotkey(true));
        let _ = app.update(Message::Draft("燃料".into()));
        let _ = app.update(Message::Send);
        let _ = app.update(Message::Muted(true));
        let _ = app.update(Message::Character("missing".into()));
        let _ = app.update(Message::Speaker(Voice {
            id: 8,
            speaker: "v".into(),
            style: "s".into(),
        }));
        let mut got = Vec::new();
        while let Ok(c) = rx.try_recv() {
            got.push(c);
        }
        assert_eq!(
            got,
            vec![
                Command::PttDown,
                Command::PttUp,
                Command::PttDown,
                Command::Ask("燃料".into()),
                Command::SetMuted(true),
                Command::SetSpeaker(8),
            ]
        );
        assert_eq!(app.draft, "");
    }
}
