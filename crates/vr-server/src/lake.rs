//! Everything driven, at the rate it was driven, for questions nobody thought
//! of in advance: Parquet files next to each session, read with SQL through an
//! embedded DuckDB. Nothing to install and no server of its own.
//!
//! `<sessions>/<id>.lake/telemetry/part-*.parquet`: the player's car, every
//! frame. `cars/part-*.parquet`: every car, ten times a second.
//! `corners/lap-*.parquet`: each corner of each lap.
//! `crossings/part-*.parquet`: the moment each car began each corner, at the
//! rate the game tells it, so every car's corner times compare with yours.
//! Laps, events and the sessions themselves are read from the session files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use duckdb::types::Value as Db;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vr_core::Frame;
use vr_engine::{Corner, LapTrace};

/// The folder of a session's files.
pub fn folder(sessions: &Path, id: &str) -> PathBuf {
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    sessions.join(format!("{safe}.lake"))
}

/// The other cars change their place on the lap slowly.
const CARS_EVERY_S: f64 = 0.1;
/// Written this often: what a crash loses, and how old the newest data is.
pub const FLUSH_EVERY: Duration = Duration::from_secs(5);

/// One frame of the player's car.
struct Row {
    t: f64,
    ts_ms: i64,
    lap: Option<i32>,
    lap_pct: Option<f64>,
    lap_m: Option<f64>,
    lap_time: Option<f64>,
    speed_kmh: Option<f64>,
    throttle_pct: Option<f64>,
    brake_pct: Option<f64>,
    clutch_pct: Option<f64>,
    gear: Option<i32>,
    rpm: Option<f64>,
    steer_deg: Option<f64>,
    lat_g: Option<f64>,
    long_g: Option<f64>,
    yaw_dps: Option<f64>,
    abs: Option<bool>,
    fuel_l: Option<f64>,
    delta_best_s: Option<f64>,
    on_track: bool,
    in_pits: bool,
    wheels: [[Option<f64>; 4]; 4],
}

/// One car at one moment.
struct CarRow {
    t: f64,
    idx: i32,
    number: String,
    driver: String,
    class: Option<String>,
    position: Option<i32>,
    class_position: Option<i32>,
    lap: Option<i32>,
    lap_pct: Option<f64>,
    in_pits: bool,
    surface: String,
    last_lap_s: Option<f64>,
    best_lap_s: Option<f64>,
    gap_to_leader_s: Option<f64>,
    is_player: bool,
}

/// A car beginning a corner.
struct Crossing {
    t: f64,
    idx: i32,
    number: String,
    driver: String,
    is_player: bool,
    lap: Option<i32>,
    corner_no: i32,
    corner: String,
    in_pits: bool,
}

/// Where the corners of a track begin, and their names.
struct Marks {
    key: String,
    at: Vec<f64>,
    names: Vec<String>,
}

impl Marks {
    /// The line, then the start of each named corner; on a track without
    /// names, twenty equal stretches.
    fn of(track: &str, length_m: f64) -> Self {
        let (at, names) = match vr_engine::tracks::bends(track, length_m) {
            Some(b) => (
                std::iter::once(0.0)
                    .chain(b.iter().map(|b| b.from))
                    .collect(),
                std::iter::once(START_STRAIGHT.to_string())
                    .chain(b.iter().map(|b| b.name.to_string()))
                    .collect(),
            ),
            None => (
                (0..STRETCHES)
                    .map(|i| i as f64 / STRETCHES as f64)
                    .collect(),
                (0..STRETCHES)
                    .map(|i| format!("{}%", i * 100 / STRETCHES))
                    .collect(),
            ),
        };
        Self {
            key: format!("{track}|{length_m}"),
            at,
            names,
        }
    }
}

/// Stretches of a lap on a track whose corners have no names.
const STRETCHES: usize = 20;
/// More of the lap than this in one frame is a jump (a tow, a reset), not driving.
const JUMP: f64 = 0.05;

/// The marks passed going from `a` to `b` (shares of the lap, over the line
/// too), each with how far along the way it lies, in order.
fn passed(marks: &[f64], a: f64, b: f64) -> Vec<(usize, f64)> {
    let mut d = b - a;
    if d < -0.5 {
        d += 1.0;
    }
    if d <= 0.0 || d > JUMP {
        return Vec::new();
    }
    let mut out: Vec<(usize, f64)> = marks
        .iter()
        .enumerate()
        .filter_map(|(i, &m)| {
            let mut x = m - a;
            if x <= 0.0 {
                x += 1.0;
            }
            (x <= d).then_some((i, x / d))
        })
        .collect();
    out.sort_by(|x, y| x.1.total_cmp(&y.1));
    out
}

const G: f64 = 9.806_65;

const TELEMETRY: &str = "CREATE TABLE telemetry (
    session VARCHAR, t DOUBLE, ts_ms BIGINT, lap INTEGER, lap_pct DOUBLE, lap_m DOUBLE,
    lap_time DOUBLE, speed_kmh DOUBLE, throttle_pct DOUBLE, brake_pct DOUBLE,
    clutch_pct DOUBLE, gear INTEGER, rpm DOUBLE, steer_deg DOUBLE, lat_g DOUBLE,
    long_g DOUBLE, yaw_dps DOUBLE, abs BOOLEAN, fuel_l DOUBLE, delta_best_s DOUBLE,
    on_track BOOLEAN, in_pits BOOLEAN,
    lf_speed_kmh DOUBLE, rf_speed_kmh DOUBLE, lr_speed_kmh DOUBLE, rr_speed_kmh DOUBLE,
    lf_brake_bar DOUBLE, rf_brake_bar DOUBLE, lr_brake_bar DOUBLE, rr_brake_bar DOUBLE,
    lf_ride_mm DOUBLE, rf_ride_mm DOUBLE, lr_ride_mm DOUBLE, rr_ride_mm DOUBLE,
    lf_shock_mps DOUBLE, rf_shock_mps DOUBLE, lr_shock_mps DOUBLE, rr_shock_mps DOUBLE)";

