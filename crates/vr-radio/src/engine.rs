//! The radio pipeline: push-to-talk → transcription → agent → speech.
//!
//! Speech starts with the first complete sentence of the reply, while the
//! agent is still writing the rest. Pressing the button again stops the
//! speech (barge-in); the agent is interrupted once there are new words for
//! it, and everything the driver said reaches it. Race events the driver
//! should know about start turns of their own.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::agent::{AgentCmd, AgentEvent, AgentLink, TokenUsage, ToolState, Turn};
use crate::audio::{Output, Recorder};
use crate::config;
use crate::context::ContextSource;
use crate::directive::{Directive, MAX_VOLUME, SPEED, Setting, Stripper};
use crate::early::Early;
use crate::stt::{SttConfig, Transcriber};
use crate::text::{self, SentenceSplitter};
use crate::voicevox::Synth;
use crate::watch::{self, Dated, Follow, Group, Proactive, Queue, RaceEvent, Timing};

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
    /// Speaks [`SAMPLE`] with a voice, even when muted.
    Preview(u32),
    /// Replaces the agent: the old session ends, a new one starts.
    SetAgent(String),
    SetStt(SttConfig),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    AgentReady {
        name: String,
    },
    /// Started again after it went away.
    AgentStarting,
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
pub type Connect = Box<dyn Fn(&str) -> AgentLink + Send>;
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
}

struct Call {
    about: String,
    /// Reply so far that may still turn out to be silence.
    lead: String,
    shown: bool,
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
}

