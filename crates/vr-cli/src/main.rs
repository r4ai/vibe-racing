//! `vr` — race-engineer telemetry CLI for AI agents.

mod client;
mod doctor;
mod logging;
mod sessions;
mod skill;
mod sources;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use vr_core::Poll;
use vr_engine::*;

use crate::client::NoData;
use crate::sources::SourceArgs;

const EXIT_NO_DATA: u8 = 3;
const EXIT_NOT_FOUND: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// Compact plain text tuned for agents answering on the radio.
    Text,
    /// Machine-readable JSON (one object).
    Json,
}

#[derive(Debug, Parser)]
#[command(
    name = "vr",
    version,
    about = "Race-engineer telemetry for AI agents (iRacing and more)"
)]
#[command(
    long_about = "Reads live race telemetry and answers the questions a race engineer gets on the radio.\n\n\
Start `vr serve` once per racing session so lap history, fuel averages and gap trends are tracked; \
query commands then read from it instantly. Without a server, query commands take a one-shot \
sample from the game (no history).\n\nTry it without a game: `vr serve --game synthetic --speed 10`."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// Output format.
    #[arg(long, short, value_enum, default_value_t = Format::Text, global = true)]
    format: Format,
    /// URL of a running `vr serve` (default: $VR_SERVER or auto-discovered).
    #[arg(long, global = true, env = "VR_SERVER")]
    server: Option<String>,
    #[command(flatten)]
    src: SourceArgs,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run the live collector + local HTTP API (keep this running while racing).
    Serve {
        /// Port on 127.0.0.1.
        #[arg(long, default_value_t = vr_server::DEFAULT_PORT)]
        port: u16,
        /// Also record normalized telemetry to this NDJSON file.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Recording rate (frames per second of session time).
        #[arg(long, default_value_t = 10.0)]
        record_hz: f64,
        /// Do nothing if a server already answers.
        #[arg(long)]
        if_needed: bool,
        /// Do not keep sessions (`vr sessions`).
        #[arg(long)]
        no_sessions: bool,
    },
    /// Everything a race engineer needs in one call: position, gaps, pace, fuel, flags, events.
    Brief,
    /// Standings table.
    Standings {
        /// Only the first N rows.
        #[arg(long)]
        top: Option<usize>,
        /// Only N rows either side of you.
        #[arg(long)]
        around: Option<usize>,
        /// Overall instead of class standings (multi-class).
        #[arg(long)]
        overall: bool,
    },
    /// Gaps to the cars ahead/behind, nearby traffic, or a specific car.
    Gap {
        /// Car number to compare against, e.g. 23.
        #[arg(long)]
        car: Option<String>,
        /// Nearby-traffic window in seconds.
        #[arg(long)]
        window: Option<f64>,
    },
    /// Fuel level, consumption per lap, laps remaining and fuel to finish.
    Fuel,
    /// Lap history (yours by default).
    Laps {
        /// Car number (default: you).
        #[arg(long)]
        car: Option<String>,
        /// Only the last N laps.
        #[arg(long, default_value_t = 10)]
        last: usize,
    },
    /// Race events (flags, position changes, pit stops, incidents, laps).
    Events {
        /// Only events newer than this sequence number.
        #[arg(long)]
        since: Option<u64>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Stream events as they happen (one per line) until interrupted.
    Watch,
    /// Full normalized telemetry frame plus metadata (large; prefer `brief`).
    Snapshot,
    /// Your car: tyres, damage, engine, pit service, push-to-pass, in-car settings.
    Car,
    /// Every value the game publishes, under its own name.
    Channels {
        /// Words the name, unit or description must contain.
        filter: Vec<String>,
    },
    /// The game's own description of the session (weekend, drivers, results).
    Info {
        /// E.g. `WeekendInfo` or `DriverInfo/Drivers/0` (default: all of it).
        path: Option<String>,
    },
    /// The garage setup and what can be adjusted while driving.
    Setup,
    /// How the car behaves (under/oversteer, wheelspin, locking) and what to adjust.
    Handling {
        /// Mean over the last N laps.
        #[arg(long, default_value_t = 3)]
        last: usize,
    },
    /// Past sessions: list, show, compare, annotate, import.
    Sessions {
        #[command(subcommand)]
        cmd: sessions::SessionsCmd,
    },
    /// Record normalized telemetry to NDJSON without serving.
    Record {
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 10.0)]
        hz: f64,
    },
    /// Diagnose game connection, server and skill installation.
    Doctor,
    /// Manage the bundled agent skill.
    Skill {
        #[command(subcommand)]
        cmd: skill::SkillCmd,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let verbose = matches!(cli.cmd, Cmd::Serve { .. } | Cmd::Record { .. });
    init_logging(verbose);
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            if e.downcast_ref::<NoData>().is_some() {
                ExitCode::from(EXIT_NO_DATA)
            } else if matches!(e.downcast_ref::<ApiError>(), Some(ApiError::NotFound(_))) {
                ExitCode::from(EXIT_NOT_FOUND)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn init_logging(verbose: bool) {
    let default = if verbose { "info" } else { "warn" };
    let filter = tracing_subscriber::EnvFilter::try_from_env("VR_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(logging::Lines::to(std::io::stderr()))
        .try_init();
}

trait Render: Serialize {
    fn text(&self) -> String;
    fn has_data(&self) -> bool {
        true
    }
}

macro_rules! render {
    ($($t:ty => $has:expr),* $(,)?) => {
        $(impl Render for $t {
            fn text(&self) -> String { self.to_text() }
            fn has_data(&self) -> bool { let f: fn(&$t) -> bool = $has; f(self) }
        })*
    };
}

render! {
    Brief => |r| r.session.is_some(),
    StandingsReport => |r| !r.rows.is_empty(),
    GapReport => |_| true,
    FuelReport => |r| r.fuel.is_some(),
    LapsReport => |r| r.car.is_some(),
    EventsReport => |_| true,
    CarReport => |r| r.car.is_some(),
    ChannelsReport => |r| r.total > 0,
    InfoReport => |r| r.value.is_some(),
    SetupReport => |r| r.setup.is_some() || !r.controls.is_empty(),
    HandlingReport => |r| r.meta.session_time_s.is_some(),
}

impl Render for SnapshotReport {
    fn text(&self) -> String {
        // The frame is too rich for a useful text form; JSON is the text.
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
    fn has_data(&self) -> bool {
        self.frame.is_some()
    }
}

fn emit<T: Render>(fmt: Format, v: ApiResult<T>) -> anyhow::Result<ExitCode> {
    let v = v?;
    match fmt {
        Format::Text => print!("{}", v.text()),
        Format::Json => println!("{}", serde_json::to_string(&v)?),
    }
    Ok(if v.has_data() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_NO_DATA)
    })
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let fmt = cli.format;
    let server = cli.server.as_deref();
    match cli.cmd {
        Cmd::Serve {
            port,
            record,
            record_hz,
            if_needed,
            no_sessions,
        } => {
            if if_needed && let Some(c) = client::find_server(server) {
                eprintln!("already served by {}", c.base());
                return Ok(ExitCode::SUCCESS);
            }
            // Demo races are not the driver's.
            let keep = !no_sessions && cli.src.game() == sources::Game::Iracing;
            serve(&cli.src, port, record.map(|p| (p, record_hz)), keep)
        }
        Cmd::Record { out, hz } => record(&cli.src, out, hz),
        Cmd::Watch => watch(&cli.src, server, fmt),
        Cmd::Doctor => doctor::run(server, fmt),
        Cmd::Skill { cmd } => skill::run(cmd),
        Cmd::Sessions { cmd } => sessions::run(cmd, fmt),
        cmd => {
            let backend = client::resolve(&cli.src, server)?;
            let api = backend.api();
            match cmd {
                Cmd::Brief => emit(fmt, api.brief()),
                Cmd::Standings {
                    top,
                    around,
                    overall,
                } => emit(
                    fmt,
                    api.standings(&StandingsParams {
                        top,
                        around,
                        class_only: overall.then_some(false),
                    }),
                ),
                Cmd::Gap { car, window } => emit(
                    fmt,
                    api.gaps(&GapParams {
                        car,
                        window_s: window,
                    }),
                ),
                Cmd::Fuel => emit(fmt, api.fuel()),
                Cmd::Laps { car, last } => emit(
                    fmt,
                    api.laps(&LapsParams {
                        car,
                        last: Some(last),
                    }),
                ),
                Cmd::Events { since, limit } => emit(
                    fmt,
                    api.events(&EventsParams {
                        since,
                        limit: Some(limit),
                    }),
                ),
                Cmd::Snapshot => emit(fmt, api.snapshot()),
                Cmd::Car => emit(fmt, api.car()),
                Cmd::Channels { filter } => emit(
                    fmt,
                    api.channels(&ChannelsParams {
                        filter: Some(filter.join(" ")),
                    }),
                ),
                Cmd::Info { path } => emit(fmt, api.info(&InfoParams { path })),
                Cmd::Setup => emit(fmt, api.setup()),
                Cmd::Handling { last } => {
                    emit(fmt, api.handling(&HandlingParams { last: Some(last) }))
                }
                _ => unreachable!(),
            }
        }
    }
}

fn serve(
    src: &SourceArgs,
    port: u16,
    record: Option<(PathBuf, f64)>,
    keep: bool,
) -> anyhow::Result<ExitCode> {
    let factory = src.factory(1.0)?;
    let state: SharedState = Arc::new(RwLock::new(RaceState::new(
        src.game_id(),
        src.source_kind(true),
        vr_core::Capabilities::ALL,
    )));
    let (tx, _) = tokio::sync::broadcast::channel(256);
    let stop = Arc::new(AtomicBool::new(false));
    let store = vr_server::SessionStore::new(vr_server::sessions_dir());
    let cfg = vr_server::CollectorConfig {
        record,
        sessions: keep.then(|| store.clone()),
        ..Default::default()
    };
    let collector =
        vr_server::spawn_collector(factory, state.clone(), tx.clone(), cfg, stop.clone())?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let game = src.game_id().to_string();
    let pid = std::process::id();
    let app = vr_server::AppState {
        state,
        events: tx,
        sessions: Some(store),
    };
    let result = rt.block_on(vr_server::serve(
        app,
        addr,
        async {
            let _ = tokio::signal::ctrl_c().await;
        },
        move |bound| {
            let url = format!("http://{bound}");
            let info = vr_server::ServerInfo {
                url: url.clone(),
                pid,
                game,
                started_ms: now_ms(),
            };
            match vr_server::write_discovery(&info) {
                Ok(p) => tracing::info!(
                    "serving on {url} (past sessions: {url}/, discovery: {})",
                    p.display()
                ),
                Err(e) => tracing::warn!("serving on {url}; could not write discovery file: {e}"),
            }
        },
    ));
    stop.store(true, Ordering::Relaxed);
    vr_server::clear_discovery(pid);
    let _ = collector.join();
    result?;
    Ok(ExitCode::SUCCESS)
}

fn record(src: &SourceArgs, out: PathBuf, hz: f64) -> anyhow::Result<ExitCode> {
    let mut factory = src.factory(1.0)?;
    let mut rec = vr_replay::Recorder::create(&out, hz)?;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        std::thread::spawn(move || {
            rt.block_on(async {
                let _ = tokio::signal::ctrl_c().await;
            });
            stop.store(true, Ordering::Relaxed);
        });
    }
    let mut frames = 0u64;
    tracing::info!("recording to {} (Ctrl-C to stop)", out.display());
    'outer: while !stop.load(Ordering::Relaxed) {
        let mut s = match factory() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("waiting for game: {e}");
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            match s.poll(Duration::from_millis(250))? {
                Poll::Frame(f) => {
                    rec.record(&f)?;
                    frames += 1;
                }
                Poll::Idle => {}
                Poll::Disconnected if s.reconnectable() => continue 'outer,
                Poll::Disconnected => break 'outer,
            }
        }
    }
    rec.flush()?;
    eprintln!("recorded {frames} frames to {}", out.display());
    Ok(ExitCode::SUCCESS)
}

