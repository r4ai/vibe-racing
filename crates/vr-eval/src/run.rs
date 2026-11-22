//! Drives the real radio pipeline through a stint: the microphone and the
//! speakers are simulated in real time, everything between them is real.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use vr_radio::agent::{TokenUsage, Turn};
use vr_radio::app;
use vr_radio::audio::{Output, Recorder, Recording};
use vr_radio::config::Config;
use vr_radio::engine::{Command, Event, Outcome};
use vr_radio::voicevox::{Synth, Voicevox};

use crate::audio::{RATE, Spec, Utterance, Voices, wav_seconds};
use crate::facts::{Fact, Sample};
use crate::scenario::{Behavior, Call, Stint, strip_pauses};
use crate::world::World;

/// The driver is busy between joining and the first call.
const SETTLE: Duration = Duration::from_secs(20);
const BETWEEN_CALLS: Duration = Duration::from_secs(4);
const TURN_TIMEOUT: Duration = Duration::from_secs(120);
/// Data read this long before the end of the question is still fresh.
const FRESH: Duration = Duration::from_secs(3);

/// A sentence the driver heard, in seconds from the end of the question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Heard {
    pub text: String,
    pub start_s: f64,
    pub end_s: f64,
}

/// What happened in one call. Times are seconds from the end of the question
/// (the last word, not the release of the button).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub config: String,
    pub trial: usize,
    pub stint: String,
    pub call: String,
    /// First call of the session.
    pub first: bool,
    pub said: String,
    pub voice: String,
    pub snr_db: Option<f32>,
    pub behavior: Behavior,
    pub facts: Vec<Fact>,
    pub points: Vec<String>,
    pub speech_s: f32,
    /// What the driver said and heard earlier in the stint.
    pub history: Vec<(String, String)>,
    pub transcript: Option<String>,
    pub transcript_s: Option<f64>,
    pub first_token_s: Option<f64>,
    pub tools: Vec<String>,
    pub heard: Vec<Heard>,
    pub reply: String,
    pub outcome: String,
    pub errors: Vec<String>,
    pub facts_stated: Vec<bool>,
    /// When the sentence with the first fact was over.
    pub informed_s: Option<f64>,
    pub truth_at_question: String,
    pub truth_at_answer: String,
    /// The moving values each second in between.
    pub truth_between: Vec<String>,
    /// Tokens of every turn that ended during the call (cancelled ones too).
    #[serde(default)]
    pub tokens: Option<Tokens>,
    /// Tokens of the primer, on the first call of the stint.
    #[serde(default)]
    pub primer_tokens: Option<Tokens>,
    /// What the session had cost when the call was over, as the agent says.
    #[serde(default)]
    pub session_cost: Option<(f64, String)>,
}

/// Token counts of one or more turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub turns: u32,
    pub total: u64,
    pub input: u64,
    pub cached_read: u64,
    pub cached_write: u64,
    pub output: u64,
    pub thought: u64,
}

impl Tokens {
    pub fn add(&mut self, u: &TokenUsage) {
        self.turns += 1;
        self.total += u.total;
        self.input += u.input;
        self.cached_read += u.cached_read;
        self.cached_write += u.cached_write;
        self.output += u.output;
        self.thought += u.thought;
    }
}

impl Record {
    /// Everything the driver heard.
    pub fn spoken(&self) -> String {
        let texts: Vec<&str> = self.heard.iter().map(|h| h.text.as_str()).collect();
        texts.join("")
    }

    pub fn first_audio_s(&self) -> Option<f64> {
        self.heard.first().map(|h| h.start_s)
    }
}

/// Plays the utterance into the pipeline while the button is down.
#[derive(Default)]
struct FakeMic {
    state: Mutex<MicState>,
}

#[derive(Default)]
struct MicState {
    utterance: Vec<f32>,
    pressed: Option<Instant>,
}

