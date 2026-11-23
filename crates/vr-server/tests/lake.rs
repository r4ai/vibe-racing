//! Everything driven is kept and can be asked about in SQL.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;
use vr_core::{Capabilities, SourceFactory, TelemetrySource};
use vr_engine::*;
use vr_replay::{SynthConfig, SynthSource};
use vr_server::lake::{self, QueryReport};
use vr_server::*;

/// A short race driven through corners, kept in `dir`.
fn driven(dir: &std::path::Path) -> SessionStore {
    let store = SessionStore::new(dir);
    let st: SharedState = Arc::new(RwLock::new(RaceState::new(
        "synthetic",
        SourceKind::Synthetic,
        Capabilities::default(),
    )));
    let (tx, _rx) = broadcast::channel(10_000);
    let cfg = CollectorConfig {
        sessions: Some(store.clone()),
        ..Default::default()
    };
    let factory: SourceFactory = Box::new(|| {
        Ok(Box::new(SynthSource::new(
            SynthConfig {
                laps: 3,
                balance: Some(Default::default()),
                ..Default::default()
            },
            0.0,
        )) as Box<dyn TelemetrySource>)
    });
    spawn_collector(factory, st, tx, cfg, Arc::new(AtomicBool::new(false)))
        .unwrap()
        .join()
        .unwrap();
    store
}

fn ask(store: &SessionStore, sql: &str) -> QueryReport {
    lake::query(store.dir(), sql).unwrap_or_else(|e| panic!("{sql}: {e:#}"))
}

fn number(r: &QueryReport, row: usize, col: usize) -> f64 {
    r.rows[row][col]
        .as_f64()
        .unwrap_or_else(|| panic!("{:?}", r.rows))
}

#[test]
fn every_frame_every_car_and_every_corner_can_be_asked_about() {
    let dir = tempfile::tempdir().unwrap();
    let store = driven(dir.path());
    let id = store.list()[0].id.clone();
    // The frames of the player, in the session's folder.
    let r = ask(
        &store,
        "SELECT count(*), count(DISTINCT lap), max(speed_kmh), min(session), max(session) FROM telemetry",
    );
    assert!(number(&r, 0, 0) > 1000.0, "{r:?}");
    assert!(number(&r, 0, 1) >= 3.0, "{r:?}");
    assert!(number(&r, 0, 2) > 100.0, "{r:?}");
    assert_eq!(r.rows[0][3], r.rows[0][4]);
    assert_eq!(r.rows[0][3].as_str(), Some(id.as_str()));
    let parts = std::fs::read_dir(lake::folder(store.dir(), &id).join("telemetry")).unwrap();
    assert!(
        parts
            .filter_map(|p| p.ok())
            .all(|p| p.path().extension().is_some_and(|x| x == "parquet"))
    );
    // Every car, ten times a second at the most.
    let r = ask(
        &store,
        "SELECT count(DISTINCT idx), max(n) FROM (SELECT idx, count(*) n, max(t) - min(t) span FROM cars GROUP BY idx)",
    );
    assert_eq!(number(&r, 0, 0), 12.0);
    let r = ask(
        &store,
        "SELECT count(*) / (max(t) - min(t)) FROM cars WHERE idx = 0",
    );
    assert!(number(&r, 0, 0) <= 10.5, "{r:?}");
    // The session files: the session, the laps of everyone, the events.
    let r = ask(
        &store,
        "SELECT id, laps_completed, best_lap_s > 0 FROM sessions",
    );
    assert_eq!(
        (r.rows[0][0].as_str(), number(&r, 0, 1)),
        (Some(id.as_str()), 3.0)
    );
    let r = ask(
        &store,
        "SELECT count(DISTINCT car_number), count(*), count(*) FILTER (is_player), count(DISTINCT driver) FROM laps WHERE NOT pit",
    );
    assert_eq!(number(&r, 0, 0), 12.0);
    // The driver's own laps say so, and everyone has a name.
    assert_eq!((number(&r, 0, 2), number(&r, 0, 3)), (3.0, 12.0), "{r:?}");
    let r = ask(
        &store,
        "SELECT count(*) FROM events WHERE type = 'lap_completed'",
    );
    assert!(number(&r, 0, 0) >= 3.0, "{r:?}");
}

