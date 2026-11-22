use std::convert::Infallible;
use std::net::SocketAddr;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use vr_engine::*;

use crate::{SessionStore, TraceLine};

#[derive(Clone)]
pub struct AppState {
    pub state: SharedState,
    pub events: broadcast::Sender<Event>,
    /// Past sessions, for the page and `/v1/sessions`.
    pub sessions: Option<SessionStore>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub pid: u32,
    pub game: String,
    pub connected: bool,
    pub frames: u64,
    pub age_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub kind: String,
}

struct AppError(ApiError);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (code, kind) = match &self.0 {
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::NotConnected(_) => (StatusCode::SERVICE_UNAVAILABLE, "not_connected"),
            ApiError::Other(_) => (StatusCode::INTERNAL_SERVER_ERROR, "other"),
        };
        (
            code,
            Json(ErrorBody {
                error: self.0.to_string(),
                kind: kind.into(),
            }),
        )
            .into_response()
    }
}

type R<T> = Result<Json<T>, AppError>;

pub fn router(app: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { Html(include_str!("sessions.html")) }))
        .route("/v1/sessions", get(sessions))
        .route("/v1/sessions/{id}", get(session))
        .route("/v1/sessions/{id}/analysis", get(analysis))
        .route("/v1/sessions/{id}/laps", get(laps))
        .route("/v1/sessions/{id}/laps/{lap}", get(lap))
        .route("/v1/sessions/{id}/laps/{lap}/compare", get(compare))
        .route("/v1/health", get(health))
        .route(
            "/v1/brief",
            get(|State(a): State<AppState>| async move { r(a.state.brief()) }),
        )
        .route(
            "/v1/standings",
            get(
                |State(a): State<AppState>, Query(p): Query<StandingsParams>| async move {
                    r(a.state.standings(&p))
                },
            ),
        )
        .route(
            "/v1/gaps",
            get(
                |State(a): State<AppState>, Query(p): Query<GapParams>| async move {
                    r(a.state.gaps(&p))
                },
            ),
        )
        .route(
            "/v1/fuel",
            get(|State(a): State<AppState>| async move { r(a.state.fuel()) }),
        )
        .route(
            "/v1/strategy",
            get(|State(a): State<AppState>| async move { r(a.state.strategy()) }),
        )
        .route(
            "/v1/laps",
            get(
                |State(a): State<AppState>, Query(p): Query<LapsParams>| async move {
                    r(a.state.laps(&p))
                },
            ),
        )
        .route(
            "/v1/events",
            get(
                |State(a): State<AppState>, Query(p): Query<EventsParams>| async move {
                    r(a.state.events(&p))
                },
            ),
        )
        .route("/v1/events/stream", get(stream))
        .route(
            "/v1/live",
            get(|State(a): State<AppState>| async move { r(a.state.live()) }),
        )
        .route("/v1/live/stream", get(live_stream))
        .route(
            "/v1/snapshot",
            get(|State(a): State<AppState>| async move { r(a.state.snapshot()) }),
        )
        .route(
            "/v1/car",
            get(|State(a): State<AppState>| async move { r(a.state.car()) }),
        )
        .route(
            "/v1/channels",
            get(
                |State(a): State<AppState>, Query(p): Query<ChannelsParams>| async move {
                    r(a.state.channels(&p))
                },
            ),
        )
        .route(
            "/v1/info",
            get(
                |State(a): State<AppState>, Query(p): Query<InfoParams>| async move {
                    r(a.state.info(&p))
                },
            ),
        )
        .route(
            "/v1/setup",
            get(|State(a): State<AppState>| async move { r(a.state.setup()) }),
        )
        .route(
            "/v1/handling",
            get(
                |State(a): State<AppState>, Query(p): Query<HandlingParams>| async move {
                    r(a.state.handling(&p))
                },
            ),
        )
        .with_state(app)
}

fn r<T>(v: ApiResult<T>) -> R<T> {
    v.map(Json).map_err(AppError)
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionLine {
    pub id: String,
    pub started_ms: i64,
    pub duration_s: f64,
    pub kind: vr_core::SessionKind,
    pub track: Option<String>,
    pub car: Option<String>,
    pub player: Option<PlayerSummary>,
    pub laps_completed: usize,
    pub best_lap_s: Option<f64>,
    pub note: Option<String>,
}

/// Reads files: off the threads that answer the radio.
async fn with_store<T: Send + 'static>(
    store: Option<SessionStore>,
    f: impl FnOnce(SessionStore) -> ApiResult<T> + Send + 'static,
) -> R<T> {
    let Some(store) = store else {
        return r(Err(ApiError::NotFound("sessions are not kept".into())));
    };
    let result = tokio::task::spawn_blocking(move || f(store)).await;
    r(result.unwrap_or_else(|e| Err(ApiError::Other(e.to_string()))))
}