const CARS: &str = "CREATE TABLE cars (
    session VARCHAR, t DOUBLE, idx INTEGER, number VARCHAR, driver VARCHAR, class VARCHAR,
    position INTEGER, class_position INTEGER, lap INTEGER, lap_pct DOUBLE, in_pits BOOLEAN,
    surface VARCHAR, last_lap_s DOUBLE, best_lap_s DOUBLE, gap_to_leader_s DOUBLE,
    is_player BOOLEAN)";

const CORNERS: &str = "CREATE TABLE corners (
    session VARCHAR, lap INTEGER, lap_time_s DOUBLE, pit BOOLEAN, number INTEGER,
    corner VARCHAR, from_pct DOUBLE, to_pct DOUBLE, time_s DOUBLE, entry_kmh DOUBLE,
    min_kmh DOUBLE, exit_kmh DOUBLE, gear INTEGER, brake_before_m DOUBLE,
    min_throttle_pct INTEGER)";

const CROSSINGS: &str = "CREATE TABLE crossings (
    session VARCHAR, t DOUBLE, idx INTEGER, car_number VARCHAR, driver VARCHAR,
    is_player BOOLEAN, lap INTEGER, number INTEGER, corner VARCHAR, in_pits BOOLEAN)";

/// Collects what is driven and writes it out now and then. Lives on the
/// thread the telemetry arrives on.
pub struct Writer {
    db: Connection,
    rows: Vec<Row>,
    cars: Vec<CarRow>,
    last_cars: Option<f64>,
    last_flush: Instant,
    crossings: Vec<Crossing>,
    /// Each car's place on the lap at the last frame.
    was: HashMap<i32, (f64, f64)>,
    marks: Option<Marks>,
}

impl Writer {
    pub fn new() -> Result<Self> {
        let db = Connection::open_in_memory()?;
        db.execute_batch(&format!("{TELEMETRY}; {CARS}; {CORNERS}; {CROSSINGS};"))?;
        Ok(Self {
            db,
            rows: Vec::new(),
            cars: Vec::new(),
            last_cars: None,
            last_flush: Instant::now(),
            crossings: Vec::new(),
            was: HashMap::new(),
            marks: None,
        })
    }

    pub fn push(&mut self, f: &Frame) {
        let t = f.session.time_s;
        let length = f.session.track.as_ref().and_then(|t| t.length_m);
        if let (Some(p), Some(car)) = (&f.player, f.player_car()) {
            let c = &p.car;
            let w = |get: fn(&vr_core::Wheel) -> Option<f64>| match &c.wheels {
                Some(w) => [get(&w.lf), get(&w.rf), get(&w.lr), get(&w.rr)],
                None => [None; 4],
            };
            self.rows.push(Row {
                t,
                ts_ms: f.timestamp_ms,
                lap: car.lap,
                lap_pct: car.lap_dist_pct,
                lap_m: car.lap_dist_pct.zip(length).map(|(p, l)| p * l),
                lap_time: p.current_lap_s.filter(|t| *t >= 0.0),
                speed_kmh: p.speed_mps.map(|v| v * 3.6),
                throttle_pct: p.throttle.map(|v| v * 100.0),
                brake_pct: p.brake.map(|v| v * 100.0),
                clutch_pct: c.clutch.map(|v| v * 100.0),
                gear: p.gear,
                rpm: p.rpm,
                steer_deg: c.steering_rad.map(f64::to_degrees),
                lat_g: c.lat_accel_mps2.map(|a| a / G),
                long_g: c.long_accel_mps2.map(|a| a / G),
                yaw_dps: c.yaw_rate_rps.map(f64::to_degrees),
                abs: c.abs_active,
                fuel_l: p.fuel_l,
                delta_best_s: p.delta_to_best_s,
                on_track: p.on_track,
                in_pits: car.on_pit_road,
                wheels: [
                    w(|w| w.speed_mps.map(|v| v * 3.6)),
                    w(|w| w.brake_bar),
                    w(|w| w.ride_height_m.map(|m| m * 1000.0)),
                    w(|w| w.shock_mps),
                ],
            });
        }
        self.cross(f);
        if self
            .last_cars
            .is_none_or(|l| t - l >= CARS_EVERY_S || t < l)
        {
            self.last_cars = Some(t);
            for c in &f.cars {
                self.cars.push(CarRow {
                    t,
                    idx: c.idx as i32,
                    number: c.number.clone(),
                    driver: c.driver.clone(),
                    class: c.class_name.clone(),
                    position: c.position.map(|p| p as i32),
                    class_position: c.class_position.map(|p| p as i32),
                    lap: c.lap,
                    lap_pct: c.lap_dist_pct,
                    in_pits: c.on_pit_road,
                    surface: format!("{:?}", c.surface).to_lowercase(),
                    last_lap_s: c.last_lap_s.filter(|t| *t > 0.0),
                    best_lap_s: c.best_lap_s.filter(|t| *t > 0.0),
                    gap_to_leader_s: c.gap_to_leader_s,
                    is_player: c.is_player,
                });
            }
        }
    }

