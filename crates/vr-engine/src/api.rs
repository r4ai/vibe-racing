use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::RaceState;
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
}