impl FakeMic {
    fn load(&self, u: &Utterance) {
        self.state.lock().unwrap().utterance = u.samples.clone();
    }

    /// Recorded since the button went down.
    fn recorded(&self) -> Vec<f32> {
        let st = self.state.lock().unwrap();
        let n = st
            .pressed
            .map_or(0, |p| (p.elapsed().as_secs_f32() * RATE as f32) as usize);
        st.utterance[..n.min(st.utterance.len())].to_vec()
    }
}

impl Recorder for FakeMic {
    fn start(&self) {
        self.state.lock().unwrap().pressed = Some(Instant::now());
    }

    fn stop(&self) -> Recording {
        let samples = self.recorded();
        self.state.lock().unwrap().pressed = None;
        Recording {
            samples,
            rate: RATE,
        }
    }

    fn peek(&self, from: usize) -> Recording {
        Recording {
            samples: self.recorded().get(from..).unwrap_or_default().to_vec(),
            rate: RATE,
        }
    }

    fn level(&self) -> f32 {
        let recorded = self.recorded();
        let tail = recorded.len().saturating_sub(RATE as usize / 50);
        vr_radio::wav::rms(&recorded[tail..])
    }
}

fn key(wav: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    wav.hash(&mut h);
    h.finish()
}

/// VOICEVOX, remembering which sound says what.
struct LoggedSynth {
    inner: Voicevox,
    texts: Arc<Mutex<HashMap<u64, String>>>,
}

impl Synth for LoggedSynth {
    fn synthesize(&self, text: &str, speaker: u32) -> Result<Vec<u8>> {
        let wav = self.inner.synthesize(text, speaker)?;
        self.texts.lock().unwrap().insert(key(&wav), text.into());
        Ok(wav)
    }
}

/// Speakers that take as long as the sound does.
struct FakeSpeakers {
    texts: Arc<Mutex<HashMap<u64, String>>>,
    queue: Mutex<Queue>,
}

struct Queue {
    ends: Instant,
    played: Vec<(String, Instant, Instant)>,
}

impl Output for FakeSpeakers {
    fn play(&self, wav: Vec<u8>) {
        let text = self.texts.lock().unwrap().get(&key(&wav)).cloned();
        let mut q = self.queue.lock().unwrap();
        let start = q.ends.max(Instant::now());
        let end = start + Duration::from_secs_f32(wav_seconds(&wav));
        q.ends = end;
        q.played.push((text.unwrap_or_default(), start, end));
    }

    fn stop(&self) {
        let now = Instant::now();
        let mut q = self.queue.lock().unwrap();
        q.ends = now;
        // Never reached the driver.
        q.played.retain(|(_, start, _)| *start < now);
        if let Some(last) = q.played.last_mut() {
            last.2 = last.2.min(now);
        }
    }

    fn is_idle(&self) -> bool {
        self.queue.lock().unwrap().ends <= Instant::now()
    }
}

struct Radio {
    cmds: UnboundedSender<Command>,
    events: UnboundedReceiver<Event>,
    mic: Arc<FakeMic>,
    speakers: Arc<FakeSpeakers>,
}

/// What a turn produced, with absolute times.
#[derive(Default)]
struct Observed {
    turn: Option<Turn>,
    transcript: Option<(String, Instant)>,
    first_token: Option<Instant>,
    tools: Vec<String>,
    reply: String,
    outcome: Option<Outcome>,
    errors: Vec<String>,
    /// Kept when a turn is replaced: the cancelled one was paid for too.
    tokens: Option<Tokens>,
    cost: Option<(f64, String)>,
}

