//! The radio pipeline: push-to-talk → transcription → agent → speech.
//!
//! Speech starts with the first complete sentence of the reply, while the
//! agent is still writing the rest. Pressing the button again stops the
//! speech (barge-in); the agent is interrupted once there are new words for
//! it, and everything the driver said reaches it. Race events the driver
//! should know about start turns of their own.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::agent::{AgentCmd, AgentEvent, AgentLink, AgentOption, TokenUsage, ToolState, Turn};
use crate::audio::{Output, Recorder};
use crate::calls::{self, Told};
use crate::config::{self, AgentOptions};
use crate::context::{Answer, ContextSource, More};
use crate::directive::{Directive, MAX_VOLUME, SPEED, Setting, Stripper};
use crate::early::Early;
use crate::expr::{Expr, Values};
use crate::gate::{self, Gate, Wish};
use crate::live::Feed;
use crate::quick;
use crate::rules::{self, Lookout, Rules, Scope, Watch};
use crate::spotter::{self, Spotter};
use crate::stt::{SttConfig, Transcriber};
use crate::text::{self, SentenceSplitter};
use crate::voicevox::Synth;
use crate::watch::{self, Dated, Follow, Group, Proactive, Queue, RaceEvent, Timing};
use crate::wav;

/// What the driver hears while the engineer looks something up.
pub const STANDBY: &str = "確認する、少し待って。";

/// In place of an output the session was sent before.
const SAME: &str = "（前に送ったものから変わっていない）";

/// What the driver hears when the agent has nothing to give any more.
pub const EXHAUSTED: &str = "AIの利用枠を使い切った。決まった質問には答えられる。";
/// What the driver hears when the agent does not answer.
pub const NO_AGENT: &str = "考えて答えるのは今できない、数字だけ伝える。";
/// What the driver hears when hardly a word was heard.
pub const UNCLEAR: &str = "聞き取れなかったから状況だけ。";

/// Told to wait once, the driver knows that answers can take a while.
const STANDBY_GAP: Duration = Duration::from_secs(120);

/// Between two calls the agent makes by itself.
const AGENT_GAP: Duration = Duration::from_secs(30);
/// After the limit of the agent was hit, it is left alone for so long.
const REST: Duration = Duration::from_secs(600);
/// Values older than this are not how things are.
const HEARD: Duration = Duration::from_secs(2);
/// Exchanges an agent that was not there is told of.
const REMEMBERED: usize = 40;

/// Recordings shorter than this are treated as accidental presses.
pub const MIN_SPEECH: f32 = 0.3;

/// Spoken when trying out a voice.
pub const SAMPLE: &str = "前との差は1.2秒。いいペースです。次の周、ボックス。";

/// Waits before starting an agent again that went away.
const RECONNECT_S: [u64; 4] = [2, 5, 15, 30];

/// `vr watch` running this long has something to watch.
const WATCHING: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    PttDown,
    PttUp,
    /// A typed question (no microphone needed).
    Ask(String),
    /// Stops the speech and the running turn.
    Cancel,
    SetSpeaker(u32),
    /// Percent.
    SetVolume(u8),
    /// Speech rate in percent.
    SetSpeed(u16),
    SetMuted(bool),
    /// Which events the engineer calls about by itself.
    SetProactive(Proactive),
    /// When to be quiet, what to look out for, what to keep in mind.
    SetRules(Rules),
    /// Speaks [`SAMPLE`] with a voice, even when muted.
    Preview(u32),
    /// Replaces the agent: the old session ends, a new one starts.
    SetAgent(String, AgentOptions),
    /// Model, effort and so on of the agent there is.
    SetAgentOptions(AgentOptions),
    SetStt(SttConfig),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    AgentReady {
        name: String,
    },
    /// Started again after it went away.
    AgentStarting,
    /// What can be chosen about the agent, and what is.
    AgentOptions(Vec<AgentOption>),
    AgentClosed {
        error: Option<String>,
    },
    /// The microphone can be used, or cannot (yet).
    Microphone(bool),
    /// The output device can be used, or cannot (yet).
    Speakers(bool),
    /// Race events are being watched.
    Telemetry(bool),
    Recording(bool),
    Level(f32),
    TurnStarted {
        turn: Turn,
    },
    /// The engineer calls by itself, because of these events.
    CallStarted {
        turn: Turn,
        about: String,
    },
    Transcript {
        turn: Turn,
        text: String,
        ms: u64,
    },
    FirstToken {
        turn: Turn,
        ms: u64,
    },
    Chunk {
        turn: Turn,
        text: String,
    },
    Tool {
        turn: Turn,
        id: String,
        title: Option<String>,
        state: ToolState,
    },
    Permission {
        turn: Turn,
        title: String,
        allowed: bool,
    },
    FirstAudio {
        turn: Turn,
        ms: u64,
    },
    TurnDone {
        turn: Turn,
        outcome: Outcome,
        ms: u64,
    },
    /// Tokens of any turn of the agent, the primer and cancelled ones too.
    Usage {
        turn: Turn,
        usage: TokenUsage,
    },
    /// Cost of the session so far, when the agent reports it.
    Cost {
        amount: f64,
        currency: String,
    },
    /// The driver had the agent change a setting.
    Setting(Setting),
    /// The rules as they are now.
    Rules(Rules),
    /// The engineer's own notes on the strategy; `None`: not any more.
    Plan(Option<String>),
    Speaking(bool),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Answered,
    NoSpeech,
    Cancelled,
    /// A call the agent found not worth making.
    Silent,
    Failed(String),
}

/// Starts an agent session from its command.
pub type Connect = Box<dyn Fn(&str, &AgentOptions) -> AgentLink + Send>;
pub type MakeTranscriber = Box<dyn Fn(&SttConfig) -> Arc<dyn Transcriber> + Send>;

pub struct Deps {
    pub recorder: Arc<dyn Recorder>,
    pub transcriber: Arc<dyn Transcriber>,
    pub synth: Arc<dyn Synth>,
    pub output: Arc<dyn Output>,
    pub make_transcriber: MakeTranscriber,
    pub agent: AgentLink,
    /// What `agent` was started from, to start it again.
    pub agent_command: String,
    pub agent_options: AgentOptions,
    pub connect: Connect,
    /// Data for the question, fetched while the speech is transcribed.
    pub context: Option<Arc<dyn ContextSource>>,
    /// `{text}` is what the driver said, `{context}` the data, `{radio}` the
    /// settings of the radio.
    pub prompt_template: String,
    pub speaker: u32,
    /// Percent.
    pub volume: u8,
    /// Percent.
    pub speed: u16,
    /// Transcribes the words before each pause while the button is held.
    pub early_stt: bool,
    /// What the driver speaks (ISO-639-1).
    pub language: Option<String>,
    /// Race events to call about.
    pub watch: Option<Follow>,
    pub proactive: Proactive,
    pub timing: Timing,
    pub rules: Rules,
    /// Values the rules are checked against.
    pub live: Option<Feed>,
    /// Silence after which the driver is told to wait; `None`: never told.
    pub standby: Option<Duration>,
    /// Where each exchange is kept, one JSON line each.
    pub transcript: Option<PathBuf>,
}

/// The data, without the outputs the session was sent before and has still:
/// it is read again with every later question. `sent` is what each command
/// printed when it was last sent.
fn unsent(sent: &mut BTreeMap<String, String>, data: &str) -> String {
    let blocks: Vec<String> = data
        .split("\n\n$ ")
        .enumerate()
        .map(|(i, b)| {
            if i == 0 {
                b.to_string()
            } else {
                format!("$ {b}")
            }
        })
        .map(|b| {
            let Some((command, out)) = b.split_once('\n').filter(|_| b.starts_with("$ ")) else {
                return b;
            };
            // The first line of most outputs tells how old they are.
            let body: String = out
                .lines()
                .skip_while(|l| l.starts_with('['))
                .collect::<Vec<_>>()
                .join("\n");
            if sent.get(command) == Some(&body) {
                format!("{command}\n{SAME}")
            } else {
                sent.insert(command.to_string(), body);
                b
            }
        })
        .collect();
    blocks.join("\n\n")
}

pub fn render_prompt(template: &str, text: &str, context: Option<&str>) -> String {
    let context = context.map_or_else(
        || "（データは付いていない。vr で取得すること）".to_string(),
        |c| format!("取得したての実データ:\n{c}"),
    );
    // The driver's words last: they must not be searched for placeholders.
    let prompt = template.replace("{context}", &context);
    if prompt.contains("{text}") {
        prompt.replacen("{text}", text, 1)
    } else {
        format!("{prompt}\n{text}")
    }
}

/// The turn at the agent.
struct Current {
    turn: Turn,
    /// When the driver finished speaking (or the question was typed).
    started: Instant,
    splitter: SentenceSplitter,
    stripper: Stripper,
    got_token: bool,
    /// An older question, whose answer comes after that of a newer one.
    spoken: bool,
    /// What a call is about; `None` for the driver's own questions.
    call: Option<Call>,
    /// The driver was told to wait.
    standby: bool,
    /// What the driver said.
    asked: String,
    /// The reply so far.
    said: String,
    /// The agent says that it cannot answer.
    failed: Option<String>,
}

/// Why the engineer does something by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// Something happened.
    Event,
    /// To look at the strategy.
    Plan,
    /// To repair what it wrote for the radio.
    Fix,
}

struct Call {
    about: String,
    why: Why,
    urgent: bool,
    /// Reply so far that may still turn out to be silence.
    lead: String,
    shown: bool,
    /// What a plan looks out for.
    planned: Vec<Watch>,
}

impl Call {
    fn new(about: String, why: Why, urgent: bool) -> Self {
        Self {
            about,
            why,
            urgent,
            lead: String::new(),
            shown: false,
            planned: Vec::new(),
        }
    }
}

/// One sentence to speak.
struct Clip {
    turn: Turn,
    generation: u64,
    started: Instant,
    first: bool,
    text: String,
    /// Overrides the selected voice (trying one out).
    voice: Option<u32>,
    /// Whose speech it is; `None`: speaks at once.
    scope: Option<Scope>,
    urgent: bool,
}

/// What was waited for before asking the agent.
enum Fetched {
    /// The driver's words, with their data.
    Words {
        turn: Turn,
        result: anyhow::Result<String>,
        ms: u64,
        more: More,
        typed: bool,
    },
    /// The data for a call.
    Call { turn: Turn, data: Option<String> },
    /// Words for the driver that need no agent.
    Told {
        about: String,
        text: Option<String>,
        urgent: bool,
    },
    /// How things are, for a question the agent did not answer.
    Instead { turn: Turn, text: Option<String> },
    /// The data for a look at the strategy.
    Plan { turn: Turn, data: Option<String> },
    /// The data for the desk.
    Study { turn: Turn, data: Option<String> },
}

/// Something the desk looks into.
struct Study {
    turn: Turn,
    started: Instant,
    /// When the desk began on it.
    began: Instant,
    asked: String,
    /// Its data is there, and it was sent to the desk.
    sent: bool,
    stripper: Stripper,
    said: String,
}

/// Studies that wait for the desk; more are not remembered by the driver either.
const STUDIES: usize = 3;
/// What the driver is told at once.
const STUDYING: &str = "了解、調べる。";
const STUDYING_YET: &str = "今調べてる。";
/// A longer call asks for something else to be looked into.
const NUDGE: usize = 8;
const STUDY_NEXT: &str = "了解、今の調査の次に調べる。";
const STUDY_FAILED: &str = "調べきれなかった。もう一度言ってくれれば調べ直す。";
/// A look-up that goes on longer is stuck: it would only use up the plan.
const STUDY_LIMIT: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 180 });
/// The same for what the engineer says by itself.
const CALL_LIMIT: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 60 });

/// Exchanges the desk is told of.
const DESK_LOG: usize = 8;

/// Words that fetch the data a plan needs.
const STRATEGY: &str = "作戦";
/// Between two looks at the strategy.
const PLAN_GAP: Duration = Duration::from_secs(180);
/// From this lap on there is something to plan with.
const PLAN_FROM_LAP: f64 = 2.0;

