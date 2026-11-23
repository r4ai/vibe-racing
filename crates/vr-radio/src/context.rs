//! Data fetched for the question before the agent is asked.
//!
//! Asking the agent to look it up costs a model round trip to decide on the
//! command, the command, and another round trip to read it. The commands
//! (`vr brief`...) take milliseconds and are nearly always the same ones, so
//! they run while the speech is being transcribed and their output goes into
//! the prompt. The agent still runs whatever else it needs.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::decide::Decide;
use crate::quick::{self, Quick};
use crate::{child, topic};

/// Data later than this is not worth waiting for: the agent looks it up.
const TIMEOUT: Duration = Duration::from_millis(1500);
/// A runaway command must not flood the prompt.
const MAX_BYTES: usize = 16 * 1024;

/// What a question needs beyond the usual data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct More {
    pub data: Option<String>,
    /// The answer will take its time.
    pub long: bool,
    /// The answer itself, worked out without the agent.
    pub answer: Option<Answer>,
}

/// An answer that needs no agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Text(String),
    /// What was said last, once more.
    Again,
    /// A word of agreement, unless something was asked.
    Ack,
    /// Hardly anything was heard: how things are.
    Unclear(String),
}

pub trait ContextSource: Send + Sync + 'static {
    /// The data as text, `None` when there is none. Blocking.
    fn fetch(&self) -> Option<String>;

    /// More, for what the driver said. Blocking.
    fn fetch_for(&self, _text: &str) -> More {
        More::default()
    }

    /// The answer to the words, if they need no agent. Blocking.
    fn answer(&self, _text: &str) -> Option<Answer> {
        None
    }

    /// Everything worth knowing for a closer look at something. Blocking.
    fn fetch_all(&self, _text: &str) -> Option<String> {
        None
    }

    /// What `vr say` says about a topic. Blocking.
    fn say(&self, _topic: &str) -> Option<String> {
        None
    }

    /// The driver begins to speak. Blocking.
    fn prepare(&self) {}
}

/// Runs commands at the same time and joins what they print.
pub struct Commands {
    pub commands: Vec<String>,
    /// Says a topic, which is added to it; empty: nothing is said without the
    /// agent.
    pub say: String,
    /// Also those for what the question is about.
    pub topics: bool,
    /// Asked when the words do not tell what the question is about.
    pub decide: Option<Arc<dyn Decide>>,
    pub cwd: PathBuf,
}

impl ContextSource for Commands {
    fn fetch(&self) -> Option<String> {
        self.output(&self.commands)
    }

    fn fetch_for(&self, text: &str) -> More {
        if !self.topics {
            return More::default();
        }
        let found = topic::read(text)
            .or_else(|| match self.decide.as_ref()?.decide(text) {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::warn!("{e:#}");
                    None
                }
            })
            .unwrap_or_default();
        let mut commands = topic::commands(&found, text);
        commands.retain(|c| !self.commands.contains(c));
        More {
            data: self.output(&commands),
            long: found.long,
            answer: None,
        }
    }

    fn fetch_all(&self, text: &str) -> Option<String> {
        if self.commands.is_empty() {
            return None;
        }
        let found = topic::read(text).unwrap_or_default();
        let mut commands: Vec<String> = self
            .commands
            .iter()
            .filter(|c| !topic::STUDY.iter().any(|s| within(c, s)))
            .cloned()
            .collect();
        commands.extend(topic::STUDY.iter().map(|c| c.to_string()));
        for c in topic::commands(&found, text) {
            if !commands.iter().any(|x| within(&c, x)) {
                commands.push(c);
            }
        }
        self.output(&commands)
    }

    fn answer(&self, text: &str) -> Option<Answer> {
        if self.say.is_empty() {
            return None;
        }
        Some(match quick::read(text)? {
            Quick::Say(topic) => {
                // The words found a topic; the decision model tells whether its
                // report is what was asked. Unsure, the agent answers.
                if let Some(d) = &self.decide {
                    match d.fits(text, quick::about(topic)) {
                        Ok(true) => {}
                        Ok(false) => return None,
                        Err(e) => {
                            tracing::warn!("{e:#}");
                            return None;
                        }
                    }
                }
                Answer::Text(self.say(topic)?)
            }
            Quick::Ack => Answer::Ack,
            Quick::Here(text) => Answer::Text(text.into()),
            Quick::Again => Answer::Again,
            Quick::Unclear => Answer::Unclear(self.say("status")?),
        })
    }

    fn say(&self, topic: &str) -> Option<String> {
        if self.say.is_empty() {
            return None;
        }
        // Also when there is no game: it says so.
        let out = output_of(&format!("{} {topic}", self.say), &self.cwd, TIMEOUT)?;
        (!out.is_empty()).then_some(out)
    }

    fn prepare(&self) {
        if let Some(d) = self.decide.as_ref().filter(|_| self.topics) {
            d.warm_up();
        }
    }
}

impl Commands {
    fn output(&self, commands: &[String]) -> Option<String> {
        let parts: Vec<String> = std::thread::scope(|s| {
            let threads: Vec<_> = commands
                .iter()
                .map(|c| s.spawn(move || run(c, &self.cwd, TIMEOUT)))
                .collect();
            threads
                .into_iter()
                .filter_map(|t| t.join().ok().flatten())
                .collect()
        });
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }
}

/// The same command, perhaps with other options.
fn within(a: &str, b: &str) -> bool {
    let name = |c: &str| -> Vec<String> {
        c.split(' ')
            .take_while(|w| !w.starts_with('-'))
            .map(String::from)
            .collect()
    };
    name(a) == name(b)
}

