//! The window (iced + shadcn-iced): conversation, latency, settings.

use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use agent_client_protocol::AcpAgent;
use iced::futures::{SinkExt, Stream};
use iced::{Subscription, Task};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::agent::{self, ToolState, Turn};
use crate::audio::{Mic, NoMic, Output, Recorder, Speakers};
use crate::config::{self, Config};
use crate::engine::{self, Command, Deps, Event, Outcome};
use crate::stt::OpenAiTranscriber;
use crate::voicevox::{Voice, Voicevox};

/// Settings for the engine subscription (`Subscription::run` takes a `fn`).
pub static CONFIG: OnceLock<Config> = OnceLock::new();
/// Result of registering the global push-to-talk key.
pub static HOTKEY: OnceLock<Result<String, String>> = OnceLock::new();

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
    Scrolled(bool),
    ScrollToEnd,
    Settings(bool),
    DismissError,
    /// Caption buttons drawn by the app (Windows). Maximize is native
    /// (`HTMAXBUTTON`), see `chrome`.
    Minimize,
    Close,
    Chrome(crate::chrome::ChromeEvent),
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

#[derive(Debug, Default)]
struct TurnView {
    id: Turn,
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
    chrome: crate::chrome::ChromeState,
}

impl App {
    pub fn new() -> Self {
        Self {
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
            chrome: Default::default(),
        }
    }

    /// The app at startup: also installs the native window chrome.
    pub fn boot() -> (Self, Task<Message>) {
        let install = iced::window::latest()
            .and_then(|id| iced::window::run(id, |w| crate::chrome::install_native_chrome(w)))
            .discard();
        (Self::new(), install)
    }

    /// A conversation in progress, for snapshots and screenshots.
    pub fn sample() -> Self {
        let mut app = Self::new();
        app.agent = AgentStatus::Ready("Claude Agent".into());
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

    fn hotkey(&self) -> Option<&'static str> {
        HOTKEY
            .get()
            .and_then(|h| h.as_ref().ok())
            .map(String::as_str)
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
                let id = CONFIG.get().map_or(3, |c| c.speaker);
                self.voice = voices.iter().find(|v| v.id == id).cloned();
                self.voices = voices;
            }
            Message::Voices(Err(e)) => self.errors.push(e),
            Message::Hotkey(down) => self.send(if down {
                Command::PttDown
            } else {
                Command::PttUp
            }),
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
                self.send(Command::SetSpeaker(v.id));
                self.voice = Some(v);
            }
            Message::Muted(m) => {
                self.muted = m;
                self.send(Command::SetMuted(m));
            }
            Message::Settings(open) => self.settings_open = open,
            Message::Minimize => {
                return iced::window::latest().and_then(|id| iced::window::minimize(id, true));
            }
            Message::Close => return iced::window::latest().and_then(iced::window::close),
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
            Event::AgentClosed { error } => {
                self.agent =
                    AgentStatus::Closed(error.unwrap_or_else(|| "connection closed".into()))
            }
            Event::Recording(r) => self.recording = r,
            Event::Level(l) => self.level = l,
            Event::Speaking(s) => self.speaking = s,
            Event::Error(e) => {
                self.errors.push(e);
                let excess = self.errors.len().saturating_sub(3);
                self.errors.drain(..excess);
            }
            Event::TurnStarted { turn } => self.turns.push(TurnView {
                id: turn,
                ..Default::default()
            }),
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
        Some((Outcome::Failed(e), _)) => parts.push(format!("失敗: {e}")),
        None => {}
    }
    parts.join(" · ")
}

impl TurnView {
    /// The driver's words as shown, `None` while transcribing.
    fn heard_label(&self) -> Option<&str> {
        self.heard
            .as_deref()
            .map(|h| if h.is_empty() { "（無音）" } else { h })
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

pub fn subscription(_: &App) -> Subscription<Message> {
    Subscription::batch([
        Subscription::run(engine_stream),
        Subscription::run(hotkey_stream),
        Subscription::run(chrome_stream),
    ])
}

/// Opens the devices, starts the agent session and the pipeline.
/// Returns the VOICEVOX voices.
pub async fn start_engine(
    cfg: &Config,
    cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
    ev_tx: UnboundedSender<Event>,
) -> Result<Vec<Voice>, String> {
    let recorder: Arc<dyn Recorder> = match Mic::open() {
        Ok(mic) => {
            tracing::info!("microphone: {}", mic.device);
            Arc::new(mic)
        }
        Err(e) => {
            let _ = ev_tx.send(Event::Error(format!("{e:#}")));
            Arc::new(NoMic)
        }
    };
    let output: Arc<dyn Output> = match Speakers::open() {
        Ok(s) => Arc::new(s),
        Err(e) => {
            let _ = ev_tx.send(Event::Error(format!("{e:#}")));
            Arc::new(Silent)
        }
    };
    let voicevox = Arc::new(Voicevox::new(cfg.voicevox_url.clone(), cfg.speed));
    let vv = voicevox.clone();
    let voices = tokio::task::spawn_blocking(move || vv.voices().map_err(|e| format!("{e:#}")))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));

    let (agent_tx, agent_cmd_rx) = unbounded_channel();
    let (agent_ev_tx, agent_rx) = unbounded_channel();
    match AcpAgent::from_str(&config::agent_command(&cfg.agent)) {
        Ok(acp) => {
            tokio::spawn(agent::run(
                acp,
                cfg.agent_cwd(),
                cfg.permissions,
                agent_cmd_rx,
                agent_ev_tx,
            ));
        }
        Err(e) => {
            let _ = agent_ev_tx.send(agent::AgentEvent::Closed {
                error: Some(format!("bad agent command: {e}")),
            });
        }
    }
    tokio::spawn(engine::run(
        Deps {
            recorder,
            transcriber: Arc::new(OpenAiTranscriber::new(cfg.stt())),
            synth: voicevox,
            output,
            agent_tx,
            agent_rx,
            prompt_template: cfg.prompt.clone(),
            speaker: cfg.speaker,
        },
        cmd_rx,
        ev_tx,
    ));
    voices
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
        let voices = start_engine(&cfg, cmd_rx, ev_tx).await;
        let _ = out.send(Message::Voices(voices)).await;
        while let Some(ev) = ev_rx.recv().await {
            if out.send(Message::Engine(ev)).await.is_err() {
                break;
            }
        }
    })
}

/// Output used when no audio device could be opened.
struct Silent;

impl Output for Silent {
    fn play(&self, _: Vec<u8>) {}
    fn stop(&self) {}
    fn is_idle(&self) -> bool {
        true
    }
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
        assert_eq!(app.turns.len(), 2);
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
