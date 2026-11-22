//! The race the driver is in: a generated race fast-forwarded to a moment,
//! then running in real time behind the same HTTP API as `vr serve`.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use vr_core::{Capabilities, Poll, TelemetrySource};
use vr_engine::{
    EventsParams, GapParams, LapsParams, RaceApi, RaceState, SharedState, SourceKind,
    StandingsParams,
};
use vr_replay::SynthSource;
use vr_server::AppState;

use crate::facts::Sample;
use crate::scenario::WorldSpec;

pub struct World {
    pub url: String,
    state: SharedState,
    stop: Arc<AtomicBool>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    started: Instant,
    trace: Arc<Mutex<Vec<(Instant, Sample)>>>,
    texts: Arc<Mutex<Vec<(Instant, String)>>>,
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

fn sample(state: &RaceState, t: f64) -> Option<Sample> {
    Some(Sample {
        t,
        brief: state.brief().ok()?,
        standings: state.standings(&StandingsParams::default()).ok()?,
    })
}

/// The telemetry of a race at a moment, without running it.
pub fn sample_at(spec: &WorldSpec, t: f64) -> Sample {
    let mut state = new_state();
    forward(spec, t, |f| state.ingest(f));
    sample(&state, 0.0).expect("an in-process state answers")
}

/// Everything the telemetry says at a moment, without running the race.
pub fn text_at(spec: &WorldSpec, t: f64) -> String {
    let mut state = new_state();
    forward(spec, t, |f| state.ingest(f));
    text(&state)
}

/// What every `vr` command would print, for the judges.
fn text(st: &RaceState) -> String {
    let part = |title: &str, text: Option<String>| {
        format!("## {title}\n{}\n", text.unwrap_or_else(|| "--".into()))
    };
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
            "my laps",
            st.laps(&LapsParams::default()).ok().map(|r| r.to_text()),
        ),
        part(
            "events",
            st.events(&EventsParams::default())
                .ok()
                .map(|r| r.to_text()),
        ),
    ]
    .join("\n")
}

impl World {
    /// `port` 0 picks a free one.
    pub async fn start(spec: &WorldSpec, port: u16) -> Result<Self> {
        let state: SharedState = Arc::new(RwLock::new(new_state()));
        let mut src = forward(spec, spec.start_s, |f| state.write().unwrap().ingest(f));
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
        let first = spec.start_s;
        std::thread::spawn(move || {
            let mut sampled = Instant::now() - Duration::from_secs(1);
            while !thread_stop.load(Ordering::Relaxed) {
                if !stale && let Ok(Poll::Frame(f)) = src.poll(Duration::ZERO) {
                    let due =
                        started + Duration::from_secs_f64((f.session.time_s - first).max(0.0));
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                    thread_state.write().unwrap().ingest(*f);
                } else {
                    std::thread::sleep(Duration::from_millis(100));
                }
                if sampled.elapsed() >= Duration::from_millis(200) {
                    sampled = Instant::now();
                    if let Some(s) = sample(&thread_state.read().unwrap(), 0.0) {
                        thread_trace.lock().unwrap().push((sampled, s));
                    }
                    let t = text(&thread_state.read().unwrap());
                    let mut texts = thread_texts.lock().unwrap();
                    texts.push((sampled, t));
                    // Two minutes are more than a turn lasts.
                    let excess = texts.len().saturating_sub(600);
                    texts.drain(..excess);
                }
            }
        });

        let (events, _) = tokio::sync::broadcast::channel(16);
        let app = AppState {
            state: state.clone(),
            events,
            sessions: None,
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
        })
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
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
        text(&self.state.read().unwrap())
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
        let w = World::start(&spec, 0).await.unwrap();
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
    async fn a_dropout_ages() {
        let spec = WorldSpec {
            start_s: 100.0,
            stale_after_start: true,
            ..WorldSpec::default()
        };
        let w = World::start(&spec, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(w.text().contains("DISCONNECTED"), "{}", w.text());
    }
}
