use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::sync::broadcast;
use vr_core::{Poll, SourceError, SourceFactory};
use vr_engine::{Event, SharedState};
use vr_replay::Recorder;

pub struct CollectorConfig {
    pub record: Option<(PathBuf, f64)>,
    pub poll_timeout: Duration,
    pub retry_delay: Duration,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            record: None,
            poll_timeout: Duration::from_millis(250),
            retry_delay: Duration::from_secs(2),
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
                while !stop.load(Ordering::Relaxed) {
                    match src.poll(cfg.poll_timeout) {
                        Ok(Poll::Frame(f)) => {
                            if let Some(r) = recorder.as_mut()
                                && let Err(e) = r.record(&f)
                            {
                                tracing::error!("recording failed, disabling: {e}");
                                recorder = None;
                            }
                            let new_events = match state.write() {
                                Ok(mut st) => {
                                    let before = st.event_log().last_seq();
                                    st.ingest(*f);
                                    st.event_log().since(before, usize::MAX)
                                }
                                Err(_) => return,
                            };
                            for e in new_events {
                                tracing::info!("event: {}", e.text);
                                let _ = events.send(e);
                            }
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
