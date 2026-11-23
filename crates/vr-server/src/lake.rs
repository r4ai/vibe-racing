//! Everything driven, at the rate it was driven, for questions nobody thought
//! of in advance: Parquet files next to each session, read with SQL through an
//! embedded DuckDB. Nothing to install and no server of its own.
//!
//! `<sessions>/<id>.lake/telemetry/part-*.parquet`: the player's car, every
//! frame. `cars/part-*.parquet`: every car, ten times a second.
//! `corners/lap-*.parquet`: each corner of each lap. Laps, events and the
//! sessions themselves are read from the session files.

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

/// Collects what is driven and writes it out now and then. Lives on the
/// thread the telemetry arrives on.
pub struct Writer {
    db: Connection,
    rows: Vec<Row>,
    cars: Vec<CarRow>,
    last_cars: Option<f64>,
    last_flush: Instant,
}

impl Writer {
    pub fn new() -> Result<Self> {
        let db = Connection::open_in_memory()?;
        db.execute_batch(&format!("{TELEMETRY}; {CARS}; {CORNERS};"))?;
        Ok(Self {
            db,
            rows: Vec::new(),
            cars: Vec::new(),
            last_cars: None,
            last_flush: Instant::now(),
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

    /// Time to write what was collected.
    pub fn due(&self) -> bool {
        self.last_flush.elapsed() >= FLUSH_EVERY && !self.rows.is_empty()
    }

    /// Writes what was collected into the session's folder.
    pub fn flush(&mut self, dir: &Path, session: &str) -> Result<()> {
        self.last_flush = Instant::now();
        let (rows, cars) = (
            std::mem::take(&mut self.rows),
            std::mem::take(&mut self.cars),
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
        let dir = folder(dir, session);
        self.copy(
            "telemetry",
            &dir.join("telemetry"),
            &format!("part-{first}"),
        )?;
        self.copy("cars", &dir.join("cars"), &format!("part-{first}"))?;
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
Examples (the current session: session = (SELECT max(id) FROM sessions); leave out pit laps and the first lap for pace):
  -- time lost per corner over the race, against its best:
  SELECT corner, round(sum(time_s - b), 2) lost FROM corners JOIN (SELECT corner c, min(time_s) b FROM corners WHERE session = (SELECT max(id) FROM sessions) AND NOT pit GROUP BY c) ON c = corner WHERE session = (SELECT max(id) FROM sessions) AND NOT pit GROUP BY corner ORDER BY lost DESC LIMIT 3
  -- one corner lap by lap:
  SELECT lap, round(time_s, 2) t, round(min_kmh) v, round(brake_before_m) brake, gear FROM corners WHERE corner = 'スプーン' AND session = (SELECT max(id) FROM sessions) ORDER BY lap
  -- the best lap of everyone, and how steady each driver was:
  SELECT car_number, driver, is_player, min(time_s) best, round(stddev(time_s), 3) spread FROM laps WHERE session = (SELECT max(id) FROM sessions) AND NOT pit AND lap > 1 GROUP BY ALL ORDER BY best LIMIT 5
  -- a car's pace lap by lap (yours: is_player):
  SELECT lap, time_s FROM laps WHERE session = (SELECT max(id) FROM sessions) AND car_number = '5' ORDER BY lap
  -- the lap each incident happened on:
  SELECT e.text, (SELECT min(l.lap) FROM laps l WHERE l.session = e.session AND l.is_player AND l.session_time_s >= e.session_time_s) lap FROM events e WHERE e.type = 'incident' AND e.session = (SELECT max(id) FROM sessions)
  -- this session against an earlier one, corner by corner:
  WITH a AS (SELECT corner, min(time_s) b FROM corners WHERE session = (SELECT max(id) FROM sessions) GROUP BY corner), o AS (SELECT corner, min(time_s) b FROM corners WHERE session = '<id from sessions>' GROUP BY corner) SELECT corner, round(a.b - o.b, 3) slower FROM a JOIN o USING (corner) ORDER BY slower DESC
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
    if sql.contains(';') {
        bail!("one statement at a time");
    }
    let dir = sessions.to_path_buf();
    let (tx, rx) = mpsc::channel();
    // A query that runs away is left behind: the process ends anyway.
    std::thread::spawn(move || {
        let ran = std::panic::catch_unwind(|| run(&dir, &sql));
        let _ =
            tx.send(ran.unwrap_or_else(|_| Err(anyhow::anyhow!("the query failed inside DuckDB"))));
    });
    rx.recv_timeout(TIMEOUT)
        .unwrap_or_else(|_| bail!("the query took longer than {TIMEOUT:?}; narrow it"))
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
CREATE VIEW events AS SELECT id AS session, CAST(e->>'seq' AS BIGINT) AS seq,
    CAST(e->>'session_time_s' AS DOUBLE) AS session_time_s, e->>'type' AS type, e->>'text' AS text, e AS detail
  FROM (SELECT id, unnest(CAST(events AS JSON[])) e FROM raw);\n"
        ));
    } else {
        out.push_str(
            "CREATE TABLE sessions (id VARCHAR, started_ms BIGINT, kind VARCHAR, track VARCHAR, car VARCHAR, laps_completed INTEGER, best_lap_s DOUBLE, position INTEGER, incidents INTEGER);
CREATE TABLE laps (session VARCHAR, car_number VARCHAR, driver VARCHAR, is_player BOOLEAN, lap INTEGER, time_s DOUBLE, pit BOOLEAN, caution BOOLEAN, position INTEGER, fuel_used_l DOUBLE, session_time_s DOUBLE);
CREATE TABLE events (session VARCHAR, seq BIGINT, session_time_s DOUBLE, type VARCHAR, text VARCHAR, detail JSON);\n",
        );
    }
    out
}

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
