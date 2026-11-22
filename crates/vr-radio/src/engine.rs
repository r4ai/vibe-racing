//! The radio pipeline: push-to-talk → transcription → agent → speech.
//!
//! Speech starts with the first complete sentence of the reply, while the
//! agent is still writing the rest. Pressing the button again interrupts
//! both the speech and the agent (barge-in).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::agent::{AgentCmd, AgentEvent, ToolState, Turn};
use crate::audio::{Output, Recorder};
use crate::stt::Transcriber;
use crate::text::SentenceSplitter;
use crate::voicevox::Synth;

/// Recordings shorter than this are treated as accidental presses.
pub const MIN_SPEECH: f32 = 0.3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    PttDown,
    PttUp,
    /// A typed question (no microphone needed).
    Ask(String),
    /// Stops the speech and the running turn.
    Cancel,
    SetSpeaker(u32),
    SetMuted(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    AgentReady {
        name: String,
    },
    AgentClosed {
        error: Option<String>,
    },
    Recording(bool),
    Level(f32),
    TurnStarted {
        turn: Turn,
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
    Speaking(bool),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Answered,
    NoSpeech,
    Cancelled,
    Failed(String),
}

pub struct Deps {
    pub recorder: Arc<dyn Recorder>,
    pub transcriber: Arc<dyn Transcriber>,
    pub synth: Arc<dyn Synth>,
    pub output: Arc<dyn Output>,
    pub agent_tx: UnboundedSender<AgentCmd>,
    pub agent_rx: UnboundedReceiver<AgentEvent>,
    /// `{text}` is replaced by what the driver said.
    pub prompt_template: String,
    pub speaker: u32,
}

pub fn render_prompt(template: &str, text: &str) -> String {
    if template.contains("{text}") {
        template.replace("{text}", text)
    } else {
        format!("{template}\n{text}")
    }
}

struct Current {
    turn: Turn,
    /// When the driver finished speaking (or the question was typed).
    started: Instant,
    splitter: SentenceSplitter,
    got_token: bool,
}

/// One sentence to speak.
struct Clip {
    turn: Turn,
    generation: u64,
    started: Instant,
    first: bool,
    text: String,
}

fn ms(since: Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

/// Synthesizes sentences in order and queues them for playback.
/// Clips of an interrupted generation are dropped.
async fn speak(
    mut clips: UnboundedReceiver<Clip>,
    synth: Arc<dyn Synth>,
    output: Arc<dyn Output>,
    generation: Arc<AtomicU64>,
    speaker: Arc<AtomicU32>,
    events: UnboundedSender<Event>,
) {
    while let Some(clip) = clips.recv().await {
        if clip.generation != generation.load(Ordering::SeqCst) {
            continue;
        }
        let synth = synth.clone();
        let text = clip.text.clone();
        let id = speaker.load(Ordering::Relaxed);
        let result = tokio::task::spawn_blocking(move || synth.synthesize(&text, id)).await;
        if clip.generation != generation.load(Ordering::SeqCst) {
            continue;
        }
        match result {
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
}

pub async fn run(deps: Deps, mut cmds: UnboundedReceiver<Command>, events: UnboundedSender<Event>) {
    let Deps {
        recorder,
        transcriber,
        synth,
        output,
        agent_tx,
        mut agent_rx,
        prompt_template,
        speaker,
    } = deps;
    let generation = Arc::new(AtomicU64::new(0));
    let speaker = Arc::new(AtomicU32::new(speaker));
    let muted = Arc::new(AtomicBool::new(false));
    let (clip_tx, clip_rx) = unbounded_channel::<Clip>();
    tokio::spawn(speak(
        clip_rx,
        synth,
        output.clone(),
        generation.clone(),
        speaker.clone(),
        events.clone(),
    ));
    let (stt_tx, mut stt_rx) = unbounded_channel::<(Turn, anyhow::Result<String>, u64)>();

    let mut next_turn: Turn = 0;
    let mut current: Option<Current> = None;
    let mut recording = false;
    let mut speaking = false;
    let mut first_sentence = true;
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let emit = |e: Event| {
        let _ = events.send(e);
    };

    // Interrupts the speech and the running turn.
    let barge_in = |current: &mut Option<Current>| {
        generation.fetch_add(1, Ordering::SeqCst);
        output.stop();
        if let Some(c) = current.take() {
            let _ = agent_tx.send(AgentCmd::Cancel);
            emit(Event::TurnDone {
                turn: c.turn,
                outcome: Outcome::Cancelled,
                ms: ms(c.started),
            });
        }
    };

    let mut begin = |current: &mut Option<Current>| -> Turn {
        next_turn += 1;
        *current = Some(Current {
            turn: next_turn,
            started: Instant::now(),
            splitter: SentenceSplitter::default(),
            got_token: false,
        });
        emit(Event::TurnStarted { turn: next_turn });
        next_turn
    };

    let send_clip = |c: &Current, text: String, first: &mut bool| {
        if muted.load(Ordering::Relaxed) {
            return;
        }
        let _ = clip_tx.send(Clip {
            turn: c.turn,
            generation: generation.load(Ordering::SeqCst),
            started: c.started,
            first: std::mem::take(first),
            text,
        });
    };

    loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    Command::PttDown => {
                        if recording {
                            continue;
                        }
                        barge_in(&mut current);
                        recorder.start();
                        recording = true;
                        emit(Event::Recording(true));
                    }
                    Command::PttUp => {
                        if !recording {
                            continue;
                        }
                        recording = false;
                        let rec = recorder.stop();
                        emit(Event::Recording(false));
                        emit(Event::Level(0.0));
                        if rec.seconds() < MIN_SPEECH {
                            continue;
                        }
                        let turn = begin(&mut current);
                        first_sentence = true;
                        let transcriber = transcriber.clone();
                        let tx = stt_tx.clone();
                        let t0 = Instant::now();
                        tokio::task::spawn_blocking(move || {
                            let result = transcriber.transcribe(&rec.to_wav());
                            let _ = tx.send((turn, result, ms(t0)));
                        });
                    }
                    Command::Ask(text) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        barge_in(&mut current);
                        let turn = begin(&mut current);
                        first_sentence = true;
                        emit(Event::Transcript { turn, text: text.clone(), ms: 0 });
                        let _ = agent_tx.send(AgentCmd::Prompt { turn, text: render_prompt(&prompt_template, &text) });
                    }
                    Command::Cancel => barge_in(&mut current),
                    Command::SetSpeaker(id) => speaker.store(id, Ordering::Relaxed),
                    Command::SetMuted(m) => {
                        muted.store(m, Ordering::Relaxed);
                        if m {
                            generation.fetch_add(1, Ordering::SeqCst);
                            output.stop();
                        }
                    }
                }
            }
            Some((turn, result, took)) = stt_rx.recv() => {
                let Some(c) = current.as_ref().filter(|c| c.turn == turn) else { continue };
                match result {
                    Ok(text) if text.trim().is_empty() => {
                        emit(Event::Transcript { turn, text: String::new(), ms: took });
                        emit(Event::TurnDone { turn, outcome: Outcome::NoSpeech, ms: ms(c.started) });
                        current = None;
                    }
                    Ok(text) => {
                        emit(Event::Transcript { turn, text: text.clone(), ms: took });
                        let _ = agent_tx.send(AgentCmd::Prompt { turn, text: render_prompt(&prompt_template, &text) });
                    }
                    Err(e) => {
                        let error = format!("transcription: {e:#}");
                        emit(Event::Error(error.clone()));
                        emit(Event::TurnDone { turn, outcome: Outcome::Failed(error), ms: ms(c.started) });
                        current = None;
                    }
                }
            }
            Some(ev) = agent_rx.recv() => match ev {
                AgentEvent::Ready { name } => emit(Event::AgentReady { name }),
                AgentEvent::Closed { error } => {
                    if let Some(c) = current.take() {
                        emit(Event::TurnDone { turn: c.turn, outcome: Outcome::Failed("agent closed".into()), ms: ms(c.started) });
                    }
                    emit(Event::AgentClosed { error });
                }
                AgentEvent::Chunk { turn, text } => {
                    let Some(c) = current.as_mut().filter(|c| c.turn == turn) else { continue };
                    if !c.got_token {
                        c.got_token = true;
                        emit(Event::FirstToken { turn, ms: ms(c.started) });
                    }
                    let sentences = c.splitter.push(&text);
                    emit(Event::Chunk { turn, text });
                    let c = current.as_ref().expect("checked above");
                    for s in sentences {
                        send_clip(c, s, &mut first_sentence);
                    }
                }
                AgentEvent::Tool { turn, id, title, state } => {
                    if current.as_ref().is_some_and(|c| c.turn == turn) {
                        emit(Event::Tool { turn, id, title, state });
                    }
                }
                AgentEvent::Permission { turn, title, allowed } => {
                    if current.as_ref().is_some_and(|c| c.turn == turn) {
                        emit(Event::Permission { turn, title, allowed });
                    }
                }
                AgentEvent::TurnEnd { turn, .. } | AgentEvent::TurnFailed { turn, .. }
                    if current.as_ref().is_some_and(|c| c.turn == turn) =>
                {
                    let mut c = current.take().expect("checked above");
                    if let Some(rest) = c.splitter.finish() {
                        send_clip(&c, rest, &mut first_sentence);
                    }
                    let outcome = match ev {
                        AgentEvent::TurnFailed { error, .. } => {
                            emit(Event::Error(format!("agent: {error}")));
                            Outcome::Failed(error)
                        }
                        _ => Outcome::Answered,
                    };
                    emit(Event::TurnDone { turn, outcome, ms: ms(c.started) });
                }
                AgentEvent::TurnEnd { .. } | AgentEvent::TurnFailed { .. } => {}
            },
            _ = tick.tick() => {
                if recording {
                    emit(Event::Level(recorder.level()));
                }
                let now = !output.is_idle();
                if now != speaking {
                    speaking = now;
                    emit(Event::Speaking(now));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::agent::{self, fake};
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
    }

    struct FakeStt(&'static str);
    impl Transcriber for FakeStt {
        fn transcribe(&self, wav: &[u8]) -> anyhow::Result<String> {
            assert_eq!(&wav[..4], b"RIFF");
            Ok(self.0.to_string())
        }
    }

    struct EchoSynth;
    impl Synth for EchoSynth {
        fn synthesize(&self, text: &str, speaker: u32) -> anyhow::Result<Vec<u8>> {
            Ok(format!("{speaker}:{text}").into_bytes())
        }
    }

    #[derive(Default)]
    struct FakeOut {
        played: Mutex<Vec<String>>,
        stops: AtomicU32,
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
    }

    struct Harness {
        cmds: UnboundedSender<Command>,
        events: UnboundedReceiver<Event>,
        out: Arc<FakeOut>,
        prompts: Arc<Mutex<Vec<String>>>,
    }

    fn start(reply: &'static [&'static str], stt: &'static str, seconds: f32) -> Harness {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let (agent_tx, agent_cmd_rx) = unbounded_channel();
        let (agent_ev_tx, agent_rx) = unbounded_channel();
        tokio::spawn(agent::run(
            fake::agent(reply, prompts.clone()),
            std::env::temp_dir(),
            Permissions::Safe,
            agent_cmd_rx,
            agent_ev_tx,
        ));
        let out = Arc::new(FakeOut::default());
        let (cmds, cmd_rx) = unbounded_channel();
        let (ev_tx, events) = unbounded_channel();
        tokio::spawn(run(
            Deps {
                recorder: Arc::new(FakeMic {
                    seconds,
                    started: AtomicBool::new(false),
                }),
                transcriber: Arc::new(FakeStt(stt)),
                synth: Arc::new(EchoSynth),
                output: out.clone(),
                agent_tx,
                agent_rx,
                prompt_template: "無線:「{text}」".into(),
                speaker: 3,
            },
            cmd_rx,
            ev_tx,
        ));
        Harness {
            cmds,
            events,
            out,
            prompts,
        }
    }

    impl Harness {
        /// Events up to and including the first one matching `stop`, minus levels.
        async fn until(&mut self, stop: impl Fn(&Event) -> bool) -> Vec<Event> {
            let mut seen = Vec::new();
            loop {
                let e = tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                    .await
                    .unwrap_or_else(|_| panic!("timed out; saw {seen:?}"))
                    .expect("engine stopped");
                let done = stop(&e);
                if !matches!(e, Event::Level(_)) {
                    seen.push(e);
                }
                if done {
                    return seen;
                }
            }
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
            vec!["無線:「前との差は？」".to_string()]
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

    #[tokio::test]
    async fn pressing_again_interrupts_speech_and_agent() {
        let mut h = start(&["了解。"], "ボックス", 1.0);
        h.until(|e| matches!(e, Event::AgentReady { .. })).await;
        h.cmds.send(Command::Ask("slow".into())).unwrap();
        h.until(|e| matches!(e, Event::TurnStarted { .. })).await;
        let stops_before = h.out.stops.load(Ordering::SeqCst);
        h.cmds.send(Command::PttDown).unwrap();
        let events = h.until(|e| matches!(e, Event::Recording(true))).await;
        assert!(matches!(
            events[..],
            [
                ..,
                Event::TurnDone {
                    turn: 1,
                    outcome: Outcome::Cancelled,
                    ..
                },
                Event::Recording(true)
            ]
        ));
        assert_eq!(h.out.stops.load(Ordering::SeqCst), stops_before + 1);

        h.cmds.send(Command::PttUp).unwrap();
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

    #[test]
    fn elapsed_milliseconds() {
        let t = Instant::now() - Duration::from_millis(50);
        let m = ms(t);
        assert!((50..10_000).contains(&m), "{m}");
    }

    #[test]
    fn prompt_template() {
        assert_eq!(render_prompt("無線「{text}」", "差は"), "無線「差は」");
        assert_eq!(render_prompt("前置き", "差は"), "前置き\n差は");
    }
}