    /// Every car that began a corner since the last frame.
    fn cross(&mut self, f: &Frame) {
        let Some(track) = &f.session.track else {
            return;
        };
        let Some(length) = track.length_m.filter(|l| *l > 0.0) else {
            return;
        };
        let key = format!("{}|{length}", track.name);
        if self.marks.as_ref().is_none_or(|m| m.key != key) {
            self.marks = Some(Marks::of(&track.name, length));
            self.was.clear();
        }
        let Some(marks) = &self.marks else {
            return;
        };
        let t = f.session.time_s;
        for c in &f.cars {
            let idx = c.idx as i32;
            let Some(now) = c.lap_dist_pct.filter(|p| (0.0..=1.0).contains(p)) else {
                self.was.remove(&idx);
                continue;
            };
            if let Some((t0, before)) = self.was.insert(idx, (t, now))
                && t > t0
            {
                for (i, along) in passed(&marks.at, before, now) {
                    self.crossings.push(Crossing {
                        t: t0 + (t - t0) * along,
                        idx,
                        number: c.number.clone(),
                        driver: c.driver.clone(),
                        is_player: c.is_player,
                        lap: c.lap,
                        corner_no: i as i32,
                        corner: marks.names[i].clone(),
                        in_pits: c.on_pit_road,
                    });
                }
            }
        }
    }

    /// Time to write what was collected.
    pub fn due(&self) -> bool {
        self.last_flush.elapsed() >= FLUSH_EVERY && !self.rows.is_empty()
    }

    /// Writes what was collected into the session's folder.
    pub fn flush(&mut self, dir: &Path, session: &str) -> Result<()> {
        self.last_flush = Instant::now();
        let (rows, cars, crossings) = (
            std::mem::take(&mut self.rows),
            std::mem::take(&mut self.cars),
            std::mem::take(&mut self.crossings),
        );
        let Some(first) = rows.first().map(|r| r.ts_ms) else {
            return Ok(());
        };
        {
            let mut app = self.db.appender("telemetry")?;
            for r in &rows {
                let [s, b, h, k] = r.wheels;
                app.append_row(params![
                    session,
                    r.t,
                    r.ts_ms,
                    r.lap,
                    r.lap_pct,
                    r.lap_m,
                    r.lap_time,
                    r.speed_kmh,
                    r.throttle_pct,
                    r.brake_pct,
                    r.clutch_pct,
                    r.gear,
                    r.rpm,
                    r.steer_deg,
                    r.lat_g,
                    r.long_g,
                    r.yaw_dps,
                    r.abs,
                    r.fuel_l,
                    r.delta_best_s,
                    r.on_track,
                    r.in_pits,
                    s[0],
                    s[1],
                    s[2],
                    s[3],
                    b[0],
                    b[1],
                    b[2],
                    b[3],
                    h[0],
                    h[1],
                    h[2],
                    h[3],
                    k[0],
                    k[1],
                    k[2],
                    k[3],
                ])?;
            }
            app.flush()?;
        }
        {
            let mut app = self.db.appender("cars")?;
            for c in &cars {
                app.append_row(params![
                    session,
                    c.t,
                    c.idx,
                    c.number,
                    c.driver,
                    c.class,
                    c.position,
                    c.class_position,
                    c.lap,
                    c.lap_pct,
                    c.in_pits,
                    c.surface,
                    c.last_lap_s,
                    c.best_lap_s,
                    c.gap_to_leader_s,
                    c.is_player,
                ])?;
            }
            app.flush()?;
        }
        {
            let mut app = self.db.appender("crossings")?;
            for c in &crossings {
                app.append_row(params![
                    session,
                    c.t,
                    c.idx,
                    c.number,
                    c.driver,
                    c.is_player,
                    c.lap,
                    c.corner_no,
                    c.corner,
                    c.in_pits,
                ])?;
            }
            app.flush()?;
        }
        let dir = folder(dir, session);
        self.copy(
            "telemetry",
            &dir.join("telemetry"),
            &format!("part-{first}"),
        )?;
        self.copy("cars", &dir.join("cars"), &format!("part-{first}"))?;
        self.copy(
            "crossings",
            &dir.join("crossings"),
            &format!("part-{first}"),
        )?;
        Ok(())
    }

    /// Each corner of a lap that was just completed.
    pub fn lap(
        &mut self,
        dir: &Path,
        session: &str,
        lap: &LapTrace,
        track: Option<&str>,
    ) -> Result<()> {
        let bends = track.and_then(|t| vr_engine::tracks::bends(t, lap.length_m));
        let corners = vr_engine::compare_laps_on(lap, lap, bends);
        if corners.is_empty() {
            return Ok(());
        }
        {
            let mut app = self.db.appender("corners")?;
            // From the line to the first corner: the rows add up to the lap.
            if let Some(first) = corners.first().filter(|c| c.from > 0.0) {
                let line =
                    |share: f64| lap.t_ms.get(at(lap, share)).map(|m| f64::from(*m) / 1000.0);
                app.append_row(params![
                    session,
                    lap.lap,
                    lap.time_s,
                    lap.pit,
                    0,
                    START_STRAIGHT,
                    0.0,
                    first.from,
                    line(first.from),
                    speed_at(lap, 0.0),
                    lap.speed
                        .get(..at(lap, first.from))
                        .and_then(|s| s.iter().min())
                        .map(|v| f64::from(*v) / 10.0),
                    speed_at(lap, first.from),
                    None::<i32>,
                    None::<f64>,
                    lap.throttle
                        .get(..at(lap, first.from))
                        .and_then(|t| t.iter().min())
                        .map(|t| i32::from(*t)),
                ])?;
            }
            for c in &corners {
                app.append_row(params![
                    session,
                    lap.lap,
                    lap.time_s,
                    lap.pit,
                    c.number as i32,
                    label(c),
                    c.from,
                    c.to,
                    time_in(lap, c),
                    c.reference_entry_kmh,
                    c.reference_min_kmh,
                    speed_at(lap, c.to),
                    c.reference_gear.map(i32::from),
                    c.reference_brake_before_m,
                    i32::from(c.reference_min_throttle),
                ])?;
            }
            app.flush()?;
        }
        self.copy(
            "corners",
            &folder(dir, session).join("corners"),
            &format!("lap-{:03}", lap.lap),
        )
    }

