use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use vr_core::{Capabilities, Extras, Poll, SourceError, TelemetrySource};

use crate::convert::{
    GAME, STATUS_LIVE, STATUS_OFF, STATUS_PAUSE, STATUS_REPLAY, Snapshot, build_frame,
};
use crate::layout::{CREWCHIEF, GRAPHICS, PHYSICS, View};
use crate::pages::{Page, Pages};
use crate::session::Tracker;

/// The game gives no signal for new data; look this often.
const POLL_EVERY: Duration = Duration::from_millis(4);

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Where the game writes the results of the last session.
pub fn results_path(documents: &Path) -> PathBuf {
    documents
        .join("Assetto Corsa")
        .join("out")
        .join("race_out.json")
}

/// `race_out.json`, once the game has written it after the source opened.
#[derive(Debug)]
pub(crate) struct Results {
    path: PathBuf,
    since: SystemTime,
    read: Option<(SystemTime, Value)>,
}

impl Results {
    pub(crate) fn new(path: PathBuf, since: SystemTime) -> Self {
        Self {
            path,
            since,
            read: None,
        }
    }

    /// Read again only when the file changed.
    pub(crate) fn get(&mut self) -> Option<Value> {
        let modified = std::fs::metadata(&self.path).ok()?.modified().ok()?;
        if modified < self.since {
            return None;
        }
        if self.read.as_ref().is_none_or(|(at, _)| *at != modified) {
            let text = std::fs::read_to_string(&self.path).ok()?;
            let v: Value = serde_json::from_str(&text).ok()?;
            self.read = Some((modified, v));
        }
        self.read.as_ref().map(|(_, v)| v.clone())
    }
}

/// Live Assetto Corsa telemetry over any [`Pages`].
pub struct AcSource<P: Pages> {
    pages: P,
    tracker: Tracker,
    last_packets: Option<(i32, i32)>,
    last: Option<Snapshot>,
    tick: u64,
    crewchief: bool,
    results: Option<Results>,
}

impl<P: Pages> AcSource<P> {
    pub fn new(mut pages: P) -> Self {
        let crewchief = pages.read(Page::CrewChief, 0, 4).is_some();
        Self {
            pages,
            tracker: Tracker::default(),
            last_packets: None,
            last: None,
            tick: 0,
            crewchief,
            results: None,
        }
    }

    /// Also tell the results the game writes here (`race_out.json`) from now.
    pub fn with_results(mut self, path: PathBuf) -> Self {
        // File times are coarser than the clock.
        let since = SystemTime::now() - Duration::from_secs(2);
        self.results = Some(Results::new(path, since));
        self
    }

    fn page(&mut self, page: Page) -> Option<Vec<u8>> {
        self.pages.read(page, 0, page.size())
    }

    fn packet(&mut self, page: Page) -> Option<i32> {
        let b = self.pages.read(page, 0, 4)?;
        Some(i32::from_le_bytes(b.try_into().ok()?))
    }
}

