use std::io::BufRead;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use vr_core::{Capabilities, SourceError, TelemetrySource};
use vr_engine::*;
use vr_replay::{SynthConfig, SynthSource};
use vr_server::*;

fn synth_factory(laps: i32) -> vr_core::SourceFactory {
    Box::new(move || {
        Ok(Box::new(SynthSource::new(
            SynthConfig {
                laps,
                ..Default::default()
            },
            0.0,
        )) as Box<dyn TelemetrySource>)
    })
}

fn new_state() -> SharedState {
    Arc::new(RwLock::new(RaceState::new(
        "synthetic",
        SourceKind::Synthetic,
        Capabilities::default(),
    )))
}

/// Run the collector over a short synthetic race to completion.
fn collected(record: Option<std::path::PathBuf>) -> (SharedState, Vec<Event>) {
    let state = new_state();
    let (tx, mut rx) = broadcast::channel(10_000);
    let cfg = CollectorConfig {
        record: record.map(|p| (p, 0.0)),
        ..Default::default()
    };
    let h = spawn_collector(
        synth_factory(3),
        state.clone(),
        tx,
        cfg,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    h.join().unwrap();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    (state, events)
}

struct Server {
    url: String,
    tx: broadcast::Sender<Event>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn start(state: SharedState) -> Self {
        let (tx, _) = broadcast::channel(64);
        let app = AppState {
            state,
            events: tx.clone(),
        };
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (addr_tx, addr_rx) = std::sync::mpsc::channel::<SocketAddr>();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(serve(
                app,
                "127.0.0.1:0".parse().unwrap(),
                async {
                    let _ = stop_rx.await;
                },
                move |a| addr_tx.send(a).unwrap(),
            ))
            .unwrap();
        });
        let addr = addr_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        Self {
            url: format!("http://{addr}"),
            tx,
            stop: Some(stop_tx),
            thread: Some(thread),
        }
    }

    fn get(&self, path: &str) -> (u16, serde_json::Value) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut r = agent.get(format!("{}{}", self.url, path)).call().unwrap();
        let code = r.status().as_u16();
        (code, r.body_mut().read_json().unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[test]
fn collector_ingests_whole_race_and_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rec.ndjson");
    let (state, events) = collected(Some(path.clone()));
    let st = state.read().unwrap();
    assert!(!st.meta().connected, "finite stream ends disconnected");
    assert_eq!(st.meta().game, "synthetic");
    assert_eq!(
        st.meta().capabilities,
        Capabilities::ALL,
        "taken from the opened source"
    );
    assert!(st.frames_ingested() > 2500);
    // Every event produced was broadcast, in order.
    assert_eq!(events.len() as u64, st.event_log().last_seq());
    assert!(events.windows(2).all(|w| w[0].seq + 1 == w[1].seq));
    let lines = std::fs::read_to_string(&path).unwrap().lines().count() as u64;
    assert_eq!(lines, st.frames_ingested());
}

#[test]
fn collector_retries_until_game_appears_then_stops() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let a = attempts.clone();
    let factory: vr_core::SourceFactory = Box::new(move || {
        if a.fetch_add(1, Ordering::SeqCst) < 2 {
            Err(SourceError::NotRunning("not yet".into()))
        } else {
            Ok(Box::new(SynthSource::new(
                SynthConfig {
                    laps: 1,
                    cars: 2,
                    ..Default::default()
                },
                0.0,
            )) as Box<dyn TelemetrySource>)
        }
    });
    let state = new_state();
    let (tx, _) = broadcast::channel(16);
    let cfg = CollectorConfig {
        retry_delay: Duration::from_millis(250),
        ..Default::default()
    };
    let t0 = Instant::now();
    spawn_collector(
        factory,
        state.clone(),
        tx,
        cfg,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
    .join()
    .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert!(
        t0.elapsed() >= Duration::from_millis(500),
        "two retry delays: {:?}",
        t0.elapsed()
    );
    assert!(state.read().unwrap().frames_ingested() > 0);
}

#[test]
fn collector_gives_up_on_unsupported_and_honours_stop() {
    let state = new_state();
    let (tx, _) = broadcast::channel(16);
    let f: vr_core::SourceFactory = Box::new(|| Err(SourceError::Unsupported("nope".into())));
    let t0 = Instant::now();
    spawn_collector(
        f,
        state.clone(),
        tx.clone(),
        CollectorConfig::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
    .join()
    .unwrap();
    assert!(t0.elapsed() < Duration::from_secs(1));

    let stop = Arc::new(AtomicBool::new(false));
    let f: vr_core::SourceFactory = Box::new(|| Err(SourceError::NotRunning("waiting".into())));
    let h = spawn_collector(f, state, tx, CollectorConfig::default(), stop.clone()).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    stop.store(true, Ordering::Relaxed);
    let t0 = Instant::now();
    h.join().unwrap();
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "stop interrupts the retry sleep"
    );
}

