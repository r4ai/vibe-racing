//! Helper processes (`vr ...`): found on the `PATH`, without a window, and
//! kept running.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::config::resolve_program;

/// The words of a command line; quotes keep one with spaces together.
fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                word.get_or_insert_default();
            }
            c if c.is_whitespace() && !quoted => words.extend(word.take()),
            c => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word);
    words
}

/// The process of a plain command line, without a window and with no input.
/// `None` for an empty one.
pub fn command(line: &str, cwd: &Path) -> Option<Command> {
    let words = words(line);
    let (program, args) = words.split_first()?;
    let path = std::env::var("PATH").ok();
    let resolved = cfg!(windows)
        .then(|| {
            resolve_program(
                program,
                path.as_deref(),
                std::env::var("PATHEXT").ok().as_deref(),
            )
        })
        .flatten()
        .unwrap_or_else(|| program.into());
    let mut cmd = Command::new(resolved);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Some(cmd)
}

/// How long to wait before starting a process (again).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// Before the first start.
    pub initial: Duration,
    /// After the first end; doubles up to `max`.
    pub first: Duration,
    pub max: Duration,
}

impl Retry {
    pub fn every(d: Duration) -> Self {
        Self {
            initial: Duration::ZERO,
            first: d,
            max: d,
        }
    }

    /// The wait after `failures` earlier runs that came to nothing.
    pub fn delay(&self, failures: u32) -> Duration {
        let times = 1u32.checked_shl(failures).unwrap_or(u32::MAX);
        self.first.saturating_mul(times).min(self.max)
    }
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    /// Running, and since when.
    child: Mutex<Option<(Child, Instant)>>,
}

impl Shared {
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some((mut c, _)) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn sleep(&self, d: Duration) {
        let until = Instant::now() + d;
        while !self.stopped() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            std::thread::sleep(left.min(Duration::from_millis(50)));
        }
    }

    /// Until the process ends or is killed.
    fn wait(&self) {
        loop {
            let mut child = self.child.lock().unwrap();
            let running = matches!(child.as_mut().map(|(c, _)| c.try_wait()), Some(Ok(None)));
            if !running {
                child.take();
                return;
            }
            drop(child);
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// All of them, for [`stop_all`].
static RUNNING: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());

/// Ends every supervised process: tasks of a runtime that goes away with the
/// process are not dropped.
pub fn stop_all() {
    let all = std::mem::take(&mut *RUNNING.lock().unwrap());
    for s in all.iter().filter_map(Weak::upgrade) {
        s.stop();
    }
}

/// A process kept running until this is dropped.
pub struct Supervised {
    shared: Arc<Shared>,
}

impl Drop for Supervised {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

impl Supervised {
    /// The process has been running for at least `d`.
    pub fn running(&self, d: Duration) -> bool {
        let child = self.shared.child.lock().unwrap();
        child.as_ref().is_some_and(|(_, t)| t.elapsed() >= d)
    }
}

/// Called with each line the process prints; `false` ends the supervision.
pub type OnLine = Box<dyn FnMut(&str) -> bool + Send>;

impl Supervised {
    /// `None` when the command is empty or the thread cannot be started.
    pub fn spawn(line: &str, cwd: PathBuf, retry: Retry, on_line: Option<OnLine>) -> Option<Self> {
        let line = line.trim().to_string();
        command(&line, &cwd)?;
        let shared = Arc::new(Shared::default());
        let for_thread = shared.clone();
        std::thread::Builder::new()
            .name("vr-radio-child".into())
            .spawn(move || supervise(&line, &cwd, retry, on_line, &for_thread))
            .ok()?;
        RUNNING.lock().unwrap().push(Arc::downgrade(&shared));
        Some(Self { shared })
    }
}

fn supervise(line: &str, cwd: &Path, retry: Retry, mut on_line: Option<OnLine>, shared: &Shared) {
    shared.sleep(retry.initial);
    let mut failures = 0;
    while !shared.stopped() {
        let Some(mut cmd) = command(line, cwd) else {
            return;
        };
        cmd.stdout(if on_line.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        match cmd.spawn() {
            Ok(mut child) => {
                let out = child.stdout.take();
                *shared.child.lock().unwrap() = Some((child, Instant::now()));
                // Stopped while it was starting.
                if shared.stopped() {
                    shared.stop();
                    return;
                }
                if let (Some(out), Some(on_line)) = (out, &mut on_line) {
                    for l in BufReader::new(out).lines() {
                        let Ok(l) = l else { break };
                        failures = 0;
                        if !on_line(&l) {
                            shared.stop();
                            return;
                        }
                    }
                }
                shared.wait();
            }
            Err(e) => tracing::warn!("{line}: {e}"),
        }
        shared.sleep(retry.delay(failures));
        failures = failures.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    const FAST: Retry = Retry {
        initial: Duration::ZERO,
        first: Duration::from_millis(10),
        max: Duration::from_millis(10),
    };

    #[test]
    fn waits_double_up_to_the_limit() {
        let r = Retry {
            initial: Duration::ZERO,
            first: Duration::from_secs(1),
            max: Duration::from_secs(30),
        };
        let secs = [0, 1, 2, 4, 5, 60].map(|n| r.delay(n).as_secs());
        assert_eq!(secs, [1, 2, 4, 16, 30, 30]);
        let r = Retry::every(Duration::from_secs(15));
        assert_eq!(
            [0, 9].map(|n| r.delay(n)),
            [Duration::from_secs(15), Duration::from_secs(15)]
        );
        assert_eq!(r.initial, Duration::ZERO);
    }

    #[test]
    fn quotes_keep_words_together() {
        assert_eq!(words("vr watch  -f json"), ["vr", "watch", "-f", "json"]);
        assert_eq!(
            words(r#" "C:\Program Files\run.exe" --port 1 "" "#),
            [r"C:\Program Files\run.exe", "--port", "1", ""]
        );
        assert!(words("  ").is_empty());
    }

    #[test]
    fn an_empty_command_is_off() {
        assert!(command("  ", &std::env::temp_dir()).is_none());
        assert!(Supervised::spawn("", std::env::temp_dir(), FAST, None).is_none());
    }

    // `rustc` is there wherever the tests are built.
    #[test]
    fn it_is_started_again_when_it_ends() {
        let (tx, rx) = mpsc::channel();
        let s = Supervised::spawn(
            "rustc --version",
            std::env::temp_dir(),
            FAST,
            Some(Box::new(move |l| tx.send(l.to_string()).is_ok())),
        );
        assert!(s.is_some());
        for _ in 0..2 {
            let l = rx.recv_timeout(Duration::from_secs(20)).unwrap();
            assert!(l.starts_with("rustc "), "{l}");
        }
        // One that does not exist is tried again too, and can be dropped.
        drop(Supervised::spawn(
            "vr-no-such-program",
            std::env::temp_dir(),
            FAST,
            None,
        ));
    }

    #[test]
    fn dropping_it_ends_the_process() {
        let line = if cfg!(windows) {
            "ping -n 30 127.0.0.1"
        } else {
            "sleep 30"
        };
        let s = Supervised::spawn(line, std::env::temp_dir(), FAST, None).unwrap();
        let shared = s.shared.clone();
        let t = Instant::now();
        while shared.child.lock().unwrap().is_none() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(shared.child.lock().unwrap().is_some());
        assert!(s.running(Duration::ZERO));
        assert!(!s.running(Duration::from_secs(60)));
        drop(s);
        assert!(shared.stopped());
        assert!(shared.child.lock().unwrap().is_none());
    }
}
