use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vr_core::{Capabilities, Extras, Poll, SourceError, TelemetrySource};

use crate::convert::{build_frame, info};
use crate::layout::{Layout, MIN_VERSION, Snapshot};
use crate::shm::SharedMemory;
use crate::tracker::Tracker;

// (Type#1) GameState values that are not a session being driven.
const GAME_EXITED: u32 = 0;
const GAME_FRONT_END: u32 = 1;
const GAME_INGAME_REPLAY: u32 = 6;
const GAME_FRONT_END_REPLAY: u32 = 7;

/// The game writes once per rendered frame; no more than this is passed on.
const MIN_INTERVAL: Duration = Duration::from_micros(16_667);
/// How long to sleep between looks at the sequence number.
const NAP: Duration = Duration::from_millis(2);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Seconds on one clock for the whole process, so a source opened again
/// continues the session clock of the one before.
fn now_s() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Live Automobilista 2 telemetry over any [`SharedMemory`].
pub struct Ams2Source<M: SharedMemory> {
    mem: M,
    tracker: Arc<Mutex<Tracker>>,
    last_seq: Option<u32>,
    last_frame: Option<Instant>,
    last_data: Vec<u8>,
}

impl<M: SharedMemory> Ams2Source<M> {
    pub fn new(mem: M) -> Self {
        Self::with_tracker(mem, Arc::default())
    }

    /// Sources that share `tracker` share the session clock and what was
    /// learnt about the track.
    pub fn with_tracker(mem: M, tracker: Arc<Mutex<Tracker>>) -> Self {
        Self {
            mem,
            tracker,
            last_seq: None,
            last_frame: None,
            last_data: Vec::new(),
        }
    }

    fn word(&self, at: usize) -> Result<u32, SourceError> {
        let b = self
            .mem
            .read(at, 4)
            .ok_or_else(|| SourceError::Format("shared memory too small".into()))?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn seq(&self) -> Result<u32, SourceError> {
        let at = Layout::get()
            .field("mSequenceNumber")
            .map_or(0, |f| f.offset);
        self.word(at)
    }

    /// A copy taken while the sequence number stayed at `seq` (even): the
    /// game wrote nothing in between.
    fn copy(&self, seq: u32) -> Result<Option<Vec<u8>>, SourceError> {
        let version = self.word(0)?;
        let len = if version < MIN_VERSION {
            4
        } else {
            Layout::get().size_for(version)
        };
        let data = self.mem.read(0, len).ok_or_else(|| {
            SourceError::Format(format!(
                "shared memory smaller than version {version} needs ({len} bytes)"
            ))
        })?;
        Ok((self.seq()? == seq).then_some(data))
    }

    fn frame(&mut self, seq: u32, data: Vec<u8>) -> Result<Poll, SourceError> {
        let s = Snapshot::new(&data);
        if s.version == 0 {
            // The game has not filled it in yet.
            return Ok(Poll::Disconnected);
        }
        if s.version < MIN_VERSION {
            return Err(SourceError::Unsupported(format!(
                "Automobilista 2 shared memory version {} is too old; set \
                 Options > System > Shared Memory to \"Project CARS 2\"",
                s.version
            )));
        }
        if matches!(
            s.u32("mGameState"),
            Some(GAME_EXITED | GAME_FRONT_END | GAME_INGAME_REPLAY | GAME_FRONT_END_REPLAY)
        ) {
            return Ok(Poll::Disconnected);
        }
        self.last_seq = Some(seq);
        self.last_frame = Some(Instant::now());
        let frame = {
            let mut t = self
                .tracker
                .lock()
                .map_err(|_| SourceError::Format("tracker poisoned".into()))?;
            build_frame(&s, &mut t, u64::from(seq / 2), now_ms(), now_s())
        };
        self.last_data = data;
        Ok(Poll::Frame(Box::new(frame)))
    }
}

impl<M: SharedMemory> TelemetrySource for Ams2Source<M> {
    fn game(&self) -> &str {
        "ams2"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            incidents: false,
            ..Capabilities::ALL
        }
    }

    fn extras(&mut self) -> Option<Extras> {
        let s = Snapshot::new(&self.last_data);
        (!self.last_data.is_empty()).then(|| Extras {
            channels: s.channels(),
            info: Some(info(&s)),
            setup: None,
        })
    }

