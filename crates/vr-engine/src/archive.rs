//! A finished (or running) session as it is kept on disk.

use serde::{Deserialize, Serialize};
use vr_core::{SessionKind, Setup, Weather};

use crate::chassis::Chassis;
use crate::handling::HandlingLap;
use crate::report::*;
use crate::{Event, RaceApi, RaceState, StandingsParams};

/// Sessions shorter than this, without a lap, are not worth keeping.
const MIN_DURATION_S: f64 = 60.0;
/// The start, worked out from the clock, may differ by this much.
const SAME_START_MS: i64 = 30_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Sorts by time: `20261122-080740-race-circuit-de-spa-francorchamps` (UTC).
    pub id: String,
    pub game: String,
    /// Wall clock at session time 0 (unix epoch milliseconds).
    pub started_ms: i64,
    pub updated_ms: i64,
    pub duration_s: f64,
    pub kind: SessionKind,
    pub track: Option<String>,
    /// Tells a restart from the same session.
    #[serde(default)]
    pub uid: Option<i64>,
    pub car: Option<String>,
    /// The player at the end.
    pub player: Option<PlayerSummary>,
    /// Number of laps the player completed with a record.
    pub laps_completed: usize,
    pub laps: Vec<LapRecord>,
    /// Everyone, in the order of the standings.
    pub standings: Vec<StandingRow>,
    /// Laps of the other cars.
    #[serde(default)]
    pub others: Vec<CarLaps>,
    pub events: Vec<Event>,
    pub fuel: Option<FuelInfo>,
    pub weather: Option<Weather>,
    pub setup: Option<Setup>,
    #[serde(default)]
    pub adjustments: Vec<Adjustment>,
    #[serde(default)]
    pub handling: Vec<HandlingLap>,
    /// Where the game tells what the wheels do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chassis: Option<Chassis>,
    /// Written by the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarLaps {
    pub car: CarRef,
    pub laps: Vec<LapRecord>,
}

/// `"Circuit de Spa-Francorchamps"` → `"circuit-de-spa-francorchamps"`.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let cut: String = out.chars().take(40).collect();
    cut.trim_end_matches('-').to_string()
}

/// `(year, month, day, hour, minute, second)` in UTC.
fn civil(ms: i64) -> (i64, u32, u32, u32, u32, u32) {
    let s = ms.div_euclid(1000);
    let (days, rest) = (s.div_euclid(86_400), s.rem_euclid(86_400) as u32);
    // Howard Hinnant's days_from_civil, inverted.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rest / 3600, rest % 3600 / 60, rest % 60)
}

/// `2026-11-22 08:07 UTC`
pub fn fmt_utc(ms: i64) -> String {
    let (y, mo, d, h, mi, _) = civil(ms);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC")
}

impl SessionRecord {
    /// The state as a record; `None` while there is nothing worth keeping.
    pub fn of(state: &RaceState) -> Option<Self> {
        let f = state.frame()?;
        let started_ms = f.timestamp_ms - (f.session.time_s * 1000.0) as i64;
        let laps = f
            .player_car_idx
            .map(|i| state.car_laps(i))
            .unwrap_or_default();
        let duration_s = f.session.time_s;
        if laps.is_empty() && duration_s < MIN_DURATION_S {
            return None;
        }
        let summary = state.session_summary(f);
        let kind = crate::state::kind_label(f.session.kind);
        let (y, mo, d, h, mi, s) = civil(started_ms);
        let track = f.session.track.as_ref().map(|t| slug(&t.name));
        let track = track.filter(|t| !t.is_empty()).map(|t| format!("-{t}"));
        let standings = state
            .standings(&StandingsParams {
                class_only: Some(false),
                ..Default::default()
            })
            .map(|s| s.rows)
            .unwrap_or_default();
        Some(Self {
            id: format!(
                "{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{kind}{}",
                track.unwrap_or_default()
            ),
            game: f.game.clone(),
            started_ms,
            updated_ms: f.timestamp_ms,
            duration_s,
            kind: f.session.kind,
            track: summary.track,
            uid: f.session.uid,
            car: f.player_car().and_then(|c| c.car_name.clone()),
            player: state.player_summary(f),
            laps_completed: laps.len(),
            laps,
            standings,
            others: f
                .cars
                .iter()
                .filter(|c| Some(c.idx) != f.player_car_idx)
                .map(|c| CarLaps {
                    car: crate::state::car_ref(c),
                    laps: state.car_laps(c.idx),
                })
                .filter(|c| !c.laps.is_empty())
                .collect(),
            events: state.session_events(),
            fuel: state.fuel_info(f),
            weather: f.weather.clone(),
            setup: state.setup_now().cloned(),
            adjustments: state.adjustments().to_vec(),
            handling: state.handling_tracker().laps().cloned().collect(),
            chassis: state.chassis(),
            note: None,
        })
    }