    /// The table's rows into a Parquet file of their own, then gone from it.
    /// Written aside first: a reader never sees half a file.
    fn copy(&self, table: &str, dir: &Path, name: &str) -> Result<()> {
        let n: i64 = self
            .db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        if n == 0 {
            return Ok(());
        }
        std::fs::create_dir_all(dir)?;
        let part = dir.join(format!("{name}.parquet.part"));
        let done = dir.join(format!("{name}.parquet"));
        self.db.execute_batch(&format!(
            "COPY {table} TO '{}' (FORMAT parquet, COMPRESSION zstd); DELETE FROM {table};",
            sql_path(&part)
        ))?;
        std::fs::rename(&part, &done).with_context(|| format!("{}", done.display()))?;
        Ok(())
    }
}

/// The row of the straight from the line to the first corner.
pub const START_STRAIGHT: &str = "ホームストレート";

fn label(c: &Corner) -> String {
    c.name
        .clone()
        .unwrap_or_else(|| format!("corner {}", c.number))
}

fn at(lap: &LapTrace, share: f64) -> usize {
    ((share * lap.speed.len() as f64) as usize).min(lap.speed.len().saturating_sub(1))
}

/// Seconds from the start of a corner to the start of the next.
fn time_in(lap: &LapTrace, c: &Corner) -> Option<f64> {
    let ms = |share: f64| -> Option<f64> {
        if share >= 1.0 {
            return Some(lap.time_s * 1000.0);
        }
        lap.t_ms.get(at(lap, share)).map(|m| f64::from(*m))
    };
    Some((ms(c.to)? - ms(c.from)?) / 1000.0)
}

fn speed_at(lap: &LapTrace, share: f64) -> f64 {
    lap.speed
        .get(at(lap, share))
        .map_or(0.0, |v| f64::from(*v) / 10.0)
}

/// A path as DuckDB reads it in a string.
fn sql_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").replace('\'', "''")
}

/// What a query found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryReport {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// More rows were found than are shown.
    pub truncated: bool,
    /// What the asker may have overlooked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl QueryReport {
    pub fn to_text(&self) -> String {
        let cell = |v: &Value| match v {
            Value::Null => "".to_string(),
            Value::String(s) => s.clone(),
            Value::Number(n) => match n.as_f64() {
                Some(f) if n.is_f64() => format!("{f:.3}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string(),
                _ => n.to_string(),
            },
            v => v.to_string(),
        };
        let mut out = self.columns.join(" | ");
        out.push('\n');
        for r in &self.rows {
            let cells: Vec<String> = r.iter().map(cell).collect();
            out.push_str(&cells.join(" | "));
            out.push('\n');
        }
        if self.rows.is_empty() {
            out.push_str("(no rows)\n");
        }
        if self.truncated {
            out.push_str(&format!(
                "(only the first {} rows; aggregate or LIMIT)\n",
                self.rows.len()
            ));
        }
        if let Some(n) = &self.note {
            out.push_str(&format!("({n})\n"));
        }
        out
    }
}