impl Observed {
    fn apply(&mut self, e: Event) {
        let mine = |t: Turn, me: &Option<Turn>| *me == Some(t);
        match e {
            // A new turn replaces an interrupted one.
            Event::TurnStarted { turn } => {
                *self = Observed {
                    turn: Some(turn),
                    errors: std::mem::take(&mut self.errors),
                    tokens: self.tokens.take(),
                    cost: self.cost.take(),
                    ..Default::default()
                }
            }
            Event::Transcript { turn, text, .. } if mine(turn, &self.turn) => {
                self.transcript = Some((text, Instant::now()))
            }
            Event::FirstToken { turn, .. } if mine(turn, &self.turn) => {
                self.first_token = Some(Instant::now())
            }
            Event::Chunk { turn, text } if mine(turn, &self.turn) => self.reply.push_str(&text),
            Event::Tool {
                turn,
                title: Some(title),
                ..
            } if mine(turn, &self.turn) && !self.tools.contains(&title) => self.tools.push(title),
            Event::TurnDone { turn, outcome, .. } if mine(turn, &self.turn) => {
                self.outcome = Some(outcome)
            }
            Event::Error(e) => self.errors.push(e),
            Event::Usage { usage, .. } => self.tokens.get_or_insert_default().add(&usage),
            Event::Cost { amount, currency } => self.cost = Some((amount, currency)),
            _ => {}
        }
    }
}

impl Radio {
    fn start(cfg: &Config) -> Self {
        let texts = Arc::new(Mutex::new(HashMap::new()));
        let mic = Arc::new(FakeMic::default());
        let speakers = Arc::new(FakeSpeakers {
            texts: texts.clone(),
            queue: Mutex::new(Queue {
                ends: Instant::now(),
                played: Vec::new(),
            }),
        });
        let synth = Arc::new(LoggedSynth {
            inner: Voicevox::new(cfg.voicevox_url.clone(), cfg.speed),
            texts,
        });
        let (cmds, cmd_rx) = unbounded_channel();
        let (ev_tx, events) = unbounded_channel();
        let deps = app::deps(cfg, mic.clone(), speakers.clone(), synth);
        tokio::spawn(vr_radio::engine::run(deps, cmd_rx, ev_tx));
        Self {
            cmds,
            events,
            mic,
            speakers,
        }
    }

    async fn ready(&mut self) -> Result<()> {
        loop {
            match tokio::time::timeout(Duration::from_secs(120), self.events.recv()).await {
                Ok(Some(Event::AgentReady { .. })) => return Ok(()),
                Ok(Some(Event::AgentClosed { error })) => bail!("agent closed: {error:?}"),
                Ok(Some(_)) => {}
                Ok(None) => bail!("the radio stopped"),
                Err(_) => bail!("the agent did not start"),
            }
        }
    }

    /// Follows events for `d`.
    async fn follow(&mut self, d: Duration, seen: &mut Observed) -> Result<()> {
        let until = tokio::time::Instant::now() + d;
        loop {
            match tokio::time::timeout_at(until, self.events.recv()).await {
                Ok(Some(e)) => seen.apply(e),
                Ok(None) => return Err(anyhow!("the radio stopped")),
                Err(_) => return Ok(()),
            }
        }
    }

    /// Presses, says the utterance and lets go. Returns when its last word ended.
    async fn say(&mut self, u: &Utterance, seen: &mut Observed) -> Result<Instant> {
        self.mic.load(u);
        let _ = self.cmds.send(Command::PttDown);
        let pressed = Instant::now();
        self.follow(Duration::from_secs_f32(u.seconds()), seen)
            .await?;
        let _ = self.cmds.send(Command::PttUp);
        Ok(pressed + Duration::from_secs_f32(u.speech_end_s))
    }

    /// Sends the words as typed; returns when they were sent.
    fn type_in(&self, words: &str) -> Instant {
        let _ = self.cmds.send(Command::Ask(words.into()));
        Instant::now()
    }

