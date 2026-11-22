use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::RaceState;
use crate::handling::{Behaviour, HandlingLap};
use crate::report::*;
use crate::state::{car_ref, is_multi_class};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StandingsParams {
    /// Only the first N rows.
    pub top: Option<usize>,
    /// Only N rows either side of the player (overrides `top`).
    pub around: Option<usize>,
    /// Restrict to the player's class (defaults to true in multi-class sessions).
    pub class_only: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GapParams {
    /// Car number to compare against, e.g. `"23"`.
    pub car: Option<String>,
    /// Window for nearby cars on track, seconds.
    pub window_s: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LapsParams {
    /// Car number (default: player).
    pub car: Option<String>,
    /// Only the last N laps (default 10).
    pub last: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventsParams {
    /// Only events with `seq > since`.
    pub since: Option<u64>,
    /// At most this many (newest kept, default 50).
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelsParams {
    /// Only channels whose name, unit or description contains all of these
    /// words (case is ignored).
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InfoParams {
    /// E.g. `"WeekendInfo/TrackNumTurns"` or `"DriverInfo/Drivers/0"`.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HandlingParams {
    /// Mean over the last N laps (default 3).
    pub last: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("not connected to the game: {0}")]
    NotConnected(String),
    #[error("{0}")]
    Other(String),
}

pub type ApiResult<T> = Result<T, ApiError>;

/// The query surface shared by every frontend. Implemented in-process for
/// [`RaceState`] / [`SharedState`] and over HTTP by the CLI's client.
pub trait RaceApi {
    fn brief(&self) -> ApiResult<Brief>;
    fn standings(&self, p: &StandingsParams) -> ApiResult<StandingsReport>;
    fn gaps(&self, p: &GapParams) -> ApiResult<GapReport>;
    fn fuel(&self) -> ApiResult<FuelReport>;
    fn laps(&self, p: &LapsParams) -> ApiResult<LapsReport>;
    fn events(&self, p: &EventsParams) -> ApiResult<EventsReport>;
    fn snapshot(&self) -> ApiResult<SnapshotReport>;
    fn car(&self) -> ApiResult<CarReport>;
    fn channels(&self, p: &ChannelsParams) -> ApiResult<ChannelsReport>;
    fn info(&self, p: &InfoParams) -> ApiResult<InfoReport>;
    fn setup(&self) -> ApiResult<SetupReport>;
    fn handling(&self, p: &HandlingParams) -> ApiResult<HandlingReport>;
}

/// Thread-safe handle used by long-running collectors.
pub type SharedState = Arc<RwLock<RaceState>>;

impl RaceApi for RaceState {
    fn brief(&self) -> ApiResult<Brief> {
        let meta = self.meta();
        let Some(f) = self.frame() else {
            return Ok(Brief {
                meta,
                session: None,
                player: None,
                ahead: None,
                behind: None,
                nearby: Vec::new(),
                fuel: None,
                recent_events: self.event_log().last(5),
                weather: None,
                rain_trend: None,
                car: None,
            });
        };
        let mut nearby = self.nearby(f, None);
        nearby.truncate(4);
        Ok(Brief {
            meta,
            session: Some(self.session_summary(f)),
            player: self.player_summary(f),
            ahead: self.adjacent(f, -1),
            behind: self.adjacent(f, 1),
            nearby,
            fuel: self.fuel_info(f),
            recent_events: self.event_log().last(5),
            weather: f.weather.clone(),
            rain_trend: self.rain_trend(),
            car: f.player.as_ref().map(|p| p.car.clone()),
        })
    }

    fn standings(&self, p: &StandingsParams) -> ApiResult<StandingsReport> {
        let meta = self.meta();
        let Some(f) = self.frame() else {
            return Ok(StandingsReport {
                meta,
                class_only: false,
                rows: Vec::new(),
            });
        };
        let player = f.player_car();
        let class_only = p.class_only.unwrap_or_else(|| is_multi_class(f)) && player.is_some();
        let pos_of = |c: &vr_core::CarState| {
            if class_only {
                c.class_position
            } else {
                c.position
            }
        };

        let mut cars: Vec<&vr_core::CarState> = f
            .cars
            .iter()
            .filter(|c| !class_only || c.class_id == player.and_then(|p| p.class_id))
            .collect();
        cars.sort_by_key(|c| pos_of(c).unwrap_or(u32::MAX));

        let mut rows: Vec<StandingRow> = Vec::with_capacity(cars.len());
        for (i, c) in cars.iter().enumerate() {
            let interval_s = if i == 0 {
                None
            } else {
                let prev = cars[i - 1];
                match (c.gap_to_leader_s, prev.gap_to_leader_s) {
                    (Some(a), Some(b)) => Some(a - b),
                    _ => None,
                }
            };
            rows.push(StandingRow {
                car: car_ref(c),
                is_player: c.is_player || Some(c.idx) == f.player_car_idx,
                laps_completed: c.laps_completed,
                gap_to_leader_s: c.gap_to_leader_s,
                interval_s,
                last_lap_s: c.last_lap_s.filter(|x| *x > 0.0),
                best_lap_s: c.best_lap_s.filter(|x| *x > 0.0),
                on_pit_road: c.on_pit_road,
            });
        }

        if let Some(n) = p.around {
            if let Some(pi) = rows.iter().position(|r| r.is_player) {
                let lo = pi.saturating_sub(n);
                let hi = (pi + n + 1).min(rows.len());
                rows = rows[lo..hi].to_vec();
            } else {
                rows.truncate(2 * n + 1);
            }
        } else if let Some(n) = p.top {
            rows.truncate(n);
        }
        Ok(StandingsReport {
            meta,
            class_only,
            rows,
        })
    }

    fn gaps(&self, p: &GapParams) -> ApiResult<GapReport> {
        let meta = self.meta();
        let Some(f) = self.frame() else {
            return Ok(GapReport {
                meta,
                ahead: None,
                behind: None,
                target: None,
                target_is_ahead: None,
                nearby: Vec::new(),
            });
        };
        let (target, target_is_ahead) = match (&p.car, f.player_car()) {
            (Some(num), Some(player)) => {
                let num = num.trim_start_matches('#');
                let other = f
                    .cars
                    .iter()
                    .find(|c| c.number == num)
                    .ok_or_else(|| ApiError::NotFound(format!("car #{num}")))?;
                let progress = |c: &vr_core::CarState| {
                    c.laps_completed.unwrap_or(0) as f64 + c.lap_dist_pct.unwrap_or(0.0)
                };
                let ahead = match (other.position, player.position) {
                    (Some(o), Some(pp)) => o < pp,
                    _ => progress(other) > progress(player),
                };
                (Some(self.gap_between(f, player, other)), Some(ahead))
            }
            _ => (None, None),
        };
        Ok(GapReport {
            meta,
            ahead: self.adjacent(f, -1),
            behind: self.adjacent(f, 1),
            target,
            target_is_ahead,
            nearby: self.nearby(f, p.window_s),
        })
    }

    fn fuel(&self) -> ApiResult<FuelReport> {
        let meta = self.meta();
        Ok(FuelReport {
            meta,
            fuel: self.frame().and_then(|f| self.fuel_info(f)),
        })
    }

    fn laps(&self, p: &LapsParams) -> ApiResult<LapsReport> {
        let meta = self.meta();
        let Some(f) = self.frame() else {
            return Ok(LapsReport {
                meta,
                car: None,
                laps: Vec::new(),
                best_lap_s: None,
                avg_clean_lap_s: None,
            });
        };
        let car = match &p.car {
            Some(num) => {
                let num = num.trim_start_matches('#');
                f.cars
                    .iter()
                    .find(|c| c.number == num)
                    .ok_or_else(|| ApiError::NotFound(format!("car #{num}")))?
            }
            None => f
                .player_car()
                .ok_or_else(|| ApiError::NotFound("player car".into()))?,
        };
        let mut laps = self.car_laps(car.idx);
        let best_lap_s = laps
            .iter()
            .filter_map(|l| l.time_s)
            .min_by(f64::total_cmp)
            .or(car.best_lap_s);
        let avg_clean_lap_s = self.avg_clean_lap(car.idx);
        let n = p.last.unwrap_or(10);
        let skip = laps.len().saturating_sub(n);
        laps.drain(..skip);
        Ok(LapsReport {
            meta,
            car: Some(car_ref(car)),
            laps,
            best_lap_s,
            avg_clean_lap_s,
        })
    }

    fn events(&self, p: &EventsParams) -> ApiResult<EventsReport> {
        Ok(EventsReport {
            meta: self.meta(),
            last_seq: self.event_log().last_seq(),
            events: self
                .event_log()
                .since(p.since.unwrap_or(0), p.limit.unwrap_or(50)),
        })
    }

    fn snapshot(&self) -> ApiResult<SnapshotReport> {
        Ok(SnapshotReport {
            meta: self.meta(),
            frame: self.frame().cloned(),
        })
    }

    fn car(&self) -> ApiResult<CarReport> {
        let f = self.frame();
        let p = f.and_then(|f| f.player.as_ref());
        Ok(CarReport {
            meta: self.meta(),
            car_name: f.and_then(|f| f.player_car()?.car_name.clone()),
            car: p.map(|p| p.car.clone()),
            water_temp_c: p.and_then(|p| p.water_temp_c),
            oil_temp_c: p.and_then(|p| p.oil_temp_c),
            incidents: p.and_then(|p| p.incidents),
            adjustments: self.adjustments().to_vec(),
        })
    }

    fn channels(&self, p: &ChannelsParams) -> ApiResult<ChannelsReport> {
        let all = self.extras().map_or(&[][..], |e| &e.channels);
        let words: Vec<String> = p
            .filter
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        Ok(ChannelsReport {
            meta: self.meta(),
            total: all.len(),
            channels: all
                .iter()
                .filter(|c| {
                    let text = format!("{} {} {}", c.name, c.unit, c.description).to_lowercase();
                    words.iter().all(|w| text.contains(w))
                })
                .cloned()
                .collect(),
        })
    }

    fn info(&self, p: &InfoParams) -> ApiResult<InfoReport> {
        let path = p.path.clone().unwrap_or_default();
        let mut value = self.extras().and_then(|e| e.info.as_ref());
        for key in path.split('/').filter(|k| !k.is_empty()) {
            let Some(v) = value else { break };
            let next = match v {
                serde_json::Value::Array(a) => key.parse::<usize>().ok().and_then(|i| a.get(i)),
                v => v.get(key),
            };
            value = Some(next.ok_or_else(|| ApiError::NotFound(format!("info {path}")))?);
        }
        Ok(InfoReport {
            meta: self.meta(),
            value: value.cloned(),
            path,
        })
    }

    fn setup(&self) -> ApiResult<SetupReport> {
        let f = self.frame();
        Ok(SetupReport {
            meta: self.meta(),
            car_name: f.and_then(|f| f.player_car()?.car_name.clone()),
            setup: self.setup_now().cloned(),
            controls: self.controls(),
            adjustments: self.adjustments().to_vec(),
        })
    }

    fn handling(&self, p: &HandlingParams) -> ApiResult<HandlingReport> {
        let n = p.last.unwrap_or(3).max(1);
        let tracker = self.handling_tracker();
        let mut laps: Vec<HandlingLap> = tracker.laps().rev().take(n).cloned().collect();
        laps.reverse();
        let each: Vec<Behaviour> = laps.iter().map(|l| l.behaviour).collect();
        let mean = Behaviour::mean(&each);
        let car = self.frame().and_then(|f| f.player.as_ref()).map(|p| &p.car);
        let advice = car.map(|car| {
            let b = mean.unwrap_or_default();
            crate::advice::advise(&b, laps.len(), car, self.setup_now())
        });
        Ok(HandlingReport {
            meta: self.meta(),
            ready: tracker.ready(),
            laps_sampled: laps.len(),
            mean,
            laps,
            zones: tracker.zones(5),
            controls: self.controls(),
            adjustments: self.adjustments().to_vec(),
            advice: advice.unwrap_or_default(),
        })
    }
}

macro_rules! delegate_locked {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        impl RaceApi for SharedState {
            $(fn $name(&self, $($arg: $ty),*) -> ApiResult<$ret> {
                let guard = self.read().map_err(|_| ApiError::Other("state lock poisoned".into()))?;
                <RaceState as RaceApi>::$name(&guard, $($arg),*)
            })*
        }
    };
}

delegate_locked! {
    brief() -> Brief;
    standings(p: &StandingsParams) -> StandingsReport;
    gaps(p: &GapParams) -> GapReport;
    fuel() -> FuelReport;
    laps(p: &LapsParams) -> LapsReport;
    events(p: &EventsParams) -> EventsReport;
    snapshot() -> SnapshotReport;
    car() -> CarReport;
    channels(p: &ChannelsParams) -> ChannelsReport;
    info(p: &InfoParams) -> InfoReport;
    setup() -> SetupReport;
    handling(p: &HandlingParams) -> HandlingReport;
}