/// The tables, as a reader of the data sees them.
pub const SCHEMA: &str = "\
Tables (DuckDB SQL; one row per sample unless said otherwise). `session` is the session id (newest sorts last; `sessions` tells track, kind and time).
sessions: id, started_ms, kind (practice|qualify|warmup|race), track, car, laps_completed, best_lap_s, position, incidents
laps: session, car_number, driver, is_player (true: you, the driver asking), lap, time_s, pit, caution, position, fuel_used_l, session_time_s. Every car's completed laps
events: session, seq, session_time_s, type (incident, flag_raised, position_changed, pit_entered, lap_completed, ...), text
telemetry (yours, 60 per second): session, t (session seconds), lap, lap_pct (0-1), lap_m (metres from the line), lap_time, speed_kmh, throttle_pct, brake_pct, clutch_pct, gear, rpm, steer_deg, lat_g, long_g, yaw_dps, abs, fuel_l, delta_best_s, on_track, in_pits, lf_/rf_/lr_/rr_speed_kmh, _brake_bar, _ride_mm, _shock_mps
cars (everyone, 10 per second): session, t, idx, number, driver, class, position, class_position, lap, lap_pct, in_pits, surface, last_lap_s, best_lap_s, gap_to_leader_s, is_player
corners (yours, one row per corner per lap): session, lap, lap_time_s, pit, number, corner (name on known tracks: スプーン, S字...; number 0 'ホームストレート' is the line to the first corner, so a lap's rows add up to the lap), from_pct, to_pct, time_s (start of it to start of the next), entry_kmh, min_kmh, exit_kmh, gear, brake_before_m (braking began this far before the slowest point), min_throttle_pct (100 = flat out)
crossings (everyone, at the frame rate): session, t, car_number, driver, is_player, lap, number, corner (as in corners; on a track without names 20 stretches '0%', '5%'...), in_pits: the moment the car began that corner
Summaries (start here; pit laps, caution laps and the first lap left out):
lap_stats: session, car_number, driver, is_player, laps, best_s, median_s, spread_s (stddev: how steady), early_s (laps 2-4), late_s (last 3), fade_s (late - early: + is slower at the end)
corner_stats: session, number, corner, laps, best_s, median_s, lost_s (time lost there over the session against its best), min_kmh_low, min_kmh_high, brake_spread_m, flat_out
car_corners (everyone, one row per corner per lap, from crossings): session, car_number, driver, is_player, lap, number, corner, time_s, pit
corner_field (per corner: you against the whole field, best laps of each): session, number, corner, cars, you_s, field_best_s, fastest_car, fastest_driver, you_lose_s, you_place (1 = the fastest there)
brakes (yours, each stop from the 60 per second telemetry, about a metre apart): session, lap, number, corner, brake_on_m, brake_off_m, brake_s, entry_kmh, peak_brake_pct, min_kmh, apex_m, throttle_on_m (50% throttle again), corner_s (that corner's time on that lap)
events also has lap (the driver's lap it happened on)
Examples (the current session: session = (SELECT max(id) FROM sessions); leave out pit laps and the first lap for pace):
  -- where the session lost most, and the lap of the best corners together:
  SELECT corner, round(lost_s, 2) lost, round(best_s, 2) best FROM corner_stats WHERE session = (SELECT max(id) FROM sessions) ORDER BY lost DESC LIMIT 3
  SELECT round(sum(best_s), 3) ideal FROM corner_stats WHERE session = (SELECT max(id) FROM sessions)
  -- one corner lap by lap:
  SELECT lap, round(time_s, 2) t, round(min_kmh) v, round(brake_before_m) brake, gear FROM corners WHERE corner = 'スプーン' AND session = (SELECT max(id) FROM sessions) ORDER BY lap
  -- the best lap of everyone, how steady and how they faded:
  SELECT car_number, driver, is_player, round(best_s, 3) best, round(spread_s, 3) spread, round(fade_s, 2) fade FROM lap_stats WHERE session = (SELECT max(id) FROM sessions) ORDER BY best LIMIT 5
  -- a car's pace lap by lap (yours: is_player):
  SELECT lap, time_s FROM laps WHERE session = (SELECT max(id) FROM sessions) AND car_number = '5' ORDER BY lap
  -- incidents lap by lap:
  SELECT lap, count(*) n, string_agg(text, '; ') FROM events WHERE type = 'incident' AND session = (SELECT max(id) FROM sessions) GROUP BY lap ORDER BY lap
  -- this session against an earlier one, corner by corner:
  WITH a AS (SELECT corner, min(time_s) b FROM corners WHERE session = (SELECT max(id) FROM sessions) GROUP BY corner), o AS (SELECT corner, min(time_s) b FROM corners WHERE session = '<id from sessions>' GROUP BY corner) SELECT corner, round(a.b - o.b, 3) slower FROM a JOIN o USING (corner) ORDER BY slower DESC
  -- where the field is faster than you, and who to learn from:
  SELECT corner, round(you_lose_s, 2) lose, fastest_car, fastest_driver, you_place FROM corner_field WHERE session = (SELECT max(id) FROM sessions) ORDER BY lose DESC LIMIT 3
  -- you against the car behind, corner by corner (its best of each corner):
  SELECT corner, round(min(time_s) FILTER (is_player) - min(time_s) FILTER (car_number = '7'), 2) slower FROM car_corners WHERE session = (SELECT max(id) FROM sessions) AND NOT pit GROUP BY number, corner ORDER BY number
  -- the brake point of one corner lap by lap, and what it gave:
  SELECT lap, round(brake_on_m) brake_m, round(entry_kmh) entry, round(min_kmh) apex, round(corner_s, 2) t FROM brakes WHERE corner = 'シケイン' AND session = (SELECT max(id) FROM sessions) ORDER BY lap
  -- the speed through a stretch of one lap:
  SELECT round(lap_m, -1) m, avg(speed_kmh), avg(brake_pct) FROM telemetry WHERE session = (SELECT max(id) FROM sessions) AND lap = 5 AND lap_m BETWEEN 3700 AND 4100 GROUP BY m ORDER BY m";

/// Rows a query returns at the most.
pub const ROWS: usize = 200;
/// Queries that take longer are given up.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// Answers a question in SQL over the saved sessions. Only reading.
pub fn query(sessions: &Path, sql: &str) -> Result<QueryReport> {
    let sql = sql.trim().trim_end_matches(';').trim().to_string();
    let first = sql
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    if !matches!(
        first.as_str(),
        "select" | "with" | "from" | "describe" | "summarize" | "show"
    ) {
        bail!("only questions: SELECT, WITH, FROM, DESCRIBE, SUMMARIZE or SHOW");
    }
    if statements(&sql) > 1 {
        bail!("one statement at a time");
    }
    let dir = sessions.to_path_buf();
    let everywhere = every_session(&sql);
    let (tx, rx) = mpsc::channel();
    // A query that runs away is left behind: the process ends anyway.
    std::thread::spawn(move || {
        let ran = std::panic::catch_unwind(|| run(&dir, &sql));
        let _ =
            tx.send(ran.unwrap_or_else(|_| Err(anyhow::anyhow!("the query failed inside DuckDB"))));
    });
    let mut r = rx
        .recv_timeout(TIMEOUT)
        .unwrap_or_else(|_| bail!("the query took longer than {TIMEOUT:?}; narrow it"))?;
    r.note = everywhere.then(|| {
        "no session in the query: every session kept is in it; for this one add session = (SELECT max(id) FROM sessions)".to_string()
    });
    Ok(r)
}

/// A question over tables of many sessions that does not say which.
fn every_session(sql: &str) -> bool {
    const TABLES: [&str; 11] = [
        "telemetry",
        "cars",
        "corners",
        "crossings",
        "laps",
        "events",
        "lap_stats",
        "corner_stats",
        "car_corners",
        "corner_field",
        "brakes",
    ];
    let sql = sql.to_lowercase();
    let words: Vec<&str> = sql
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    !words.iter().any(|w| *w == "session" || *w == "id") && words.iter().any(|w| TABLES.contains(w))
}

/// Statements in the text: a `;` within quotes ends none.
fn statements(sql: &str) -> usize {
    let mut quoted = None;
    let mut n = 1;
    for c in sql.chars() {
        match (quoted, c) {
            (None, '\'' | '"') => quoted = Some(c),
            (Some(q), c) if c == q => quoted = None,
            (None, ';') => n += 1,
            _ => {}
        }
    }
    n
}

fn run(dir: &Path, sql: &str) -> Result<QueryReport> {
    let db = Connection::open_in_memory()?;
    db.execute_batch(&views(dir))?;
    let mut stmt = db.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let columns: Vec<String> = rows.as_ref().map(|s| s.column_names()).unwrap_or_default();
    // A column of no type (a bare NULL) cannot be read: it is null.
    let typeless: Vec<bool> = (0..columns.len())
        .map(|i| {
            rows.as_ref()
                .is_some_and(|s| s.column_type(i) == duckdb::arrow::datatypes::DataType::Null)
        })
        .collect();
    let mut out = Vec::new();
    let mut truncated = false;
    while let Some(r) = rows.next()? {
        if out.len() == ROWS {
            truncated = true;
            break;
        }
        let cell = |i: usize| {
            if typeless[i] {
                return Ok(Value::Null);
            }
            r.get::<_, Db>(i).map(json)
        };
        let row: Vec<Value> = (0..columns.len())
            .map(cell)
            .collect::<duckdb::Result<_>>()?;
        out.push(row);
    }
    Ok(QueryReport {
        columns,
        rows: out,
        truncated,
        note: None,
    })
}

fn json(v: Db) -> Value {
    match v {
        Db::Null => Value::Null,
        Db::Boolean(b) => b.into(),
        Db::TinyInt(i) => i.into(),
        Db::SmallInt(i) => i.into(),
        Db::Int(i) => i.into(),
        Db::BigInt(i) => i.into(),
        Db::HugeInt(i) => (i as f64).into(),
        Db::UTinyInt(i) => i.into(),
        Db::USmallInt(i) => i.into(),
        Db::UInt(i) => i.into(),
        Db::UBigInt(i) => i.into(),
        Db::Float(f) => f64::from(f).into(),
        Db::Double(f) => f.into(),
        Db::Text(s) => s.into(),
        Db::Decimal(d) => d
            .to_string()
            .parse::<f64>()
            .map_or_else(|_| d.to_string().into(), Into::into),
        Db::List(l) | Db::Array(l) => Value::Array(l.into_iter().map(json).collect()),
        other => format!("{other:?}").into(),
    }
}

/// The tables over the files there are; a table without files is empty.
fn views(dir: &Path) -> String {
    let d = sql_path(dir);
    let mut out = String::new();
    let files = |sub: &str| has_files(dir, sub);
    for (name, sub, create) in [
        ("telemetry", "telemetry", TELEMETRY),
        ("cars", "cars", CARS),
        ("corners", "corners", CORNERS),
        ("crossings", "crossings", CROSSINGS),
    ] {
        if files(sub) {
            out.push_str(&format!(
                "CREATE VIEW {name} AS SELECT * FROM read_parquet('{d}/*.lake/{sub}/*.parquet', union_by_name = true);\n"
            ));
        } else {
            out.push_str(&format!("{create};\n"));
        }
    }
    let json = format!(
        "read_json('{d}/*.json', format = 'auto', union_by_name = true, maximum_object_size = 268435456, \
         columns = {{id: 'VARCHAR', started_ms: 'BIGINT', kind: 'VARCHAR', track: 'VARCHAR', car: 'VARCHAR', \
         laps_completed: 'INTEGER', player: 'JSON', laps: 'JSON', others: 'JSON', events: 'JSON'}})"
    );
    if std::fs::read_dir(dir).is_ok_and(|mut r| {
        r.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "json")))
    }) {
        out.push_str(&format!(
            "CREATE VIEW raw AS SELECT * FROM {json};
CREATE VIEW sessions AS SELECT id, started_ms, kind, track, car, laps_completed,
    (SELECT min(CAST(l->>'time_s' AS DOUBLE)) FROM (SELECT unnest(CAST(laps AS JSON[])) l) WHERE NOT CAST(l->>'pit' AS BOOLEAN)) AS best_lap_s,
    CAST(player->'car'->>'position' AS INTEGER) AS position,
    CAST(player->>'incidents' AS INTEGER) AS incidents
  FROM raw;
CREATE VIEW laps AS
  SELECT id AS session, coalesce(player->'car'->>'number', '') AS car_number, player->'car'->>'driver' AS driver, true AS is_player, CAST(l->>'lap' AS INTEGER) AS lap, CAST(l->>'time_s' AS DOUBLE) AS time_s,
    CAST(l->>'pit' AS BOOLEAN) AS pit, CAST(l->>'caution' AS BOOLEAN) AS caution, CAST(l->>'position' AS INTEGER) AS position,
    CAST(l->>'fuel_used_l' AS DOUBLE) AS fuel_used_l, CAST(l->>'session_time_s' AS DOUBLE) AS session_time_s
  FROM (SELECT id, player, unnest(CAST(laps AS JSON[])) l FROM raw)
  UNION ALL
  SELECT id, o->'car'->>'number', o->'car'->>'driver', false, CAST(l->>'lap' AS INTEGER), CAST(l->>'time_s' AS DOUBLE), CAST(l->>'pit' AS BOOLEAN),
    CAST(l->>'caution' AS BOOLEAN), CAST(l->>'position' AS INTEGER), CAST(l->>'fuel_used_l' AS DOUBLE), CAST(l->>'session_time_s' AS DOUBLE)
  FROM (SELECT id, o, unnest(CAST(o->'laps' AS JSON[])) l FROM (SELECT id, unnest(CAST(others AS JSON[])) o FROM raw));
CREATE VIEW events_raw AS SELECT id AS session, CAST(e->>'seq' AS BIGINT) AS seq,
    CAST(e->>'session_time_s' AS DOUBLE) AS session_time_s, e->>'type' AS type, e->>'text' AS text, e AS detail
  FROM (SELECT id, unnest(CAST(events AS JSON[])) e FROM raw);\n"
        ));
    } else {
        out.push_str(
            "CREATE TABLE sessions (id VARCHAR, started_ms BIGINT, kind VARCHAR, track VARCHAR, car VARCHAR, laps_completed INTEGER, best_lap_s DOUBLE, position INTEGER, incidents INTEGER);
CREATE TABLE laps (session VARCHAR, car_number VARCHAR, driver VARCHAR, is_player BOOLEAN, lap INTEGER, time_s DOUBLE, pit BOOLEAN, caution BOOLEAN, position INTEGER, fuel_used_l DOUBLE, session_time_s DOUBLE);
CREATE TABLE events_raw (session VARCHAR, seq BIGINT, session_time_s DOUBLE, type VARCHAR, text VARCHAR, detail JSON);\n",
        );
    }
    out.push_str(SUMMARIES);
    out
}

