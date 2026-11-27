//! The race the driver is in: a generated race fast-forwarded to a moment,
//! then running in real time behind the same HTTP API as `vr serve`.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use vr_core::{Capabilities, Poll, TelemetrySource};
use vr_engine::{
    EventsParams, GapParams, HandlingParams, LapsParams, RaceApi, RaceState, SectorsParams,
    SessionRecord, SharedState, SourceKind, StandingsParams,
};
use vr_replay::{SynthConfig, SynthSource};
use vr_server::{AppState, SessionStore};

use crate::facts::Sample;
use crate::scenario::{PastSpec, WorldSpec};

pub struct World {
    pub url: String,
    state: SharedState,
    stop: Arc<AtomicBool>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    started: Instant,
    trace: Arc<Mutex<Vec<(Instant, Sample)>>>,
    texts: Arc<Mutex<Vec<(Instant, String)>>>,
    past_text: String,
    start_s: f64,
}

/// As the game would report it: the agent must not see that it is a test.
fn new_state() -> RaceState {
    RaceState::new("iracing", SourceKind::Live, Capabilities::ALL)
}

/// Runs the race to `start_s` as fast as possible.
fn forward(spec: &WorldSpec, until: f64, mut ingest: impl FnMut(vr_core::Frame)) -> SynthSource {
    let mut src = SynthSource::new(spec.synth(), 0.0);
    while let Ok(Poll::Frame(f)) = src.poll(Duration::ZERO) {
        let t = f.session.time_s;
        ingest(*f);
        if t >= until {
            break;
        }
    }
    src
}

fn sample(state: &RaceState, t: f64, past: &[SessionRecord]) -> Option<Sample> {
    Some(Sample {
        t,
        past_best_s: past.first().and_then(SessionRecord::best_lap_s),
        brief: state.brief().ok()?,
        standings: state.standings(&StandingsParams::default()).ok()?,
        strategy: state.strategy().ok(),
    })
}

/// The telemetry of a race at a moment, without running it.
pub fn sample_at(spec: &WorldSpec, t: f64) -> Sample {
    let mut state = new_state();
    forward(spec, t, |f| state.ingest(f));
    sample(&state, 0.0, &past_sessions(spec)).expect("an in-process state answers")
}

/// Everything the telemetry says at a moment, without running the race.
pub fn text_at(spec: &WorldSpec, t: f64) -> String {
    let mut state = new_state();
    let mut src = forward(spec, t, |f| state.ingest(f));
    if let Some(e) = src.extras() {
        state.set_extras(e);
    }
    text(&state) + &past_text(&past_sessions(spec))
}

/// The sessions before this one, the latest first.
fn past_sessions(spec: &WorldSpec) -> Vec<SessionRecord> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    spec.past.iter().filter_map(|p| past(p, now)).collect()
}

fn past(p: &PastSpec, now_ms: i64) -> Option<SessionRecord> {
    let mut state = new_state();
    let mut src = SynthSource::new(
        SynthConfig {
            laps: p.laps,
            player_idx: p.player_idx,
            fuel_per_lap_l: p.fuel_per_lap_l,
            player_pit_lap: None,
            rival_pit_lap: None,
            yellow: None,
            incident_at: None,
            ..SynthConfig::default()
        },
        0.0,
    );
    let began = now_ms - i64::from(p.days_ago) * 86_400_000;
    while let Ok(Poll::Frame(mut f)) = src.poll(Duration::ZERO) {
        f.timestamp_ms += began;
        state.ingest(*f);
    }
    state.set_extras(src.extras()?);
    SessionRecord::of(&state)
}

