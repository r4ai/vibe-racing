//! `vr sessions`: past sessions.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use vr_engine::{LapRecord, RaceState, SessionRecord, SourceKind, render};
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
        #[arg(long, default_value_t = 30.0)]
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

fn import(store: &SessionStore, file: &std::path::Path, hz: f64) -> anyhow::Result<Vec<String>> {
    let mut src = vr_iracing::IbtSource::open(file, hz)?;
    let caps = vr_core::TelemetrySource::capabilities(&src);
    let mut state = RaceState::new("iracing", SourceKind::Replay, caps);
    read_sessions(&mut src, &mut state)?
        .into_iter()
        .map(|r| store.save(r))
        .collect()
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