/// What most questions need, worked out once and right: the laps that count
/// are clean ones, the lap of an event is the driver's lap it fell in.
const SUMMARIES: &str = "
CREATE VIEW events AS SELECT e.*, (SELECT min(l.lap) FROM laps l WHERE l.session = e.session AND l.is_player
    AND l.session_time_s >= e.session_time_s) AS lap FROM events_raw e;
CREATE VIEW lap_stats AS
  WITH clean AS (SELECT *, row_number() OVER (PARTITION BY session, car_number ORDER BY lap DESC) AS from_end
    FROM laps WHERE NOT pit AND NOT caution AND lap > 1 AND time_s > 0)
  SELECT session, car_number, driver, is_player, count(*) AS laps, min(time_s) AS best_s, median(time_s) AS median_s,
    stddev(time_s) AS spread_s, avg(time_s) FILTER (lap <= 4) AS early_s, avg(time_s) FILTER (from_end <= 3) AS late_s,
    avg(time_s) FILTER (from_end <= 3) - avg(time_s) FILTER (lap <= 4) AS fade_s
  FROM clean GROUP BY ALL;
CREATE VIEW corner_stats AS
  SELECT session, number, corner, count(*) AS laps, min(time_s) AS best_s, median(time_s) AS median_s,
    sum(time_s) - count(*) * min(time_s) AS lost_s, min(min_kmh) AS min_kmh_low, max(min_kmh) AS min_kmh_high,
    stddev(brake_before_m) AS brake_spread_m, bool_and(min_throttle_pct = 100) AS flat_out
  FROM corners WHERE NOT pit GROUP BY ALL;
