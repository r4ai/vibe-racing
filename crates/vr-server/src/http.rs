use std::convert::Infallible;
use std::net::SocketAddr;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use vr_engine::*;

#[derive(Clone)]
pub struct AppState {
    pub state: SharedState,
    pub events: broadcast::Sender<Event>,
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
            "/v1/snapshot",
            get(|State(a): State<AppState>| async move { r(a.state.snapshot()) }),
        )
        .with_state(app)
}

fn r<T>(v: ApiResult<T>) -> R<T> {
    v.map(Json).map_err(AppError)
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
