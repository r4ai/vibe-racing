use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vr_core::{Capabilities, Extras, Poll, SourceError, TelemetrySource};

use crate::convert::build_frame;
use crate::layout::{HEADER_LEN, Header, VAR_HEADER_LEN, VarTable};
use crate::session_info::{SessionInfo, decode};
use crate::shm::SharedMemory;

/// Live iRacing telemetry over any [`SharedMemory`].
pub struct IRacingSource<M: SharedMemory> {
    mem: M,
    vars: VarTable,
    var_key: Option<(i32, i32)>,
    session_info: SessionInfo,
    session_info_update: Option<i32>,
    last_tick: Option<i32>,
    last_data: Vec<u8>,
}

impl<M: SharedMemory> IRacingSource<M> {
    pub fn new(mem: M) -> Self {
        Self {
            mem,
            vars: VarTable::default(),
            var_key: None,
            session_info: SessionInfo::default(),
            session_info_update: None,
            last_tick: None,
            last_data: Vec::new(),
        }
    }

    pub fn session_info(&self) -> &SessionInfo {
        &self.session_info
    }

    fn header(&self) -> Result<Header, SourceError> {
        let b = self
            .mem
            .read(0, HEADER_LEN)
            .ok_or_else(|| SourceError::Format("shared memory too small for header".into()))?;
        Header::parse(&b).ok_or_else(|| SourceError::Format("bad header".into()))
    }

