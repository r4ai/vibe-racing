//! Log output that never holds up the program.
//!
//! Writing to a Windows console blocks while the user selects text in it, and
//! writing to a full pipe blocks until somebody reads. Either would stop the
//! telemetry for as long as it lasts, so a thread of its own writes the lines
//! and lines that do not fit are dropped.

use std::io::Write;
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};

use tracing_subscriber::fmt::MakeWriter;

/// Lines kept while the output is blocked.
const QUEUE: usize = 512;

#[derive(Clone)]
pub struct Lines(SyncSender<Vec<u8>>);

impl Lines {
    /// Writes to `out` from a thread of its own.
    pub fn to(mut out: impl Write + Send + 'static) -> Self {
        let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE);
        let _ = std::thread::Builder::new()
            .name("vr-log".into())
            .spawn(move || {
                while let Ok(line) = rx.recv() {
                    let _ = out.write_all(&line);
                    let _ = out.flush();
                }
            });
        Self(tx)
    }
}

impl Write for Lines {
    fn write(&mut self, line: &[u8]) -> std::io::Result<usize> {
        match self.0.try_send(line.to_vec()) {
            Ok(()) | Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => Ok(line.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Lines {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};
    use std::time::{Duration, Instant};

    use super::*;

    /// An output that blocks until it is released.
    struct Stuck(Receiver<()>);

    impl Write for Stuck {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.recv();
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_stuck_output_does_not_block() {
        let (release, stuck) = channel();
        let mut lines = Lines::to(Stuck(stuck));
        let t = Instant::now();
        for _ in 0..QUEUE * 2 {
            assert_eq!(lines.write(b"line\n").unwrap(), 5);
        }
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
        drop(release);
    }

    #[test]
    fn lines_arrive_in_order() {
        struct Out(std::sync::mpsc::Sender<Vec<u8>>);
        impl Write for Out {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                let _ = self.0.send(b.to_vec());
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (tx, rx) = channel();
        let mut lines = Lines::to(Out(tx));
        lines.write_all(b"a").unwrap();
        lines.write_all(b"b").unwrap();
        let wait = Duration::from_secs(5);
        assert_eq!(rx.recv_timeout(wait).unwrap(), b"a");
        assert_eq!(rx.recv_timeout(wait).unwrap(), b"b");
    }
}