    /// Whether both describe the same session, e.g. before and after a
    /// restart of the collector.
    pub fn same_session(&self, o: &Self) -> bool {
        self.game == o.game
            && self.kind == o.kind
            && self.track == o.track
            && self.uid == o.uid
            && (self.started_ms - o.started_ms).abs() < SAME_START_MS
    }

    /// Keeps what only the older record knows.
    pub fn merge_older(&mut self, old: &Self) {
        self.id.clone_from(&old.id);
        self.started_ms = old.started_ms;
        if self.note.is_none() {
            self.note.clone_from(&old.note);
        }
        merge_laps(&mut self.laps, &old.laps);
        self.laps_completed = self.laps.len();
        for o in &old.others {
            match self
                .others
                .iter_mut()
                .find(|c| c.car.number == o.car.number)
            {
                Some(c) => merge_laps(&mut c.laps, &o.laps),
                None => self.others.push(o.clone()),
            }
        }
        let first = self.events.first().map(|e| e.timestamp_ms);
        let mut events: Vec<Event> = old
            .events
            .iter()
            .filter(|e| first.is_none_or(|f| e.timestamp_ms < f))
            .cloned()
            .collect();
        events.append(&mut self.events);
        self.events = events;
        for h in old.handling.iter().rev() {
            if !self.handling.iter().any(|n| n.lap == h.lap) {
                self.handling.insert(0, h.clone());
            }
        }
        for a in old.adjustments.iter().rev() {
            if !self.adjustments.contains(a) {
                self.adjustments.insert(0, a.clone());
            }
        }
        if self.setup.is_none() {
            self.setup.clone_from(&old.setup);
        }
        if self.chassis.is_none() {
            self.chassis.clone_from(&old.chassis);
        }
        // A telemetry file knows the player's car only.
        if old.standings.len() > self.standings.len() {
            self.standings.clone_from(&old.standings);
            self.player.clone_from(&old.player);
        }
    }

    pub fn best_lap_s(&self) -> Option<f64> {
        self.laps
            .iter()
            .filter_map(|l| l.time_s)
            .min_by(f64::total_cmp)
            .or(self.player.as_ref().and_then(|p| p.best_lap_s))
    }

    /// Mean of the clean laps.
    pub fn avg_clean_lap_s(&self) -> Option<f64> {
        let t: Vec<f64> = self
            .laps
            .iter()
            .filter(|l| l.is_clean())
            .filter_map(|l| l.time_s)
            .collect();
        crate::util::mean(&t)
    }

    pub fn fuel_per_lap_l(&self) -> Option<f64> {
        let t: Vec<f64> = self
            .laps
            .iter()
            .filter(|l| !l.pit && !l.caution)
            .filter_map(|l| l.fuel_used_l.filter(|f| *f > 0.0))
            .collect();
        crate::util::mean(&t)
    }
}

fn merge_laps(new: &mut Vec<LapRecord>, old: &[LapRecord]) {
    for l in old {
        match new.iter_mut().find(|n| n.lap == l.lap) {
            Some(n) => {
                n.position = n.position.or(l.position);
                n.caution |= l.caution;
                n.fuel_used_l = n.fuel_used_l.or(l.fuel_used_l);
            }
            None => new.push(l.clone()),
        }
    }
    new.sort_by_key(|l| l.lap);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates() {
        assert_eq!(fmt_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(civil(1_795_334_860_000), (2026, 11, 22, 8, 7, 40));
        assert_eq!(fmt_utc(1_795_334_860_000), "2026-11-22 08:07 UTC");
        assert_eq!(civil(951_782_399_000), (2000, 2, 28, 23, 59, 59));
        assert_eq!(civil(951_782_400_000), (2000, 2, 29, 0, 0, 0));
        assert_eq!(civil(951_868_800_000), (2000, 3, 1, 0, 0, 0));
    }

    #[test]
    fn slugs_are_file_names() {
        assert_eq!(
            slug("Circuit de Spa-Francorchamps"),
            "circuit-de-spa-francorchamps"
        );
        assert_eq!(slug("../../etc: x?"), "etc-x");
        assert_eq!(slug("鈴鹿"), "");
        assert_eq!(slug(&"a".repeat(60)).len(), 40);
        assert_eq!(slug(&format!("{} b", "a".repeat(39))), "a".repeat(39));
    }
}
