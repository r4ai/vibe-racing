use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use vr_core::{Poll, SourceError, SourceFactory};
use vr_engine::{Event, LapTrace, SessionRecord, SharedState};
use vr_replay::Recorder;

use crate::SessionStore;

pub struct CollectorConfig {
    pub record: Option<(PathBuf, f64)>,
    pub poll_timeout: Duration,
    pub retry_delay: Duration,
    /// Where sessions are kept; `None` keeps nothing.
    pub sessions: Option<SessionStore>,
    /// How often the session in progress is saved.
    pub save_every: Duration,
    /// A live game that sends nothing for this long is opened again.
    pub stall: Duration,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            record: None,
            poll_timeout: Duration::from_millis(250),
            retry_delay: Duration::from_secs(2),
            sessions: None,
            save_every: Duration::from_secs(30),
            stall: Duration::from_secs(3),
        }
    }
}

/// Run `factory`'s source in a background thread, feeding `state` and
/// publishing new events on `events`. Reconnects to live games; stops at the
/// end of finite replays. Set `stop` to end the loop.
pub fn spawn_collector(
    mut factory: SourceFactory,
    state: SharedState,
    events: broadcast::Sender<Event>,
    cfg: CollectorConfig,
    stop: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    let mut recorder = match &cfg.record {
        Some((path, hz)) => Some(Recorder::create(path, *hz)?),
        None => None,
    };
    std::thread::Builder::new()
        .name("vr-collector".into())
        .spawn(move || {
            let mut warned = false;
            let mut place = None;
            while !stop.load(Ordering::Relaxed) {
                let mut src = match factory() {
                    Ok(s) => s,
                    Err(e) => {
                        if !warned {
                            tracing::warn!("waiting for game: {e}");
                            warned = true;
                        }
                        set_connected(&state, false);
                        if matches!(
                            e,
                            SourceError::Unsupported(_)
                                | SourceError::Io(_)
                                | SourceError::Format(_)
                        ) {
                            tracing::error!("source cannot be opened: {e}");
                            return;
                        }
                        sleep_unless(&stop, cfg.retry_delay);
                        continue;
                    }
                };
                warned = false;
                tracing::info!("connected to {}", src.game());
                if let Ok(mut st) = state.write() {
                    st.set_source(src.game(), src.capabilities());
                }
                // Everything driven, for questions asked later (`lake`).
                let mut lake =
                    cfg.sessions
                        .as_ref()
                        .and_then(|_| match crate::lake::Writer::new() {
                            Ok(w) => Some(w),
                            Err(e) => {
                                tracing::warn!("telemetry is not kept: {e:#}");
                                None
                            }
                        });
                let mut session_id: Option<String> = None;
                let mut last_frame = Instant::now();
                let mut last_extras: Option<Instant> = None;
                let mut last_save = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    match src.poll(cfg.poll_timeout) {
                        Ok(Poll::Frame(f)) => {
                            last_frame = Instant::now();
                            if let Some(r) = recorder.as_mut()
                                && let Err(e) = r.record(&f)
                            {
                                tracing::error!("recording failed, disabling: {e}");
                                recorder = None;
                            }
                            // Copied outside the lock: queries must not wait.
                            let extras = last_extras
                                .is_none_or(|t| t.elapsed() >= EXTRAS_EVERY)
                                .then(|| src.extras())
                                .flatten();
                            if let Some(l) = lake.as_mut() {
                                l.push(&f);
                            }
                            let (new_events, finished, laps, at) = match state.write() {
                                Ok(mut st) => {
                                    let before = st.event_log().last_seq();
                                    st.ingest(*f);
                                    // Ended sessions keep their own extras.
                                    let finished = st.take_finished();
                                    let at = st.track_and_car();
                                    if let Some(e) = extras {
                                        last_extras = Some(Instant::now());
                                        st.set_extras(e);
                                    }
                                    let laps = st.take_traces();
                                    let events = st.event_log().since(before, usize::MAX);
                                    (events, finished, laps, at)
                                }
                                Err(_) => return,
                            };
                            for e in new_events {
                                tracing::info!("event: {}", e.text);
                                let _ = events.send(e);
                            }
                            if at.is_some() && at != place {
                                place = at;
                                if let (Some(p), Some(s)) = (place.clone(), &cfg.sessions) {
                                    recall_pit_loss(s.clone(), p, state.clone());
                                }
                            }
                            if let Some(store) = &cfg.sessions {
                                for r in finished {
                                    let id = keep(store, r);
                                    // What was collected belongs to the session that ended.
                                    if let (Some(l), Some(id)) = (lake.as_mut(), id.as_deref()) {
                                        flush(l, store, id);
                                    }
                                    session_id = None;
                                }
                                if last_save.elapsed() >= cfg.save_every || !laps.is_empty() {
                                    last_save = Instant::now();
                                    let id = keep_current(store, &state);
                                    if id.is_some() {
                                        session_id = id.clone();
                                    }
                                    let track = place.as_ref().map(|a| a.0.clone());
                                    for lap in laps {
                                        keep_lap(store, id.as_deref(), &lap);
                                        if let (Some(l), Some(id)) = (lake.as_mut(), id.as_deref())
                                            && let Err(e) =
                                                l.lap(store.dir(), id, &lap, track.as_deref())
                                        {
                                            tracing::warn!("corners not kept: {e:#}");
                                        }
                                    }
                                }
                                if let (Some(l), Some(id)) = (lake.as_mut(), session_id.as_deref())
                                    && l.due()
                                {
                                    flush(l, store, id);
                                }
                            }
                        }
                        // E.g. the game replaced its shared memory.
                        Ok(Poll::Idle)
                            if src.reconnectable() && last_frame.elapsed() >= cfg.stall =>
                        {
                            tracing::warn!("no telemetry for a while; opening it again");
                            break;
                        }
                        Ok(Poll::Idle) => {}
                        Ok(Poll::Disconnected) => break,
                        Err(e) => {
                            tracing::warn!("telemetry error: {e}");
                            break;
                        }
                    }
                }
                set_connected(&state, false);
                if let Some(store) = &cfg.sessions {
                    let id = keep_current(store, &state).or(session_id.take());
                    if let (Some(l), Some(id)) = (lake.as_mut(), id.as_deref()) {
                        flush(l, store, id);
                    }
                }
                if let Some(r) = recorder.as_mut() {
                    let _ = r.flush();
                }
                if !src.reconnectable() {
                    tracing::info!("{} stream ended; still serving the final state", src.game());
                    return;
                }
                tracing::info!("disconnected from {}; retrying", src.game());
                sleep_unless(&stop, cfg.retry_delay);
            }
        })
}