CREATE VIEW car_corners AS
  WITH x AS (SELECT *, lead(t) OVER w AS next_t, lead(number) OVER w AS next_no, lead(in_pits) OVER w AS next_pits,
      max(number) OVER (PARTITION BY session) AS last_no
    FROM crossings WINDOW w AS (PARTITION BY session, idx ORDER BY t))
  SELECT session, car_number, driver, is_player, lap, number, corner, next_t - t AS time_s, in_pits OR next_pits AS pit
  FROM x WHERE next_no = CASE WHEN number = last_no THEN 0 ELSE number + 1 END AND next_t - t < 120;
CREATE VIEW corner_field AS
  WITH b AS (SELECT session, number, corner, car_number, driver, is_player, min(time_s) AS best_s,
      rank() OVER (PARTITION BY session, number ORDER BY min(time_s)) AS place
    FROM car_corners WHERE NOT pit GROUP BY session, number, corner, car_number, driver, is_player)
  SELECT session, number, corner, count(*) AS cars, min(best_s) FILTER (is_player) AS you_s, min(best_s) AS field_best_s,
    arg_min(car_number, best_s) AS fastest_car, arg_min(driver, best_s) AS fastest_driver,
    min(best_s) FILTER (is_player) - min(best_s) AS you_lose_s, min(place) FILTER (is_player) AS you_place
  FROM b GROUP BY session, number, corner;