/// `$ command` and its output; `None` when it fails or prints nothing.
fn run(command: &str, cwd: &Path, timeout: Duration) -> Option<String> {
    let text = output_of(command, cwd, timeout)?;
    (!text.is_empty()).then(|| format!("$ {command}\n{text}"))
}

/// What the command prints; `None` when it fails.
fn output_of(command: &str, cwd: &Path, timeout: Duration) -> Option<String> {
    let mut cmd = child::command(command, cwd)?;
    cmd.stdout(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.take(MAX_BYTES as u64).read_to_end(&mut out);
        out
    });
    let started = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader.join().ok()?;
    let text = String::from_utf8_lossy(&out).trim().to_string();
    ok.then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(list: &[&str]) -> Commands {
        Commands {
            commands: list.iter().map(|c| c.to_string()).collect(),
            say: String::new(),
            topics: true,
            decide: None,
            cwd: std::env::temp_dir(),
        }
    }

    // `rustc` is there wherever the tests are built.
    #[test]
    fn output_is_labelled_and_joined_in_order() {
        let text = commands(&["rustc --version", "cargo --version"])
            .fetch()
            .unwrap();
        assert!(text.starts_with("$ rustc --version\nrustc "), "{text}");
        assert!(text.contains("\n\n$ cargo --version\ncargo "), "{text}");
    }

    #[test]
    fn failures_are_left_out() {
        assert_eq!(commands(&["vr-no-such-program x"]).fetch(), None);
        assert_eq!(commands(&["rustc --no-such-option"]).fetch(), None);
        assert_eq!(commands(&[""]).fetch(), None);
        assert_eq!(commands(&[]).fetch(), None);
        let text = commands(&["vr-no-such-program", "rustc --version"]).fetch();
        assert!(text.unwrap().starts_with("$ rustc --version"));
    }

    struct FakeDecide(anyhow::Result<topic::Found>);
    impl Decide for FakeDecide {
        fn decide(&self, _: &str) -> anyhow::Result<topic::Found> {
            match &self.0 {
                Ok(f) => Ok(f.clone()),
                Err(e) => Err(anyhow::anyhow!("{e}")),
            }
        }
    }

    #[test]
    fn the_decision_counts_only_when_the_words_do_not_tell() {
        let long = || topic::Found {
            topics: Vec::new(),
            long: true,
        };
        let with = |d, topics| Commands {
            decide: Some(Arc::new(FakeDecide(d))),
            topics,
            ..commands(&[])
        };
        let unclear = "ケツが出る";
        assert!(with(Ok(long()), true).fetch_for(unclear).long);
        assert!(!with(Ok(long()), false).fetch_for(unclear).long);
        assert!(!with(Ok(long()), true).fetch_for("前との差は").long);
        let failed = with(Err(anyhow::anyhow!("offline")), true);
        assert_eq!(failed.fetch_for(unclear), More::default());
        assert_eq!(commands(&[]).fetch_for(unclear), More::default());
        assert!(commands(&[]).fetch_for("コースレコード調べて").long);
    }

    #[test]
    fn plain_questions_are_answered_by_the_command() {
        let echo = if cfg!(windows) { "cmd /c echo" } else { "echo" };
        let c = Commands {
            say: echo.into(),
            ..commands(&[])
        };
        assert_eq!(c.say("fuel").as_deref(), Some("fuel"));
        let text = |t: &str| Some(Answer::Text(t.into()));
        assert_eq!(c.answer("前との差は"), text("ahead"));
        assert_eq!(c.answer("今ない。"), text("position"));
        assert_eq!(c.answer("了解"), Some(Answer::Ack));
        assert_eq!(c.answer("もう一回"), Some(Answer::Again));
        assert_eq!(c.answer("んん"), Some(Answer::Unclear("status".into())));
        assert_eq!(c.answer("なんで遅いの"), None);
        // Not switched on, or not there.
        assert_eq!(commands(&[]).answer("前との差は"), None);
        assert_eq!(commands(&[]).say("fuel"), None);
        let broken = Commands {
            say: "vr-no-such-program say".into(),
            ..commands(&[])
        };
        assert_eq!(broken.answer("前との差は"), None);
        assert_eq!(broken.answer("了解"), Some(Answer::Ack));
    }

    /// Says whether a report answers a call: yes, no, or it is not there.
    struct Fits(Option<bool>);
    impl Decide for Fits {
        fn decide(&self, _: &str) -> anyhow::Result<topic::Found> {
            Ok(topic::Found::default())
        }
        fn fits(&self, _: &str, _: &str) -> anyhow::Result<bool> {
            self.0.ok_or_else(|| anyhow::anyhow!("offline"))
        }
    }

    #[test]
    fn a_report_is_read_out_only_when_it_answers_the_call() {
        let echo = if cfg!(windows) { "cmd /c echo" } else { "echo" };
        let with = |fits| Commands {
            say: echo.into(),
            decide: Some(Arc::new(Fits(fits))),
            ..commands(&[])
        };
        assert_eq!(
            with(Some(true)).answer("ダメージは"),
            Some(Answer::Text("damage".into()))
        );
        // Not what was asked, or not known: the agent answers.
        assert_eq!(with(Some(false)).answer("ダメージは"), None);
        assert_eq!(with(None).answer("ダメージは"), None);
        // Words that need no report need no decision.
        assert_eq!(with(Some(false)).answer("了解"), Some(Answer::Ack));
    }

    #[test]
    fn slow_commands_are_given_up() {
        let slow = if cfg!(windows) {
            "ping -n 6 127.0.0.1"
        } else {
            "sleep 5"
        };
        let t = Instant::now();
        assert_eq!(
            run(slow, &std::env::temp_dir(), Duration::from_millis(200)),
            None
        );
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }
}
