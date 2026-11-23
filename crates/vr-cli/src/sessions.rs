//! `vr sessions`: past sessions.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use vr_engine::{CornersReport, LapRecord, RaceState, SessionRecord, SourceKind, render};
use vr_server::{FindError, SessionStore, read_sessions, sessions_dir};

use crate::{EXIT_NOT_FOUND, Format};

#[derive(Debug, Clone, Subcommand)]
pub enum SessionsCmd {
    /// Kept sessions, newest first.
    List {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Only sessions whose track contains this.
        #[arg(long)]
        track: Option<String>,
        /// Only sessions whose car contains this.
        #[arg(long)]
        car: Option<String>,
        /// Only this kind: practice, qualify, warmup, race.
        #[arg(long)]
        kind: Option<String>,
    },
    /// One session: result, laps, events, adjustments.
    Show {
        /// Id, the beginning of one, `last`, or a number (1 is the newest).
        session: String,
    },
    /// The garage setup of a session.
    Setup { session: String },
    /// What a run tells about the setup: balance, what the wheels did, what to
    /// change and what earlier changes did.
    Analyse {
        /// Default: the newest run in which the wheels were measured.
        session: Option<String>,
    },
    /// One lap against the best, corner by corner: braking, slowest speed,
    /// throttle, and where time is usually lost.
    Corners {
        /// Default: the newest with laps that were recorded.
        session: Option<String>,
        /// Default: the last one at racing speed.
        #[arg(long)]
        lap: Option<i32>,
        /// Default: the fastest of the session.
        #[arg(long)]
        against: Option<i32>,
    },
    /// Fill the corner table of `vr query` from the laps kept before it was
    /// written live.
    Lake,
    /// Two sessions side by side, with what differs in the setup.
    Compare { a: String, b: String },
    /// Write a note on a session (empty removes it).
    Note { session: String, note: String },
    /// Delete a session for good.
    Delete {
        session: String,
        /// Really delete it.
        #[arg(long)]
        yes: bool,
    },
    /// Write a session as JSON to a file.
    Export {
        session: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Add sessions from iRacing telemetry files (.ibt).
    Import {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Samples per second to read.
        #[arg(long, default_value_t = 60.0)]
        hz: f64,
    },
    /// The directory sessions are kept in.
    Dir,
}

fn print(fmt: Format, r: &SessionRecord) -> anyhow::Result<()> {
    match fmt {
        Format::Text => print!("{}", r.to_text()),
        Format::Json => println!("{}", serde_json::to_string(r)?),
    }
    Ok(())
}

pub fn run(cmd: SessionsCmd, fmt: Format) -> anyhow::Result<ExitCode> {
    let store = SessionStore::new(sessions_dir());
    match dispatch(&store, cmd, fmt) {
        Err(e) if e.downcast_ref::<FindError>().is_some() => {
            eprintln!("error: {e}");
            Ok(ExitCode::from(EXIT_NOT_FOUND))
        }
        other => other.map(|()| ExitCode::SUCCESS),
    }
}

fn contains(hay: Option<&str>, needle: Option<&String>) -> bool {
    needle.is_none_or(|n| hay.is_some_and(|h| h.to_lowercase().contains(&n.to_lowercase())))
}

fn kind_name(r: &SessionRecord) -> String {
    serde_json::to_value(r.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn dispatch(store: &SessionStore, cmd: SessionsCmd, fmt: Format) -> anyhow::Result<()> {
    match cmd {
        SessionsCmd::List {
            limit,
            track,
            car,
            kind,
        } => {
            let rows: Vec<SessionRecord> = store
                .list()
                .into_iter()
                .filter(|r| contains(r.track.as_deref(), track.as_ref()))
                .filter(|r| contains(r.car.as_deref(), car.as_ref()))
                .filter(|r| contains(Some(&kind_name(r)), kind.as_ref()))
                .take(limit)
                .collect();
            match fmt {
                Format::Json => println!("{}", serde_json::to_string(&rows)?),
                Format::Text if rows.is_empty() => println!("no sessions"),
                Format::Text => {
                    for (i, r) in rows.iter().enumerate() {
                        println!("{}. {}", i + 1, r.line());
                    }
                }
            }
        }
        SessionsCmd::Show { session } => print(fmt, &store.find(&session)?)?,
        SessionsCmd::Setup { session } => {
            let r = store.find(&session)?;
            match (fmt, &r.setup) {
                (Format::Json, s) => println!("{}", serde_json::to_string(s)?),
                (Format::Text, None) => println!("no setup was kept"),
                (Format::Text, Some(s)) => {
                    println!("{}", s.name.as_deref().unwrap_or("--"));
                    for i in &s.items {
                        println!("  {}: {}", i.path, i.value);
                    }
                }
            }
        }
        SessionsCmd::Analyse { session } => {
            let all = store.list();
            let r = match session {
                Some(key) => store.find(&key)?,
                None => newest_measured(&all)
                    .ok_or_else(|| anyhow::anyhow!("no sessions yet"))?
                    .clone(),
            };
            let a = vr_engine::analyse(&r, &all);
            match fmt {
                Format::Json => println!("{}", serde_json::to_string(&a)?),
                Format::Text => print!("{}", a.to_text()),
            }
        }
        SessionsCmd::Lake => {
            let mut lake = vr_server::lake::Writer::new()?;
            let (mut sessions, mut laps) = (0, 0);
            for r in store.list() {
                let folder = vr_server::lake::folder(store.dir(), &r.id).join("corners");
                let lines = store.traces(&r.id);
                if lines.is_empty() || folder.exists() {
                    continue;
                }
                sessions += 1;
                for line in lines {
                    if let Some(t) = store.trace(&r.id, line.lap) {
                        lake.lap(store.dir(), &r.id, &t, r.track.as_deref())?;
                        laps += 1;
                    }
                }
            }
            println!("corners of {laps} laps in {sessions} sessions");
        }
        SessionsCmd::Corners {
            session,
            lap,
            against,
        } => {
            let c = corners(store, session, lap, against)?;
            match fmt {
                Format::Json => println!("{}", serde_json::to_string(&c)?),
                Format::Text => print!("{}", c.to_text()),
            }
        }
        SessionsCmd::Compare { a, b } => {
            let (a, b) = (store.find(&a)?, store.find(&b)?);
            match fmt {
                Format::Json => println!("{}", serde_json::to_string(&[&a, &b])?),
                Format::Text => print!("{}", compare(&a, &b)),
            }
        }
        SessionsCmd::Note { session, note } => print(fmt, &store.note(&session, &note)?)?,
        SessionsCmd::Delete { session, yes } => {
            let r = store.find(&session)?;
            if !yes {
                anyhow::bail!("would delete {}; repeat with --yes to do it", r.id);
            }
            store.delete(&r.id)?;
            eprintln!("deleted {}", r.id);
        }
        SessionsCmd::Export { session, out } => {
            let r = store.find(&session)?;
            std::fs::write(&out, serde_json::to_vec_pretty(&r)?)?;
            eprintln!("wrote {}", out.display());
        }
        SessionsCmd::Import { files, hz } => {
            let mut failed = false;
            for file in files {
                match import(store, &file, hz) {
                    Ok(ids) if ids.is_empty() => {
                        eprintln!("{}: nothing to keep", file.display());
                    }
                    Ok(ids) => ids.iter().for_each(|id| println!("{id}")),
                    Err(e) => {
                        failed = true;
                        eprintln!("{}: {e:#}", file.display());
                    }
                }
            }
            if failed {
                anyhow::bail!("not every file could be imported");
            }
        }
        SessionsCmd::Dir => println!("{}", store.dir().display()),
    }
    Ok(())
}

/// The newest session in which the wheels were measured, else the newest.
/// Only of the car and track of the newest one: another car's setup is of no
/// use.
fn newest_measured(all: &[SessionRecord]) -> Option<&SessionRecord> {
    let newest = all.first()?;
    all.iter()
        .filter(|r| r.track == newest.track && r.car == newest.car)
        .find(|r| r.chassis.is_some() && r.laps_completed >= 3)
        .or(Some(newest))
}

pub fn import(
    store: &SessionStore,
    file: &std::path::Path,
    hz: f64,
) -> anyhow::Result<Vec<String>> {
    let mut src = vr_iracing::IbtSource::open(file, hz)?;
    let caps = vr_core::TelemetrySource::capabilities(&src);
    let mut state = RaceState::new("iracing", SourceKind::Replay, caps);
    read_sessions(&mut src, &mut state)?
        .into_iter()
        .map(|(r, laps)| {
            let id = store.save(r)?;
            for lap in &laps {
                store.save_trace(&id, lap)?;
            }
            Ok(id)
        })
        .collect()
}

/// Laps that tell what is usual.
const USUAL: usize = 5;
/// Slower laps than this, against the fastest, were not driven for time.
const FOR_TIME: f64 = 1.07;

/// A session saved this long ago is still being driven.
const LIVE_MS: i64 = 120_000;

/// The corners of the session being driven, when two laps of it were kept.
pub fn live_corners() -> Option<CornersReport> {
    let store = SessionStore::new(vr_server::sessions_dir());
    let now = store.list().into_iter().next()?;
    (vr_engine::now_ms() - now.updated_ms < LIVE_MS)
        .then(|| corners(&store, Some(now.id), None, None).ok())
        .flatten()
}

fn corners(
    store: &SessionStore,
    session: Option<String>,
    lap: Option<i32>,
    against: Option<i32>,
) -> anyhow::Result<CornersReport> {
    let none = || anyhow::anyhow!("no two laps were recorded in that session yet");
    let id = match session {
        Some(key) => store.find(&key)?.id,
        None => {
            store
                .list()
                .into_iter()
                .find(|r| store.traces(&r.id).iter().filter(|t| !t.pit).count() >= 2)
                .ok_or_else(none)?
                .id
        }
    };
    let all = store.traces(&id);
    let clean = || all.iter().filter(|t| !t.pit);
    let fastest = clean()
        .min_by(|a, b| a.time_s.total_cmp(&b.time_s))
        .ok_or_else(none)?;
    let reference = against.unwrap_or(fastest.lap);
    let last = match lap {
        Some(l) => l,
        None => {
            clean()
                .rev()
                .find(|t| t.lap != reference && t.time_s <= fastest.time_s * FOR_TIME)
                .ok_or_else(none)?
                .lap
        }
    };
    let load = |l: i32| {
        store
            .trace(&id, l)
            .ok_or_else(|| anyhow::anyhow!("lap {l} of {id} was not recorded"))
    };
    let mut laps: Vec<i32> = clean()
        .filter(|t| t.lap < last && t.time_s <= fastest.time_s * FOR_TIME)
        .map(|t| t.lap)
        .collect();
    laps.drain(..laps.len().saturating_sub(USUAL - 1));
    laps.push(last);
    let laps = laps
        .into_iter()
        .map(load)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let reference = load(reference)?;
    let track = store.find(&id).ok().and_then(|r| r.track);
    let bends = track
        .as_deref()
        .and_then(|t| vr_engine::tracks::bends(t, reference.length_m));
    CornersReport::on(&laps, &reference, bends).ok_or_else(none)
}

fn lap(v: Option<f64>) -> String {
    v.map(render::fmt_lap).unwrap_or_else(|| "--".into())
}

fn compare(a: &SessionRecord, b: &SessionRecord) -> String {
    use std::fmt::Write as _;
    let mut out = format!("A {}\nB {}\n", a.line(), b.line());
    let delta = |x: Option<f64>, y: Option<f64>| {
        x.zip(y)
            .map(|(x, y)| format!("{:+.3}s", y - x))
            .unwrap_or_else(|| "--".into())
    };
    let _ = writeln!(
        out,
        "best: {} / {} (B {})",
        lap(a.best_lap_s()),
        lap(b.best_lap_s()),
        delta(a.best_lap_s(), b.best_lap_s())
    );
    let _ = writeln!(
        out,
        "clean average: {} / {} (B {})",
        lap(a.avg_clean_lap_s()),
        lap(b.avg_clean_lap_s()),
        delta(a.avg_clean_lap_s(), b.avg_clean_lap_s())
    );
    if let (Some(x), Some(y)) = (a.fuel_per_lap_l(), b.fuel_per_lap_l()) {
        let _ = writeln!(out, "fuel per lap: {x:.2}L / {y:.2}L");
    }
    let find = |laps: &[LapRecord], n: i32| laps.iter().find(|l| l.lap == n).and_then(|l| l.time_s);
    let last = a.laps.iter().chain(&b.laps).map(|l| l.lap).max();
    if let Some(last) = last {
        out.push_str("laps (A / B):\n");
        for n in 1..=last {
            let (x, y) = (find(&a.laps, n), find(&b.laps, n));
            if x.is_some() || y.is_some() {
                let _ = writeln!(out, "  {n}: {} / {} ({})", lap(x), lap(y), delta(x, y));
            }
        }
    }
    if let (Some(sa), Some(sb)) = (&a.setup, &b.setup) {
        let mut diff = Vec::new();
        for i in &sb.items {
            let was = sa.items.iter().find(|o| o.path == i.path);
            if was.is_none_or(|w| w.value != i.value) {
                let was = was.map_or("--", |w| w.value.as_str());
                diff.push(format!("  {}: {was} -> {}", i.path, i.value));
            }
        }
        if diff.is_empty() {
            out.push_str("setup: the same\n");
        } else {
            let _ = writeln!(out, "setup differences (A -> B):\n{}", diff.join("\n"));
        }
    }
    out
}
