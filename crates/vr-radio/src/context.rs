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
use std::time::{Duration, Instant};

use crate::child;

/// Data later than this is not worth waiting for: the agent looks it up.
const TIMEOUT: Duration = Duration::from_millis(1500);
/// A runaway command must not flood the prompt.
const MAX_BYTES: usize = 16 * 1024;

pub trait ContextSource: Send + Sync + 'static {
    /// The data as text, `None` when there is none. Blocking.
    fn fetch(&self) -> Option<String>;
}

/// Runs commands at the same time and joins what they print.
pub struct Commands {
    pub commands: Vec<String>,
    pub cwd: PathBuf,
}

impl ContextSource for Commands {
    fn fetch(&self) -> Option<String> {
        let parts: Vec<String> = std::thread::scope(|s| {
            let threads: Vec<_> = self
                .commands
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

/// `$ command` and its output; `None` when it fails or prints nothing.
fn run(command: &str, cwd: &Path, timeout: Duration) -> Option<String> {
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
    (ok && !text.is_empty()).then(|| format!("$ {command}\n{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(list: &[&str]) -> Commands {
        Commands {
            commands: list.iter().map(|c| c.to_string()).collect(),
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