    fn refresh_tables(&mut self, h: &Header) -> Result<(), SourceError> {
        let key = (h.num_vars, h.var_header_offset);
        if self.var_key != Some(key) {
            let n = h.num_vars.max(0) as usize;
            let b = self
                .mem
                .read(h.var_header_offset.max(0) as usize, n * VAR_HEADER_LEN)
                .ok_or_else(|| SourceError::Format("variable table out of bounds".into()))?;
            self.vars = VarTable::parse(&b, n);
            self.var_key = Some(key);
        }
        if self.session_info_update != Some(h.session_info_update) {
            if let Some(b) = self.mem.read(
                h.session_info_offset.max(0) as usize,
                h.session_info_len.max(0) as usize,
            ) {
                // Keep the previous info if this update fails to parse.
                if let Ok(si) = SessionInfo::parse(&decode(&b)) {
                    self.session_info = si;
                }
            }
            self.session_info_update = Some(h.session_info_update);
        }
        Ok(())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl<M: SharedMemory> TelemetrySource for IRacingSource<M> {
    fn game(&self) -> &str {
        "iracing"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::ALL
    }

    fn extras(&mut self) -> Option<Extras> {
        (!self.last_data.is_empty()).then(|| Extras {
            channels: self.vars.view(&self.last_data).channels(),
            info: self.session_info.raw.clone(),
            setup: self.session_info.setup.clone(),
        })
    }

    fn poll(&mut self, timeout: Duration) -> Result<Poll, SourceError> {
        let deadline = Instant::now() + timeout;
        loop {
            let h = self.header()?;
            if !h.connected() {
                return Ok(Poll::Disconnected);
            }
            self.refresh_tables(&h)?;
            if let Some((i, buf)) = h.latest_buf()
                && Some(buf.tick_count) != self.last_tick
            {
                let data = self
                    .mem
                    .read(buf.offset.max(0) as usize, h.buf_len.max(0) as usize);
                // Discard if the producer rotated into this buffer while we copied.
                let after = self.header()?.bufs[i].tick_count;
                if let Some(data) = data
                    && after == buf.tick_count
                {
                    self.last_tick = Some(buf.tick_count);
                    let frame = build_frame(
                        &self.vars.view(&data),
                        &self.session_info,
                        buf.tick_count as u64,
                        now_ms(),
                    );
                    self.last_data = data;
                    return Ok(Poll::Frame(Box::new(frame)));
                }
                continue;
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(Poll::Idle);
            }
            self.mem.wait(deadline - now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::testutil::{Image, Var};
    use crate::session_info::SAMPLE;
    use crate::shm::MemoryImage;
    use vr_core::{Flag, SessionKind};

    fn vars() -> Vec<Var> {
        let one = |name, ty| Var { name, ty, count: 1 };
        let arr = |name, ty| Var {
            name,
            ty,
            count: 64,
        };
        vec![
            one("SessionTime", 5),
            one("SessionNum", 2),
            one("SessionState", 2),
            one("SessionFlags", 3),
            one("SessionLapsRemainEx", 2),
            one("SessionTimeRemain", 5),
            one("PlayerCarIdx", 2),
            one("FuelLevel", 4),
            one("FuelUsePerHour", 4),
            one("IsOnTrack", 1),
            arr("CarIdxPosition", 2),
            arr("CarIdxLapCompleted", 2),
            arr("CarIdxLapDistPct", 4),
            arr("CarIdxF2Time", 4),
            arr("CarIdxOnPitRoad", 1),
            arr("CarIdxTrackSurface", 2),
            arr("CarIdxLastLapTime", 4),
        ]
    }

    #[test]
    fn reads_frames_from_memory_image() {
        let mut img = Image::new(&vars(), SAMPLE);
        img.set_f64("SessionTime", 100.0);
        img.set_i32("SessionNum", 0, 2);
        img.set_i32("SessionState", 0, 4);
        img.set_i32("SessionFlags", 0, 0x4 | 0x20);
        img.set_i32("SessionLapsRemainEx", 0, 12);
        img.set_f64("SessionTimeRemain", 604800.0);
        img.set_i32("PlayerCarIdx", 0, 1);
        img.set_f32("FuelLevel", 0, 30.5);
        img.set_f32("FuelUsePerHour", 0, 75.0);
        img.set_bool("IsOnTrack", 0, true);
        for (car, pos, f2) in [(1usize, 2, 1.5f32), (2, 1, 0.0)] {
            img.set_i32("CarIdxPosition", car, pos);
            img.set_i32("CarIdxLapCompleted", car, 7);
            img.set_f32("CarIdxLapDistPct", car, 0.5);
            img.set_f32("CarIdxF2Time", car, f2);
            img.set_i32("CarIdxTrackSurface", car, 3);
            img.set_f32("CarIdxLastLapTime", car, 137.25);
        }
        let mem = MemoryImage(std::sync::Arc::new(std::sync::Mutex::new(
            img.bytes.clone(),
        )));
        let mut src = IRacingSource::new(mem.clone());

        assert_eq!(src.game(), "iracing");
        assert_eq!(src.extras(), None, "nothing was read yet");
        assert_eq!(src.capabilities(), Capabilities::ALL);
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let Poll::Frame(f) = src.poll(Duration::ZERO).unwrap() else {
            panic!("expected frame")
        };
        assert!(
            (before..before + 5_000).contains(&f.timestamp_ms),
            "wall clock stamp"
        );
        assert_eq!(src.session_info().player_car_idx, Some(1));
        assert_eq!(f.session.kind, SessionKind::Race);
        assert_eq!(f.session.flags, vec![Flag::Green, Flag::Blue]);
        assert_eq!(f.session.laps_remaining, Some(12));
        assert_eq!(f.session.laps_total, Some(20));
        assert_eq!(f.session.time_remaining_s, None, "604800 means unlimited");
        assert_eq!(f.cars.len(), 2, "pace car excluded");
        let me = f.player_car().unwrap();
        assert_eq!(me.number, "23");
        assert_eq!(me.position, Some(2));
        assert_eq!(me.gap_to_leader_s, Some(1.5));
        assert_eq!(me.last_lap_s, Some(137.25));
        let p = f.player.unwrap();
        assert_eq!(p.fuel_l, Some(30.5));
        assert!(
            (p.fuel_use_lph.unwrap() - 100.0).abs() < 1e-6,
            "75 kg/h / 0.75 kg/L"
        );

        let extras = src.extras().unwrap();
        assert_eq!(extras.channels.len(), vars().len());
        let value = |n: &str| {
            let c = extras.channels.iter().find(|c| c.name == n);
            c.unwrap().value.clone()
        };
        assert_eq!(value("FuelLevel"), 30.5);
        assert_eq!(value("IsOnTrack"), true);
        assert_eq!(value("SessionFlags"), 0x24);
        assert_eq!(value("CarIdxPosition")[2], 1);
        assert_eq!(value("CarIdxPosition").as_array().unwrap().len(), 64);
        assert!(extras.channels.is_sorted_by_key(|c| &c.name));
        assert_eq!(extras.info.unwrap()["DriverInfo"]["DriverCarIdx"], 1);
        assert_eq!(extras.setup, None);

        // Same tick -> idle after waiting out the timeout; new tick -> frame.
        let t0 = Instant::now();
        assert!(matches!(
            src.poll(Duration::from_millis(30)).unwrap(),
            Poll::Idle
        ));
        assert!(t0.elapsed() >= Duration::from_millis(30));
        img.bump_tick();
        *mem.0.lock().unwrap() = img.bytes.clone();
        assert!(matches!(src.poll(Duration::ZERO).unwrap(), Poll::Frame(_)));

        // Sim leaves the session.
        img.bytes[4..8].copy_from_slice(&0i32.to_le_bytes());
        *mem.0.lock().unwrap() = img.bytes.clone();
        assert!(matches!(
            src.poll(Duration::ZERO).unwrap(),
            Poll::Disconnected
        ));
    }
}