/// What `vr sessions` says about them, for the judges.
fn past_text(past: &[SessionRecord]) -> String {
    let mut out = String::from("\n## past sessions (latest first)\n");
    if past.is_empty() {
        out.push_str("none\n");
    }
    let time = |t: Option<f64>| t.map_or("--".into(), vr_engine::render::fmt_lap);
    for r in past {
        let player = r.player.as_ref();
        let position = player.and_then(|p| p.car.position).unwrap_or(0);
        let total = player.map_or(0, |p| p.cars_total);
        out.push_str(&format!(
            "{} | {} | P{position}/{total} | {} laps | best {} | avg {} | fuel {:.2}L/lap over all laps, {:.2}L/lap typical | setup {}\n",
            r.id,
            vr_engine::fmt_utc(r.started_ms),
            r.laps_completed,
            time(r.best_lap_s()),
            time(player.and_then(|p| p.avg_lap_s)),
            r.fuel_per_lap_l().unwrap_or(f64::NAN),
            r.fuel
                .as_ref()
                .and_then(|f| f.per_lap_l)
                .unwrap_or(f64::NAN),
            r.setup
                .as_ref()
                .map(|s| s
                    .items
                    .iter()
                    .map(|i| format!("{}={}", i.path, i.value))
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
        ));
        for l in &r.laps {
            out.push_str(&format!("  {}: {}\n", l.lap, time(l.time_s)));
        }
    }
    out
}

/// The last laps of every other car, one line each.
fn others(st: &RaceState) -> Option<String> {
    let rows = st.standings(&StandingsParams::default()).ok()?.rows;
    let lines: Vec<String> = rows
        .iter()
        .filter(|r| !r.is_player)
        .map(|r| {
            let laps = st
                .laps(&LapsParams {
                    car: Some(r.car.number.clone()),
                    last: Some(5),
                })
                .map(|r| r.laps)
                .unwrap_or_default();
            let last: Vec<String> = laps
                .iter()
                .map(|l| {
                    let time = l.time_s.map_or("--".into(), vr_engine::render::fmt_lap);
                    format!("L{} {time}{}", l.lap, if l.pit { " pit" } else { "" })
                })
                .collect();
            format!("#{} {}: {}", r.car.number, r.car.driver, last.join(", "))
        })
        .collect();
    Some(lines.join(
        "
",
    ))
}

/// What every `vr` command would print, for the judges.
fn text(st: &RaceState) -> String {
    let part = |title: &str, text: Option<String>| {
        format!("## {title}\n{}\n", text.unwrap_or_else(|| "--".into()))
    };
    let others = part("laps of the other cars", others(st));
    [
        part("brief", st.brief().ok().map(|r| r.to_text())),
        part(
            "standings",
            st.standings(&StandingsParams::default())
                .ok()
                .map(|r| r.to_text()),
        ),
        part(
            "gaps",
            st.gaps(&GapParams::default()).ok().map(|r| r.to_text()),
        ),
        part(
            "sectors",
            st.sectors(&SectorsParams::default())
                .ok()
                .map(|r| r.to_text()),
        ),
        part("forecast", st.forecast().ok().map(|r| r.to_text())),
        part(
            "my laps",
            st.laps(&LapsParams::default()).ok().map(|r| r.to_text()),
        ),
        part(
            "events",
            st.events(&EventsParams::default())
                .ok()
                .map(|r| r.to_text()),
        ),
        others,
        part("strategy", st.strategy().ok().map(|r| r.to_text())),
        part("car", st.car().ok().map(|r| r.to_text())),
        part(
            "handling",
            st.handling(&HandlingParams::default())
                .ok()
                .map(|r| r.to_text()),
        ),
        part("setup", st.setup().ok().map(|r| r.to_text())),
    ]
    .join("\n")
}

impl World {
    /// `port` 0 picks a free one. The past sessions are put into `sessions`,
    /// where `vr sessions` finds them.
    pub async fn start(spec: &WorldSpec, port: u16, sessions: Option<&Path>) -> Result<Self> {
        let state: SharedState = Arc::new(RwLock::new(new_state()));
        let mut src = forward(spec, spec.start_s, |f| state.write().unwrap().ingest(f));
        if let Some(e) = src.extras() {
            state.write().unwrap().set_extras(e);
        }
        let past = past_sessions(spec);
        let store = sessions.map(SessionStore::new);
        if let Some(store) = &store {
            for r in &past {
                store.save(r.clone())?;
            }
        }
        let past_text = past_text(&past);
        let stop = Arc::new(AtomicBool::new(false));
        let trace = Arc::new(Mutex::new(Vec::new()));
        let texts = Arc::new(Mutex::new(Vec::new()));
        let started = Instant::now();

        if spec.stale_after_start {
            state.write().unwrap().set_connected(false);
        }
        let stale = spec.stale_after_start;
        let (thread_state, thread_stop, thread_trace) =
            (state.clone(), stop.clone(), trace.clone());
        let thread_texts = texts.clone();
        let thread_past = past_text.clone();
        let (events, _) = tokio::sync::broadcast::channel(64);
        let thread_events = events.clone();
        let first = spec.start_s;
        std::thread::spawn(move || {
            let mut sampled = Instant::now() - Duration::from_secs(1);
            while !thread_stop.load(Ordering::Relaxed) {
                if !stale && let Ok(Poll::Frame(f)) = src.poll(Duration::ZERO) {
                    let due =
                        started + Duration::from_secs_f64((f.session.time_s - first).max(0.0));
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                    let new = {
                        let mut st = thread_state.write().unwrap();
                        let before = st.event_log().last_seq();
                        st.ingest(*f);
                        st.event_log().since(before, usize::MAX)
                    };
                    for e in new {
                        // Nobody may be listening.
                        let _ = thread_events.send(e);
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(100));
                }
                if sampled.elapsed() >= Duration::from_millis(200) {
                    sampled = Instant::now();
                    if let Some(s) = sample(&thread_state.read().unwrap(), 0.0, &past) {
                        thread_trace.lock().unwrap().push((sampled, s));
                    }
                    let t = text(&thread_state.read().unwrap()) + &thread_past;
                    let mut texts = thread_texts.lock().unwrap();
                    texts.push((sampled, t));
                    // Two minutes are more than a turn lasts.
                    let excess = texts.len().saturating_sub(600);
                    texts.drain(..excess);
                }
            }
        });

        let app = AppState {
            state: state.clone(),
            events,
            sessions: store,
        };
        let (shutdown, rx) = tokio::sync::oneshot::channel::<()>();
        let (bound_tx, bound_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(vr_server::serve(
            app,
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            async {
                let _ = rx.await;
            },
            move |addr| {
                let _ = bound_tx.send(addr);
            },
        ));
        let addr = bound_rx.await.context("the race API did not start")?;
        Ok(Self {
            url: format!("http://{addr}"),
            state,
            stop,
            shutdown: Some(shutdown),
            started,
            trace,
            texts,
            past_text,
            start_s: spec.start_s,
        })
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// When the race is (or was) at a session time.
    pub fn instant_of(&self, session_s: f64) -> Instant {
        self.started + Duration::from_secs_f64((session_s - self.start_s).max(0.0))
    }

    /// Samples between two moments, their time relative to `origin`.
    pub fn between(&self, origin: Instant, from: Instant, to: Instant) -> Vec<Sample> {
        let trace = self.trace.lock().unwrap();
        trace
            .iter()
            .filter(|(at, _)| *at >= from && *at <= to)
            .map(|(at, s)| Sample {
                t: if *at >= origin {
                    (*at - origin).as_secs_f64()
                } else {
                    -(origin - *at).as_secs_f64()
                },
                ..s.clone()
            })
            .collect()
    }

    /// Everything the telemetry said at a moment.
    pub fn text_before(&self, at: Instant) -> String {
        let texts = self.texts.lock().unwrap();
        let found = texts.iter().rev().find(|(t, _)| *t <= at);
        found.map_or_else(String::new, |(_, t)| t.clone())
    }

    /// Everything the telemetry says now.
    pub fn text(&self) -> String {
        text(&self.state.read().unwrap()) + &self.past_text
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_in_real_time_behind_the_api() {
        let spec = WorldSpec {
            start_s: 100.0,
            ..WorldSpec::default()
        };
        let w = World::start(&spec, 0, None).await.unwrap();
        let t0 = Instant::now();
        tokio::time::sleep(Duration::from_millis(900)).await;
        let samples = w.between(t0, t0, Instant::now());
        assert!(samples.len() >= 2, "{}", samples.len());
        let first = samples[0].brief.meta.session_time_s.unwrap();
        let last = samples.last().unwrap().brief.meta.session_time_s.unwrap();
        assert!(
            first >= 100.0 && last <= 101.5 && last > first,
            "{first} {last}"
        );
        assert_eq!(samples[0].brief.meta.game, "iracing");
        assert!(w.url.starts_with("http://127.0.0.1:"));
        assert!(w.text().contains("## my laps"));
        assert!(w.text_before(Instant::now()).contains("## my laps"));
        assert_eq!(w.text_before(t0 - Duration::from_secs(60)), "");
    }

    #[tokio::test]
    async fn past_sessions_are_there_to_look_up() {
        let spec = WorldSpec {
            start_s: 100.0,
            past: vec![PastSpec {
                days_ago: 1,
                laps: 3,
                player_idx: 1,
                fuel_per_lap_l: 2.8,
            }],
            ..WorldSpec::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let w = World::start(&spec, 0, Some(dir.path())).await.unwrap();
        let kept = SessionStore::new(dir.path()).list();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].laps_completed, 3);
        let best = kept[0].best_lap_s().unwrap();
        let text = w.text();
        assert!(text.contains(&vr_engine::render::fmt_lap(best)), "{text}");
        assert!(text.contains("## handling") && text.contains("## car"));
        assert_eq!(sample_at(&spec, 100.0).past_best_s, Some(best));
        assert_eq!(
            w.instant_of(130.0) - w.instant_of(100.0),
            Duration::from_secs(30)
        );
    }

    #[tokio::test]
    async fn a_dropout_ages() {
        let spec = WorldSpec {
            start_s: 100.0,
            stale_after_start: true,
            ..WorldSpec::default()
        };
        let w = World::start(&spec, 0, None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(w.text().contains("DISCONNECTED"), "{}", w.text());
    }
}