async fn sessions(State(a): State<AppState>) -> R<Vec<SessionLine>> {
    with_store(a.sessions, |store| {
        let line = |r: SessionRecord| SessionLine {
            best_lap_s: r.best_lap_s(),
            id: r.id,
            started_ms: r.started_ms,
            duration_s: r.duration_s,
            kind: r.kind,
            track: r.track,
            car: r.car,
            player: r.player,
            laps_completed: r.laps_completed,
            note: r.note,
        };
        Ok(store.list().into_iter().map(line).collect())
    })
    .await
}

async fn session(State(a): State<AppState>, Path(id): Path<String>) -> R<SessionRecord> {
    with_store(a.sessions, move |store| {
        store
            .find(&id)
            .map_err(|e| ApiError::NotFound(e.to_string()))
    })
    .await
}

async fn analysis(State(a): State<AppState>, Path(id): Path<String>) -> R<SetupAnalysis> {
    with_store(a.sessions, move |store| {
        let all = store.list();
        let r = all
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| ApiError::NotFound(format!("no session {id}")))?;
        Ok(analyse(r, &all))
    })
    .await
}

async fn laps(State(a): State<AppState>, Path(id): Path<String>) -> R<Vec<TraceLine>> {
    with_store(a.sessions, move |store| Ok(store.traces(&id))).await
}

fn find_lap(store: &SessionStore, id: &str, lap: i32) -> ApiResult<LapTrace> {
    store
        .trace(id, lap)
        .ok_or_else(|| ApiError::NotFound(format!("lap {lap} of {id} was not recorded")))
}

async fn lap(State(a): State<AppState>, Path((id, lap)): Path<(String, i32)>) -> R<LapTrace> {
    with_store(a.sessions, move |store| find_lap(&store, &id, lap)).await
}

/// The lap a lap is compared with; of the same session by default.
#[derive(Debug, Clone, Deserialize)]
pub struct CompareParams {
    pub session: Option<String>,
    pub lap: i32,
}

async fn compare(
    State(a): State<AppState>,
    Path((id, lap)): Path<(String, i32)>,
    Query(p): Query<CompareParams>,
) -> R<Vec<Corner>> {
    with_store(a.sessions, move |store| {
        let with = p.session.as_deref().unwrap_or(&id);
        Ok(compare_laps(
            &find_lap(&store, &id, lap)?,
            &find_lap(&store, with, p.lap)?,
        ))
    })
    .await
}

async fn health(State(a): State<AppState>) -> Json<Health> {
    let st = a.state.read().expect("state lock");
    let meta = st.meta();
    Json(Health {
        ok: true,
        version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
        game: meta.game,
        connected: meta.connected,
        frames: st.frames_ingested(),
        age_ms: meta.age_ms,
    })
}

async fn stream(
    State(a): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let s = BroadcastStream::new(a.events.subscribe()).filter_map(|e| {
        let e = e.ok()?;
        Some(Ok(SseEvent::default().event("race").json_data(&e).ok()?))
    });
    Sse::new(s).keep_alive(KeepAlive::default())
}

#[derive(Debug, Clone, Deserialize)]
pub struct LiveParams {
    /// Samples per second.
    pub hz: Option<f64>,
}

const LIVE_HZ: f64 = 10.0;
const MAX_LIVE_HZ: f64 = 60.0;

/// The live values, as long as they change.
async fn live_stream(
    State(a): State<AppState>,
    Query(p): Query<LiveParams>,
) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let hz = p.hz.unwrap_or(LIVE_HZ);
    let hz = if hz.is_finite() {
        hz.clamp(0.1, MAX_LIVE_HZ)
    } else {
        LIVE_HZ
    };
    let mut interval = tokio::time::interval(std::time::Duration::from_secs_f64(1.0 / hz));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last = None;
    let s = tokio_stream::wrappers::IntervalStream::new(interval).filter_map(move |_| {
        let frames = a.state.read().ok()?.frames_ingested();
        if last.replace(frames) == Some(frames) {
            return None;
        }
        let live = a.state.live().ok()?;
        Some(Ok(SseEvent::default()
            .event("live")
            .json_data(&live)
            .ok()?))
    });
    Sse::new(s).keep_alive(KeepAlive::default())
}

const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// Serve the API on `addr` until `shutdown` resolves (open connections get
/// [`SHUTDOWN_GRACE`] to finish).
pub async fn serve(
    app: AppState,
    addr: SocketAddr,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    on_bound: impl FnOnce(SocketAddr),
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    on_bound(listener.local_addr()?);
    let (fired_tx, mut fired_rx) = tokio::sync::watch::channel(false);
    let graceful = async move {
        shutdown.await;
        let _ = fired_tx.send(true);
    };
    let server = axum::serve(listener, router(app)).with_graceful_shutdown(graceful);
    tokio::select! {
        r = server => r?,
        // Long-lived SSE clients would otherwise keep graceful shutdown waiting.
        _ = async {
            let _ = fired_rx.wait_for(|f| *f).await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => tracing::debug!("shutdown grace period elapsed; closing remaining connections"),
    }
    Ok(())
}