/// WAV clips by voice, speed and text.
type Phrases = Arc<std::sync::Mutex<BTreeMap<(u32, u16, &'static str), Vec<u8>>>>;

/// Something said on the radio, for an agent that was not part of it.
struct Exchange {
    driver: String,
    engineer: String,
    /// The agent knows of it.
    known: bool,
}

/// One exchange at the end of the transcript: what the driver asked (empty
/// for a call of the radio's own) and what was said.
fn keep(path: &Path, asked: &str, said: &str) -> anyhow::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let line = serde_json::json!({
        "ts_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64),
        "asked": asked,
        "said": said,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

impl Exchange {
    fn line(&self) -> String {
        if self.driver.is_empty() {
            format!("連絡「{}」", self.engineer)
        } else {
            format!("無線「{}」→ 返答「{}」", self.driver, self.engineer)
        }
    }
}

/// The limit of the agent's plan is used up.
fn exhausted(error: &str) -> bool {
    let e = error.to_lowercase();
    e.contains("usage limit")
        || e.contains("usagelimit")
        || e.contains("rate limit")
        || e.contains("quota")
}

/// The answer that needs no agent.
async fn local(context: Option<Arc<dyn ContextSource>>, text: String) -> Option<Answer> {
    let c = context.filter(|_| !text.trim().is_empty())?;
    tokio::task::spawn_blocking(move || c.answer(&text))
        .await
        .ok()?
}

/// Fetches what the driver's words call for. Adds the usual data, which is on
/// its way meanwhile.
async fn with_more(
    context: Option<Arc<dyn ContextSource>>,
    text: String,
    data: impl Future<Output = Option<String>>,
) -> More {
    let more = async {
        match context.filter(|_| !text.trim().is_empty()) {
            Some(c) => tokio::task::spawn_blocking(move || c.fetch_for(&text))
                .await
                .unwrap_or_default(),
            None => More::default(),
        }
    };
    let (data, more) = tokio::join!(data, more);
    let parts: Vec<String> = data.into_iter().chain(more.data).collect();
    More {
        data: (!parts.is_empty()).then(|| parts.join("\n\n")),
        ..more
    }
}

/// What the driver's words call for: the answer, or the data for the agent.
async fn look_up(
    quick: bool,
    context: Option<Arc<dyn ContextSource>>,
    text: String,
    data: impl Future<Output = Option<String>>,
) -> More {
    if quick && let Some(answer) = local(context.clone(), text.clone()).await {
        return More {
            answer: Some(answer),
            ..More::default()
        };
    }
    with_more(context, text, data).await
}

/// Starts fetching the data at once; the future waits for it.
fn fetch(
    context: &Option<Arc<dyn ContextSource>>,
) -> impl Future<Output = Option<String>> + Send + 'static {
    let task = context
        .clone()
        .map(|c| tokio::task::spawn_blocking(move || c.fetch()));
    async move { task?.await.ok()? }
}

fn ms(since: Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

/// A reply to a call that says nothing.
fn silence(reply: &str) -> bool {
    matches!(reply.trim(), "" | "-" | "ー" | "−" | "－" | "―" | "—")
}

/// Starting the agent again would end the same way.
fn permanent(error: Option<&str>) -> bool {
    let e = error.unwrap_or_default().to_lowercase();
    e.contains("auth") || e.contains("login") || e.contains("log in") || e.contains("bad agent")
}

/// What speaking takes.
struct Speech {
    synth: Arc<dyn Synth>,
    output: Arc<dyn Output>,
    generation: Arc<AtomicU64>,
    speaker: Arc<AtomicU32>,
    pending: Arc<AtomicUsize>,
    gate: Arc<Gate>,
    events: UnboundedSender<Event>,
}

/// Synthesizes sentences in order and queues them for playback.
/// Clips of an interrupted generation are dropped.
async fn speak(mut clips: UnboundedReceiver<Clip>, speech: Speech) {
    let Speech {
        synth,
        output,
        generation,
        speaker,
        pending,
        gate,
        events,
    } = speech;
    while let Some(clip) = clips.recv().await {
        if clip.generation == generation.load(Ordering::SeqCst) {
            let synth = synth.clone();
            let text = clip.text.clone();
            let id = clip
                .voice
                .unwrap_or_else(|| speaker.load(Ordering::Relaxed));
            let result = tokio::task::spawn_blocking(move || synth.synthesize(&text, id)).await;
            let current = || clip.generation == generation.load(Ordering::SeqCst);
            if let (Ok(Ok(wav)), Some(scope)) = (&result, clip.scope) {
                // Begins when the sentence before it has ended.
                while gate.watches(scope) && current() && !output.is_idle() {
                    tokio::time::sleep(Duration::from_millis(15)).await;
                }
                let wish = Wish {
                    scope,
                    len: gate::length(wav),
                    urgent: clip.urgent,
                    since: Instant::now(),
                };
                gate.wait(wish, current).await;
            }
            match result {
                _ if !current() => {}
                Ok(Ok(wav)) => {
                    output.play(wav);
                    if clip.first {
                        let _ = events.send(Event::FirstAudio {
                            turn: clip.turn,
                            ms: ms(clip.started),
                        });
                    }
                }
                Ok(Err(e)) => {
                    let _ = events.send(Event::Error(format!("speech: {e:#}")));
                }
                Err(e) => {
                    let _ = events.send(Event::Error(format!("speech task: {e}")));
                }
            }
        }
        pending.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Engine {
    recorder: Arc<dyn Recorder>,
    transcriber: Arc<dyn Transcriber>,
    synth: Arc<dyn Synth>,
    output: Arc<dyn Output>,
    make_transcriber: MakeTranscriber,
    agent: AgentLink,
    agent_command: String,
    agent_options: AgentOptions,
    agent_ready: bool,
    /// When to start the agent again, and how often it was tried.
    reconnect: Option<Instant>,
    attempts: usize,
    connect: Connect,
    context: Option<Arc<dyn ContextSource>>,
    prompt_template: String,
    early_stt: bool,
    language: Option<String>,
    events: UnboundedSender<Event>,
    fetched: UnboundedSender<Fetched>,
    clips: UnboundedSender<Clip>,
    /// Clips on their way to the output.
    pending_clips: Arc<AtomicUsize>,
    generation: Arc<AtomicU64>,
    speaker: Arc<AtomicU32>,
    muted: bool,
    volume: u8,
    speed: u16,

    next_turn: Turn,
    current: Option<Current>,
    first_sentence: bool,
    recording: bool,
    early: Option<Early>,
    speaking: bool,
    /// Questions not at the agent yet, and since when.
    waiting: BTreeMap<Turn, Instant>,
    /// Those of them that may hold no words.
    undecided: BTreeSet<Turn>,
    /// The newest question that was sent.
    newest: Turn,
    /// Words that came after those of a newer question.
    late: VecDeque<(Turn, Instant, String, Option<String>)>,
    /// Sentences not spoken because the driver presses the button: spoken
    /// after all if nothing is said.
    held: Vec<Clip>,

    proactive: Proactive,
    queue: Queue,
    rules: Rules,
    /// No calls that can wait.
    quiet: Option<Expr>,
    hushed: bool,
    gate: Arc<Gate>,
    lookout: Lookout,
    box_call: calls::BoxCall,
    plan: Option<String>,
    plan_due: bool,
    plan_lap: Option<f64>,
    last_plan: Option<Instant>,
    /// A plan whose data is being fetched.
    planning: Option<(Turn, Instant)>,
    /// What the agent wrote for the radio and could not be read.
    unreadable: Vec<String>,
    standby: Option<Duration>,
    /// When the driver was last told to wait.
    stood_by: Option<Instant>,
    /// A call whose data is being fetched.
    calling: Option<(Turn, Instant, Vec<Dated>)>,
    watching: Option<bool>,
    spotter: Spotter,
    /// What the spotter said last: newer words take its place.
    spotted: Arc<AtomicU64>,
    /// Its phrases as they sound, by voice and speed.
    phrases: Phrases,
    warmed: Option<(u32, u16)>,
    /// The race as it was when last heard of.
    now: Values,
    heard: Option<Instant>,
    log: VecDeque<Exchange>,
    transcript: Option<PathBuf>,
    /// What each command printed when it was last sent to this session.
    sent: BTreeMap<String, String>,
    /// What was said last, and whether it was a question.
    last_said: String,
    /// When the agent spoke of its own accord.
    last_agent_call: Option<Instant>,
    /// The agent is left alone until then.
    resting: Option<Instant>,
    /// What the desk does, and what it does next.
    study: Option<Study>,
    studies: VecDeque<Study>,
}

impl Engine {
    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn done(&self, turn: Turn, outcome: Outcome, started: Instant) {
        self.emit(Event::TurnDone {
            turn,
            outcome,
            ms: ms(started),
        });
    }

    fn warm(&self, id: u32) {
        let synth = self.synth.clone();
        // A failure shows on the first sentence instead.
        tokio::task::spawn_blocking(move || {
            let _ = synth.warm_up(id);
        });
    }

    /// Stops the speech.
    fn hush(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.output.stop();
    }

    /// The driver may be saying something that replaces the answer.
    fn holding(&self) -> bool {
        self.recording || !self.undecided.is_empty()
    }

    /// Nothing was said: the answer goes on.
    fn release(&mut self) {
        if self.holding() {
            return;
        }
        for clip in std::mem::take(&mut self.held) {
            self.queue_clip(Clip {
                generation: self.generation.load(Ordering::SeqCst),
                ..clip
            });
        }
    }

    fn queue_clip(&self, clip: Clip) {
        self.pending_clips.fetch_add(1, Ordering::SeqCst);
        let _ = self.clips.send(clip);
    }

    /// Ends the turn at the agent on this side; its events are ignored.
    fn drop_current(&mut self) {
        self.unreadable.clear();
        let Some(c) = self.current.take() else {
            return;
        };
        // Planned another time.
        self.plan_due |= c.call.as_ref().is_some_and(|c| c.why == Why::Plan);
        if c.call.as_ref().is_none_or(|call| call.shown) {
            self.done(c.turn, Outcome::Cancelled, c.started);
        }
    }

    /// The plan makes way for what cannot wait.
    fn stop_planning(&mut self) {
        self.plan_due |= self.planning.take().is_some();
        let plan = |c: &Current| {
            c.call
                .as_ref()
                .is_some_and(|c| c.why == Why::Plan && !c.shown)
        };
        if self.current.as_ref().is_some_and(plan) {
            let _ = self.agent.tx.send(AgentCmd::Cancel);
            self.drop_current();
        }
    }

    /// Stops everything, also questions on their way.
    fn cancel_all(&mut self) {
        self.hush();
        self.held.clear();
        let _ = self.agent.tx.send(AgentCmd::Cancel);
        self.drop_current();
        if self.study.as_ref().is_some_and(|s| s.sent) {
            let _ = self.agent.desk.send(AgentCmd::Cancel);
        }
        let studies: Vec<Study> = self
            .study
            .take()
            .into_iter()
            .chain(self.studies.drain(..))
            .collect();
        for s in studies {
            self.done(s.turn, Outcome::Cancelled, s.started);
        }
        self.calling = None;
        self.planning = None;
        self.plan_due = false;
        self.undecided.clear();
        for (turn, started) in std::mem::take(&mut self.waiting) {
            self.done(turn, Outcome::Cancelled, started);
        }
        for (turn, started, ..) in std::mem::take(&mut self.late) {
            self.done(turn, Outcome::Cancelled, started);
        }
    }

    fn begin(&mut self) -> Turn {
        self.next_turn += 1;
        self.waiting.insert(self.next_turn, Instant::now());
        self.emit(Event::TurnStarted {
            turn: self.next_turn,
        });
        self.next_turn
    }

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::PttDown => {
                if self.recording {
                    return;
                }
                self.hush();
                self.stop_planning();
                self.recorder.start();
                self.recording = true;
                self.early = self.early_stt.then(Early::default);
                self.emit(Event::Recording(true));
                if let Some(c) = self.context.clone() {
                    tokio::task::spawn_blocking(move || c.prepare());
                }
            }
            Command::PttUp => self.released(),
            Command::Ask(text) => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    return;
                }
                self.hush();
                self.held.clear();
                self.stop_planning();
                self.drop_current();
                let turn = self.begin();
                self.emit(Event::Transcript {
                    turn,
                    text: text.clone(),
                    ms: 0,
                });
                let data = fetch(&self.context);
                let context = self.context.clone();
                let tx = self.fetched.clone();
                let quick = self.rules.quick;
                tokio::spawn(async move {
                    let more = look_up(quick, context, text.clone(), data).await;
                    let _ = tx.send(Fetched::Words {
                        turn,
                        result: Ok(text),
                        ms: 0,
                        more,
                        typed: true,
                    });
                });
            }
            Command::Cancel => self.cancel_all(),
            Command::Preview(voice) => {
                self.hush();
                self.queue_clip(Clip {
                    turn: 0,
                    generation: self.generation.load(Ordering::SeqCst),
                    started: Instant::now(),
                    first: false,
                    text: SAMPLE.into(),
                    voice: Some(voice),
                    scope: None,
                    urgent: false,
                });
            }
            Command::SetAgentOptions(o) => {
                let changed = |new: &[(String, String)], old: &[(String, String)]| {
                    let new = new.iter().filter(|o| !old.contains(o));
                    new.cloned().collect::<Vec<_>>()
                };
                let radio = changed(&o.radio, &self.agent_options.radio);
                // What the desk shares with the radio changes there too.
                let shared = radio
                    .iter()
                    .filter(|(k, _)| o.desk.iter().all(|(d, _)| d != k));
                let mut desk: Vec<_> = shared.cloned().collect();
                desk.extend(changed(&o.desk, &self.agent_options.desk));
                for (tx, new) in [(&self.agent.tx, radio), (&self.agent.desk, desk)] {
                    if !new.is_empty() {
                        let _ = tx.send(AgentCmd::Set(new));
                    }
                }
                self.agent_options = o;
            }
            Command::SetAgent(command, options) => {
                self.cancel_all();
                self.agent_command = command;
                self.agent_options = options;
                self.attempts = 0;
                self.start_agent();
            }
            Command::SetStt(cfg) => self.transcriber = (self.make_transcriber)(&cfg),
            Command::SetSpeaker(id) => {
                self.speaker.store(id, Ordering::Relaxed);
                self.warm(id);
            }
            Command::SetVolume(v) => self.set_volume(v),
            Command::SetSpeed(v) => self.set_speed(v),
            Command::SetMuted(m) => self.set_muted(m),
            Command::SetProactive(p) => self.set_proactive(p),
            Command::SetRules(r) => self.set_rules(r),
        }
    }

    /// The button was let go.
    fn released(&mut self) {
        if !self.recording {
            return;
        }
        self.recording = false;
        let rec = self.recorder.stop();
        self.emit(Event::Recording(false));
        self.emit(Event::Level(0.0));
        if rec.seconds() < MIN_SPEECH {
            self.release();
            return;
        }
        let turn = self.begin();
        self.undecided.insert(turn);
        let tx = self.fetched.clone();
        let t0 = Instant::now();
        // The data as of the end of the question, while the words are being
        // made out.
        let data = fetch(&self.context);
        let context = self.context.clone();
        let early = self.early.take().unwrap_or_default();
        let text = early.finish(rec, self.transcriber.clone());
        let quick = self.rules.quick;
        tokio::spawn(async move {
            let result = text.await;
            let ms = ms(t0);
            let words = result.as_ref().map(String::clone).unwrap_or_default();
            let more = look_up(quick, context, words, data).await;
            let _ = tx.send(Fetched::Words {
                turn,
                result,
                ms,
                more,
                typed: false,
            });
        });
    }

    fn fetched(&mut self, f: Fetched) {
        match f {
            Fetched::Words {
                turn,
                result,
                ms: took,
                more,
                typed,
            } => {
                // Cancelled meanwhile.
                let Some(started) = self.waiting.remove(&turn) else {
                    return;
                };
                self.undecided.remove(&turn);
                match result {
                    Ok(text) if text.trim().is_empty() => {
                        self.emit(Event::Transcript {
                            turn,
                            text: String::new(),
                            ms: took,
                        });
                        self.done(turn, Outcome::NoSpeech, started);
                        self.release();
                    }
                    Ok(text) => {
                        if !typed {
                            self.emit(Event::Transcript {
                                turn,
                                text: text.clone(),
                                ms: took,
                            });
                        }
                        self.words(turn, started, text, more);
                    }
                    Err(e) => {
                        let error = format!("transcription: {e:#}");
                        self.emit(Event::Error(error.clone()));
                        self.done(turn, Outcome::Failed(error), started);
                        self.release();
                    }
                }
            }
            Fetched::Call { turn, data } => {
                let Some((t, started, events)) = self.calling.take() else {
                    return;
                };
                if t != turn {
                    self.calling = Some((t, started, events));
                } else if self.busy() {
                    // The driver was quicker.
                    self.queue.restore(events);
                } else {
                    self.call(turn, started, &events, data.as_deref());
                }
            }
            Fetched::Told {
                about,
                text,
                urgent,
            } => {
                let Some(text) = text else { return };
                if self.holding() && !urgent {
                    return;
                }
                self.next_turn += 1;
                let (turn, started) = (self.next_turn, Instant::now());
                self.emit(Event::CallStarted { turn, about });
                self.tell(turn, started, &text, Scope::Calls, urgent);
                self.done(turn, Outcome::Answered, started);
                self.remember(String::new(), text, false);
            }
            Fetched::Instead { turn, text } => {
                // Unless the driver went on.
                if let Some(text) = text
                    && turn >= self.newest
                    && self.waiting.is_empty()
                {
                    self.tell(turn, Instant::now(), &text, Scope::All, false);
                    self.remember(String::new(), text, false);
                }
            }
            Fetched::Study { turn, data } => {
                let recent = self.recent();
                let template = config::STUDY_PROMPT.replace("{radio}", &self.status());
                let Some(s) = self.study.as_mut().filter(|s| s.turn == turn) else {
                    return;
                };
                s.sent = true;
                let text = recent + &render_prompt(&template, &s.asked, data.as_deref());
                let cmd = AgentCmd::Prompt {
                    turn,
                    text,
                    keep: true,
                };
                if self.agent.desk.send(cmd).is_err() {
                    self.study_end(Some("エージェントに接続していません".into()));
                }
            }
            Fetched::Plan { turn, data } => {
                let Some((t, started)) = self.planning.filter(|(t, _)| *t == turn) else {
                    return;
                };
                self.planning = None;
                if self.busy() || self.calling.is_some() || !self.queue.is_empty() {
                    self.plan_due = true;
                } else {
                    self.start_plan(t, started, data.as_deref());
                }
            }
        }
    }

    /// Passes the driver's words on.
    fn words(&mut self, turn: Turn, started: Instant, text: String, more: More) {
        if let Some(answer) = more.answer.clone().and_then(|a| self.answer(a)) {
            // The agent goes on with what was asked before.
            if self.current.as_ref().is_some_and(|c| c.call.is_some()) {
                let _ = self.agent.tx.send(AgentCmd::Cancel);
                self.drop_current();
            }
            self.held.clear();
            self.tell(turn, started, &answer, Scope::All, false);
            self.remember(text, answer, false);
            self.done(turn, Outcome::Answered, started);
            return;
        }
        if more.long && self.agent_ready && self.resting.is_none() {
            self.held.clear();
            // Nothing but "look it up": that is what it does.
            if self.study.is_some() && text.chars().count() <= NUDGE {
                self.tell(turn, started, STUDYING_YET, Scope::All, false);
                self.done(turn, Outcome::Answered, started);
                return;
            }
            let said = if self.study.is_some() {
                STUDY_NEXT
            } else {
                STUDYING
            };
            self.tell(turn, started, said, Scope::All, false);
            self.study(turn, started, text);
            return;
        }
        if turn < self.newest && self.current.is_some() {
            // A newer question is being answered: this one follows, unspoken.
            self.late.push_back((turn, started, text, more.data));
            return;
        }
        self.drop_current();
        self.held.clear();
        let spoken = turn > self.newest;
        self.ask(turn, started, &text, more.data.as_deref(), spoken);
    }

    /// Has the desk look into something. Nobody waits for it.
    fn study(&mut self, turn: Turn, started: Instant, asked: String) {
        let s = Study {
            turn,
            started,
            began: Instant::now(),
            asked,
            sent: false,
            stripper: Stripper::default(),
            said: String::new(),
        };
        if self.study.is_none() {
            self.begin_study(s);
        } else if self.studies.len() < STUDIES {
            self.studies.push_back(s);
        } else {
            self.done(turn, Outcome::Cancelled, started);
        }
    }

    fn begin_study(&mut self, mut s: Study) {
        s.began = Instant::now();
        let (turn, text) = (s.turn, s.asked.clone());
        self.study = Some(s);
        let context = self.context.clone();
        let tx = self.fetched.clone();
        tokio::task::spawn_blocking(move || {
            let data = context.and_then(|c| c.fetch_all(&text));
            let _ = tx.send(Fetched::Study { turn, data });
        });
    }

    /// The desk is done, or cannot go on.
    fn study_end(&mut self, error: Option<String>) {
        let Some(mut s) = self.study.take() else {
            return;
        };
        let (rest, found) = s.stripper.finish();
        s.said.push_str(&rest);
        for d in found {
            match d.and_then(known) {
                // It is the desk.
                Ok(Directive::Study(_) | Directive::Plan(_)) => {}
                Ok(d) => self.apply(d),
                Err(e) => tracing::warn!("desk: {e}"),
            }
        }
        let said = s.said.trim().to_string();
        let spent = error.as_deref().is_some_and(exhausted)
            || (exhausted(&said) && text::foreign(&said, self.language.as_deref()));
        if spent {
            self.resting = Some(Instant::now() + REST);
        }
        let error = error.or_else(|| spent.then(|| said.clone()));
        let error = error.or_else(|| said.is_empty().then(|| "no answer".into()));
        let (text, outcome) = match error {
            Some(e) => {
                self.emit(Event::Error(format!("agent: {e}")));
                let text = if spent { EXHAUSTED } else { STUDY_FAILED };
                (text.to_string(), Outcome::Failed(e))
            }
            None => (said, Outcome::Answered),
        };
        self.tell(s.turn, s.started, &text, Scope::Calls, false);
        self.remember(format!("調査: {}", s.asked), text, false);
        self.done(s.turn, outcome, s.started);
        if let Some(next) = self.studies.pop_front() {
            self.begin_study(next);
        }
    }

    /// What was said lately, for the desk.
    fn recent(&self) -> String {
        let skip = self.log.len().saturating_sub(DESK_LOG);
        let lines: Vec<String> = self.log.iter().skip(skip).map(Exchange::line).collect();
        if lines.is_empty() {
            return String::new();
        }
        format!("直前までの交信（古い順）:\n{}\n\n", lines.join("\n"))
    }

    /// The words of an answer that needs no agent; `None`: it does.
    fn answer(&self, a: Answer) -> Option<String> {
        // Whatever the driver says now is the reply.
        let asked = self.last_said.trim_end().ends_with(['？', '?']);
        match a {
            Answer::Text(t) => Some(t),
            Answer::Again if self.last_said.is_empty() => None,
            Answer::Again => Some(self.last_said.clone()),
            Answer::Ack if asked => None,
            Answer::Ack => Some("了解。".into()),
            Answer::Unclear(_) if asked => None,
            Answer::Unclear(status) => Some(format!("{UNCLEAR}{status}")),
        }
    }

    /// Shows and speaks words that are complete.
    fn tell(&mut self, turn: Turn, started: Instant, text: &str, scope: Scope, urgent: bool) {
        self.emit(Event::FirstToken {
            turn,
            ms: ms(started),
        });
        self.emit(Event::Chunk {
            turn,
            text: text.into(),
        });
        if self.muted {
            return;
        }
        let mut splitter = SentenceSplitter::default();
        let mut sentences = splitter.push(text);
        sentences.extend(splitter.finish());
        for (i, text) in sentences.into_iter().enumerate() {
            let clip = Clip {
                turn,
                generation: self.generation.load(Ordering::SeqCst),
                started,
                first: i == 0 && scope == Scope::All,
                text,
                voice: None,
                scope: Some(scope),
                urgent,
            };
            self.speak(clip);
        }
    }

    fn remember(&mut self, driver: String, engineer: String, known: bool) {
        if engineer.trim().is_empty() {
            return;
        }
        self.last_said = engineer.clone();
        if let Some(path) = &self.transcript
            && let Err(e) = keep(path, &driver, &engineer)
        {
            tracing::warn!("transcript {}: {e:#}", path.display());
        }
        self.log.push_back(Exchange {
            driver,
            engineer,
            known,
        });
        while self.log.len() > REMEMBERED {
            self.log.pop_front();
        }
    }

    /// What the agent missed, for the next prompt.
    fn missed(&mut self) -> String {
        let lines: Vec<String> = self
            .log
            .iter_mut()
            .filter_map(|e| {
                if std::mem::replace(&mut e.known, true) {
                    return None;
                }
                Some(e.line())
            })
            .collect();
        if lines.is_empty() {
            String::new()
        } else {
            format!(
                "これまでの交信（済んだこと。古い順）:\n{}\n\n",
                lines.join("\n")
            )
        }
    }

    /// The agent did not answer: how things are, instead of nothing.
    fn instead(&mut self, turn: Turn, error: &str) {
        let spent = exhausted(error);
        if spent {
            self.resting = Some(Instant::now() + REST);
        }
        let said = if spent { EXHAUSTED } else { NO_AGENT };
        let Some(c) = self.context.clone() else {
            return;
        };
        let tx = self.fetched.clone();
        tokio::task::spawn_blocking(move || {
            let text = c.say("status").map(|s| format!("{said}{s}"));
            let _ = tx.send(Fetched::Instead { turn, text });
        });
    }

    /// Tells the driver that the answer is coming, unless it already is.
    fn stand_by(&mut self) {
        let japanese = self.language.as_deref().is_none_or(|l| l == "ja");
        let Some(c) = &mut self.current else { return };
        if c.standby || c.got_token || !c.spoken || c.call.is_some() {
            return;
        }
        c.standby = true;
        if self.standby.is_none() || self.muted || !japanese {
            return;
        }
        if self.stood_by.is_some_and(|t| t.elapsed() < STANDBY_GAP) {
            return;
        }
        self.stood_by = Some(Instant::now());
        let clip = Clip {
            turn: c.turn,
            generation: self.generation.load(Ordering::SeqCst),
            started: c.started,
            // Only the answer counts as one.
            first: false,
            text: STANDBY.into(),
            voice: None,
            scope: Some(Scope::All),
            urgent: false,
        };
        self.speak(clip);
    }

    fn ask(&mut self, turn: Turn, started: Instant, text: &str, data: Option<&str>, spoken: bool) {
        self.newest = self.newest.max(turn);
        let mut radio = self.status();
        if let Some(fight) = calls::pressure(&self.now) {
            radio = format!("{radio}\n{fight}");
        }
        let template = self.prompt_template.replace("{radio}", &radio);
        let data = data.map(|d| unsent(&mut self.sent, d));
        let prompt = self.missed() + &render_prompt(&template, text, data.as_deref());
        let mut cmd = AgentCmd::Prompt {
            turn,
            text: prompt,
            keep: true,
        };
        // The agent went away: started again for this question.
        if let Err(e) = self.agent.tx.send(cmd) {
            cmd = e.0;
            if self.reconnect.is_some() {
                self.start_agent();
                self.emit(Event::AgentStarting);
            }
            if self.agent.tx.send(cmd).is_err() {
                let error = "エージェントに接続していません";
                self.emit(Event::Error(format!("{error}（設定から接続してください）")));
                self.done(turn, Outcome::Failed(error.into()), started);
                self.instead(turn, error);
                return;
            }
        }
        self.start_turn(turn, started, spoken, None);
        if let Some(c) = &mut self.current {
            c.asked = text.into();
        }
    }

    fn start_turn(&mut self, turn: Turn, started: Instant, spoken: bool, call: Option<Call>) {
        self.first_sentence = true;
        self.current = Some(Current {
            turn,
            started,
            splitter: SentenceSplitter::default(),
            stripper: Stripper::default(),
            got_token: false,
            spoken,
            call,
            standby: false,
            asked: String::new(),
            said: String::new(),
            failed: None,
        });
    }

    /// The engineer calls by itself, if the agent finds it worth it.
    fn call(&mut self, turn: Turn, started: Instant, events: &[Dated], data: Option<&str>) {
        let about = watch::summary(events);
        // What the driver asked for waits for a quiet moment like the rest.
        let urgent = events
            .iter()
            .any(|(_, e)| e.urgent() && e.kind != watch::WATCH);
        let template = config::CALL_PROMPT.replace("{radio}", &self.status());
        let text = self.missed() + &render_prompt(&template, &about, data);
        self.last_agent_call = Some(started);
        self.by_itself(turn, started, text, Call::new(about, Why::Event, urgent));
    }

    /// The engineer looks at the strategy, unasked and unheard.
    fn start_plan(&mut self, turn: Turn, started: Instant, data: Option<&str>) {
        self.last_plan = Some(started);
        let template = config::PLAN_PROMPT.replace("{radio}", &self.status());
        let text = render_prompt(&template, "", data);
        let call = Call::new("作戦の見直し".into(), Why::Plan, false);
        self.by_itself(turn, started, text, call);
    }

    /// Has the agent repair what it wrote for the radio.
    fn repair(&mut self, errors: &[String]) {
        self.next_turn += 1;
        let text = config::FIX_PROMPT.replace("{text}", &errors.join(" / "));
        let call = Call::new("設定の書き直し".into(), Why::Fix, false);
        self.by_itself(self.next_turn, Instant::now(), text, call);
    }

    fn by_itself(&mut self, turn: Turn, started: Instant, text: String, call: Call) {
        let cmd = AgentCmd::Prompt {
            turn,
            text,
            keep: false,
        };
        if self.resting.is_none() && self.agent.tx.send(cmd).is_ok() {
            self.start_turn(turn, started, true, Some(call));
        }
    }

    fn start_agent(&mut self) {
        // Dropping the old link ends its process.
        self.agent = (self.connect)(&self.agent_command, &self.agent_options);
        self.agent_ready = false;
        self.reconnect = None;
        self.forgotten();
    }

    fn studies_as(&self, turn: Turn) -> bool {
        self.study.as_ref().is_some_and(|s| s.turn == turn)
    }

    /// The agent is a new one.
    fn forgotten(&mut self) {
        for e in &mut self.log {
            e.known = false;
        }
        self.sent.clear();
    }

    fn agent_event(&mut self, ev: AgentEvent) {
        let mine = |c: &Option<Current>, turn| c.as_ref().is_some_and(|c| c.turn == turn);
        match ev {
            AgentEvent::Cost {
                amount, currency, ..
            } => self.emit(Event::Cost { amount, currency }),
            AgentEvent::Renewed => self.forgotten(),
            AgentEvent::Options(o) => self.emit(Event::AgentOptions(o)),
            AgentEvent::Chunk { turn, text } if self.studies_as(turn) => {
                if let Some(s) = &mut self.study {
                    let (text, found) = s.stripper.push(&text);
                    s.said.push_str(&text);
                    for d in found.into_iter().flatten() {
                        if !matches!(d, Directive::Study(_) | Directive::Plan(_))
                            && let Ok(d) = known(d)
                        {
                            self.apply(d);
                        }
                    }
                }
            }
            AgentEvent::TurnEnd {
                turn,
                usage,
                stop: _,
            } if self.studies_as(turn) => {
                if let Some(usage) = usage {
                    self.emit(Event::Usage { turn, usage });
                }
                self.study_end(None);
            }
            AgentEvent::TurnFailed { turn, error } if self.studies_as(turn) => {
                self.study_end(Some(error));
            }
            AgentEvent::Ready { name } => {
                self.agent_ready = true;
                self.attempts = 0;
                self.emit(Event::AgentReady { name });
            }
            AgentEvent::Closed { error } => {
                self.agent_ready = false;
                if let Some(c) = self.current.take() {
                    self.done(c.turn, Outcome::Failed("agent closed".into()), c.started);
                }
                for s in self.study.take().into_iter().chain(self.studies.drain(..)) {
                    let _ = self.events.send(Event::TurnDone {
                        turn: s.turn,
                        outcome: Outcome::Failed("agent closed".into()),
                        ms: ms(s.started),
                    });
                }
                if !permanent(error.as_deref()) {
                    let wait = RECONNECT_S[self.attempts.min(RECONNECT_S.len() - 1)];
                    self.attempts += 1;
                    self.reconnect = Some(Instant::now() + Duration::from_secs(wait));
                }
                self.emit(Event::AgentClosed { error });
            }
            AgentEvent::Chunk { turn, text } => {
                if let Some(c) = self.current.as_mut().filter(|c| c.turn == turn) {
                    let (text, found) = c.stripper.push(&text);
                    self.settings(found);
                    self.say(text);
                }
            }
            AgentEvent::Tool {
                turn,
                id,
                title,
                state,
            } => {
                let desk = self.studies_as(turn);
                if let Some(s) = self.study.as_mut().filter(|_| desk) {
                    // What it said before was not the answer.
                    s.said.clear();
                }
                if mine(&self.current, turn) || desk {
                    self.emit(Event::Tool {
                        turn,
                        id,
                        title,
                        state,
                    });
                }
            }
            AgentEvent::Permission {
                turn,
                title,
                allowed,
            } => {
                if mine(&self.current, turn) || self.studies_as(turn) {
                    self.emit(Event::Permission {
                        turn,
                        title,
                        allowed,
                    });
                }
            }
            AgentEvent::TurnEnd { turn, usage, .. } => {
                if let Some(usage) = usage {
                    self.emit(Event::Usage { turn, usage });
                }
                if mine(&self.current, turn) {
                    self.turn_end(None);
                }
            }
            AgentEvent::TurnFailed { turn, error } => {
                if mine(&self.current, turn) {
                    self.turn_end(Some(error));
                }
            }
        }
    }

    /// The reply of the current turn is complete, or there will be none.
    fn turn_end(&mut self, error: Option<String>) {
        if let Some(c) = &mut self.current {
            let (text, found) = c.stripper.finish();
            self.settings(found);
            self.say(text);
        }
        let Some(mut c) = self.current.take() else {
            return;
        };
        if let Some(rest) = c.splitter.finish() {
            self.say_sentence(&c, rest);
        }
        let silent = c.call.as_ref().is_some_and(|call| !call.shown);
        if let Some(call) = c.call.as_mut().filter(|c| c.why == Why::Plan) {
            let planned = std::mem::take(&mut call.planned);
            if error.is_none() {
                self.planned(planned);
            }
        }
        let error = error.or(c.failed.take());
        match (&error, c.call.is_some()) {
            // Nothing was said: at least how things are.
            (Some(e), false) if c.said.trim().is_empty() || exhausted(e) => {
                let e = e.clone();
                self.instead(c.turn, &e);
            }
            (Some(e), _) if exhausted(e) => self.resting = Some(Instant::now() + REST),
            (None, _) => {
                let (asked, said) = (std::mem::take(&mut c.asked), std::mem::take(&mut c.said));
                let shown = c.call.as_ref().is_none_or(|call| call.shown);
                if shown && c.spoken {
                    self.remember(asked, said, true);
                }
            }
            _ => {}
        }
        // Once: a model that cannot write it will not learn now.
        let errors = std::mem::take(&mut self.unreadable);
        let repair = error.is_none() && c.call.is_none() && !errors.is_empty();
        let outcome = match error {
            // Nobody asked: not worth an error in the driver's face.
            Some(e) if silent => {
                tracing::warn!("call: {e}");
                Outcome::Failed(e)
            }
            Some(e) => {
                self.emit(Event::Error(format!("agent: {e}")));
                Outcome::Failed(e)
            }
            None if silent => Outcome::Silent,
            None => Outcome::Answered,
        };
        self.done(c.turn, outcome, c.started);
        if let Some((turn, started, text, data)) = self.late.pop_front() {
            self.ask(turn, started, &text, data.as_deref(), false);
        } else if repair && !self.busy() {
            self.repair(&errors);
        }
    }

    /// Shows and speaks text of the current turn.
    fn say(&mut self, mut text: String) {
        let Some(c) = &mut self.current else { return };
        if let Some(call) = c.call.as_mut().filter(|call| !call.shown) {
            call.lead.push_str(&text);
            if silence(&call.lead) || (self.hushed && call.why == Why::Plan) {
                return;
            }
            call.shown = true;
            text = std::mem::take(&mut call.lead);
            let _ = self.events.send(Event::CallStarted {
                turn: c.turn,
                about: call.about.clone(),
            });
        }
        if text.is_empty() {
            return;
        }
        if !c.got_token {
            c.got_token = true;
            let _ = self.events.send(Event::FirstToken {
                turn: c.turn,
                ms: ms(c.started),
            });
        }
        let sentences = c.splitter.push(&text);
        c.said.push_str(&text);
        let _ = self.events.send(Event::Chunk { turn: c.turn, text });
        // Codex writes its errors into the reply.
        if exhausted(&c.said) && text::foreign(&c.said, self.language.as_deref()) {
            c.failed = Some(c.said.clone());
            c.splitter = SentenceSplitter::default();
            return;
        }
        if let Some(c) = self.current.take() {
            for s in sentences {
                self.say_sentence(&c, s);
            }
            self.current = Some(c);
        }
    }

    fn say_sentence(&mut self, c: &Current, text: String) {
        if text::foreign(&text, self.language.as_deref()) {
            self.emit(Event::Error(text));
            return;
        }
        if self.muted || !c.spoken {
            return;
        }
        let clip = Clip {
            turn: c.turn,
            generation: self.generation.load(Ordering::SeqCst),
            started: c.started,
            first: std::mem::take(&mut self.first_sentence),
            text,
            voice: None,
            scope: Some(if c.call.is_some() {
                Scope::Calls
            } else {
                Scope::All
            }),
            urgent: c.call.as_ref().is_some_and(|c| c.urgent),
        };
        self.speak(clip);
    }

    fn speak(&mut self, clip: Clip) {
        if self.holding() {
            self.held.push(clip);
        } else {
            self.queue_clip(clip);
        }
    }

    /// Applies what the agent wrote into its reply, if the driver asked for
    /// it. A plan may set watches of its own.
    fn settings(&mut self, found: Vec<Result<Directive, String>>) {
        for f in found {
            let Some(c) = &mut self.current else { return };
            let why = c.call.as_ref().map(|c| c.why);
            let f = f.and_then(known);
            match (why, f) {
                (Some(Why::Plan), Ok(Directive::Plan(p))) => self.set_plan(Some(p)),
                (Some(Why::Plan), Ok(Directive::Watch(w))) => {
                    if let Some(call) = &mut c.call {
                        call.planned.push(Watch { planned: true, ..w });
                    }
                }
                (Some(Why::Event | Why::Plan), f) => tracing::warn!("call: ignored {f:?}"),
                (None, Ok(Directive::Study(asked))) => {
                    self.next_turn += 1;
                    let turn = self.next_turn;
                    self.emit(Event::CallStarted {
                        turn,
                        about: format!("調査: {asked}"),
                    });
                    self.study(turn, Instant::now(), asked);
                }
                // It worked the first time.
                (Some(Why::Fix), Ok(d)) if d.relative() => {}
                (None | Some(Why::Fix), Ok(d)) => self.apply(d),
                (None, Err(e)) => {
                    self.unreadable.push(e.clone());
                    self.emit(Event::Error(e));
                }
                (Some(Why::Fix), Err(e)) => self.emit(Event::Error(e)),
            }
        }
    }

    fn set_plan(&mut self, plan: Option<String>) {
        if self.plan != plan {
            self.plan = plan.clone();
            self.emit(Event::Plan(plan));
        }
    }

    /// The plan is made: its watches take the place of those before.
    fn planned(&mut self, watches: Vec<Watch>) {
        let mut rules = self.rules.clone();
        rules.watches.retain(|w| !w.planned);
        for w in watches {
            // What the driver asked for stays as it is.
            if rules.watches.iter().all(|x| x.when != w.when) {
                rules.watch(w);
            }
        }
        self.change_rules(rules);
    }

    /// Changes the rules and tells the window.
    fn change_rules(&mut self, rules: Rules) {
        if rules != self.rules {
            self.set_rules(rules);
            self.emit(Event::Rules(self.rules.clone()));
        }
    }

    fn set_rules(&mut self, rules: Rules) {
        if rules.interval_s != self.rules.interval_s {
            let d = Duration::from_secs(rules.interval_s.into());
            self.queue.set_interval(d);
        }
        self.rules = rules;
        let quiet = self.rules.hold(Scope::Quiet);
        self.quiet = quiet.and_then(|q| q.parse().ok());
        self.hushed &= self.quiet.is_some();
        self.gate.set(&self.rules);
        self.lookout.set(&self.rules.watches);
        if self.rules.planning == 0 {
            self.plan_due = false;
        }
    }

    fn apply(&mut self, d: Directive) {
        let setting = match d {
            Directive::Volume(a) => {
                let v = a.of(f64::from(self.volume)).round();
                self.set_volume(v.clamp(0.0, f64::from(MAX_VOLUME)) as u8);
                Setting::Volume(self.volume)
            }
            Directive::Speed(a) => {
                let v = (a.of(f64::from(self.speed) / 100.0) * 100.0).round();
                self.set_speed(v.clamp(0.0, f64::from(u16::MAX)) as u16);
                Setting::Speed(self.speed)
            }
            Directive::Muted(m) => {
                self.set_muted(m);
                Setting::Muted(m)
            }
            Directive::Speaker(s) => Setting::Speaker(s),
            Directive::Proactive(on) => {
                self.set_proactive(Proactive {
                    enabled: on,
                    ..self.proactive
                });
                Setting::Proactive(on)
            }
            Directive::Event(group, on) => {
                let mut p = self.proactive;
                p.set(group, on);
                self.set_proactive(p);
                Setting::Event(group, on)
            }
            Directive::Plan(p) => {
                self.set_plan(Some(p));
                return;
            }
            // Has a turn of its own.
            Directive::Study(_) => return,
            rule => {
                let mut rules = self.rules.clone();
                let Some(setting) = change(&mut rules, rule) else {
                    return;
                };
                self.change_rules(rules);
                setting
            }
        };
        self.emit(Event::Setting(setting));
    }

    fn set_volume(&mut self, v: u8) {
        self.volume = v.min(MAX_VOLUME);
        self.output.set_volume(f32::from(self.volume) / 100.0);
    }

    /// Has the spotter's words ready: a call must not wait for the voice.
    fn warm_spotter(&mut self) {
        let key = (self.speaker.load(Ordering::Relaxed), self.speed);
        if self.warmed == Some(key) || !self.proactive.wants_spotter() {
            return;
        }
        self.warmed = Some(key);
        let (synth, phrases) = (self.synth.clone(), self.phrases.clone());
        tokio::task::spawn_blocking(move || {
            phrases.lock().unwrap().retain(|k, _| (k.0, k.1) == key);
            for text in spotter::PHRASES {
                if let Ok(wav) = synth.synthesize(text, key.0) {
                    let at = (key.0, key.1, text);
                    let wav = wav::begin_at_once(wav);
                    phrases.lock().unwrap().insert(at, wav);
                }
            }
        });
    }

    /// The spotter speaks over everything else.
    fn spot(&mut self, text: &'static str) {
        self.next_turn += 1;
        let (turn, started) = (self.next_turn, Instant::now());
        self.emit(Event::CallStarted {
            turn,
            about: Group::Spotter.label().into(),
        });
        self.emit(Event::Chunk {
            turn,
            text: text.into(),
        });
        self.done(turn, Outcome::Answered, started);
        let n = self.spotted.fetch_add(1, Ordering::SeqCst) + 1;
        let (speaker, speed) = (self.speaker.load(Ordering::Relaxed), self.speed);
        let ready = self
            .phrases
            .lock()
            .unwrap()
            .get(&(speaker, speed, text))
            .cloned();
        if let Some(wav) = ready {
            self.output.interject(wav);
            return;
        }
        let (synth, output, spotted) = (
            self.synth.clone(),
            self.output.clone(),
            self.spotted.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let wav = synth.synthesize(text, speaker);
            // Said by now something newer, or too late to be true.
            let current = spotted.load(Ordering::SeqCst) == n;
            if let Ok(wav) = wav
                && current
                && started.elapsed() < Duration::from_millis(600)
            {
                output.interject(wav::begin_at_once(wav));
            }
        });
    }

    fn set_speed(&mut self, v: u16) {
        self.speed = v.clamp(*SPEED.start(), *SPEED.end());
        self.synth.set_speed(f64::from(self.speed) / 100.0);
    }

    fn set_muted(&mut self, m: bool) {
        self.muted = m;
        if m {
            self.held.clear();
            self.hush();
        }
    }

    fn set_proactive(&mut self, p: Proactive) {
        self.proactive = p;
        self.queue.retain(p);
    }

    /// The settings as the agent sees them, for questions about them.
    fn status(&self) -> String {
        let off: Vec<&str> = Group::ALL
            .into_iter()
            .filter(|g| !self.proactive.get(*g))
            .map(Group::key)
            .collect();
        format!(
            "ラジオの現在値: volume={} mute={} speed={:.2} proactive={}（オフの event: {}）",
            self.volume,
            if self.muted { "on" } else { "off" },
            f64::from(self.speed) / 100.0,
            if self.proactive.enabled { "on" } else { "off" },
            if off.is_empty() {
                "なし".into()
            } else {
                off.join(", ")
            }
        ) + &format!(
            " {}{}",
            self.rules.status(),
            match &self.plan {
                Some(p) => format!("\n作戦メモ: {p}"),
                None => String::new(),
            }
        )
    }

    /// The driver is talking or waiting for an answer.
    fn busy(&self) -> bool {
        self.holding()
            || !self.waiting.is_empty()
            || !self.late.is_empty()
            || self.current.is_some()
    }

    fn race_event(&mut self, e: RaceEvent) {
        match (e.kind.as_str(), e.advice()) {
            ("session_changed", _) => {
                self.plan_due = false;
                self.plan_lap = None;
                self.set_plan(None);
                // What was asked for this race ends with it.
                let rules = self.rules.lasting();
                self.change_rules(rules);
            }
            ("strategy", "plan") if self.rules.planning > 0 => self.plan_due |= self.to_plan(),
            _ => {}
        }
        if self.proactive.wants(&e) {
            if e.urgent() {
                self.stop_planning();
            }
            self.queue.push(Instant::now(), e);
        }
    }

    /// There is something to plan: a stop. Without one the plan is to drive.
    fn to_plan(&self) -> bool {
        let enough = self.now.get("fuel_spare").is_some_and(|f| *f > 0.0);
        !enough || self.now.contains_key("stop_in")
    }

    /// The race as it is now.
    fn live(&mut self, v: Values) {
        let now = Instant::now();
        self.now.clone_from(&v);
        self.heard = Some(now);
        let spots = self.proactive.wants_spotter() && !self.muted && !self.recording;
        if spots && !self.held_back(&v) {
            self.warm_spotter();
            if let Some(text) = self.spotter.check(&v, now) {
                self.spot(text);
            }
        }
        for w in self.lookout.check(&v, now) {
            // The spotter told, or will.
            let beside = w
                .when
                .parse::<Expr>()
                .is_ok_and(|e| calls::spotters(e.keys()));
            if !(beside && self.proactive.wants_spotter()) {
                self.came_true(&w, &v);
            }
        }
        let boxing = self.proactive.enabled && self.proactive.get(Group::Strategy);
        if let Some(text) = self.box_call.check(&v).filter(|_| boxing) {
            // Before pit road, whatever the car does: it cannot wait.
            self.call_out("ピットの合図".into(), text, true);
        }
        let on = |k: &str| v.get(k).is_some_and(|x| *x > 0.0);
        if self.rules.planning > 0
            && on("racing")
            && on("on_track")
            && let Some(lap) = v.get("lap").copied().filter(|l| *l >= PLAN_FROM_LAP)
        {
            let every = f64::from(self.rules.planning);
            if self.plan_lap.is_none_or(|l| lap >= l + every || lap < l) && self.to_plan() {
                self.plan_lap = Some(lap);
                self.plan_due = true;
            }
        }
        self.hushed = self.quiet.as_ref().is_some_and(|q| q.holds(&v));
        self.gate.update(v, now);
    }

    /// Nothing may be said, whoever says it.
    fn held_back(&self, v: &Values) -> bool {
        let hold = self.rules.hold(Scope::All);
        hold.and_then(|h| h.parse::<Expr>().ok())
            .is_some_and(|h| h.holds(v))
    }

    /// A watch: told as it is, or a matter for the engineer.
    fn came_true(&mut self, w: &Watch, v: &Values) {
        if w.once {
            let mut rules = self.rules.clone();
            rules.watches.retain(|x| x != w);
            self.change_rules(rules);
        }
        let order = w
            .act
            .as_deref()
            .filter(|_| self.rules.quick && !w.planned)
            .and_then(quick::order);
        let topic = w.tell.clone().or(order.map(String::from));
        if w.say.is_none()
            && let Some(topic) = topic
        {
            self.tell_topics(vec![Ok(topic)], format!("見張り: {}", w.when), false);
            return;
        }
        let Some(say) = &w.say else {
            let act = w.act.as_deref().unwrap_or("今の値を知らせる");
            let text = format!("見張り「{}」が成立。指示: {act}", w.when);
            self.race_event(if w.planned {
                RaceEvent::planned(format!("作戦で決めた{text}"))
            } else {
                RaceEvent::watch(text)
            });
            return;
        };
        let text = rules::fill(say, v);
        // Told at once, but not on the brakes.
        self.call_out(format!("見張り: {}", w.when), &text, false);
    }

    /// Says words of the engineer's own, as a call of its own.
    fn call_out(&mut self, about: String, text: &str, urgent: bool) {
        self.next_turn += 1;
        let (turn, started) = (self.next_turn, Instant::now());
        self.emit(Event::CallStarted { turn, about });
        self.tell(turn, started, text, Scope::Calls, urgent);
        self.done(turn, Outcome::Answered, started);
        self.remember(String::new(), text.to_string(), false);
    }

    /// Says what needs no agent: words, or topics (`Ok`) of `vr say`.
    fn tell_topics(&mut self, parts: Vec<Result<String, String>>, about: String, urgent: bool) {
        let context = self.context.clone();
        let tx = self.fetched.clone();
        tokio::task::spawn_blocking(move || {
            let words: Vec<String> = parts
                .into_iter()
                .filter_map(|p| match p {
                    Ok(topic) => context.as_ref()?.say(&topic),
                    Err(text) => Some(text),
                })
                .collect();
            let text = (!words.is_empty()).then(|| words.concat());
            let _ = tx.send(Fetched::Told {
                about,
                text,
                urgent,
            });
        });
    }

    /// Events to tell: in words that need no agent, or a matter for it.
    /// Returns those it has to think about.
    fn tell_events(&mut self, events: Vec<Dated>) -> Vec<Dated> {
        let mut ask = Vec::new();
        let mut parts = Vec::new();
        // Old gaps are not told as new ones.
        let fresh = self.heard.is_some_and(|t| t.elapsed() < HEARD);
        let race = if fresh {
            self.now.clone()
        } else {
            Values::new()
        };
        let all: Vec<RaceEvent> = events.iter().map(|(_, e)| e.clone()).collect();
        let at = events.first().map_or_else(Instant::now, |(t, _)| *t);
        let mut about: Vec<String> = Vec::new();
        let mut urgent = false;
        for e in calls::merge(&all) {
            let part = match calls::tell(&e, &race) {
                Told::Nothing => continue,
                Told::Ask => {
                    ask.push((at, e));
                    continue;
                }
                Told::Say(text) => Err(text),
                Told::Topic(topic) => Ok(topic.to_string()),
            };
            urgent |= e.urgent();
            let label = calls::about(&e);
            if !about.contains(&label) {
                about.push(label);
            }
            if !parts.contains(&part) {
                parts.push(part);
            }
        }
        if !parts.is_empty() {
            self.tell_topics(parts, about.join("・"), urgent);
        }
        ask
    }

    /// Stops turns of the agent that do not end.
    fn give_up(&mut self) {
        if self
            .study
            .as_ref()
            .is_some_and(|s| s.began.elapsed() >= STUDY_LIMIT)
        {
            let _ = self.agent.desk.send(AgentCmd::Cancel);
            self.study_end(Some("時間切れ".into()));
        }
        let stuck = |c: &Current| c.call.is_some() && c.started.elapsed() >= CALL_LIMIT;
        if self.current.as_ref().is_some_and(stuck) {
            let _ = self.agent.tx.send(AgentCmd::Cancel);
            if let Some(c) = self.current.take() {
                // Not tried again: it would be stuck again.
                tracing::warn!("call: no end after {CALL_LIMIT:?}");
                self.done(c.turn, Outcome::Failed("時間切れ".into()), c.started);
            }
        }
    }

    /// The agent may speak of its own accord.
    fn agent_may_call(&self) -> bool {
        self.agent_ready
            && self.resting.is_none()
            && self
                .last_agent_call
                .is_none_or(|t| t.elapsed() >= AGENT_GAP)
    }

    fn tick(&mut self, telemetry: Option<bool>) {
        if self.recording {
            self.emit(Event::Level(self.recorder.level()));
            if let Some(e) = &mut self.early {
                e.update(self.recorder.as_ref(), &self.transcriber);
            }
        }
        let now = !self.output.is_idle();
        if now != self.speaking {
            self.speaking = now;
            self.emit(Event::Speaking(now));
        }
        if telemetry != self.watching {
            self.watching = telemetry;
            if let Some(t) = telemetry {
                self.emit(Event::Telemetry(t));
            }
        }
        if self.reconnect.is_some_and(|t| t <= Instant::now()) {
            self.start_agent();
            self.emit(Event::AgentStarting);
        }
        if let (Some(c), Some(wait)) = (&self.current, self.standby)
            && c.started.elapsed() >= wait
        {
            self.stand_by();
        }
        self.give_up();
        if self.resting.is_some_and(|t| t <= Instant::now()) {
            self.resting = None;
        }
        let idle = !self.busy()
            && self.calling.is_none()
            && !self.speaking
            && self.pending_clips.load(Ordering::SeqCst) == 0;
        if self.hushed {
            self.queue.keep_urgent();
        }
        let due = self.queue.due(Instant::now(), idle);
        let ask = due
            .map(|events| self.tell_events(events))
            .unwrap_or_default();
        // What cannot be asked now is old by the time it can.
        if !ask.is_empty() && self.agent_may_call() {
            let events = ask;
            self.next_turn += 1;
            let turn = self.next_turn;
            let strategy = events.iter().any(|(_, e)| e.kind == "strategy");
            self.calling = Some((turn, Instant::now(), events));
            let data = fetch(&self.context);
            let context = self.context.clone().filter(|_| strategy);
            let tx = self.fetched.clone();
            tokio::spawn(async move {
                let data = with_more(context, STRATEGY.into(), data).await.data;
                let _ = tx.send(Fetched::Call { turn, data });
            });
        } else if idle
            && self.agent_ready
            && self.resting.is_none()
            && self.plan_due
            && self.planning.is_none()
            && self.queue.is_empty()
            && self.last_plan.is_none_or(|t| t.elapsed() >= PLAN_GAP)
        {
            self.plan_due = false;
            self.next_turn += 1;
            let turn = self.next_turn;
            self.planning = Some((turn, Instant::now()));
            let data = fetch(&self.context);
            let context = self.context.clone();
            let tx = self.fetched.clone();
            tokio::spawn(async move {
                let data = with_more(context, STRATEGY.into(), data).await.data;
                let _ = tx.send(Fetched::Plan { turn, data });
            });
        }
    }
}

