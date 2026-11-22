//! Telemetry files iRacing writes to disk (`.ibt`).
//!
//! The same header, variable table and session YAML as in shared memory,
//! followed by one record per sample. Only the player's own variables are
//! recorded, so there are no gaps or standings in them.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use vr_core::{Capabilities, Extras, Poll, SourceError, TelemetrySource};

use crate::convert::build_frame;
use crate::layout::{HEADER_LEN, Header, VAR_HEADER_LEN, VarTable};
use crate::session_info::{SessionInfo, decode};

/// Start of the disk header: the wall clock the recording began at.
const START_DATE: u64 = HEADER_LEN as u64;
/// Larger than any real file part; guards against damaged headers.
const MAX_PART: usize = 64 * 1024 * 1024;

pub struct IbtSource<R> {
    file: R,
    vars: VarTable,
    info: SessionInfo,
    record: Vec<u8>,
    data_offset: u64,
    next: u64,
    /// Every `step`-th record is used.
    step: u64,
    start_ms: i64,
    /// Session time of the first record.
    first_time_s: Option<f64>,
    tick_rate: f64,
}

fn part<R: Read + Seek>(f: &mut R, offset: u64, len: usize) -> Result<Vec<u8>, SourceError> {
    if len > MAX_PART {
        return Err(SourceError::Format("damaged telemetry file".into()));
    }
    f.seek(SeekFrom::Start(offset))?;
    let mut b = vec![0; len];
    f.read_exact(&mut b)
        .map_err(|_| SourceError::Format("truncated telemetry file".into()))?;
    Ok(b)
}

impl IbtSource<File> {
    /// Opens a file; `hz` samples per second are used.
    pub fn open(path: &Path, hz: f64) -> Result<Self, SourceError> {
        Self::new(File::open(path)?, hz)
    }
}

impl<R: Read + Seek> IbtSource<R> {
    pub fn new(mut file: R, hz: f64) -> Result<Self, SourceError> {
        let b = part(&mut file, 0, HEADER_LEN)?;
        let h = Header::parse(&b).ok_or_else(|| SourceError::Format("bad header".into()))?;
        let n = h.num_vars.max(0) as usize;
        if n == 0 || h.buf_len <= 0 {
            return Err(SourceError::Format("not an iRacing telemetry file".into()));
        }
        let table = part(
            &mut file,
            h.var_header_offset.max(0) as u64,
            n.saturating_mul(VAR_HEADER_LEN),
        )?;
        let yaml = part(
            &mut file,
            h.session_info_offset.max(0) as u64,
            h.session_info_len.max(0) as usize,
        )?;
        let info = SessionInfo::parse(&decode(&yaml)).map_err(SourceError::Format)?;
        let date = part(&mut file, START_DATE, 8)?;
        let start_s = i64::from_le_bytes(date.try_into().expect("8 bytes"));
        let tick_rate = f64::from(h.tick_rate.max(1));
        Ok(Self {
            vars: VarTable::parse(&table, n),
            info,
            record: vec![0; h.buf_len as usize],
            data_offset: h.bufs[0].offset.max(0) as u64,
            next: 0,
            step: ((tick_rate / hz.max(0.1)).round() as u64).max(1),
            start_ms: start_s.saturating_mul(1000),
            first_time_s: None,
            tick_rate,
            file,
        })
    }
}