    /// Until the turn is answered and the driver heard it.
    async fn answer(&mut self, seen: &mut Observed) -> Result<()> {
        let began = Instant::now();
        let mut quiet = 0;
        while began.elapsed() < TURN_TIMEOUT {
            self.follow(Duration::from_millis(100), seen).await?;
            let over =
                seen.outcome.is_some() || (seen.turn.is_none() && began.elapsed().as_secs() >= 5);
            // Speech of the last sentence may still be on its way.
            quiet = if over && self.speakers.is_idle() {
                quiet + 1
            } else {
                0
            };
            if quiet >= 20 {
                break;
            }
        }
        Ok(())
    }
}

pub struct Runner {
    pub config_name: String,
    pub trial: usize,
    pub cfg: Config,
    pub voices: Voices,
    pub port: u16,
    /// Typed questions instead of voices.
    pub text: bool,
}

/// Calls of a stint in text mode: a call without words has nothing to type.
pub fn typed_calls(stint: &Stint) -> impl Iterator<Item = &Call> {
    stint.calls.iter().filter(|c| !c.words().is_empty())
}

impl Runner {
    pub async fn stint(&self, stint: &Stint) -> Result<Vec<Record>> {
        // Voices first: the race must not run while they are made.
        let calls = if self.text {
            typed_calls(stint).map(|c| (c, None)).collect()
        } else {
            stint
                .calls
                .iter()
                .map(|c| Ok((c, Some(self.utterances(c)?))))
                .collect::<Result<Vec<_>>>()?
        };
        let world = World::start(&stint.world, self.port).await?;
        let mut radio = Radio::start(&self.cfg);
        radio.ready().await?;
        let mut seen = Observed::default();
        radio.follow(SETTLE, &mut seen).await?;
        let mut primer_tokens = seen.tokens;
        let mut cost = seen.cost;

        let mut records = Vec::new();
        let mut history = Vec::new();
        for (call, voices) in &calls {
            let r = self
                .call(stint, call, voices.as_ref(), &world, &mut radio, &history)
                .await?;
            history.push((r.said.clone(), r.spoken()));
            cost = r.session_cost.clone().or(cost);
            records.push(Record {
                first: records.is_empty(),
                primer_tokens: primer_tokens.take(),
                session_cost: cost.clone(),
                ..r
            });
            radio
                .follow(BETWEEN_CALLS, &mut Observed::default())
                .await?;
        }
        Ok(records)
    }

    fn utterances(&self, c: &Call) -> Result<(Utterance, Option<Utterance>)> {
        let snr_db = c.noise.map(|n| n.snr_db);
        let main = self.voices.utterance(&Spec {
            say: &c.say,
            voice: &c.voice,
            snr_db,
            cut_start_s: c.cut_start_s,
            cut_end_s: c.cut_end_s,
            duration_s: c.duration_s,
        })?;
        let barge = match &c.barge_in {
            Some(b) => Some(self.voices.utterance(&Spec {
                say: &b.say,
                voice: &c.voice,
                snr_db,
                cut_start_s: 0.0,
                cut_end_s: 0.0,
                duration_s: 0.0,
            })?),
            None => None,
        };
        Ok((main, barge))
    }