/// Reads only values there are.
fn known(d: Directive) -> Result<Directive, String> {
    let check = |e: &Expr| match e.keys().into_iter().find(|k| !config::live_key(k)) {
        Some(k) => Err(format!("条件に無い値: {k}（{e}）")),
        None => Ok(()),
    };
    match &d {
        Directive::Hold(_, Some(e)) => check(e)?,
        Directive::Watch(w) => check(&w.when.parse()?)?,
        _ => {}
    }
    Ok(d)
}

/// Seconds the driver may ask for.
const INTERVAL: std::ops::RangeInclusive<f64> = 0.0..=3600.0;
const HOLD_MAX: std::ops::RangeInclusive<f64> = 0.0..=120.0;

/// Applies a rule; `None` when there is nothing to tell.
fn change(r: &mut Rules, d: Directive) -> Option<Setting> {
    let seconds = |a: crate::directive::Amount, now: u16, range: std::ops::RangeInclusive<f64>| {
        a.of(f64::from(now))
            .round()
            .clamp(*range.start(), *range.end()) as u16
    };
    Some(match d {
        Directive::Interval(a) => {
            r.interval_s = seconds(a, r.interval_s, INTERVAL);
            Setting::Interval(r.interval_s)
        }
        Directive::HoldMax(a) => {
            r.hold_max_s = seconds(a, r.hold_max_s, HOLD_MAX);
            Setting::HoldMax(r.hold_max_s)
        }
        Directive::Hold(scope, when) => {
            let when = when.map(|e| e.to_string());
            r.set_hold(scope, when.clone());
            Setting::Hold(scope, when)
        }
        Directive::Note(n) => {
            r.note(&n);
            Setting::Note(n)
        }
        Directive::Forget(what) => Setting::Forgot(r.forget(&what)),
        Directive::Watch(w) => {
            let text = w.to_string();
            r.watch(w);
            Setting::Watch(text)
        }
        Directive::Planning(n) => {
            r.planning = n;
            Setting::Planning(n)
        }
        Directive::Quick(on) => {
            r.quick = on;
            Setting::Quick(on)
        }
        _ => return None,
    })
}