/// How often the values that are not in every frame are refreshed.
const EXTRAS_EVERY: Duration = Duration::from_secs(1);

/// Saves a session and returns its id.
fn keep(store: &SessionStore, r: SessionRecord) -> Option<String> {
    match store.save(r) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("session not saved: {e:#}");
            None
        }
    }
}

/// What stops cost on the track in earlier sessions. Reads them all: not on
/// the thread the telemetry arrives on.
fn recall_pit_loss(store: SessionStore, place: (String, String), state: SharedState) {
    std::thread::spawn(move || {
        let loss = store.pit_loss(&place.0, &place.1);
        if let Ok(mut st) = state.write() {
            st.set_prior_pit_loss(loss);
        }
    });
}

fn flush(lake: &mut crate::lake::Writer, store: &SessionStore, id: &str) {
    if let Err(e) = lake.flush(store.dir(), id) {
        tracing::warn!("telemetry not kept: {e:#}");
    }
}

fn keep_lap(store: &SessionStore, id: Option<&str>, lap: &LapTrace) {
    if let Some(id) = id
        && let Err(e) = store.save_trace(id, lap)
    {
        tracing::warn!("lap not saved: {e:#}");
    }
}

fn keep_current(store: &SessionStore, state: &SharedState) -> Option<String> {
    let record = state.read().ok().and_then(|st| SessionRecord::of(&st));
    keep(store, record?)
}

fn set_connected(state: &SharedState, c: bool) {
    if let Ok(mut st) = state.write() {
        st.set_connected(c);
    }
}

fn sleep_unless(stop: &AtomicBool, d: Duration) {
    let step = Duration::from_millis(100);
    let mut left = d;
    while !stop.load(Ordering::Relaxed) && !left.is_zero() {
        let s = left.min(step);
        std::thread::sleep(s);
        left -= s;
    }
}