    #[allow(clippy::too_many_arguments)]
    async fn call(
        &self,
        stint: &Stint,
        call: &Call,
        voices: Option<&(Utterance, Option<Utterance>)>,
        world: &World,
        radio: &mut Radio,
        history: &[(String, String)],
    ) -> Result<Record> {
        radio.speakers.queue.lock().unwrap().played.clear();
        let mut seen = Observed::default();
        let mut question_end = match voices {
            Some((main, _)) => radio.say(main, &mut seen).await?,
            None => radio.type_in(&call.words()),
        };
        let (mut said, mut facts, mut points) =
            (call.words(), call.facts.clone(), call.points.clone());
        let mut speech_s = voices.map_or(0.0, |(m, _)| m.speech_end_s);
        let mut history = history.to_vec();
        if let Some(b) = &call.barge_in {
            radio
                .follow(Duration::from_secs_f32(b.after_s), &mut seen)
                .await?;
            question_end = match voices {
                Some((_, Some(u))) => {
                    speech_s = u.speech_end_s;
                    radio.say(u, &mut seen).await?
                }
                _ => radio.type_in(&strip_pauses(&b.say)),
            };
            history.push((said, "（割り込み）".into()));
            said = strip_pauses(&b.say);
            facts = b.facts.clone();
            points = b.points.clone();
        }
        let truth_at_question = world.text();
        radio.answer(&mut seen).await?;
        let _ = radio.cmds.send(Command::Cancel);

        let since = |t: Instant| {
            if t >= question_end {
                (t - question_end).as_secs_f64()
            } else {
                -(question_end - t).as_secs_f64()
            }
        };
        let played = radio.speakers.queue.lock().unwrap().played.clone();
        let heard: Vec<Heard> = played
            .iter()
            .map(|(text, s, e)| Heard {
                text: text.clone(),
                start_s: since(*s),
                end_s: since(*e),
            })
            .collect();
        // Data read until the last sentence may be in it. A preface ("I am
        // checking") must not close the window before the answer.
        let answered = played.last().map_or_else(Instant::now, |(_, s, _)| *s);
        let truth_at_answer = world.text_before(answered);
        let samples = world.between(question_end, question_end - FRESH, answered);
        let spoken: String = heard.iter().map(|h| h.text.as_str()).collect();
        Ok(Record {
            config: self.config_name.clone(),
            trial: self.trial,
            stint: stint.id.clone(),
            call: call.id.clone(),
            first: false,
            said,
            voice: call.voice.clone(),
            snr_db: call.noise.map(|n| n.snr_db),
            behavior: call.behavior,
            informed_s: informed_s(&facts, &heard, &samples),
            facts_stated: facts.iter().map(|f| f.stated(&spoken, &samples)).collect(),
            facts,
            points,
            speech_s,
            history,
            transcript_s: seen.transcript.as_ref().map(|(_, t)| since(*t)),
            transcript: seen.transcript.map(|(t, _)| t),
            first_token_s: seen.first_token.map(since),
            tools: seen.tools,
            heard,
            reply: seen.reply,
            outcome: format!("{:?}", seen.outcome),
            errors: seen.errors,
            truth_at_question,
            truth_at_answer,
            truth_between: each_second(&samples),
            tokens: seen.tokens,
            primer_tokens: None,
            session_cost: seen.cost,
        })
    }
}

/// One line per second of the trace.
fn each_second(samples: &[Sample]) -> Vec<String> {
    let mut last = f64::NEG_INFINITY;
    samples
        .iter()
        .filter(|s| {
            let keep = s.t - last >= 0.95;
            if keep {
                last = s.t;
            }
            keep
        })
        .map(Sample::line)
        .collect()
}