#[test]
fn only_questions_are_answered_and_answers_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path());
    // Nothing kept yet: empty tables, not errors.
    for t in ["telemetry", "cars", "corners", "sessions", "laps", "events"] {
        let r = ask(&store, &format!("SELECT count(*) FROM {t}"));
        assert_eq!(number(&r, 0, 0), 0.0, "{t}");
    }
    for bad in [
        "DROP TABLE laps",
        "COPY laps TO 'x.csv'",
        "SELECT 1; DROP TABLE laps",
        "SELECT ';'; DROP TABLE laps",
        "INSERT INTO laps VALUES (1)",
        "",
    ] {
        assert!(lake::query(store.dir(), bad).is_err(), "{bad}");
    }
    let r = ask(&store, "SELECT * FROM range(1000)");
    assert_eq!((r.rows.len(), r.truncated), (lake::ROWS, true));
    assert!(
        r.to_text()
            .ends_with("(only the first 200 rows; aggregate or LIMIT)\n")
    );
    let r = ask(
        &store,
        "WITH x AS (SELECT 1.5 a, 'b' s, NULL n, true t) SELECT * FROM x;",
    );
    assert_eq!(r.columns, ["a", "s", "n", "t"]);
    assert_eq!(r.to_text(), "a | s | n | t\n1.5 | b |  | true\n");
    // A `;` in a string is no second statement.
    let r = ask(&store, "SELECT 'a; b' s, \"x;y\" FROM (SELECT 1 \"x;y\")");
    assert_eq!(r.rows[0][0], "a; b");
    let none = ask(&store, "SELECT 1 WHERE false");
    assert!(none.to_text().ends_with("(no rows)\n"));
    assert!(lake::query(store.dir(), "SELECT * FROM nowhere").is_err());
}

/// A lap of Suzuka that slows for the hairpin and for Spoon.
fn suzuka_lap(lap: i32, slow: f64) -> LapTrace {
    let v = |i: usize| {
        let dip = |c: f64, w: f64, d: f64| {
            let x = (i as f64 / TRACE_POINTS as f64 - c) / w;
            d * (-x * x).exp()
        };
        250.0 - dip(0.505, 0.02, 170.0 + slow) - dip(0.684, 0.02, 110.0)
    };
    let mut out = LapTrace {
        lap,
        length_m: 5756.0,
        ..Default::default()
    };
    let mut t = 0.0;
    for i in 0..TRACE_POINTS {
        out.t_ms.push((t * 1000.0) as u32);
        out.speed.push((v(i) * 10.0) as u16);
        out.brake.push(if (470..500).contains(&i) { 80 } else { 0 });
        out.throttle
            .push(if (470..510).contains(&i) { 0 } else { 100 });
        out.gear.push(if (480..520).contains(&i) { 1 } else { 5 });
        t += 5.756 / (v(i) / 3.6);
    }
    out.time_s = t;
    out
}

#[test]
fn each_corner_of_each_lap_is_kept_by_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path());
    let mut w = lake::Writer::new().unwrap();
    let track = Some("Suzuka International Racing Course - Grand Prix");
    for (lap, slow) in [(2, 0.0), (3, 8.0)] {
        w.lap(store.dir(), "s1", &suzuka_lap(lap, slow), track)
            .unwrap();
    }
    let r = ask(
        &store,
        "SELECT lap, corner, round(min_kmh) v, gear, brake_before_m, min_throttle_pct, time_s > 0 FROM corners WHERE corner IN ('ヘアピン', 'スプーン') ORDER BY corner, lap",
    );
    let rows: Vec<String> = r.rows.iter().map(|r| format!("{r:?}")).collect();
    assert_eq!(r.rows.len(), 4, "{rows:?}");
    assert_eq!(r.rows[0][1], "スプーン");
    assert_eq!(number(&r, 0, 2), 140.0);
    assert_eq!(r.rows[2][1], "ヘアピン");
    // Slower by 8 km/h on lap 3, first gear, braking before the slowest point.
    assert_eq!(number(&r, 2, 2) - number(&r, 3, 2), 8.0);
    assert_eq!(number(&r, 2, 3), 1.0);
    assert!(
        number(&r, 2, 4) > 0.0 && number(&r, 2, 5) == 0.0,
        "{rows:?}"
    );
    assert!(
        r.rows[1][4].is_null() && number(&r, 1, 5) == 100.0,
        "{rows:?}"
    );
    assert!(r.rows.iter().all(|r| r[6] == true));
    // Thirteen places of Suzuka and the straight before them, every lap:
    // together the lap.
    let r = ask(
        &store,
        "SELECT lap, count(*), abs(sum(time_s) - any_value(lap_time_s)) < 0.01 FROM corners GROUP BY lap ORDER BY lap",
    );
    assert_eq!((number(&r, 0, 1), number(&r, 1, 1)), (14.0, 14.0));
    assert!(r.rows.iter().all(|r| r[2] == true), "{:?}", r.rows);
    let r = ask(
        &store,
        "SELECT corner, min_throttle_pct FROM corners WHERE number = 0 LIMIT 1",
    );
    assert_eq!(
        (r.rows[0][0].as_str(), number(&r, 0, 1)),
        (Some(lake::START_STRAIGHT), 100.0)
    );
    // A lap without corners writes nothing.
    w.lap(store.dir(), "s1", &LapTrace::default(), track)
        .unwrap();
}