fn print_event(fmt: Format, e: &Event) {
    match fmt {
        Format::Text => println!("{}", render::event_to_text(e)),
        Format::Json => println!("{}", serde_json::to_string(e).unwrap_or_default()),
    }
}

fn watch(src: &SourceArgs, server: Option<&str>, fmt: Format) -> anyhow::Result<ExitCode> {
    if !src.explicit()
        && let Some(c) = client::find_server(server)
    {
        eprintln!("watching events from {} (Ctrl-C to stop)", c.base());
        c.stream_events(|e| {
            print_event(fmt, &e);
            true
        })?;
        return Ok(ExitCode::SUCCESS);
    }
    // No server: run the source in-process.
    let mut factory = src.factory(1.0)?;
    let mut st = RaceState::new(
        src.game_id(),
        src.source_kind(true),
        vr_core::Capabilities::ALL,
    );
    let mut s = factory().map_err(|e| anyhow::Error::new(NoData(e.to_string())))?;
    eprintln!("watching {} in-process (Ctrl-C to stop)", s.game());
    loop {
        match s.poll(Duration::from_millis(250))? {
            Poll::Frame(f) => {
                let before = st.event_log().last_seq();
                st.ingest(*f);
                for e in st.event_log().since(before, usize::MAX) {
                    print_event(fmt, &e);
                }
            }
            Poll::Idle => {}
            Poll::Disconnected => break,
        }
    }
    Ok(ExitCode::SUCCESS)
}