/// When the driver knew: the end of the sentence that completes the first
/// fact, or of the first sentence when nothing is checked by number.
fn informed_s(facts: &[Fact], heard: &[Heard], samples: &[Sample]) -> Option<f64> {
    let Some(fact) = facts.first() else {
        return heard.first().map(|h| h.end_s);
    };
    let mut said = String::new();
    heard.iter().find_map(|h| {
        said.push_str(&h.text);
        fact.stated(&said, samples).then_some(h.end_s)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::WorldSpec;
    use crate::world::sample_at;

    fn heard(text: &str, start_s: f64, end_s: f64) -> Heard {
        Heard {
            text: text.into(),
            start_s,
            end_s,
        }
    }

    #[test]
    fn informed_by_the_sentence_with_the_fact() {
        let s = [sample_at(&WorldSpec::default(), 620.0)];
        let h = [
            heard("了解、確認する。", 1.0, 2.0),
            heard("前まで0.6。", 4.0, 5.5),
            heard("離されるな。", 5.5, 7.0),
        ];
        assert_eq!(informed_s(&[Fact::GapAhead], &h, &s), Some(5.5));
        assert_eq!(informed_s(&[Fact::FuelAdd], &h, &s), None);
        assert_eq!(informed_s(&[], &h, &s), Some(2.0));
        assert_eq!(informed_s(&[], &[], &s), None);
    }

    #[test]
    fn the_microphone_runs_in_real_time() {
        let mic = FakeMic::default();
        mic.load(&Utterance {
            samples: vec![0.1; RATE as usize],
            speech_end_s: 1.0,
        });
        mic.start();
        std::thread::sleep(Duration::from_millis(200));
        let r = mic.stop();
        assert!((0.19..0.5).contains(&r.seconds()), "{}", r.seconds());
        mic.start();
        assert!(mic.level() >= 0.0);
    }

    #[test]
    fn speakers_queue_and_drop_what_was_not_heard() {
        let texts = Arc::new(Mutex::new(HashMap::new()));
        let wav = vr_radio::wav::encode_pcm16(&vec![0.1; 16_000], 16_000);
        texts.lock().unwrap().insert(key(&wav), "a".to_string());
        let s = FakeSpeakers {
            texts,
            queue: Mutex::new(Queue {
                ends: Instant::now(),
                played: Vec::new(),
            }),
        };
        s.play(wav.clone());
        s.play(wav);
        assert!(!s.is_idle());
        {
            let q = s.queue.lock().unwrap();
            assert_eq!(q.played[0].0, "a");
            assert_eq!(q.played[1].1, q.played[0].2);
        }
        std::thread::sleep(Duration::from_millis(20));
        s.stop();
        assert!(s.is_idle());
        let q = s.queue.lock().unwrap();
        assert_eq!(q.played.len(), 1);
        assert!(q.played[0].2 - q.played[0].1 < Duration::from_millis(900));
    }

    #[test]
    fn typing_skips_the_calls_without_words() {
        let dev = crate::scenario::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/dev.yaml"),
        )
        .unwrap();
        let typed: Vec<&str> = dev
            .iter()
            .flat_map(typed_calls)
            .map(|c| c.id.as_str())
            .collect();
        let all: usize = dev.iter().map(|s| s.calls.len()).sum();
        assert_eq!(typed.len(), all - 1);
        assert!(!typed.contains(&"noise-only"));
        assert!(typed.contains(&"cut-off"));
    }

    #[test]
    fn a_new_turn_replaces_the_interrupted_one() {
        let mut o = Observed::default();
        o.apply(Event::TurnStarted { turn: 1 });
        o.apply(Event::Chunk {
            turn: 1,
            text: "a".into(),
        });
        o.apply(Event::TurnStarted { turn: 2 });
        o.apply(Event::Chunk {
            turn: 1,
            text: "late".into(),
        });
        o.apply(Event::Chunk {
            turn: 2,
            text: "b".into(),
        });
        o.apply(Event::TurnDone {
            turn: 1,
            outcome: Outcome::Cancelled,
            ms: 1,
        });
        assert_eq!(o.reply, "b");
        assert_eq!(o.outcome, None);
        o.apply(Event::Usage {
            turn: 1,
            usage: TokenUsage {
                total: 100,
                input: 10,
                cached_read: 80,
                cached_write: 0,
                output: 10,
                thought: 4,
            },
        });
        o.apply(Event::TurnStarted { turn: 3 });
        o.apply(Event::Usage {
            turn: 3,
            usage: TokenUsage {
                total: 50,
                input: 5,
                cached_read: 40,
                cached_write: 1,
                output: 5,
                thought: 0,
            },
        });
        o.apply(Event::Cost {
            amount: 0.25,
            currency: "USD".into(),
        });
        assert_eq!(
            o.tokens,
            Some(Tokens {
                turns: 2,
                total: 150,
                input: 15,
                cached_read: 120,
                cached_write: 1,
                output: 15,
                thought: 4,
            })
        );
        assert_eq!(o.cost, Some((0.25, "USD".to_string())));
    }
}