#[test]
fn http_endpoints_serve_race_api() {
    let (state, _) = collected(None);
    let s = Server::start(state.clone());

    let (code, h) = s.get("/v1/health");
    assert_eq!(code, 200);
    assert_eq!(h["ok"], true);
    assert_eq!(h["pid"], std::process::id());
    assert_eq!(h["game"], "synthetic");
    assert!(h["frames"].as_u64().unwrap() > 0);

    let (code, b) = s.get("/v1/brief");
    assert_eq!(code, 200);
    let b: Brief = serde_json::from_value(b).unwrap();
    assert_eq!(
        b,
        state
            .brief()
            .unwrap_or_else(|_| unreachable!())
            .clone_with_age(b.meta.age_ms)
    );

    let (_, v) = s.get("/v1/standings?top=2");
    assert_eq!(v["rows"].as_array().unwrap().len(), 2);
    let (_, v) = s.get("/v1/standings?around=1&class_only=false");
    assert_eq!(v["rows"].as_array().unwrap().len(), 3);
    assert_eq!(v["class_only"], false);

    let (_, v) = s.get("/v1/laps?last=2");
    assert_eq!(v["laps"].as_array().unwrap().len(), 2);
    let (_, v) = s.get("/v1/fuel");
    assert!(v["fuel"]["fuel_l"].as_f64().is_some());
    let (_, v) = s.get("/v1/events?since=3&limit=2");
    let evs = v["events"].as_array().unwrap();
    assert_eq!(evs.len(), 2);
    assert!(evs.iter().all(|e| e["seq"].as_u64().unwrap() > 3));
    let (_, v) = s.get("/v1/snapshot");
    assert_eq!(v["frame"]["game"], "synthetic");

    let leader = state
        .standings(&StandingsParams {
            top: Some(1),
            ..Default::default()
        })
        .unwrap()
        .rows[0]
        .car
        .number
        .clone();
    let (code, v) = s.get(&format!("/v1/gaps?car={leader}&window_s=5"));
    assert_eq!(code, 200, "{v}");
    assert!(v["target"].is_object());

    let (code, v) = s.get("/v1/gaps?car=999");
    assert_eq!(code, 404);
    assert_eq!(v["kind"], "not_found");
    let (code, v) = s.get("/v1/laps?car=999");
    assert_eq!(code, 404);
    assert!(v["error"].as_str().unwrap().contains("#999"));
}

#[test]
fn sse_streams_published_events() {
    let s = Server::start(new_state());
    let url = format!("{}/v1/events/stream", s.url);
    let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let resp = ureq::get(&url).call().unwrap();
        for line in std::io::BufReader::new(resp.into_body().into_reader()).lines() {
            let Ok(line) = line else { return };
            if line_tx.send(line).is_err() {
                return;
            }
        }
    });
    let ev = Event {
        seq: 42,
        session_time_s: 1.0,
        timestamp_ms: 0,
        kind: EventKind::Incident { added: 1, total: 1 },
        text: "incident".into(),
    };
    // Keep publishing until the subscriber is attached and receives one.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = None;
    while Instant::now() < deadline && got.is_none() {
        let _ = s.tx.send(ev.clone());
        while let Ok(line) = line_rx.recv_timeout(Duration::from_millis(100)) {
            if let Some(d) = line.strip_prefix("data:") {
                got = Some(serde_json::from_str::<Event>(d.trim()).unwrap());
                break;
            }
            assert!(
                line.is_empty() || line.starts_with("event:") || line.starts_with(':'),
                "{line}"
            );
        }
    }
    assert_eq!(got, Some(ev));
}

#[test]
fn discovery_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("server.json");
    // SAFETY: this is the only test in this binary touching VR_DISCOVERY.
    unsafe { std::env::set_var("VR_DISCOVERY", &path) };
    assert_eq!(discovery_path(), path);
    assert!(read_discovery().is_none());
    let info = ServerInfo {
        url: "http://127.0.0.1:1".into(),
        pid: 7,
        game: "g".into(),
        started_ms: 1,
    };
    assert_eq!(write_discovery(&info).unwrap(), path);
    assert_eq!(read_discovery(), Some(info));
    clear_discovery(8);
    assert!(path.exists(), "another pid's file is left alone");
    clear_discovery(7);
    assert!(!path.exists());
    unsafe { std::env::remove_var("VR_DISCOVERY") };
    assert!(
        discovery_path().ends_with("vibe-racing/server.json")
            || discovery_path().ends_with("vibe-racing\\server.json")
    );
}

trait WithAge {
    fn clone_with_age(&self, age: Option<i64>) -> Self;
}

impl WithAge for Brief {
    fn clone_with_age(&self, age: Option<i64>) -> Self {
        let mut b = self.clone();
        b.meta.age_ms = age;
        b
    }
}