/// What the agent is shown works as shown.
#[test]
fn the_examples_of_the_schema_run() {
    let empty = tempfile::tempdir().unwrap();
    let full = tempfile::tempdir().unwrap();
    let driven = driven(full.path());
    let examples: Vec<String> = lake::SCHEMA
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("SELECT") || l.starts_with("WITH"))
        .map(|l| l.replace("<id from sessions>", "x"))
        .collect();
    assert!(examples.len() >= 7, "{examples:?}");
    for sql in &examples {
        ask(&SessionStore::new(empty.path()), sql);
        ask(&driven, sql);
    }
}

/// The summaries agree with the rows they come from.
#[test]
fn the_summaries_are_worked_out_right() {
    let dir = tempfile::tempdir().unwrap();
    let store = driven(dir.path());
    // Every driver once, the driver asking among them; laps 2 and 3 count.
    let r = ask(
        &store,
        "SELECT count(*), count(*) FILTER (is_player), max(laps), bool_and(best_s <= median_s) FROM lap_stats",
    );
    assert_eq!(
        (number(&r, 0, 0), number(&r, 0, 1), number(&r, 0, 2)),
        (12.0, 1.0, 2.0),
        "{r:?}"
    );
    assert_eq!(r.rows[0][3], true);
    let r = ask(
        &store,
        "SELECT abs(s.best_s - (SELECT min(time_s) FROM laps l WHERE l.is_player AND l.lap > 1 AND NOT l.pit)) < 1e-9 FROM lap_stats s WHERE is_player",
    );
    assert_eq!(r.rows[0][0], true);
    // Each event on the driver's lap it happened in.
    let r = ask(
        &store,
        "SELECT bool_and(lap IS NOT NULL) FROM events WHERE type = 'lap_completed'",
    );
    assert_eq!(r.rows[0][0], true);
    // Corners: what was lost against the best, over the laps.
    let mut w = lake::Writer::new().unwrap();
    let track = Some("Suzuka International Racing Course - Grand Prix");
    for (lap, slow) in [(2, 0.0), (3, 8.0), (4, 4.0)] {
        w.lap(store.dir(), "s1", &suzuka_lap(lap, slow), track)
            .unwrap();
    }
    let r = ask(
        &store,
        "SELECT laps, round(lost_s, 6) = round((SELECT sum(time_s) - 3 * min(time_s) FROM corners WHERE session = 's1' AND corner = 'ヘアピン'), 6), min_kmh_high - min_kmh_low, flat_out FROM corner_stats WHERE session = 's1' AND corner = 'ヘアピン'",
    );
    assert_eq!(number(&r, 0, 0), 3.0, "{r:?}");
    assert_eq!(r.rows[0][1], true);
    assert_eq!(number(&r, 0, 2).round(), 8.0);
    assert_eq!(r.rows[0][3], false);
    let r = ask(
        &store,
        "SELECT flat_out FROM corner_stats WHERE session = 's1' AND corner = '130R'",
    );
    assert_eq!(r.rows[0][0], true);
}