    fn poll(&mut self, timeout: Duration) -> Result<Poll, SourceError> {
        let deadline = Instant::now() + timeout;
        loop {
            let seq = self.seq()?;
            // Until the next frame may be passed on.
            let wait = self
                .last_frame
                .map_or(Duration::ZERO, |t| MIN_INTERVAL.saturating_sub(t.elapsed()));
            if seq % 2 == 0
                && Some(seq) != self.last_seq
                && wait.is_zero()
                && let Some(data) = self.copy(seq)?
            {
                return self.frame(seq, data);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(Poll::Idle);
            }
            std::thread::sleep(wait.max(NAP).min(deadline - now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::testutil::Image;
    use crate::shm::MemoryImage;
    use crate::tracker::GAME_INGAME_PLAYING;
    use vr_core::{Flag, SessionKind, SessionPhase};

    fn race() -> Image {
        let mut img = Image::new();
        img.set_u32("mGameState", 0, GAME_INGAME_PLAYING);
        img.set_u32("mSessionState", 0, 5);
        img.set_u32("mRaceState", 0, 2);
        img.set_i32("mNumParticipants", 0, 2);
        img.set_i32("mViewedParticipantIndex", 0, 1);
        img.set_str("mTrackLocation", 0, "Interlagos");
        img.set_str("mTrackVariation", 0, "GP");
        img.set_f32("mTrackLength", 0, 4300.0);
        img.set_u32("mLapsInEvent", 0, 20);
        img.set_f32("mFuelLevel", 0, 0.5);
        img.set_f32("mFuelCapacity", 0, 100.0);
        img.set_u32("mHighestFlagColour", 0, 2);
        for (car, name, pos) in [(0, "Leader", 1), (1, "Me", 2)] {
            img.part_bool(car, "mIsActive", true);
            img.part_str(car, "mName", name);
            img.part_u32(car, "mRacePosition", pos);
            img.part_u32(car, "mLapsCompleted", 3);
            img.part_u32(car, "mCurrentLap", 4);
            img.part_f32(car, "mCurrentLapDistance", 0, 2150.0);
            img.set_str("mCarClassNames", car, "F-Ultimate");
        }
        img.set_u32("mSequenceNumber", 0, 10);
        img
    }

    #[test]
    fn reads_frames_from_memory_image() {
        let mut img = race();
        let mem = MemoryImage::new(img.bytes.clone());
        let mut src = Ams2Source::new(mem.clone());
        assert_eq!(src.game(), "ams2");
        assert!(src.reconnectable());
        assert!(!src.capabilities().incidents);
        assert!(src.capabilities().gaps);
        assert_eq!(src.extras(), None, "nothing read yet");

        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let Poll::Frame(f) = src.poll(Duration::ZERO).unwrap() else {
            panic!("expected a frame")
        };
        assert_eq!(f.game, "ams2");
        assert_eq!(f.tick, 5);
        assert!(
            (before..before + 5_000).contains(&f.timestamp_ms),
            "wall clock stamp"
        );
        assert_eq!(f.session.kind, SessionKind::Race);
        assert_eq!(f.session.phase, SessionPhase::Racing);
        assert_eq!(f.session.flags, vec![Flag::Blue]);
        assert_eq!(f.session.laps_total, Some(20));
        assert_eq!(f.session.laps_remaining, Some(17));
        assert_eq!(f.player_car_idx, Some(1));
        let me = f.player_car().unwrap();
        assert_eq!((me.driver.as_str(), me.number.as_str()), ("Me", "2"));
        assert_eq!(me.lap_dist_pct, Some(0.5));
        assert_eq!(me.class_position, Some(2));
        assert_eq!(f.player.as_ref().unwrap().fuel_l, Some(50.0));

        let extras = src.extras().unwrap();
        let value = |n: &str| {
            let c = extras.channels.iter().find(|c| c.name == n);
            c.unwrap().value.clone()
        };
        assert_eq!(value("mFuelLevel"), 0.5);
        assert_eq!(value("mParticipantInfo.mName")[1], "Me");
        assert_eq!(extras.info.as_ref().unwrap()["track"], "Interlagos");
        assert_eq!(extras.info.unwrap()["participants"][0]["name"], "Leader");
        assert_eq!(extras.setup, None);

        // The same sequence number: nothing new.
        let t0 = Instant::now();
        assert!(matches!(
            src.poll(Duration::from_millis(30)).unwrap(),
            Poll::Idle
        ));
        assert!(t0.elapsed() >= Duration::from_millis(30));
        // Being written (odd): waits.
        img.set_u32("mSequenceNumber", 0, 11);
        mem.set(img.bytes.clone());
        assert!(matches!(
            src.poll(Duration::from_millis(20)).unwrap(),
            Poll::Idle
        ));
        img.set_u32("mSequenceNumber", 0, 12);
        mem.set(img.bytes.clone());
        assert!(matches!(
            src.poll(Duration::from_millis(50)).unwrap(),
            Poll::Frame(_)
        ));
        // No more than 60 a second: the next one waits its turn.
        img.set_u32("mSequenceNumber", 0, 14);
        mem.set(img.bytes.clone());
        let t0 = Instant::now();
        assert!(matches!(
            src.poll(Duration::from_millis(100)).unwrap(),
            Poll::Frame(_)
        ));
        assert!(t0.elapsed() >= Duration::from_millis(10));

        // Back in the menus.
        img.set_u32("mGameState", 0, GAME_FRONT_END);
        img.set_u32("mSequenceNumber", 0, 16);
        mem.set(img.bytes.clone());
        assert!(matches!(
            src.poll(Duration::from_millis(50)).unwrap(),
            Poll::Disconnected
        ));
        for state in [GAME_EXITED, GAME_INGAME_REPLAY, GAME_FRONT_END_REPLAY] {
            img.set_u32("mGameState", 0, state);
            mem.set(img.bytes.clone());
            let mut src = Ams2Source::new(mem.clone());
            assert!(matches!(
                src.poll(Duration::ZERO).unwrap(),
                Poll::Disconnected
            ));
        }
    }

    #[test]
    fn the_oldest_layout_read() {
        let mut img = race();
        img.set_u32("mVersion", 0, MIN_VERSION);
        let mem = MemoryImage::new(img.bytes[..Layout::get().size_for(MIN_VERSION)].to_vec());
        let Poll::Frame(f) = Ams2Source::new(mem).poll(Duration::ZERO).unwrap() else {
            panic!("expected a frame")
        };
        assert_eq!(f.session.track.unwrap().name, "Interlagos");
        assert_eq!(f.session.time_total_s, None, "not in version 9");
    }

    #[test]
    fn old_or_empty_memory() {
        let mut img = race();
        img.set_u32("mVersion", 0, 0);
        let mem = MemoryImage::new(img.bytes.clone());
        assert!(matches!(
            Ams2Source::new(mem.clone()).poll(Duration::ZERO).unwrap(),
            Poll::Disconnected
        ));
        img.set_u32("mVersion", 0, 5);
        mem.set(img.bytes.clone());
        let err = Ams2Source::new(mem.clone())
            .poll(Duration::ZERO)
            .unwrap_err();
        assert!(err.to_string().contains("Project CARS 2"), "{err}");
        mem.set(vec![0; 8]);
        assert!(matches!(
            Ams2Source::new(mem.clone()).poll(Duration::ZERO),
            Err(SourceError::Format(_))
        ));
        // Too short for what its version says.
        let mut short = race().bytes;
        short.truncate(Layout::get().size_for(13));
        mem.set(short);
        assert!(matches!(
            Ams2Source::new(mem).poll(Duration::ZERO),
            Err(SourceError::Format(_))
        ));
    }

    #[test]
    fn a_reopened_source_keeps_the_clock() {
        let img = race();
        let mem = MemoryImage::new(img.bytes.clone());
        let tracker = Arc::new(Mutex::new(Tracker::default()));
        let mut a = Ams2Source::with_tracker(mem.clone(), tracker.clone());
        let Poll::Frame(f1) = a.poll(Duration::ZERO).unwrap() else {
            panic!()
        };
        std::thread::sleep(Duration::from_millis(20));
        let mut b = Ams2Source::with_tracker(mem, tracker);
        let Poll::Frame(f2) = b.poll(Duration::ZERO).unwrap() else {
            panic!()
        };
        assert_eq!(f1.session.session_num, f2.session.session_num);
        assert!(f2.session.time_s >= f1.session.time_s + 0.015);
    }
}