async fn next_values(live: &mut Option<Feed>) -> Option<Values> {
    match live {
        Some(f) => f.values.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_event(watch: &mut Option<Follow>) -> Option<RaceEvent> {
    match watch {
        Some(w) => w.events.recv().await,
        None => std::future::pending().await,
    }
}

pub async fn run(deps: Deps, mut cmds: UnboundedReceiver<Command>, events: UnboundedSender<Event>) {
    let Deps {
        recorder,
        transcriber,
        synth,
        output,
        make_transcriber,
        agent,
        agent_command,
        agent_options,
        connect,
        context,
        prompt_template,
        speaker,
        volume,
        speed,
        early_stt,
        language,
        mut watch,
        proactive,
        timing,
        rules,
        mut live,
        standby,
        transcript,
    } = deps;
    let gate = Arc::new(Gate::default());
    let generation = Arc::new(AtomicU64::new(0));
    let speaker = Arc::new(AtomicU32::new(speaker));
    let pending_clips = Arc::new(AtomicUsize::new(0));
    let (clip_tx, clip_rx) = unbounded_channel::<Clip>();
    let (fetched_tx, mut fetched_rx) = unbounded_channel::<Fetched>();
    tokio::spawn(speak(
        clip_rx,
        Speech {
            synth: synth.clone(),
            output: output.clone(),
            generation: generation.clone(),
            speaker: speaker.clone(),
            pending: pending_clips.clone(),
            gate: gate.clone(),
            events: events.clone(),
        },
    ));
    let mut engine = Engine {
        recorder,
        transcriber,
        synth,
        output,
        make_transcriber,
        agent,
        agent_command,
        agent_options,
        agent_ready: false,
        reconnect: None,
        attempts: 0,
        connect,
        context,
        prompt_template,
        early_stt,
        language,
        events,
        fetched: fetched_tx,
        clips: clip_tx,
        pending_clips,
        generation,
        speaker,
        muted: false,
        volume,
        speed,
        next_turn: 0,
        current: None,
        first_sentence: true,
        recording: false,
        early: None,
        speaking: false,
        waiting: BTreeMap::new(),
        undecided: BTreeSet::new(),
        newest: 0,
        late: VecDeque::new(),
        held: Vec::new(),
        proactive,
        queue: Queue::new(timing),
        rules: Rules::default(),
        quiet: None,
        hushed: false,
        gate,
        lookout: Lookout::default(),
        box_call: calls::BoxCall::default(),
        plan: None,
        plan_due: false,
        plan_lap: None,
        last_plan: None,
        planning: None,
        unreadable: Vec::new(),
        standby,
        calling: None,
        watching: None,
        spotter: Spotter::default(),
        spotted: Arc::default(),
        phrases: Arc::default(),
        warmed: None,
        now: Values::new(),
        heard: None,
        log: VecDeque::new(),
        sent: BTreeMap::new(),
        last_said: String::new(),
        last_agent_call: None,
        resting: None,
        study: None,
        studies: VecDeque::new(),
        stood_by: None,
        transcript,
    };
    engine.warm(engine.speaker.load(Ordering::Relaxed));
    engine.set_volume(volume);
    engine.set_speed(speed);
    engine.set_rules(rules);

    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(cmd) => engine.command(cmd),
                None => break,
            },
            Some(f) = fetched_rx.recv() => engine.fetched(f),
            Some(ev) = engine.agent.rx.recv() => engine.agent_event(ev),
            Some(ev) = next_event(&mut watch) => engine.race_event(ev),
            Some(v) = next_values(&mut live) => engine.live(v),
            _ = tick.tick() => engine.tick(watch.as_ref().map(|w| w.running(WATCHING))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn each_exchange_is_kept_on_a_line_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radio").join("radio.jsonl");
        keep(&path, "前との差は？", "1.2秒。").unwrap();
        keep(&path, "", "残り5周。").unwrap();
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            (lines[0]["asked"].as_str(), lines[0]["said"].as_str()),
            (Some("前との差は？"), Some("1.2秒。"))
        );
        assert_eq!(lines[1]["asked"], "");
        assert!(lines[1]["ts_ms"].as_u64().unwrap() >= lines[0]["ts_ms"].as_u64().unwrap());
    }
    use crate::agent::{SessionConfig, fake};
    use crate::audio::Recording;
    use crate::policy::Permissions;

    struct FakeMic {
        seconds: f32,
        started: AtomicBool,
    }
    impl Recorder for FakeMic {
        fn start(&self) {
            self.started.store(true, Ordering::SeqCst);
        }
        fn stop(&self) -> Recording {
            Recording {
                samples: vec![0.1; (self.seconds * 16_000.0) as usize],
                rate: 16_000,
            }
        }
        fn level(&self) -> f32 {
            0.25
        }
        fn peek(&self, from: usize) -> Recording {
            let mut r = self.stop();
            r.samples.drain(..from.min(r.samples.len()));
            r
        }
    }

    struct FakeStt(&'static str);
    impl Transcriber for FakeStt {
        fn transcribe(&self, wav: &[u8]) -> anyhow::Result<String> {
            assert_eq!(&wav[..4], b"RIFF");
            Ok(self.0.to_string())
        }
    }

    struct FakeContext;
    impl ContextSource for FakeContext {
        fn fetch(&self) -> Option<String> {
            Some("P4".into())
        }
        fn fetch_for(&self, text: &str) -> More {
            More {
                data: text.contains("BB").then(|| "BB 54".into()),
                long: text.contains("調べて"),
                answer: None,
            }
        }
        fn answer(&self, text: &str) -> Option<Answer> {
            match text {
                "順位" => Some(Answer::Text("4番手。前まで0.62秒。".into())),
                "もう一回" => Some(Answer::Again),
                "はい" => Some(Answer::Ack),
                "ん" => Some(Answer::Unclear("今4番手。".into())),
                _ => None,
            }
        }
        fn say(&self, topic: &str) -> Option<String> {
            Some(format!("{topic}の答え。"))
        }
        fn fetch_all(&self, text: &str) -> Option<String> {
            Some(format!("{text}の全データ"))
        }
    }

    struct EchoSynth(Arc<Mutex<Vec<u32>>>, Arc<Mutex<Vec<f64>>>);
    impl Synth for EchoSynth {
        fn synthesize(&self, text: &str, speaker: u32) -> anyhow::Result<Vec<u8>> {
            Ok(format!("{speaker}:{text}").into_bytes())
        }
        fn warm_up(&self, speaker: u32) -> anyhow::Result<()> {
            self.0.lock().unwrap().push(speaker);
            Ok(())
        }
        fn set_speed(&self, speed: f64) {
            self.1.lock().unwrap().push(speed);
        }
    }

    #[derive(Default)]
    struct FakeOut {
        played: Mutex<Vec<String>>,
        stops: AtomicU32,
        volumes: Mutex<Vec<f32>>,
    }
    impl Output for FakeOut {
        fn play(&self, wav: Vec<u8>) {
            self.played
                .lock()
                .unwrap()
                .push(String::from_utf8(wav).unwrap());
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
        fn is_idle(&self) -> bool {
            true
        }
        fn set_volume(&self, v: f32) {
            self.volumes.lock().unwrap().push(v);
        }
    }

    struct Harness {
        race: UnboundedSender<RaceEvent>,
        live: UnboundedSender<Values>,
        speeds: Arc<Mutex<Vec<f64>>>,
        connects: Arc<Mutex<Vec<String>>>,
        cmds: UnboundedSender<Command>,
        events: UnboundedReceiver<Event>,
        out: Arc<FakeOut>,
        prompts: Arc<Mutex<Vec<String>>>,
        warmed: Arc<Mutex<Vec<u32>>>,
    }

    const FAST: Timing = Timing {
        coalesce: Duration::from_millis(100),
        max_age: Duration::from_secs(10),
        min_interval: Duration::from_millis(400),
    };

    fn start(reply: &'static [&'static str], stt: &'static str, seconds: f32) -> Harness {
        start_with(
            reply,
            Arc::new(FakeStt(stt)),
            seconds,
            "無線:「{text}」{context}",
        )
    }

    fn start_with(
        reply: &'static [&'static str],
        stt: Arc<dyn Transcriber>,
        seconds: f32,
        template: &str,
    ) -> Harness {
        start_full(reply, stt, seconds, template, None)
    }

    fn start_full(
        reply: &'static [&'static str],
        stt: Arc<dyn Transcriber>,
        seconds: f32,
        template: &str,
        standby: Option<Duration>,
    ) -> Harness {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let link = |reply, prompts| {
            AgentLink::spawn(
                fake::agent(reply, prompts),
                SessionConfig {
                    cwd: std::env::temp_dir(),
                    permissions: Permissions::Safe,
                    options: vec![],
                    primer: None,
                    desk_primer: None,
                    desk_options: vec![],
                    renew: crate::agent::Renew::NEVER,
                },
            )
        };
        let other = prompts.clone();
        let out = Arc::new(FakeOut::default());
        let warmed = Arc::new(Mutex::new(Vec::new()));
        let (cmds, cmd_rx) = unbounded_channel();
        let (ev_tx, events) = unbounded_channel();
        let (race, race_rx) = unbounded_channel();
        let (live, live_rx) = unbounded_channel();
        let speeds = Arc::new(Mutex::new(Vec::new()));
        let connects = Arc::new(Mutex::new(Vec::new()));
        let connected = connects.clone();
        tokio::spawn(run(
            Deps {
                recorder: Arc::new(FakeMic {
                    seconds,
                    started: AtomicBool::new(false),
                }),
                transcriber: stt,
                make_transcriber: Box::new(|cfg| {
                    assert_eq!(cfg.model, "new");
                    Arc::new(FakeStt("聞き直し"))
                }),
                synth: Arc::new(EchoSynth(warmed.clone(), speeds.clone())),
                output: out.clone(),
                agent: link(reply, prompts.clone()),
                agent_command: "other".into(),
                agent_options: AgentOptions::default(),
                standby,
                transcript: None,
                connect: Box::new(move |command, _| {
                    connected.lock().unwrap().push(command.to_string());
                    match command {
                        "other" => link(&["別の返答。"], other.clone()),
                        e => AgentLink::failed(e.into()),
                    }
                }),
                context: Some(Arc::new(FakeContext)),
                prompt_template: template.into(),
                speaker: 3,
                volume: 100,
                speed: 120,
                early_stt: true,
                language: Some("ja".into()),
                watch: Some(Follow::of(race_rx)),
                proactive: Proactive::default(),
                timing: FAST,
                rules: Rules {
                    planning: 2,
                    ..Rules::default()
                },
                live: Some(Feed::of(live_rx)),
            },
            cmd_rx,
            ev_tx,
        ));
        Harness {
            race,
            live,
            speeds,
            connects,
            cmds,
            events,
            out,
            prompts,
            warmed,
        }
    }

    impl Harness {
        /// Events up to and including the first one matching `stop`, minus levels and telemetry.
        async fn until(&mut self, stop: impl Fn(&Event) -> bool) -> Vec<Event> {
            let mut seen = Vec::new();
            loop {
                let e = tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                    .await
                    .unwrap_or_else(|_| panic!("timed out; saw {seen:?}"))
                    .expect("engine stopped");
                let done = stop(&e);
                if !matches!(e, Event::Level(_) | Event::Telemetry(_)) {
                    seen.push(e);
                }
                if done {
                    return seen;
                }
            }
        }

        async fn prompted(&self, n: usize) {
            for _ in 0..200 {
                if self.prompts.lock().unwrap().len() >= n {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("{n} prompts expected");
        }

        async fn played(&self, n: usize) -> Vec<String> {
            for _ in 0..200 {
                if self.out.played.lock().unwrap().len() >= n {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            self.out.played.lock().unwrap().clone()
        }
    }

    fn is_done(e: &Event) -> bool {
        matches!(e, Event::TurnDone { .. })
    }

    /// The voice is loaded at start and when it changes, before it speaks.
    #[tokio::test]
    async fn the_voice_is_warmed_up_before_it_is_needed() {
        let h = start(&["了解。"], "q", 1.0);
        h.cmds.send(Command::SetSpeaker(13)).unwrap();
        for _ in 0..200 {
            if h.warmed.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut w = h.warmed.lock().unwrap().clone();
        w.sort();
        assert_eq!(w, [3, 13]);
    }

    #[tokio::test]
    async fn push_to_talk_round_trip_speaks_each_sentence() {
        let mut h = start(
            &["前の Ito まで 0.", "6。毎周 0.4 ", "詰めてる"],
            "前との差は？",
            1.0,
        );
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::PttDown).unwrap();
        h.cmds.send(Command::PttUp).unwrap();
        let events = h.until(is_done).await;

        let kinds: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Recording(r) => Some(format!("rec:{r}")),
                Event::TurnStarted { turn } => Some(format!("start:{turn}")),
                Event::Transcript { text, .. } => Some(format!("heard:{text}")),
                Event::FirstToken { .. } => Some("token".into()),
                Event::Tool { title, state, .. } => Some(format!("tool:{title:?}:{state:?}")),
                Event::Chunk { text, .. } => Some(format!("chunk:{text}")),
                Event::TurnDone { outcome, .. } => Some(format!("done:{outcome:?}")),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "rec:true",
                "rec:false",
                "start:1",
                "heard:前との差は？",
                "tool:Some(\"`vr brief`\"):Running",
                "tool:None:Done",
                "token",
                "chunk:前の Ito まで 0.",
                "chunk:6。毎周 0.4 ",
                "chunk:詰めてる",
                "done:Answered",
            ]
        );
        assert_eq!(
            h.played(2).await,
            vec!["3:前のItoまで0.6。", "3:毎周0.4詰めてる"]
        );
        assert_eq!(
            *h.prompts.lock().unwrap(),
            vec!["無線:「前との差は？」取得したての実データ:\nP4".to_string()]
        );
        let audio = h.until(|e| matches!(e, Event::FirstAudio { .. })).await;
        assert!(matches!(
            audio.last(),
            Some(Event::FirstAudio { turn: 1, .. })
        ));
    }

    fn patient(reply: &'static [&'static str], standby: u64) -> Harness {
        start_full(
            reply,
            Arc::new(FakeStt("BBは")),
            1.0,
            "{text}{context}",
            Some(Duration::from_millis(standby)),
        )
    }

    fn waited() -> String {
        format!("3:{STANDBY}")
    }

    #[tokio::test]
    async fn data_for_the_question_goes_along() {
        let mut h = patient(&["54。"], 60_000);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::PttDown).unwrap();
        h.cmds.send(Command::PttUp).unwrap();
        h.until(is_done).await;
        h.cmds.send(Command::Ask("BBと順位".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(
            *h.prompts.lock().unwrap(),
            [
                "BBは取得したての実データ:\nP4\n\nBB 54",
                "BBと順位取得したての実データ:\nP4\n\nBB 54"
            ]
        );
    }

    #[test]
    fn what_the_session_has_is_not_sent_again() {
        let mut sent = BTreeMap::new();
        let first = "$ vr brief
[live age=3ms]
P4

$ vr sessions corners
スプーン +0.1";
        assert_eq!(unsent(&mut sent, first), first);
        // The age changes, nothing else: the same.
        let again = "$ vr brief
[live age=9ms]
P4

$ vr sessions corners
スプーン +0.2";
        assert_eq!(
            unsent(&mut sent, again),
            format!(
                "$ vr brief
{SAME}

$ vr sessions corners
スプーン +0.2"
            )
        );
        // What is not a command goes as it is.
        for _ in 0..2 {
            assert_eq!(
                unsent(
                    &mut sent,
                    "P4

BB 54"
                ),
                "P4

BB 54"
            );
        }
        // A new session has nothing yet.
        sent.clear();
        assert_eq!(unsent(&mut sent, again), again);
    }

    #[tokio::test]
    async fn looking_something_up_is_not_announced() {
        // The fake agent runs a command before it answers.
        let mut h = patient(&["54。"], 60_000);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("何か".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(1).await, ["3:54。"]);
        // The answer is what the latency is measured by.
        let events = h.until(|e| matches!(e, Event::FirstAudio { .. })).await;
        assert!(events.iter().all(|e| !matches!(e, Event::Error(_))));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(h.out.played.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_desk_looks_into_it_while_the_driver_asks_on() {
        let mut h = start(
            &[
                "はい。",
                "@調査@[[radio volume=80]]空力は無傷。",
                "@見たか@見た。",
            ],
            "x",
            1.0,
        );
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("順位".into())).unwrap();
        h.until(is_done).await;
        h.played(2).await;
        h.cmds.send(Command::Ask("空力を調べて".into())).unwrap();
        let events = h.until(is_done).await;
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                Event::Chunk { turn: 2, text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "了解、調べる。空力は無傷。");
        assert!(events.contains(&Event::Setting(Setting::Volume(80))));
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone {
                turn: 2,
                outcome: Outcome::Answered,
                ..
            })
        ));
        let played = h.played(4).await;
        assert_eq!(played[2..], ["3:了解、調べる。", "3:空力は無傷。"]);
        let asked = h.prompts.lock().unwrap()[0].clone();
        assert!(
            asked.starts_with("直前までの交信（古い順）:\n無線「順位」→ 返答「4番手。"),
            "{asked}"
        );
        assert!(asked.contains("「空力を調べて」") && asked.contains("空力を調べての全データ"));
        // The radio knows of it.
        h.cmds.send(Command::Ask("見たか".into())).unwrap();
        h.until(is_done).await;
        let next = h.prompts.lock().unwrap()[1].clone();
        assert!(
            next.contains("無線「調査: 空力を調べて」→ 返答「空力は無傷。」"),
            "{next}"
        );
    }

    #[tokio::test]
    async fn studies_wait_for_each_other_and_not_the_driver_for_them() {
        let mut h = start(&["はい。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("slow 調べて".into())).unwrap();
        h.prompted(1).await;
        h.cmds.send(Command::Ask("調べて".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(2).await[1], "3:今調べてる。");
        h.cmds
            .send(Command::Ask("次はタイヤも調べて".into()))
            .unwrap();
        h.played(3).await;
        // The radio answers meanwhile.
        h.cmds.send(Command::Ask("何か".into())).unwrap();
        let events = h.until(is_done).await;
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone { turn: 4, .. })
        ));
        assert_eq!(
            h.played(4).await,
            [
                "3:了解、調べる。",
                "3:今調べてる。",
                "3:了解、今の調査の次に調べる。",
                "3:はい。"
            ]
        );
        // Everything stops when the driver says so.
        h.cmds.send(Command::Cancel).unwrap();
        for turn in [1, 3] {
            let e = h.until(is_done).await;
            let done = Event::TurnDone {
                turn,
                outcome: Outcome::Cancelled,
                ms: 0,
            };
            let same = |e: &Event| matches!((e, &done), (Event::TurnDone { turn: a, outcome: x, .. }, Event::TurnDone { turn: b, outcome: y, .. }) if a == b && x == y);
            assert!(same(e.last().unwrap()), "{e:?}");
        }
        // No more than three wait.
        h.cmds.send(Command::Ask("slow 調べて".into())).unwrap();
        h.prompted(3).await;
        for _ in 0..4 {
            h.cmds
                .send(Command::Ask("ほかのことも調べて".into()))
                .unwrap();
        }
        let e = h.until(is_done).await;
        assert!(matches!(
            e.last(),
            Some(Event::TurnDone {
                turn: 6..=9,
                outcome: Outcome::Cancelled,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_look_up_without_end_is_given_up() {
        let mut h = start(&["はい。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("slow 調べて".into())).unwrap();
        h.prompted(1).await;
        let e = h.until(is_done).await;
        assert!(
            matches!(e.last(), Some(Event::TurnDone { turn: 1, outcome: Outcome::Failed(f), ms }) if f == "時間切れ" && *ms >= 2000),
            "{e:?}"
        );
        assert_eq!(
            h.played(3).await,
            [
                "3:了解、調べる。",
                "3:調べきれなかった。",
                "3:もう一度言ってくれれば調べ直す。"
            ]
        );
        // The desk is free again.
        h.cmds.send(Command::Ask("空力を調べて".into())).unwrap();
        h.prompted(2).await;
    }

    #[tokio::test]
    async fn the_engineer_may_have_the_desk_look_into_it() {
        let mut h = start(
            &[
                "左前が減る。[[radio study=\"左前の原因\"]]",
                "@調査@キャンバーの付けすぎ。",
            ],
            "x",
            1.0,
        );
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("何か".into())).unwrap();
        let events = h
            .until(|e| matches!(e, Event::TurnDone { turn: 2, .. }))
            .await;
        assert!(events.contains(&Event::CallStarted {
            turn: 2,
            about: "調査: 左前の原因".into()
        }));
        assert_eq!(
            h.played(2).await,
            ["3:左前が減る。", "3:キャンバーの付けすぎ。"]
        );
    }

    #[tokio::test]
    async fn a_study_that_fails_is_told() {
        let mut h = start(
            &[
                "はい。",
                "@調査@You have hit your usage limit. Try again later.",
            ],
            "x",
            1.0,
        );
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("調べて".into())).unwrap();
        let e = h.until(is_done).await;
        assert!(matches!(
            e.last(),
            Some(Event::TurnDone {
                outcome: Outcome::Failed(_),
                ..
            })
        ));
        let played = h.played(2).await.concat();
        assert!(
            played.contains("調べる。") && played.contains("利用枠"),
            "{played}"
        );
        // While the limit lasts, the radio does not promise anything.
        h.cmds.send(Command::Ask("調べて".into())).unwrap();
        h.prompted(2).await;
        assert!(h.prompts.lock().unwrap()[1].starts_with("これまでの交信"));
    }

    #[tokio::test]
    async fn options_of_the_agent_can_be_changed() {
        let mut h = start(&["はい。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
        let o = AgentOptions {
            radio: vec![pair("model", "b")],
            desk: vec![pair("effort", "high")],
        };
        // Nothing to set in an agent that offers nothing: it goes on.
        h.cmds.send(Command::SetAgentOptions(o)).unwrap();
        h.cmds.send(Command::Ask("何か".into())).unwrap();
        let e = h.until(is_done).await;
        assert!(e.iter().all(|e| !matches!(e, Event::Error(_))));
    }

    #[tokio::test]
    async fn silence_is_explained_after_a_while() {
        let mut h = patient(&["54。"], 300);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("slow".into())).unwrap();
        h.prompted(1).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(h.out.played.lock().unwrap().is_empty());
        assert_eq!(h.played(1).await, [waited()]);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(h.out.played.lock().unwrap().len(), 1);
        // Once is enough for a while.
        h.cmds.send(Command::Ask("slow".into())).unwrap();
        h.prompted(2).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(h.out.played.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn calls_are_never_announced() {
        let mut h = patient(&["イエロー。"], 1);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.race.send(yellow()).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(1).await, ["3:イエロー、イエロー。"]);
    }

    #[tokio::test]
    async fn short_press_is_ignored() {
        let mut h = start(&["x。"], "x", MIN_SPEECH - 0.01);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::PttDown).unwrap();
        h.cmds.send(Command::PttUp).unwrap();
        h.cmds.send(Command::Ask("燃料は？".into())).unwrap();
        let events = h.until(|e| matches!(e, Event::TurnStarted { .. })).await;
        // The typed question is turn 1: the short press started no turn.
        assert_eq!(
            events,
            vec![
                Event::Recording(true),
                Event::Recording(false),
                Event::TurnStarted { turn: 1 }
            ]
        );
    }

    #[tokio::test]
    async fn empty_transcript_ends_the_turn_without_prompting() {
        let mut h = start(&["x。"], "  ", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::PttDown).unwrap();
        h.cmds.send(Command::PttUp).unwrap();
        let events = h.until(is_done).await;
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone {
                turn: 1,
                outcome: Outcome::NoSpeech,
                ..
            })
        ));
        assert!(h.prompts.lock().unwrap().is_empty());
    }

    /// The speech stops at the press, the agent only for new words.
    #[tokio::test]
    async fn pressing_again_interrupts_speech_and_agent() {
        let mut h = start(&["了解。"], "ボックス", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("slow".into())).unwrap();
        h.until(|e| matches!(e, Event::TurnStarted { .. })).await;
        h.prompted(1).await;
        let stops_before = h.out.stops.load(Ordering::SeqCst);
        h.cmds.send(Command::PttDown).unwrap();
        let events = h.until(|e| matches!(e, Event::Recording(true))).await;
        assert!(!events.iter().any(is_done));
        assert_eq!(h.out.stops.load(Ordering::SeqCst), stops_before + 1);

        h.cmds.send(Command::PttUp).unwrap();
        let events = h.until(is_done).await;
        assert!(matches!(
            events[..],
            [
                Event::Recording(false),
                Event::TurnStarted { turn: 2 },
                Event::Transcript { turn: 2, .. },
                Event::TurnDone {
                    turn: 1,
                    outcome: Outcome::Cancelled,
                    ..
                }
            ]
        ));
        let events = h.until(is_done).await;
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone {
                turn: 2,
                outcome: Outcome::Answered,
                ..
            })
        ));
        assert_eq!(h.played(1).await, vec!["3:了解。"]);
        assert_eq!(h.prompts.lock().unwrap().len(), 2);
    }

    /// A slip of the thumb, or a press without a word, costs nothing.
    #[tokio::test]
    async fn a_press_without_words_leaves_the_answer_alone() {
        for (stt, seconds) in [("x", MIN_SPEECH - 0.01), ("  ", 1.0)] {
            let mut h = start(&["前は 0.6。", "毎周 0.4 詰めてる。"], stt, seconds);
            h.until(|e| matches!(e, Event::AgentReady { .. })).await;
            h.cmds.send(Command::PttDown).unwrap();
            // Asked while the button is held, so that the answer arrives then.
            h.cmds.send(Command::Ask("差は".into())).unwrap();
            let events = h.until(is_done).await;
            assert!(matches!(
                events.last(),
                Some(Event::TurnDone {
                    turn: 1,
                    outcome: Outcome::Answered,
                    ..
                })
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(h.out.played.lock().unwrap().is_empty());
            h.cmds.send(Command::PttUp).unwrap();
            assert_eq!(
                h.played(2).await,
                vec!["3:前は0.6。", "3:毎周0.4詰めてる。"]
            );
            assert_eq!(h.prompts.lock().unwrap().len(), 1);
        }
    }

    /// Words that take the given times to make out, one per recording.
    struct SlowStt(Mutex<VecDeque<(&'static str, u64)>>);
    impl Transcriber for SlowStt {
        fn transcribe(&self, _: &[u8]) -> anyhow::Result<String> {
            let (text, ms) = self.0.lock().unwrap().pop_front().unwrap();
            std::thread::sleep(Duration::from_millis(ms));
            Ok(text.to_string())
        }
    }

    /// Without early transcription, which would transcribe twice.
    async fn press(h: &Harness) {
        h.cmds.send(Command::PttDown).unwrap();
        h.cmds.send(Command::PttUp).unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    fn outcomes(events: &[Event]) -> Vec<(Turn, Outcome)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::TurnDone { turn, outcome, .. } => Some((*turn, outcome.clone())),
                _ => None,
            })
            .collect()
    }

    /// Words still being made out when the driver goes on talking reach the
    /// agent as well, and only the answer to the last ones is spoken.
    #[tokio::test]
    async fn words_overtaken_by_newer_ones_are_sent_first() {
        let stt = SlowStt(Mutex::new([("slow 一つ目", 200), ("二つ目", 500)].into()));
        let mut h = start_with(&["答え。"], Arc::new(stt), 1.0, "{text}");
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        press(&h).await;
        press(&h).await;
        let mut events = h.until(is_done).await;
        events.extend(h.until(is_done).await);
        assert_eq!(
            outcomes(&events),
            [(1, Outcome::Cancelled), (2, Outcome::Answered)]
        );
        assert_eq!(*h.prompts.lock().unwrap(), ["slow 一つ目", "二つ目"]);
        assert_eq!(h.played(1).await, ["3:答え。"]);
    }

    /// Words that come after those of a newer question are sent when that
    /// has been answered, and their answer is not spoken.
    #[tokio::test]
    async fn words_that_come_too_late_still_reach_the_agent() {
        let stt = SlowStt(Mutex::new([("一つ目", 400), ("slow 二つ目", 50)].into()));
        let mut h = start_with(&["答え。"], Arc::new(stt), 1.0, "{text}");
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        press(&h).await;
        press(&h).await;
        h.prompted(1).await;
        h.until(|e| matches!(e, Event::Transcript { turn: 1, .. }))
            .await;
        assert_eq!(*h.prompts.lock().unwrap(), ["slow 二つ目"]);
        // The fake agent answers "slow" only when it is cancelled.
        h.cmds.send(Command::Ask("三つ目".into())).unwrap();
        let mut events = Vec::new();
        while outcomes(&events).len() < 3 {
            events.extend(h.until(is_done).await);
        }
        assert_eq!(
            outcomes(&events),
            [
                (2, Outcome::Cancelled),
                (3, Outcome::Answered),
                (1, Outcome::Answered)
            ]
        );
        assert_eq!(
            *h.prompts.lock().unwrap(),
            ["slow 二つ目", "三つ目", "一つ目"]
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.played(1).await, ["3:答え。"]);
    }

    #[tokio::test]
    async fn cancel_stops_the_agent_and_what_is_on_its_way() {
        let stt = SlowStt(Mutex::new([("slow", 0), ("二つ目", 300)].into()));
        let mut h = start_with(&["答え。"], Arc::new(stt), 1.0, "{text}");
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        press(&h).await;
        h.prompted(1).await;
        press(&h).await;
        h.cmds.send(Command::Cancel).unwrap();
        let mut events = h.until(is_done).await;
        events.extend(h.until(is_done).await);
        assert_eq!(
            outcomes(&events),
            [(1, Outcome::Cancelled), (2, Outcome::Cancelled)]
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(*h.prompts.lock().unwrap(), ["slow"]);
    }

    #[tokio::test]
    async fn settings_in_the_reply_are_applied_and_not_spoken() {
        let mut h = start_with(
            &[
                "[",
                "[radio volume=-20 speed=+0.1]",
                "]音量を下げた。[[radio event.lap=on mute=off proactive=off]]",
                "[[radio color=red]][[radio speaker=ずんだもん]]",
            ],
            Arc::new(FakeStt("x")),
            1.0,
            "{radio}/{text}",
        );
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("うるさい".into())).unwrap();
        let events = h.until(is_done).await;
        let settings: Vec<&Setting> = events
            .iter()
            .filter_map(|e| match e {
                Event::Setting(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(
            settings,
            [
                &Setting::Volume(80),
                &Setting::Speed(130),
                &Setting::Event(Group::Lap, true),
                &Setting::Muted(false),
                &Setting::Proactive(false),
                &Setting::Speaker("ずんだもん".into()),
            ]
        );
        assert!(events.contains(&Event::Error("ラジオの設定を読めません: color=red".into())));
        let shown: String = events
            .iter()
            .filter_map(|e| match e {
                Event::Chunk { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(shown, "音量を下げた。");
        assert_eq!(h.played(1).await[0], "3:音量を下げた。");
        assert_eq!(*h.out.volumes.lock().unwrap(), [1.0, 0.8]);
        assert_eq!(*h.speeds.lock().unwrap(), [1.2, 1.3]);
        // What could not be read is written again; what could is not.
        h.until(is_done).await;
        assert_eq!(*h.speeds.lock().unwrap(), [1.2, 1.3]);
        // The next question tells the agent the new values.
        h.cmds.send(Command::Ask("今は".into())).unwrap();
        h.until(is_done).await;
        let prompts = h.prompts.lock().unwrap().clone();
        assert_eq!(prompts[0], format!("{STATUS}/うるさい"));
        assert!(
            prompts[2].starts_with(
                "ラジオの現在値: volume=80 mute=off speed=1.30 proactive=off（オフの event: adjustment）"
            ) && prompts[2].ends_with("/今は"),
            "{prompts:?}"
        );
    }

    #[tokio::test]
    async fn settings_stay_within_their_limits() {
        let mut h = start(
            &["[[radio volume=+500 speed=9]][[radio volume=-900]][[radio speed=0]]"],
            "x",
            1.0,
        );
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::SetVolume(255)).unwrap();
        h.cmds.send(Command::SetSpeed(60)).unwrap();
        h.cmds.send(Command::Ask("a".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(*h.out.volumes.lock().unwrap(), [1.0, 1.5, 1.5, 0.0]);
        assert_eq!(*h.speeds.lock().unwrap(), [1.2, 0.6, 2.0, 0.5]);
    }

    fn yellow() -> RaceEvent {
        RaceEvent {
            kind: "flag_raised".into(),
            text: "yellow flag".into(),
            flag: Some("yellow".into()),
            advice: None,
            rest: Default::default(),
        }
    }

    fn race(kind: &str) -> RaceEvent {
        RaceEvent {
            kind: kind.into(),
            text: kind.into(),
            flag: None,
            advice: None,
            rest: Default::default(),
        }
    }

    /// Something only the agent can put into words.
    fn thought(text: &str) -> RaceEvent {
        RaceEvent {
            text: text.into(),
            advice: Some(watch::Advice {
                call: "unheard_of".into(),
                ..Default::default()
            }),
            ..race("strategy")
        }
    }

    #[tokio::test]
    async fn events_are_told_without_the_agent() {
        let mut h = start(&["考えた。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.race.send(yellow()).unwrap();
        h.race.send(race("phase_changed")).unwrap();
        h.race.send(race("weather_changed")).unwrap();
        let events = h.until(is_done).await;
        assert_eq!(
            events[0],
            Event::CallStarted {
                turn: 1,
                about: "旗・天候".into()
            }
        );
        assert!(events.contains(&Event::Chunk {
            turn: 1,
            text: "イエロー、イエロー。weatherの答え。".into()
        }));
        assert_eq!(
            h.played(2).await,
            ["3:イエロー、イエロー。", "3:weatherの答え。"]
        );
        assert!(h.prompts.lock().unwrap().is_empty(), "nobody was asked");
    }

    #[tokio::test]
    async fn the_engineer_calls_about_events() {
        let mut h = start(&["考えた。", "[[radio volume=10]]"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.race.send(thought("new advice")).unwrap();
        let events = h.until(is_done).await;
        assert!(events.contains(&Event::CallStarted {
            turn: 1,
            about: "new advice".into()
        }));
        // Nobody asked for a setting.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Setting(_) | Event::Error(_)))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::TurnStarted { .. }))
        );
        assert_eq!(h.played(1).await, ["3:考えた。"]);
        assert_eq!(
            *h.prompts.lock().unwrap(),
            [render_prompt(
                &config::CALL_PROMPT.replace("{radio}", STATUS),
                "new advice",
                Some("P4")
            )]
        );
        // Not again and again: every prompt costs.
        h.race.send(thought("more advice")).unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(h.prompts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_usual_questions_do_not_reach_the_agent() {
        let mut h = start(&["考えた。"], "順位", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::PttDown).unwrap();
        h.cmds.send(Command::PttUp).unwrap();
        let events = h.until(is_done).await;
        assert!(events.contains(&Event::Chunk {
            turn: 1,
            text: "4番手。前まで0.62秒。".into()
        }));
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone {
                turn: 1,
                outcome: Outcome::Answered,
                ..
            })
        ));
        assert_eq!(h.played(2).await, ["3:4番手。", "3:前まで0.62秒。"]);
        // Said again, acknowledged, or hardly heard.
        for (text, want) in [
            ("もう一回", "4番手。前まで0.62秒。"),
            ("はい", "了解。"),
            ("ん", "聞き取れなかったから状況だけ。今4番手。"),
        ] {
            h.cmds.send(Command::Ask(text.into())).unwrap();
            let events = h.until(is_done).await;
            let said = events
                .iter()
                .any(|e| matches!(e, Event::Chunk { text, .. } if text == want));
            assert!(said, "{text}: {events:?}");
        }
        assert!(h.prompts.lock().unwrap().is_empty());
        // The agent is told what it missed, once.
        for _ in 0..2 {
            h.cmds.send(Command::Ask("抜けそう".into())).unwrap();
            h.until(is_done).await;
        }
        let prompts = h.prompts.lock().unwrap().clone();
        assert!(
            prompts[0].starts_with(
                "これまでの交信（済んだこと。古い順）:\n無線「順位」→ 返答「4番手。前まで0.62秒。」\n"
            ),
            "{}",
            prompts[0]
        );
        assert!(prompts[0].contains("無線「ん」→ 返答「聞き取れなかった"));
        assert_eq!(prompts[1], "無線:「抜けそう」取得したての実データ:\nP4");
    }

    #[tokio::test]
    async fn an_answer_to_the_engineer_is_for_the_agent() {
        let mut h = start(&["ピットに入る？"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("どうする".into())).unwrap();
        h.until(is_done).await;
        for (i, text) in ["はい", "ん"].into_iter().enumerate() {
            h.cmds.send(Command::Ask(text.into())).unwrap();
            h.until(is_done).await;
            assert_eq!(h.prompts.lock().unwrap().len(), i + 2, "{text}");
        }
    }

    #[tokio::test]
    async fn quick_answers_can_be_switched_off() {
        let mut h = start(&["考えた。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        let off = Rules {
            quick: false,
            ..Rules::default()
        };
        h.cmds.send(Command::SetRules(off)).unwrap();
        h.cmds.send(Command::Ask("順位".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(1).await, ["3:考えた。"]);
        // Nothing was said yet that could be said again.
        let mut h = start(&["考えた。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("もう一回".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(h.prompts.lock().unwrap().len(), 1);
        // By talking.
        let (_, events) = asked(&["[[radio quick=off]]了解。"]).await;
        assert!(events.contains(&Event::Setting(Setting::Quick(false))));
        assert!(!rules_of(&events)[0].quick);
    }

    #[tokio::test]
    async fn the_driver_hears_when_the_agent_is_used_up() {
        let mut h = start(
            &["You have hit your usage limit, try again at 1:24 AM.\n"],
            "x",
            1.0,
        );
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("抜けそう".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(
            h.played(3).await,
            [
                "3:AIの利用枠を使い切った。",
                "3:決まった質問には答えられる。",
                "3:statusの答え。"
            ]
        );
        // It is left alone, what needs no agent goes on.
        let mut v = lap(2.0);
        v.insert("stop_in".into(), 3.0);
        h.live.send(v).unwrap();
        h.race.send(thought("new advice")).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(h.prompts.lock().unwrap().len(), 1);
        h.race.send(yellow()).unwrap();
        assert_eq!(h.played(4).await[3], "3:イエロー、イエロー。");
        h.cmds.send(Command::Ask("順位".into())).unwrap();
        assert_eq!(h.played(5).await[4], "3:4番手。");
    }

    #[test]
    fn limits_of_the_plan_are_recognized() {
        for e in [
            "You've hit your usage limit.",
            "usageLimitExceeded",
            "Rate limit reached",
            "insufficient_quota",
        ] {
            assert!(exhausted(e), "{e}");
        }
        assert!(!exhausted("agent closed"));
    }

    fn beside(left: f64, right: f64) -> Values {
        values(&[
            ("left", left),
            ("right", right),
            ("alongside", (left + right).min(1.0)),
            ("on_track", 1.0),
            ("speed", 150.0),
        ])
    }

    #[tokio::test]
    async fn the_spotter_speaks_at_once_and_without_the_agent() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        // What the driver asked for once is the spotter's work now.
        let rules = Rules {
            watches: vec![Watch {
                when: "alongside == 1".into(),
                act: Some("左右を知らせる".into()),
                ..Watch::default()
            }],
            ..Rules::default()
        };
        h.cmds.send(Command::SetRules(rules.clone())).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.live.send(beside(0.0, 0.0)).unwrap();
        // On the brakes, too.
        let mut v = beside(1.0, 0.0);
        v.insert("brake".into(), 80.0);
        h.live.send(v).unwrap();
        let events = h.until(is_done).await;
        assert!(events.contains(&Event::CallStarted {
            turn: 1,
            about: "スポッター".into()
        }));
        assert_eq!(h.played(1).await, ["3:左。"]);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(h.prompts.lock().unwrap().is_empty());
        // Muted, talking, held, or switched off: not a word.
        h.cmds.send(Command::SetMuted(true)).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.live.send(beside(0.0, 1.0)).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.out.played.lock().unwrap().len(), 1);
        h.cmds.send(Command::SetMuted(false)).unwrap();
        let hold = Rules {
            hold: Some("speed > 100".into()),
            ..Rules::default()
        };
        h.cmds.send(Command::SetRules(hold)).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.live.send(beside(0.0, 1.0)).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.out.played.lock().unwrap().len(), 1);
        h.cmds.send(Command::SetRules(rules)).unwrap();
        let off = Proactive {
            spotter: false,
            ..Proactive::default()
        };
        h.cmds.send(Command::SetProactive(off)).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Without the spotter the driver's own watch counts again.
        h.live.send(beside(0.0, 0.0)).unwrap();
        h.live.send(beside(0.0, 1.0)).unwrap();
        h.prompted(1).await;
    }

    #[tokio::test]
    async fn a_watch_may_tell_a_topic() {
        let (mut h, events) = asked(&["[[radio when=\"lap_pct > 5\" tell=lap]]了解。"]).await;
        assert_eq!(rules_of(&events)[0].watches[0].tell.as_deref(), Some("lap"));
        h.played(1).await;
        h.live.send(values(&[("lap_pct", 2.0)])).unwrap();
        h.live.send(values(&[("lap_pct", 6.0)])).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(2).await[1], "3:lapの答え。");
        assert_eq!(h.prompts.lock().unwrap().len(), 1);
        // So does an order that takes no thought.
        let (mut h, _) =
            asked(&["[[radio when=\"lap_pct > 5\" do=\"トップとの差を言う\"]]了解。"]).await;
        h.played(1).await;
        h.live.send(values(&[("lap_pct", 2.0)])).unwrap();
        h.live.send(values(&[("lap_pct", 6.0)])).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(2).await[1], "3:leaderの答え。");
        assert_eq!(h.prompts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn there_is_no_planning_without_a_stop() {
        let h = start(PLANS, "x", 1.0);
        let mut v = lap(2.0);
        v.insert("fuel_spare".into(), 0.1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.live.send(v.clone()).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(h.prompts.lock().unwrap().is_empty());
        // Short of fuel, or with a stop planned.
        v.insert("fuel_spare".into(), 0.0);
        h.live.send(v).unwrap();
        h.prompted(1).await;
        let h = start(PLANS, "x", 1.0);
        let mut v = lap(2.0);
        v.insert("fuel_spare".into(), 5.0);
        v.insert("stop_in".into(), 4.0);
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.live.send(v).unwrap();
        h.prompted(1).await;
    }

    const STATUS: &str = "ラジオの現在値: volume=100 mute=off speed=1.20 proactive=on\
        （オフの event: lap, adjustment） interval=8 hold=\"off\" hold_calls=\"brake > 10\" \
        quiet=\"off\" hold_max=8 quick=on planning=2";

    fn values(pairs: &[(&str, f64)]) -> Values {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    /// The harness, with the rules the driver asked for.
    async fn asked(reply: &'static [&'static str]) -> (Harness, Vec<Event>) {
        let mut h = start(reply, "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("頼む".into())).unwrap();
        let events = h.until(is_done).await;
        (h, events)
    }

    fn rules_of(events: &[Event]) -> Vec<&Rules> {
        let rules = events.iter().filter_map(|e| match e {
            Event::Rules(r) => Some(r),
            _ => None,
        });
        rules.collect()
    }

    #[tokio::test]
    async fn rules_are_changed_by_talking() {
        let (mut h, events) = asked(&[
            "[[radio interval=60 hold=\"brake > 5\" hold_max=3 note=\"短く\"]]",
            "[[radio when=\"lap > 3\" say=\"4周目\" planning=off quiet=\"lap < 2\"]]",
            "了解。",
        ])
        .await;
        let rules = *rules_of(&events).last().unwrap();
        assert_eq!(
            *rules,
            Rules {
                interval_s: 60,
                hold: Some("brake > 5".into()),
                hold_max_s: 3,
                notes: vec!["短く".into()],
                quiet: Some("lap < 2".into()),
                planning: 0,
                watches: vec![Watch {
                    when: "lap > 3".into(),
                    say: Some("4周目".into()),
                    every: rules::EVERY_S,
                    ..Watch::default()
                }],
                ..Rules::default()
            }
        );
        let told: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                Event::Setting(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            told,
            [
                "自動連絡の間隔 60秒以上",
                "話しかけない条件 brake > 5",
                "話すのを待つ上限 3秒",
                "覚えた指示: 短く",
                "見張り: lap > 3 → 4周目",
                "裏での作戦立案 オフ",
                "自動連絡をしない条件 lap < 2",
            ]
        );
        // The next race begins without what was asked for this one.
        h.race.send(race("session_changed")).unwrap();
        let events = h.until(|e| matches!(e, Event::Rules(_))).await;
        assert_eq!(*rules_of(&events)[0], rules.lasting());
    }

    #[tokio::test]
    async fn amounts_stay_within_reason() {
        let (_, events) = asked(&["[[radio interval=-999 hold_max=9999]]了解。"]).await;
        let r = *rules_of(&events).last().unwrap();
        assert_eq!((r.interval_s, r.hold_max_s), (0, 120));
        let (_, events) = asked(&["[[radio interval=99999 forget=all]]了解。"]).await;
        assert_eq!(rules_of(&events).last().unwrap().interval_s, 3600);
        assert!(events.contains(&Event::Setting(Setting::Forgot(0))));
    }

    #[tokio::test]
    async fn speech_waits_until_the_condition_is_over() {
        let (mut h, _) = asked(&["[[radio hold=\"brake > 5\"]]了解。"]).await;
        h.played(1).await;
        h.live.send(values(&[("brake", 80.0)])).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.cmds.send(Command::Ask("次".into())).unwrap();
        h.until(is_done).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(h.out.played.lock().unwrap().len(), 1);
        h.live.send(values(&[("brake", 0.0)])).unwrap();
        assert_eq!(h.played(2).await, ["3:了解。", "3:了解。"]);
        // A sample of the voice is heard at once.
        h.live.send(values(&[("brake", 80.0)])).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.cmds.send(Command::Preview(8)).unwrap();
        assert_eq!(h.played(3).await[2], format!("8:{SAMPLE}"));
        // Held back, then replaced by the driver: never said.
        h.cmds.send(Command::Ask("三つ目".into())).unwrap();
        h.until(is_done).await;
        h.cmds.send(Command::Cancel).unwrap();
        h.live.send(values(&[("brake", 0.0)])).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(h.out.played.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn answers_do_not_wait_where_only_calls_do() {
        let mut h = start(&["答え。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.live.send(values(&[("brake", 80.0)])).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        h.cmds.send(Command::Ask("前は".into())).unwrap();
        assert_eq!(h.played(1).await, ["3:答え。"]);
        h.until(is_done).await;
        h.race.send(race("weather_changed")).unwrap();
        h.until(is_done).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(h.out.played.lock().unwrap().len(), 1);
        h.live.send(values(&[("brake", 5.0)])).unwrap();
        assert_eq!(h.played(2).await[1], "3:weatherの答え。");
    }

    #[tokio::test]
    async fn a_watch_speaks_by_itself_or_asks_the_engineer() {
        let (mut h, _) = asked(&[
            "[[radio when=\"gap_behind < 1\" say=\"後ろ{gap_behind}秒\" once=on]]",
            "[[radio when=\"fuel_laps < 2\" do=\"残りを言う\"]]了解。",
            "@見張り「@燃料2周。",
        ])
        .await;
        h.played(1).await;
        let v = |gap, fuel| values(&[("gap_behind", gap), ("fuel_laps", fuel)]);
        h.live.send(v(3.0, 5.0)).unwrap();
        h.live.send(v(0.84, 5.0)).unwrap();
        let events = h.until(is_done).await;
        assert!(events.contains(&Event::CallStarted {
            turn: 2,
            about: "見張り: gap_behind < 1".into()
        }));
        assert_eq!(rules_of(&events)[0].watches.len(), 1, "once");
        assert_eq!(h.played(2).await[1], "3:後ろ0.8秒");
        assert_eq!(h.prompts.lock().unwrap().len(), 1, "nobody was asked");
        // Also when calls are switched off: the driver asked for it.
        h.cmds
            .send(Command::SetProactive(Proactive {
                enabled: false,
                ..Default::default()
            }))
            .unwrap();
        h.live.send(v(0.8, 1.9)).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(3).await[2], "3:燃料2周。");
        let prompts = h.prompts.lock().unwrap().clone();
        assert!(
            prompts[1].contains("見張り「fuel_laps < 2」が成立。指示: 残りを言う"),
            "{}",
            prompts[1]
        );
    }

    const PLANS: &[&str] = &[
        "了解。",
        "@作戦会議@[[radio plan=\"15周目に入る\"]][[radio when=\"stop_in == 0\" do=\"ピットと言う\"]]",
        "@作戦会議@[[radio volume=10]]-",
    ];

    fn lap(n: f64) -> Values {
        values(&[("racing", 1.0), ("on_track", 1.0), ("lap", n)])
    }

    #[tokio::test]
    async fn the_engineer_plans_unasked_and_unheard() {
        let mut h = start(PLANS, "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.live.send(lap(1.0)).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(h.prompts.lock().unwrap().is_empty(), "nothing to plan with");
        h.live.send(lap(2.0)).unwrap();
        let events = h.until(is_done).await;
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone {
                outcome: Outcome::Silent,
                ..
            })
        ));
        assert!(events.contains(&Event::Plan(Some("15周目に入る".into()))));
        let rules = rules_of(&events)[0];
        assert!(rules.watches[0].planned && rules.lasting().watches.is_empty());
        assert!(!events.iter().any(|e| matches!(e, Event::Setting(_))));
        // The memo goes along with the next question.
        h.cmds.send(Command::Ask("作戦は".into())).unwrap();
        h.until(is_done).await;
        let status =
            format!("{STATUS}\n見張り w1: stop_in == 0 → ピットと言う\n作戦メモ: 15周目に入る");
        let template = config::CALL_PROMPT.replace("{radio}", &status);
        h.race.send(thought("new advice")).unwrap();
        h.until(is_done).await;
        let prompts = h.prompts.lock().unwrap().clone();
        assert!(prompts[0].starts_with("作戦会議") && prompts[0].contains("P4"));
        assert_eq!(
            prompts[2],
            render_prompt(&template, "new advice", Some("P4"))
        );
        // Another race, another plan.
        h.race.send(race("session_changed")).unwrap();
        h.until(|e| *e == Event::Plan(None)).await;
        let events = h.until(|e| matches!(e, Event::Rules(_))).await;
        assert!(rules_of(&events)[0].watches.is_empty());
    }

    #[tokio::test]
    async fn what_a_plan_looks_out_for_is_the_engineers_own_matter() {
        let mut h = start(PLANS, "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.live.send(lap(2.0)).unwrap();
        h.until(is_done).await;
        let mut v = lap(2.0);
        v.insert("stop_in".into(), 0.0);
        h.live.send(v).unwrap();
        h.until(is_done).await;
        let prompts = h.prompts.lock().unwrap().clone();
        assert!(
            prompts[1].contains("作戦で決めた見張り「stop_in == 0」が成立。指示: ピットと言う"),
            "{}",
            prompts[1]
        );
    }

    #[tokio::test]
    async fn the_driver_interrupts_the_planning() {
        let mut h = start(PLANS, "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds
            .send(Command::SetRules(Rules {
                notes: vec!["slow".into()],
                ..Rules::default()
            }))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.live.send(lap(2.0)).unwrap();
        h.prompted(1).await;
        let t = Instant::now();
        h.cmds.send(Command::Ask("前は".into())).unwrap();
        let events = h.until(is_done).await;
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone {
                outcome: Outcome::Answered,
                ..
            })
        ));
        assert_eq!(h.played(1).await, ["3:了解。"]);
    }

    #[tokio::test]
    async fn no_planning_when_switched_off() {
        let mut h = start(PLANS, "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds
            .send(Command::SetRules(Rules {
                planning: 0,
                ..Rules::default()
            }))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.live.send(lap(2.0)).unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(h.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn advice_is_told_at_once_and_a_new_plan_thought_through() {
        let mut h = start(&["-", "@作戦会議@[[radio plan=\"今\"]]-"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        let strategy = |call: &str| RaceEvent {
            advice: Some(watch::Advice {
                call: call.into(),
                ..Default::default()
            }),
            ..race("strategy")
        };
        h.race.send(strategy("plan")).unwrap();
        let events = h.until(|e| matches!(e, Event::Plan(_))).await;
        assert!(events.contains(&Event::Plan(Some("今".into()))));
        h.race.send(strategy("box_this_lap")).unwrap();
        let played = h.played(2).await;
        assert!(
            played.contains(&"3:ピット無しで完走する。".to_string()),
            "{played:?}"
        );
        assert!(played.contains(&"3:この周ボックスだ。".to_string()));
        assert_eq!(h.prompts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn quiet_drops_what_can_wait() {
        let (mut h, _) = asked(&["[[radio quiet=\"lap < 13\"]]了解。"]).await;
        h.played(1).await;
        h.live.send(lap(1.0)).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.race.send(race("weather_changed")).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(h.out.played.lock().unwrap().len(), 1);
        // A flag is told all the same.
        h.race.send(yellow()).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(2).await[1], "3:イエロー、イエロー。");
    }

    #[tokio::test]
    async fn what_the_agent_got_wrong_it_repairs_once() {
        let (mut h, events) = asked(&[
            "[[radio hold=\"braking > 5\"]]了解。",
            "@使えなかった@[[radio hold=\"brake > 5\"]]",
        ])
        .await;
        let error = "条件に無い値: braking（braking > 5）";
        assert!(events.contains(&Event::Error(error.into())));
        let events = h.until(is_done).await;
        assert_eq!(
            rules_of(&events)[0].hold.as_deref(),
            Some("brake > 5"),
            "{events:?}"
        );
        let prompts = h.prompts.lock().unwrap().clone();
        assert_eq!(prompts[1], config::FIX_PROMPT.replace("{text}", error));
        assert_eq!(h.played(1).await, ["3:了解。"]);

        // Not again and again.
        let (mut h, _) = asked(&["[[radio hold=\"x > 5\"]]了解。"]).await;
        h.until(is_done).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(h.prompts.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_call_may_say_nothing() {
        let mut h = start(&[" ", "-", "\n"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.race.send(thought("new advice")).unwrap();
        let events = h.until(is_done).await;
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone {
                turn: 1,
                outcome: Outcome::Silent,
                ..
            })
        ));
        assert!(!events.iter().any(|e| matches!(
            e,
            Event::CallStarted { .. } | Event::Chunk { .. } | Event::FirstToken { .. }
        )));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(h.out.played.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn calls_wait_for_the_driver_and_make_way() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::PttDown).unwrap();
        h.race.send(yellow()).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(h.prompts.lock().unwrap().is_empty());
        h.cmds.send(Command::PttUp).unwrap();
        let events = h.until(is_done).await;
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone {
                turn: 1,
                outcome: Outcome::Answered,
                ..
            })
        ));
        // Then it is told.
        h.until(|e| matches!(e, Event::CallStarted { turn: 2, .. }))
            .await;
        h.until(is_done).await;
        // Switched off, nothing is called about.
        h.cmds
            .send(Command::SetProactive(Proactive {
                enabled: false,
                ..Default::default()
            }))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.race.send(yellow()).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(h.prompts.lock().unwrap().len(), 1);
        assert_eq!(h.out.played.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_agent_that_went_away_is_started_again_for_a_question() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds
            .send(Command::SetAgent("broken".into(), AgentOptions::default()))
            .unwrap();
        h.until(|e| matches!(e, Event::AgentClosed { .. })).await;
        h.cmds.send(Command::Ask("a".into())).unwrap();
        let events = h.until(is_done).await;
        assert!(events.contains(&Event::AgentStarting));
        assert_eq!(*h.connects.lock().unwrap(), ["broken", "broken"]);
    }

    #[tokio::test]
    async fn an_agent_that_went_away_is_started_again_by_itself() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds
            .send(Command::SetAgent("broken".into(), AgentOptions::default()))
            .unwrap();
        let t = Instant::now();
        h.until(|e| matches!(e, Event::AgentStarting)).await;
        assert!((2000..3500).contains(&ms(t)), "{}", ms(t));
        // Not after an expired login, which would end the same way.
        h.cmds
            .send(Command::SetAgent(
                "Authentication required".into(),
                AgentOptions::default(),
            ))
            .unwrap();
        h.until(|e| matches!(e, Event::AgentClosed { error: Some(e) } if e.starts_with("Auth")))
            .await;
        tokio::time::sleep(Duration::from_millis(2300)).await;
        assert_eq!(
            *h.connects.lock().unwrap(),
            ["broken", "broken", "Authentication required"]
        );
    }

    #[test]
    fn errors_that_come_again() {
        for e in [
            "Authentication required",
            "please log in",
            "run codex login",
            "bad agent command: x",
        ] {
            assert!(permanent(Some(e)), "{e}");
        }
        assert!(!permanent(Some("connection closed")));
        assert!(!permanent(None));
    }

    #[test]
    fn replies_that_say_nothing() {
        for r in ["", " \n", "-", " - ", "ー", "−", "－", "―", "—"] {
            assert!(silence(r), "{r:?}");
        }
        assert!(!silence("- 黄旗"));
        assert!(!silence("--"));
    }

    #[tokio::test]
    async fn muted_and_speaker_change() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::SetMuted(true)).unwrap();
        h.cmds.send(Command::Ask("a".into())).unwrap();
        h.until(is_done).await;
        h.cmds.send(Command::SetMuted(false)).unwrap();
        h.cmds.send(Command::SetSpeaker(8)).unwrap();
        h.cmds.send(Command::Ask("b".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(1).await, vec!["8:了解。"]);
    }

    #[tokio::test]
    async fn the_agent_is_replaced_and_its_turn_cancelled() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("slow".into())).unwrap();
        h.until(|e| matches!(e, Event::TurnStarted { .. })).await;
        h.cmds
            .send(Command::SetAgent("other".into(), AgentOptions::default()))
            .unwrap();
        let events = h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        assert!(events.iter().any(|e| matches!(
            e,
            Event::TurnDone {
                turn: 1,
                outcome: Outcome::Cancelled,
                ..
            }
        )));
        // The old session ends silently.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::AgentClosed { .. }))
        );
        h.cmds.send(Command::Ask("b".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(1).await, vec!["3:別の返答。"]);
    }

    #[tokio::test]
    async fn a_question_without_an_agent_fails_instead_of_waiting() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds
            .send(Command::SetAgent("broken".into(), AgentOptions::default()))
            .unwrap();
        let events = h.until(|e| matches!(e, Event::AgentClosed { .. })).await;
        assert_eq!(
            events.last(),
            Some(&Event::AgentClosed {
                error: Some("broken".into())
            })
        );
        h.cmds.send(Command::Ask("b".into())).unwrap();
        let events = h.until(is_done).await;
        let Some(Event::TurnDone {
            turn: 1, outcome, ..
        }) = events.last()
        else {
            panic!("{events:?}")
        };
        assert_eq!(
            *outcome,
            Outcome::Failed("エージェントに接続していません".into())
        );
        assert!(events.iter().any(|e| matches!(e, Event::Error(_))));
        // Until then at least the numbers are told.
        assert_eq!(
            h.played(3).await,
            [
                "3:考えて答えるのは今できない、",
                "3:数字だけ伝える。",
                "3:statusの答え。"
            ]
        );
        // Connecting makes it usable again.
        h.cmds
            .send(Command::SetAgent("other".into(), AgentOptions::default()))
            .unwrap();
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("c".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(4).await[3], "3:別の返答。");
    }

    #[tokio::test]
    async fn an_error_of_the_agent_is_shown_and_not_spoken() {
        let mut h = start(
            &["Failed to authenticate: please log in\n", "了解。"],
            "x",
            1.0,
        );
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("a".into())).unwrap();
        let events = h.until(is_done).await;
        assert!(events.contains(&Event::Error(
            "Failed to authenticate: please log in".into()
        )));
        assert_eq!(h.played(1).await, vec!["3:了解。"]);
    }

    #[tokio::test]
    async fn the_transcriber_is_replaced() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds
            .send(Command::SetStt(SttConfig {
                backend: Default::default(),
                base_url: String::new(),
                api_key: None,
                model: "new".into(),
                language: None,
                prompt: None,
                refine_model: None,
            }))
            .unwrap();
        h.cmds.send(Command::PttDown).unwrap();
        h.cmds.send(Command::PttUp).unwrap();
        let events = h.until(is_done).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Transcript { text, .. } if text == "聞き直し"))
        );
    }

    #[tokio::test]
    async fn a_sample_is_spoken_even_when_muted() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::SetMuted(true)).unwrap();
        let stops = h.out.stops.load(Ordering::SeqCst);
        h.cmds.send(Command::Preview(20)).unwrap();
        assert_eq!(h.played(1).await, vec![format!("20:{SAMPLE}")]);
        // Stopped once for muting, once for the sample.
        assert_eq!(h.out.stops.load(Ordering::SeqCst), stops + 2);
    }

    #[test]
    fn elapsed_milliseconds() {
        let t = Instant::now() - Duration::from_millis(50);
        let m = ms(t);
        assert!((50..10_000).contains(&m), "{m}");
    }

    #[test]
    fn prompt_template() {
        assert_eq!(
            render_prompt("無線「{text}」", "差は", None),
            "無線「差は」"
        );
        assert_eq!(render_prompt("前置き", "差は", None), "前置き\n差は");
        assert_eq!(
            render_prompt("{context}/{text}", "{context}", Some("P3")),
            "取得したての実データ:\nP3/{context}"
        );
        assert!(render_prompt("{context}{text}", "x", None).contains("vr で取得"));
    }
}
