use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use vr_core::{Capabilities, Frame, Poll, SourceError, TelemetrySource};

use crate::Pacer;

/// Replays frames recorded by [`Recorder`].
pub struct ReplaySource {
    game: String,
    lines: std::io::Lines<BufReader<File>>,
    pending: Option<Frame>,
    pacer: Pacer,
    done: bool,
}

impl ReplaySource {
    pub fn open(path: impl AsRef<Path>, speed: f64) -> Result<Self, SourceError> {
        let file = File::open(path.as_ref())?;
        let mut s = Self {
            game: "replay".into(),
            lines: BufReader::new(file).lines(),
            pending: None,
            pacer: Pacer::new(speed),
            done: false,
        };
        // Peek the first frame to learn which game was recorded.
        s.pending = s.next_frame()?;
        if let Some(f) = &s.pending {
            s.game = f.game.clone();
        }
        Ok(s)
    }

    fn next_frame(&mut self) -> Result<Option<Frame>, SourceError> {
        for line in self.lines.by_ref() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let f: Frame = serde_json::from_str(&line)
                .map_err(|e| SourceError::Format(format!("bad replay line: {e}")))?;
            return Ok(Some(f));
        }
        Ok(None)
    }
}

impl TelemetrySource for ReplaySource {
    fn game(&self) -> &str {
        &self.game
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::ALL
    }

    fn poll(&mut self, timeout: Duration) -> Result<Poll, SourceError> {
        if self.done {
            return Ok(Poll::Disconnected);
        }
        if self.pending.is_none() {
            self.pending = self.next_frame()?;
        }
        let Some(f) = self.pending.take() else {
            self.done = true;
            return Ok(Poll::Disconnected);
        };
        if !self.pacer.wait_until(f.session.time_s, timeout) {
            self.pending = Some(f);
            return Ok(Poll::Idle);
        }
        Ok(Poll::Frame(Box::new(f)))
    }

    fn reconnectable(&self) -> bool {
        false
    }
}

/// Writes frames as NDJSON (buffered; flushed on [`Recorder::flush`] and on drop), downsampled to at most `hz` frames per second of
/// session time (`hz <= 0` keeps every frame).
pub struct Recorder {
    out: BufWriter<File>,
    min_dt: f64,
    last_t: Option<(i32, f64)>,
}

impl Recorder {
    pub fn create(path: impl AsRef<Path>, hz: f64) -> std::io::Result<Self> {
        if let Some(dir) = path.as_ref().parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)?;
        }
        Ok(Self {
            out: BufWriter::new(File::create(path)?),
            min_dt: if hz > 0.0 { 1.0 / hz } else { 0.0 },
            last_t: None,
        })
    }

    pub fn record(&mut self, f: &Frame) -> std::io::Result<()> {
        let key = (f.session.session_num, f.session.time_s);
        if let Some((sn, t)) = self.last_t
            && sn == key.0
            && key.1 >= t
            && key.1 - t < self.min_dt - 1e-6
        {
            return Ok(());
        }
        self.last_t = Some(key);
        serde_json::to_writer(&mut self.out, f).map_err(std::io::Error::other)?;
        self.out.write_all(b"\n")
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}
