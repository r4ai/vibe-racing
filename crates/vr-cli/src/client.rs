//! Backends implementing [`RaceApi`]: a running `vr serve` over HTTP, or an
//! in-process state filled from a source on demand.

use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use serde::Serialize;
use serde::de::DeserializeOwned;
use vr_core::{Poll, SourceError};
use vr_engine::*;
use vr_server::{ErrorBody, Health};

use crate::sources::SourceArgs;

pub struct HttpClient {
    base: String,
    agent: ureq::Agent,
}

impl HttpClient {
    pub fn new(base: impl Into<String>, timeout: Option<Duration>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(timeout)
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            agent,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn get<T: DeserializeOwned>(&self, path: &str, params: &impl Serialize) -> ApiResult<T> {
        let mut req = self.agent.get(format!("{}{}", self.base, path));
        if let Ok(serde_json::Value::Object(m)) = serde_json::to_value(params) {
            for (k, v) in m {
                let v = match v {
                    serde_json::Value::Null => continue,
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                req = req.query(k, v);
            }
        }
        let mut resp = req
            .call()
            .map_err(|e| ApiError::Other(format!("vr serve unreachable at {}: {e}", self.base)))?;
        if resp.status().is_success() {
            resp.body_mut()
                .read_json::<T>()
                .map_err(|e| ApiError::Other(format!("bad response: {e}")))
        } else {
            let body: ErrorBody = resp.body_mut().read_json().unwrap_or(ErrorBody {
                error: format!("HTTP {}", resp.status()),
                kind: "other".into(),
            });
            Err(match body.kind.as_str() {
                "not_found" => ApiError::NotFound(body.error),
                "not_connected" => ApiError::NotConnected(body.error),
                _ => ApiError::Other(body.error),
            })
        }
    }

    pub fn health(&self) -> ApiResult<Health> {
        self.get("/v1/health", &())
    }

    /// Stream events as they happen (blocking).
    pub fn stream_events(&self, on_event: impl FnMut(Event) -> bool) -> anyhow::Result<()> {
        self.stream("/v1/events/stream", on_event)
    }

    /// Stream the live values (blocking).
    pub fn stream_live(
        &self,
        hz: f64,
        on_values: impl FnMut(LiveReport) -> bool,
    ) -> anyhow::Result<()> {
        self.stream(&format!("/v1/live/stream?hz={hz}"), on_values)
    }

    fn stream<T: DeserializeOwned>(
        &self,
        path: &str,
        mut on_item: impl FnMut(T) -> bool,
    ) -> anyhow::Result<()> {
        use std::io::BufRead;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(None)
            .build()
            .into();
        let resp = agent.get(format!("{}{path}", self.base)).call()?;
        let reader = std::io::BufReader::new(resp.into_body().into_reader());
        for line in reader.lines() {
            let line = line?;
            if let Some(data) = line.strip_prefix("data:")
                && let Ok(item) = serde_json::from_str::<T>(data.trim())
                && !on_item(item)
            {
                break;
            }
        }
        Ok(())
    }
}

impl RaceApi for HttpClient {
    fn brief(&self) -> ApiResult<Brief> {
        self.get("/v1/brief", &())
    }
    fn standings(&self, p: &StandingsParams) -> ApiResult<StandingsReport> {
        self.get("/v1/standings", p)
    }
    fn gaps(&self, p: &GapParams) -> ApiResult<GapReport> {
        self.get("/v1/gaps", p)
    }
    fn strategy(&self) -> ApiResult<StrategyReport> {
        self.get("/v1/strategy", &())
    }

    fn forecast(&self) -> ApiResult<ForecastReport> {
        self.get("/v1/forecast", &())
    }

    fn live(&self) -> ApiResult<LiveReport> {
        self.get("/v1/live", &())
    }

    fn fuel(&self) -> ApiResult<FuelReport> {
        self.get("/v1/fuel", &())
    }
    fn laps(&self, p: &LapsParams) -> ApiResult<LapsReport> {
        self.get("/v1/laps", p)
    }
    fn sectors(&self, p: &SectorsParams) -> ApiResult<SectorsReport> {
        self.get("/v1/sectors", p)
    }
    fn events(&self, p: &EventsParams) -> ApiResult<EventsReport> {
        self.get("/v1/events", p)
    }
    fn snapshot(&self) -> ApiResult<SnapshotReport> {
        self.get("/v1/snapshot", &())
    }
    fn car(&self) -> ApiResult<CarReport> {
        self.get("/v1/car", &())
    }
    fn channels(&self, p: &ChannelsParams) -> ApiResult<ChannelsReport> {
        self.get("/v1/channels", p)
    }
    fn info(&self, p: &InfoParams) -> ApiResult<InfoReport> {
        self.get("/v1/info", p)
    }
    fn setup(&self) -> ApiResult<SetupReport> {
        self.get("/v1/setup", &())
    }
    fn handling(&self, p: &HandlingParams) -> ApiResult<HandlingReport> {
        self.get("/v1/handling", p)
    }
}

pub enum Backend {
    Http(HttpClient),
    Local(Box<RaceState>),
}

impl Backend {
    pub fn api(&self) -> &dyn RaceApi {
        match self {
            Backend::Http(c) => c,
            Backend::Local(s) => s.as_ref(),
        }
    }
}

/// Where a running server is, in priority order: `--server`, `VR_SERVER`,
/// the discovery file written by `vr serve`, the default port (unless the
/// discovery file was moved, e.g. by tests).
pub fn server_url(flag: Option<&str>) -> Option<String> {
    flag.map(str::to_string)
        .or_else(|| std::env::var("VR_SERVER").ok().filter(|s| !s.is_empty()))
        .or_else(|| vr_server::read_discovery().map(|i| i.url))
        .or_else(|| {
            let moved = std::env::var_os("VR_DISCOVERY").is_some_and(|p| !p.is_empty());
            (!moved).then(|| format!("http://127.0.0.1:{}", vr_server::DEFAULT_PORT))
        })
}

pub fn find_server(flag: Option<&str>) -> Option<HttpClient> {
    let url = server_url(flag)?;
    let c = HttpClient::new(url, Some(Duration::from_secs(3)));
    let probe = HttpClient::new(c.base().to_string(), Some(Duration::from_millis(500)));
    probe.health().ok().map(|_| c)
}

/// Error raised when no telemetry can be obtained at all.
#[derive(Debug)]
pub struct NoData(pub String);

impl std::error::Error for NoData {}

impl std::fmt::Display for NoData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn resolve(src: &SourceArgs, server: Option<&str>) -> anyhow::Result<Backend> {
    if !src.explicit() {
        if let Some(c) = find_server(server) {
            return Ok(Backend::Http(c));
        }
        if let Some(url) = server {
            return Err(anyhow!("vr serve not reachable at {url}"));
        }
    }
    Ok(Backend::Local(Box::new(load_local(src)?)))
}

/// Fill a fresh state from the source: a single sample for live games, or the
/// whole recording/synthetic race (up to `--until`) otherwise.
pub fn load_local(src: &SourceArgs) -> anyhow::Result<RaceState> {
    let mut factory = src.factory(0.0)?;
    let mut st = RaceState::new(
        src.game_id(),
        src.source_kind(false),
        vr_core::Capabilities::ALL,
    );
    let mut source = factory().map_err(|e| match e {
        SourceError::NotRunning(m) | SourceError::Unsupported(m) => anyhow::Error::new(NoData(
            format!("{m}. Start the sim (or `vr serve`), or try `--game synthetic` for a demo."),
        )),
        other => anyhow::Error::new(other),
    })?;
    st.set_source(source.game(), source.capabilities());
    let live = source.reconnectable();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match source
            .poll(Duration::from_millis(200))
            .context("reading telemetry")?
        {
            Poll::Frame(f) => {
                let stop = src.until.is_some_and(|u| f.session.time_s >= u);
                st.ingest(*f);
                if live || stop {
                    if let Some(e) = source.extras() {
                        st.set_extras(e);
                    }
                    break;
                }
            }
            Poll::Idle if live && Instant::now() > deadline => {
                return Err(NoData(
                    "connected to the game but no telemetry arrived within 2s".into(),
                )
                .into());
            }
            Poll::Idle => {}
            Poll::Disconnected => {
                if live {
                    return Err(NoData(format!(
                        "{} telemetry is present but not live (the sim is not in a session)",
                        game_name(source.game())
                    ))
                    .into());
                }
                st.set_connected(false);
                if let Some(e) = source.extras() {
                    st.set_extras(e);
                }
                break;
            }
        }
    }
    Ok(st)
}

/// How a game is called in messages.
fn game_name(id: &str) -> &str {
    match id {
        "iracing" => "iRacing",
        "ams2" => "Automobilista 2",
        other => other,
    }
}

#[cfg(test)]
mod game_name_tests {
    use super::game_name;

    #[test]
    fn games_by_their_names() {
        assert_eq!(game_name("iracing"), "iRacing");
        assert_eq!(game_name("ams2"), "Automobilista 2");
        assert_eq!(game_name("other"), "other");
    }
}
