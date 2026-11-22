//! Kept sessions, and a collector that survives a game that goes quiet.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use vr_core::*;
use vr_engine::*;
use vr_replay::{SynthConfig, SynthSource, synth_frames};
use vr_server::*;

fn record(id: &str, started_ms: i64) -> SessionRecord {
    let mut st = RaceState::new("synthetic", SourceKind::Replay, Capabilities::ALL);
    let cfg = SynthConfig {
        laps: 3,
        cars: 3,
        ..Default::default()
    };
    for f in synth_frames(cfg, Some(200.0)) {
        st.ingest(f);
    }
    let mut r = SessionRecord::of(&st).unwrap();
    r.id = id.into();
    r.started_ms = started_ms;
    r
}

#[test]
fn sessions_are_listed_found_noted_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions"));
    assert!(store.list().is_empty());
    assert_eq!(store.find("last"), Err(FindError::None("last".into())));
    for (id, at) in [
        ("20260101-000000-race-a", 1_000_000),
        ("20260102-000000-race-b", 3_000_000),
        ("20260102-120000-race-b", 2_000_000),
    ] {
        assert_eq!(store.save(record(id, at)).unwrap(), id);
    }
    // Not a session, or damaged: left out.
    std::fs::write(store.dir().join("x.txt"), "no").unwrap();
    std::fs::write(store.dir().join("bad.json"), "{").unwrap();
    let ids: Vec<String> = store.list().into_iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        [
            "20260102-000000-race-b",
            "20260102-120000-race-b",
            "20260101-000000-race-a"
        ]
    );
    let find = |k: &str| store.find(k).map(|r| r.id);
    assert_eq!(find("last").unwrap(), "20260102-000000-race-b");
    assert_eq!(find("1").unwrap(), "20260102-000000-race-b");
    assert_eq!(find("3").unwrap(), "20260101-000000-race-a");
    assert_eq!(find("4"), Err(FindError::None("4".into())));
    assert_eq!(find("0"), Err(FindError::None("0".into())));
    assert_eq!(find("20260101").unwrap(), "20260101-000000-race-a");
    assert!(matches!(find("20260102"), Err(FindError::Several(..))));
    assert_eq!(find("2027"), Err(FindError::None("2027".into())));

    assert_eq!(
        store.note("3", "  good tyres ").unwrap().note.as_deref(),
        Some("good tyres")
    );
    assert!(
        store
            .find("3")
            .unwrap()
            .line()
            .ends_with("| note: good tyres")
    );
    assert_eq!(store.note("3", " ").unwrap().note, None);

    store.delete("20260101-000000-race-a").unwrap();
    assert_eq!(store.list().len(), 2);
    assert!(store.delete("20260101").is_err());
}

#[test]
fn ids_cannot_leave_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions"));
    store.save(record("../../evil", 1)).unwrap();
    assert!(store.dir().join("evil.json").exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn saving_again_joins_the_same_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path());
    let first = record("20260101-000000-race-a", 1_000_000);
    store.save(first.clone()).unwrap();
    store.note("last", "mine").unwrap();
    // The collector was restarted: another id, nearly the same start.
    let mut again = record("20260101-000005-race-a", 1_005_000);
    again.laps.retain(|l| l.lap > 1);
    assert_eq!(store.save(again).unwrap(), first.id);
    let all = store.list();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].note.as_deref(), Some("mine"));
    assert_eq!(all[0].laps, first.laps);
    // A different one is kept apart.
    let other = record("20260101-010000-race-a", 4_600_000);
    store.save(other).unwrap();
    assert_eq!(store.list().len(), 2);
}

fn state() -> SharedState {
    Arc::new(RwLock::new(RaceState::new(
        "synthetic",
        SourceKind::Live,
        Capabilities::ALL,
    )))
}

/// Plays a race, but goes quiet after a few frames when opened first.
struct Quiet {
    inner: SynthSource,
    frames_left: usize,
}

impl TelemetrySource for Quiet {
    fn game(&self) -> &str {
        "quiet"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::ALL
    }
    fn poll(&mut self, _timeout: Duration) -> Result<Poll, SourceError> {
        if self.frames_left == 0 {
            std::thread::sleep(Duration::from_millis(5));
            return Ok(Poll::Idle);
        }
        self.frames_left -= 1;
        self.inner.poll(Duration::ZERO)
    }
    fn extras(&mut self) -> Option<Extras> {
        self.inner.extras()
    }
}

#[test]
fn a_live_game_that_goes_quiet_is_opened_again() {
    let opened = Arc::new(AtomicUsize::new(0));
    let count = opened.clone();
    let factory: SourceFactory = Box::new(move || {
        let first = count.fetch_add(1, Ordering::SeqCst) == 0;
        Ok(Box::new(Quiet {
            inner: SynthSource::new(SynthConfig::default(), 0.0),
            frames_left: if first { 5 } else { 100 },
        }) as Box<dyn TelemetrySource>)
    });
    let st = state();
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, _rx) = broadcast::channel(64);
    let cfg = CollectorConfig {
        stall: Duration::from_millis(60),
        retry_delay: Duration::from_millis(1),
        ..Default::default()
    };
    let t = Instant::now();
    let h = spawn_collector(factory, st.clone(), tx, cfg, stop.clone()).unwrap();
    while st.read().unwrap().frames_ingested() < 105 {
        assert!(t.elapsed() < Duration::from_secs(10), "never reopened");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(t.elapsed() >= Duration::from_millis(60));
    // Quiet again: the driver is told that the data is old.
    while st.read().unwrap().meta().connected {
        assert!(t.elapsed() < Duration::from_secs(10), "still connected");
        std::thread::sleep(Duration::from_millis(1));
    }
    stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
}

#[test]
fn the_collector_keeps_sessions_and_extras() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path());
    let st = state();
    let (tx, _rx) = broadcast::channel(10_000);
    let cfg = CollectorConfig {
        sessions: Some(store.clone()),
        ..Default::default()
    };
    let factory: SourceFactory = Box::new(|| {
        Ok(Box::new(SynthSource::new(
            SynthConfig {
                laps: 3,
                ..Default::default()
            },
            0.0,
        )) as Box<dyn TelemetrySource>)
    });
    spawn_collector(
        factory,
        st.clone(),
        tx,
        cfg,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
    .join()
    .unwrap();
    let all = store.list();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].laps_completed, 3);
    assert_eq!(all[0].others.len(), 11);
    assert_eq!(
        all[0].setup.as_ref().unwrap().name.as_deref(),
        Some("baseline")
    );
    assert_eq!(st.channels(&ChannelsParams::default()).unwrap().total, 1);
}

#[test]
fn finite_sources_are_read_into_sessions() {
    let mut src = SynthSource::new(
        SynthConfig {
            laps: 2,
            ..Default::default()
        },
        0.0,
    );
    let mut st = RaceState::new("synthetic", SourceKind::Replay, Capabilities::ALL);
    let got = read_sessions(&mut src, &mut st).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].laps_completed, 2);
    assert!(got[0].setup.is_some());
}