impl<P: Pages> TelemetrySource for AcSource<P> {
    fn game(&self) -> &str {
        GAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            multi_car: self.crewchief,
            gaps: self.crewchief,
            fuel: true,
            incidents: false,
            weather: true,
            flags: true,
        }
    }

    fn extras(&mut self) -> Option<Extras> {
        let snap = self.last.as_ref()?;
        let mut info = serde_json::Map::new();
        info.insert("static".into(), Value::Object(snap.statics().object()));
        if let Some(b) = &snap.crewchief {
            let mut head = View::new(&CREWCHIEF, b).object();
            head.insert("vehicles".into(), json!(snap.vehicles().len()));
            info.insert("crewchief".into(), Value::Object(head));
        }
        let channels = snap.channels();
        if let Some(r) = self.results.as_mut().and_then(Results::get) {
            info.insert("results".into(), r);
        }
        Some(Extras {
            channels,
            info: Some(Value::Object(info)),
            setup: None,
        })
    }

    fn poll(&mut self, timeout: Duration) -> Result<Poll, SourceError> {
        let deadline = Instant::now() + timeout;
        loop {
            let Some(graphics) = self.page(Page::Graphics) else {
                return Ok(Poll::Disconnected);
            };
            let g = View::new(&GRAPHICS, &graphics);
            match g.i32("status") {
                None | Some(STATUS_OFF) => return Ok(Poll::Disconnected),
                // A replay or a pause tells nothing new about the session.
                Some(STATUS_REPLAY | STATUS_PAUSE) => {}
                Some(STATUS_LIVE) => {
                    let physics = self.page(Page::Physics).unwrap_or_default();
                    let id = View::new(&PHYSICS, &physics).i32("packetId");
                    let packets = id.zip(g.i32("packetId"));
                    if packets.is_some() && packets != self.last_packets {
                        // The game writes as we copy; keep only an unchanged step.
                        if self.packet(Page::Physics) == id {
                            self.last_packets = packets;
                            let crewchief = self.page(Page::CrewChief);
                            self.crewchief = crewchief.is_some();
                            let snap = Snapshot {
                                physics,
                                graphics,
                                statics: self.page(Page::Static).unwrap_or_default(),
                                crewchief,
                            };
                            self.tick += 1;
                            let frame = build_frame(&snap, &mut self.tracker, self.tick, now_ms());
                            self.last = Some(snap);
                            return Ok(Poll::Frame(Box::new(frame)));
                        }
                        // Copied while written: read again, within the timeout.
                    }
                }
                Some(_) => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Poll::Idle);
            }
            std::thread::sleep(POLL_EVERY.min(left));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::testutil::{crewchief, snapshot};
    use crate::layout::Writer;
    use crate::pages::MemoryPages;

    fn put(m: &MemoryPages, p: &Writer<'_>, g: &Writer<'_>, s: &Writer<'_>) {
        m.set(Page::Physics, p.bytes.clone());
        m.set(Page::Graphics, g.bytes.clone());
        m.set(Page::Static, s.bytes.clone());
    }

    #[test]
    fn reads_new_steps_only_while_driving() {
        let (mut p, mut g, s) = snapshot();
        let m = MemoryPages::default();
        let mut src = AcSource::new(m.clone());
        assert_eq!(src.game(), "ac");
        assert!(!src.capabilities().multi_car);
        assert!(src.extras().is_none(), "nothing read yet");
        assert!(matches!(
            src.poll(Duration::ZERO).unwrap(),
            Poll::Disconnected
        ));

        put(&m, &p, &g, &s);
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let Poll::Frame(f) = src.poll(Duration::ZERO).unwrap() else {
            panic!("no frame")
        };
        assert!(
            (before..before + 5_000).contains(&f.timestamp_ms),
            "wall clock"
        );
        assert_eq!(f.tick, 1);
        assert_eq!(f.player_car().unwrap().laps_completed, Some(2));

        // The same step again: nothing new.
        let t0 = Instant::now();
        assert!(matches!(
            src.poll(Duration::from_millis(20)).unwrap(),
            Poll::Idle
        ));
        assert!(t0.elapsed() >= Duration::from_millis(20));
        // The next step.
        p.i32("packetId", 101);
        put(&m, &p, &g, &s);
        let Poll::Frame(f) = src.poll(Duration::ZERO).unwrap() else {
            panic!("no frame")
        };
        assert_eq!(f.tick, 2);
        // Graphics only moving on is new too.
        g.i32("packetId", 51);
        put(&m, &p, &g, &s);
        assert!(matches!(src.poll(Duration::ZERO).unwrap(), Poll::Frame(_)));

        // Paused and replaying: no frames, but still there.
        for status in [STATUS_PAUSE, STATUS_REPLAY, 9] {
            g.i32("status", status).i32("packetId", 60 + status);
            put(&m, &p, &g, &s);
            assert!(
                matches!(src.poll(Duration::ZERO).unwrap(), Poll::Idle),
                "{status}"
            );
        }
        // Back to the menus.
        g.i32("status", STATUS_OFF);
        put(&m, &p, &g, &s);
        assert!(matches!(
            src.poll(Duration::ZERO).unwrap(),
            Poll::Disconnected
        ));
        // The game closed.
        m.remove(Page::Graphics);
        assert!(matches!(
            src.poll(Duration::ZERO).unwrap(),
            Poll::Disconnected
        ));
    }

    #[test]
    fn extras_tell_every_value_crewchief_and_the_results() {
        let (p, g, s) = snapshot();
        let m = MemoryPages::default();
        put(&m, &p, &g, &s);
        m.set(
            Page::CrewChief,
            crewchief(&[
                (0, "Ada Lovelace", 2, 0.3, 3, 2, false),
                (4, "Grace Hopper", 2, 0.35, 1, 1, false),
            ]),
        );
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path();
        let path = results_path(docs);
        assert!(path.ends_with("Assetto Corsa/out/race_out.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut src = AcSource::new(m.clone()).with_results(path.clone());
        assert!(src.capabilities().multi_car && src.capabilities().gaps);
        let Poll::Frame(f) = src.poll(Duration::ZERO).unwrap() else {
            panic!("no frame")
        };
        assert_eq!(f.cars.len(), 2);
        let x = src.extras().unwrap();
        assert!(
            x.channels
                .iter()
                .any(|c| c.name == "crewchief.vehicle[4].driverName")
        );
        let info = x.info.unwrap();
        assert_eq!(info["static"]["carModel"], "ks_ferrari_488_gt3");
        assert_eq!(info["static"]["sectorCount"], 3);
        assert_eq!(info["crewchief"]["serverName"], "Test Server");
        assert_eq!(info["crewchief"]["vehicles"], 2);
        assert!(info.get("results").is_none(), "not written yet");
        assert_eq!(x.setup, None);

        std::fs::write(&path, r#"{"track":"monza","sessions":[{"type":3}]}"#).unwrap();
        let info = src.extras().unwrap().info.unwrap();
        assert_eq!(info["results"]["track"], "monza");
        let modified = || std::fs::metadata(&path).unwrap().modified().unwrap();
        let written = modified();
        let mut r = Results::new(path.clone(), written + Duration::from_millis(1));
        assert_eq!(r.get(), None, "older than the source");
        let mut r = Results::new(path.clone(), written);
        assert_eq!(r.get().unwrap()["track"], "monza", "written as it opened");
        // Read again only when the file changes.
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        std::fs::write(&path, r#"{"track":"spa"}"#).unwrap();
        f.set_modified(written).unwrap();
        assert_eq!(r.get().unwrap()["track"], "monza", "unchanged time: kept");
        f.set_modified(written + Duration::from_secs(1)).unwrap();
        assert_eq!(r.get().unwrap()["track"], "spa");
        std::fs::write(&path, "not json").unwrap();
        let mut r = Results::new(path, SystemTime::UNIX_EPOCH);
        assert_eq!(r.get(), None, "not json");

        // CrewChief's app stopped.
        m.remove(Page::CrewChief);
        let mut p = p;
        p.i32("packetId", 101);
        put(&m, &p, &g, &s);
        let Poll::Frame(f) = src.poll(Duration::ZERO).unwrap() else {
            panic!("no frame")
        };
        assert_eq!(f.cars.len(), 1);
        assert!(!src.capabilities().multi_car);
        assert!(
            src.extras()
                .unwrap()
                .info
                .unwrap()
                .get("crewchief")
                .is_none()
        );
    }

    /// Changes the physics page between the copy and the check, `torn`
    /// times.
    struct Tearing {
        inner: MemoryPages,
        reads: usize,
        torn: usize,
    }

    impl Pages for Tearing {
        fn read(&mut self, page: Page, offset: usize, len: usize) -> Option<Vec<u8>> {
            if page == Page::Physics && len == 4 {
                self.reads += 1;
                if self.reads <= self.torn {
                    return Some(999i32.to_le_bytes().to_vec());
                }
            }
            self.inner.read(page, offset, len)
        }
    }

    #[test]
    fn a_step_written_while_copied_is_read_again() {
        let (p, g, s) = snapshot();
        let m = MemoryPages::default();
        put(&m, &p, &g, &s);
        let mut src = AcSource::new(Tearing {
            inner: m.clone(),
            reads: 0,
            torn: 1,
        });
        let Poll::Frame(f) = src.poll(Duration::from_secs(1)).unwrap() else {
            panic!("no frame")
        };
        assert_eq!(f.tick, 1, "one frame, from the second copy");
        assert_eq!(src.pages.reads, 2);

        // Never settling: the timeout still holds.
        let mut src = AcSource::new(Tearing {
            inner: m,
            reads: 0,
            torn: usize::MAX,
        });
        let t0 = Instant::now();
        assert!(matches!(
            src.poll(Duration::from_millis(30)).unwrap(),
            Poll::Idle
        ));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert!(src.pages.reads > 1, "tried again");
    }
}