impl<R: Read + Seek + Send> TelemetrySource for IbtSource<R> {
    fn game(&self) -> &str {
        "iracing"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            multi_car: false,
            gaps: false,
            ..Capabilities::ALL
        }
    }

    fn poll(&mut self, _timeout: Duration) -> Result<Poll, SourceError> {
        let at = self.data_offset + self.next * self.record.len() as u64;
        self.file.seek(SeekFrom::Start(at))?;
        match self.file.read_exact(&mut self.record) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(Poll::Disconnected);
            }
            Err(e) => return Err(e.into()),
        }
        let view = self.vars.view(&self.record);
        let t = view.f64("SessionTime").unwrap_or(0.0);
        let first = *self.first_time_s.get_or_insert(t);
        // A restart within the file starts the clock again: fall back to the
        // position in the file.
        let since = if t >= first {
            t - first
        } else {
            self.next as f64 / self.tick_rate
        };
        let frame = build_frame(
            &view,
            &self.info,
            self.next,
            self.start_ms + (since * 1000.0) as i64,
        );
        self.next += self.step;
        Ok(Poll::Frame(Box::new(frame)))
    }

    fn extras(&mut self) -> Option<Extras> {
        (self.next > 0).then(|| Extras {
            channels: self.vars.view(&self.record).channels(),
            info: self.info.raw.clone(),
            setup: self.info.setup.clone(),
        })
    }

    fn reconnectable(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::layout::testutil::{Image, Var};
    use crate::session_info::SAMPLE;

    /// A file of two records, 60 Hz.
    fn file() -> Vec<u8> {
        let one = |name, ty| Var { name, ty, count: 1 };
        let vars = [
            one("SessionTime", 5),
            one("SessionNum", 2),
            one("PlayerCarIdx", 2),
            one("Lap", 2),
            one("LapCompleted", 2),
            one("LapDistPct", 4),
            one("LapLastLapTime", 4),
            one("PlayerCarPosition", 2),
            one("OnPitRoad", 1),
            one("dcBrakeBias", 4),
        ];
        let mut img = Image::new(&vars, SAMPLE);
        img.set_f64("SessionTime", 100.0);
        img.set_i32("SessionNum", 0, 2);
        img.set_i32("PlayerCarIdx", 0, 1);
        img.set_i32("Lap", 0, 4);
        img.set_i32("LapCompleted", 0, 3);
        img.set_f32("LapDistPct", 0, 0.25);
        img.set_f32("LapLastLapTime", 0, 137.5);
        img.set_i32("PlayerCarPosition", 0, 5);
        img.set_bool("OnPitRoad", 0, true);
        img.set_f32("dcBrakeBias", 0, 56.5);
        let mut b = img.bytes.clone();
        // Insert the disk header after the first one, moving everything else.
        let disk = 32i32;
        let mut head = b.split_off(HEADER_LEN);
        for at in [20usize, 28, 48 + 4] {
            let v = i32::from_le_bytes(b[at..at + 4].try_into().unwrap()) + disk;
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[8..12].copy_from_slice(&60i32.to_le_bytes());
        b.extend(1_000_000i64.to_le_bytes());
        b.extend([0u8; 24]);
        b.append(&mut head);
        let len = i32::from_le_bytes(b[36..40].try_into().unwrap()) as usize;
        let start = i32::from_le_bytes(b[52..56].try_into().unwrap()) as usize;
        b.truncate(start + len);
        // The second record: one sample later.
        let mut second = b[start..].to_vec();
        second[..8].copy_from_slice(&100.5f64.to_le_bytes());
        b.extend(second);
        b
    }

    #[test]
    fn reads_the_players_own_variables() {
        let mut src = IbtSource::new(Cursor::new(file()), 60.0).unwrap();
        assert_eq!(src.game(), "iracing");
        assert!(!src.reconnectable());
        assert!(!src.capabilities().gaps);
        assert_eq!(src.extras(), None, "nothing was read yet");
        let Poll::Frame(f) = src.poll(Duration::ZERO).unwrap() else {
            panic!("expected frame")
        };
        assert_eq!(f.timestamp_ms, 1_000_000_000);
        let me = f.player_car().unwrap();
        assert_eq!(me.position, Some(5));
        assert_eq!(me.lap, Some(4));
        assert_eq!(me.laps_completed, Some(3));
        assert_eq!(me.lap_dist_pct, Some(0.25));
        assert_eq!(me.last_lap_s, Some(137.5));
        assert!(me.on_pit_road);
        let other = f.car(2).unwrap();
        assert_eq!(other.position, None);
        assert_eq!(other.lap, None);
        let controls = &f.player.as_ref().unwrap().car.controls;
        assert_eq!(controls[0].id, "brake_bias");
        assert_eq!(controls[0].value, 56.5);
        let Poll::Frame(f) = src.poll(Duration::ZERO).unwrap() else {
            panic!("expected frame")
        };
        assert_eq!(f.timestamp_ms, 1_000_000_500);
        assert_eq!(src.extras().unwrap().channels.len(), 10);
        assert!(matches!(
            src.poll(Duration::ZERO).unwrap(),
            Poll::Disconnected
        ));
    }

    #[test]
    fn lower_rates_skip_records() {
        let mut src = IbtSource::new(Cursor::new(file()), 30.0).unwrap();
        assert!(matches!(src.poll(Duration::ZERO).unwrap(), Poll::Frame(_)));
        assert!(matches!(
            src.poll(Duration::ZERO).unwrap(),
            Poll::Disconnected
        ));
    }

    #[test]
    fn damaged_files_are_errors() {
        let b = file();
        for cut in [0, 50, 200, 400] {
            assert!(
                IbtSource::new(Cursor::new(b[..cut].to_vec()), 60.0).is_err(),
                "{cut}"
            );
        }
        let mut huge = b.clone();
        huge[24..28].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(IbtSource::new(Cursor::new(huge), 60.0).is_err());
        let mut none = b;
        none[24..28].copy_from_slice(&0i32.to_le_bytes());
        assert!(IbtSource::new(Cursor::new(none), 60.0).is_err());
    }
}