/// What was waited for before asking the agent.
enum Fetched {
    /// The driver's words, with their data.
    Words {
        turn: Turn,
        result: anyhow::Result<String>,
        ms: u64,
        data: Option<String>,
        typed: bool,
    },
    /// The data for a call.
    Call { turn: Turn, data: Option<String> },
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

/// Synthesizes sentences in order and queues them for playback.
/// Clips of an interrupted generation are dropped.
async fn speak(
    mut clips: UnboundedReceiver<Clip>,
    synth: Arc<dyn Synth>,
    output: Arc<dyn Output>,
    generation: Arc<AtomicU64>,
    speaker: Arc<AtomicU32>,
    pending: Arc<AtomicUsize>,
    events: UnboundedSender<Event>,
) {
    while let Some(clip) = clips.recv().await {
        if clip.generation == generation.load(Ordering::SeqCst) {
            let synth = synth.clone();
            let text = clip.text.clone();
            let id = clip
                .voice
                .unwrap_or_else(|| speaker.load(Ordering::Relaxed));
            let result = tokio::task::spawn_blocking(move || synth.synthesize(&text, id)).await;
            let current = clip.generation == generation.load(Ordering::SeqCst);
            match result {
                _ if !current => {}
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
    /// A call whose data is being fetched.
    calling: Option<(Turn, Instant, Vec<Dated>)>,
    watching: Option<bool>,
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
        if let Some(c) = self.current.take()
            && c.call.as_ref().is_none_or(|call| call.shown)
        {
            self.done(c.turn, Outcome::Cancelled, c.started);
        }
    }

    /// Stops everything, also questions on their way.
    fn cancel_all(&mut self) {
        self.hush();
        self.held.clear();
        let _ = self.agent.tx.send(AgentCmd::Cancel);
        self.drop_current();
        self.calling = None;
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
                self.recorder.start();
                self.recording = true;
                self.early = self.early_stt.then(Early::default);
                self.emit(Event::Recording(true));
            }
            Command::PttUp => self.released(),
            Command::Ask(text) => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    return;
                }
                self.hush();
                self.held.clear();
                self.drop_current();
                let turn = self.begin();
                self.emit(Event::Transcript {
                    turn,
                    text: text.clone(),
                    ms: 0,
                });
                let data = fetch(&self.context);
                let tx = self.fetched.clone();
                tokio::spawn(async move {
                    let _ = tx.send(Fetched::Words {
                        turn,
                        result: Ok(text),
                        ms: 0,
                        data: data.await,
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
                });
            }
            Command::SetAgent(command) => {
                self.cancel_all();
                self.agent_command = command;
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
        let early = self.early.take().unwrap_or_default();
        let text = early.finish(rec, self.transcriber.clone());
        tokio::spawn(async move {
            let result = text.await;
            let ms = ms(t0);
            let data = data.await;
            let _ = tx.send(Fetched::Words {
                turn,
                result,
                ms,
                data,
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
                data,
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
                        self.words(turn, started, text, data);
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
        }
    }

    /// Passes the driver's words on.
    fn words(&mut self, turn: Turn, started: Instant, text: String, data: Option<String>) {
        if turn < self.newest && self.current.is_some() {
            // A newer question is being answered: this one follows, unspoken.
            self.late.push_back((turn, started, text, data));
            return;
        }
        self.drop_current();
        self.held.clear();
        let spoken = turn > self.newest;
        self.ask(turn, started, &text, data.as_deref(), spoken);
    }

    fn ask(&mut self, turn: Turn, started: Instant, text: &str, data: Option<&str>, spoken: bool) {
        self.newest = self.newest.max(turn);
        let template = self.prompt_template.replace("{radio}", &self.status());
        let prompt = render_prompt(&template, text, data);
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
                return;
            }
        }
        self.start_turn(turn, started, spoken, None);
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
        });
    }

    /// The engineer calls by itself, if the agent finds it worth it.
    fn call(&mut self, turn: Turn, started: Instant, events: &[Dated], data: Option<&str>) {
        let about = watch::summary(events);
        let text = render_prompt(config::CALL_PROMPT, &about, data);
        let cmd = AgentCmd::Prompt {
            turn,
            text,
            keep: false,
        };
        if self.agent.tx.send(cmd).is_ok() {
            let call = Call {
                about,
                lead: String::new(),
                shown: false,
            };
            self.start_turn(turn, started, true, Some(call));
        }
    }

    fn start_agent(&mut self) {
        // Dropping the old link ends its process.
        self.agent = (self.connect)(&self.agent_command);
        self.agent_ready = false;
        self.reconnect = None;
    }

    fn agent_event(&mut self, ev: AgentEvent) {
        let mine = |c: &Option<Current>, turn| c.as_ref().is_some_and(|c| c.turn == turn);
        match ev {
            AgentEvent::Cost {
                amount, currency, ..
            } => self.emit(Event::Cost { amount, currency }),
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
                    let driver = c.call.is_none();
                    self.settings(found, driver);
                    self.say(text);
                }
            }
            AgentEvent::Tool {
                turn,
                id,
                title,
                state,
            } => {
                if mine(&self.current, turn) {
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
                if mine(&self.current, turn) {
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
            let driver = c.call.is_none();
            self.settings(found, driver);
            self.say(text);
        }
        let Some(mut c) = self.current.take() else {
            return;
        };
        if let Some(rest) = c.splitter.finish() {
            self.say_sentence(&c, rest);
        }
        let silent = c.call.as_ref().is_some_and(|call| !call.shown);
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
        }
    }

    /// Shows and speaks text of the current turn.
    fn say(&mut self, mut text: String) {
        let Some(c) = &mut self.current else { return };
        if let Some(call) = c.call.as_mut().filter(|call| !call.shown) {
            call.lead.push_str(&text);
            if silence(&call.lead) {
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
        let _ = self.events.send(Event::Chunk { turn: c.turn, text });
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
        };
        if self.holding() {
            self.held.push(clip);
        } else {
            self.queue_clip(clip);
        }
    }

    /// Applies what the agent wrote into its reply, if the driver asked.
    fn settings(&mut self, found: Vec<Result<Directive, String>>, driver: bool) {
        for f in found {
            match f {
                _ if !driver => tracing::warn!("call: ignored {f:?}"),
                Ok(d) => self.apply(d),
                Err(e) => self.emit(Event::Error(e)),
            }
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
        };
        self.emit(Event::Setting(setting));
    }

    fn set_volume(&mut self, v: u8) {
        self.volume = v.min(MAX_VOLUME);
        self.output.set_volume(f32::from(self.volume) / 100.0);
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
        if self.proactive.wants(&e) {
            self.queue.push(Instant::now(), e);
        }
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
        let idle = self.agent_ready
            && !self.busy()
            && self.calling.is_none()
            && !self.speaking
            && self.pending_clips.load(Ordering::SeqCst) == 0;
        if let Some(events) = self.queue.due(Instant::now(), idle) {
            self.next_turn += 1;
            let turn = self.next_turn;
            self.calling = Some((turn, Instant::now(), events));
            let data = fetch(&self.context);
            let tx = self.fetched.clone();
            tokio::spawn(async move {
                let data = data.await;
                let _ = tx.send(Fetched::Call { turn, data });
            });
        }
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
    } = deps;
    let generation = Arc::new(AtomicU64::new(0));
    let speaker = Arc::new(AtomicU32::new(speaker));
    let pending_clips = Arc::new(AtomicUsize::new(0));
    let (clip_tx, clip_rx) = unbounded_channel::<Clip>();
    let (fetched_tx, mut fetched_rx) = unbounded_channel::<Fetched>();
    tokio::spawn(speak(
        clip_rx,
        synth.clone(),
        output.clone(),
        generation.clone(),
        speaker.clone(),
        pending_clips.clone(),
        events.clone(),
    ));
    let mut engine = Engine {
        recorder,
        transcriber,
        synth,
        output,
        make_transcriber,
        agent,
        agent_command,
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
        calling: None,
        watching: None,
    };
    engine.warm(engine.speaker.load(Ordering::Relaxed));
    engine.set_volume(volume);
    engine.set_speed(speed);

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
            _ = tick.tick() => engine.tick(watch.as_ref().map(|w| w.running(WATCHING))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    use super::*;
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
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let link = |reply, prompts| {
            AgentLink::spawn(
                fake::agent(reply, prompts),
                SessionConfig {
                    cwd: std::env::temp_dir(),
                    permissions: Permissions::Safe,
                    options: vec![],
                    primer: None,
                },
            )
        };
        let other = prompts.clone();
        let out = Arc::new(FakeOut::default());
        let warmed = Arc::new(Mutex::new(Vec::new()));
        let (cmds, cmd_rx) = unbounded_channel();
        let (ev_tx, events) = unbounded_channel();
        let (race, race_rx) = unbounded_channel();
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
                connect: Box::new(move |command| {
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
            },
            cmd_rx,
            ev_tx,
        ));
        Harness {
            race,
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
            vec!["3:前の Ito まで 0.6。", "3:毎周 0.4 詰めてる"]
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
                vec!["3:前は 0.6。", "3:毎周 0.4 詰めてる。"]
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
        assert_eq!(h.played(1).await, ["3:音量を下げた。"]);
        assert_eq!(*h.out.volumes.lock().unwrap(), [1.0, 0.8]);
        assert_eq!(*h.speeds.lock().unwrap(), [1.2, 1.3]);
        // The next question tells the agent the new values.
        h.cmds.send(Command::Ask("今は".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(
            *h.prompts.lock().unwrap(),
            [
                "ラジオの現在値: volume=100 mute=off speed=1.20 proactive=on（オフの event: lap, adjustment）/うるさい",
                "ラジオの現在値: volume=80 mute=off speed=1.30 proactive=off（オフの event: adjustment）/今は"
            ]
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
        }
    }

    fn race(kind: &str) -> RaceEvent {
        RaceEvent {
            kind: kind.into(),
            text: kind.into(),
            flag: None,
        }
    }

    #[tokio::test]
    async fn the_engineer_calls_about_events() {
        let mut h = start(&["イエロー。", "[[radio volume=10]]"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.race.send(yellow()).unwrap();
        h.race.send(race("lap_completed")).unwrap();
        h.race.send(race("pit_entry")).unwrap();
        let events = h.until(is_done).await;
        assert_eq!(
            events[..1],
            [Event::Tool {
                turn: 1,
                id: "t1".into(),
                title: Some("`vr brief`".into()),
                state: ToolState::Running
            }]
        );
        let started = events
            .iter()
            .position(|e| matches!(e, Event::CallStarted { .. }))
            .unwrap();
        assert_eq!(
            events[started],
            Event::CallStarted {
                turn: 1,
                about: "yellow flag / pit_entry".into()
            }
        );
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
        assert_eq!(h.played(1).await, ["3:イエロー。"]);
        assert_eq!(
            *h.prompts.lock().unwrap(),
            [render_prompt(
                config::CALL_PROMPT,
                "yellow flag / pit_entry",
                Some("P4")
            )]
        );
    }

    #[tokio::test]
    async fn a_call_may_say_nothing() {
        let mut h = start(&[" ", "-", "\n"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.race.send(yellow()).unwrap();
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
        assert_eq!(h.prompts.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_agent_that_went_away_is_started_again_for_a_question() {
        let mut h = start(&["了解。"], "x", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::SetAgent("broken".into())).unwrap();
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
        h.cmds.send(Command::SetAgent("broken".into())).unwrap();
        let t = Instant::now();
        h.until(|e| matches!(e, Event::AgentStarting)).await;
        assert!((2000..3500).contains(&ms(t)), "{}", ms(t));
        // Not after an expired login, which would end the same way.
        h.cmds
            .send(Command::SetAgent("Authentication required".into()))
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
        h.cmds.send(Command::SetAgent("other".into())).unwrap();
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
        h.cmds.send(Command::SetAgent("broken".into())).unwrap();
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
        // Connecting makes it usable again.
        h.cmds.send(Command::SetAgent("other".into())).unwrap();
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("c".into())).unwrap();
        h.until(is_done).await;
        assert_eq!(h.played(1).await, vec!["3:別の返答。"]);
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