CREATE VIEW brakes AS
  WITH f AS (SELECT session, t, lap, lap_pct, lap_m, speed_kmh, brake_pct, throttle_pct,
      brake_pct > 5 AND coalesce(lag(brake_pct) OVER w, 0) <= 5 AS starts
    FROM telemetry WHERE NOT in_pits WINDOW w AS (PARTITION BY session ORDER BY t)),
  g AS (SELECT *, sum(CAST(starts AS INTEGER)) OVER (PARTITION BY session ORDER BY t) AS stop FROM f),
  s AS (SELECT session, stop, min(t) AS on_t, max(t) FILTER (brake_pct > 5) AS off_t, max(brake_pct) AS peak_brake_pct
    FROM g WHERE stop > 0 GROUP BY session, stop),
  w AS (SELECT s.*, (SELECT min(g.t) FROM g WHERE g.session = s.session AND g.stop = s.stop AND g.t > s.off_t
      AND g.throttle_pct >= 50) AS throttle_t FROM s),
  r AS (SELECT w.session, w.stop, w.peak_brake_pct, w.off_t - w.on_t AS brake_s,
      arg_min(g.lap, g.t) AS lap, arg_min(g.lap_m, g.t) AS brake_on_m, arg_min(g.speed_kmh, g.t) AS entry_kmh,
      arg_min(g.lap_m, abs(g.t - w.off_t)) AS brake_off_m, min(g.speed_kmh) AS min_kmh,
      arg_min(g.lap_m, g.speed_kmh) AS apex_m, arg_min(g.lap_pct, g.speed_kmh) AS apex_pct,
      arg_min(g.lap_m, abs(g.t - w.throttle_t)) FILTER (w.throttle_t IS NOT NULL) AS throttle_on_m
    FROM w JOIN g ON g.session = w.session AND g.stop = w.stop AND g.t <= coalesce(w.throttle_t, w.off_t)
    GROUP BY w.session, w.stop, w.peak_brake_pct, w.on_t, w.off_t, w.throttle_t)
  SELECT r.session, r.lap, c.number, c.corner, r.brake_on_m, r.brake_off_m, r.brake_s, r.entry_kmh, r.peak_brake_pct,
    r.min_kmh, r.apex_m, r.throttle_on_m, c.time_s AS corner_s
  FROM r LEFT JOIN corners c ON c.session = r.session AND c.lap = r.lap AND r.apex_pct >= c.from_pct AND r.apex_pct < c.to_pct
  WHERE r.brake_s >= 0.15;
";

fn has_files(dir: &Path, sub: &str) -> bool {
    let Ok(all) = std::fs::read_dir(dir) else {
        return false;
    };
    all.filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "lake"))
        .any(|e| {
            std::fs::read_dir(e.path().join(sub)).is_ok_and(|mut r| {
                r.any(|f| f.is_ok_and(|f| f.path().extension().is_some_and(|x| x == "parquet")))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_passed_between_two_frames() {
        let m = [0.0, 0.25, 0.5];
        let p = passed(&m, 0.24, 0.26);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, 1);
        assert!((p[0].1 - 0.5).abs() < 1e-9);
        // Exactly on a mark counts once: on arriving, not on leaving.
        assert_eq!(passed(&m, 0.23, 0.25), vec![(1, 1.0)]);
        assert_eq!(passed(&m, 0.25, 0.26), vec![]);
        // Over the line.
        let over = passed(&m, 0.99, 0.01);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].0, 0);
        assert!((over[0].1 - 0.5).abs() < 1e-9);
        // Backwards, standing still or jumping: nothing passed.
        assert_eq!(passed(&m, 0.26, 0.24), vec![]);
        assert_eq!(passed(&m, 0.30, 0.30), vec![]);
        assert_eq!(passed(&m, 0.20, 0.20 + JUMP + 0.01), vec![]);
        assert_eq!(passed(&m, 0.24, 0.24 + JUMP).len(), 1);
    }

    #[test]
    fn a_question_over_every_session_is_noted() {
        assert!(every_session(
            "SELECT corner, lost_s FROM corner_stats ORDER BY lost_s DESC"
        ));
        assert!(!every_session(
            "SELECT * FROM corner_stats WHERE session = 'x'"
        ));
        assert!(!every_session("SELECT id, track FROM sessions"));
        assert!(!every_session("SELECT 1"));
        // Over every session on purpose, by naming it.
        assert!(!every_session(
            "SELECT session, min(best_s) FROM lap_stats GROUP BY session"
        ));
    }

    #[test]
    fn marks_are_the_corners_or_twenty_stretches() {
        let s = Marks::of("Suzuka International Racing Course - Grand Prix", 5756.0);
        assert_eq!((s.at.len(), s.at[0], s.at[1]), (14, 0.0, 0.120));
        assert_eq!(
            (s.names[0].as_str(), s.names[10].as_str()),
            (START_STRAIGHT, "スプーン")
        );
        let u = Marks::of("Nowhere", 4000.0);
        assert_eq!(
            (u.at.len(), u.at[1], u.names[19].as_str()),
            (20, 0.05, "95%")
        );
    }
}
