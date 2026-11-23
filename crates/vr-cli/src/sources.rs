//! Game adapter registry. Adding a game = adding a variant here plus a
//! `TelemetrySource` implementation crate.

use std::path::PathBuf;

use clap::{Args, ValueEnum};
use vr_core::{SourceError, SourceFactory, TelemetrySource};
use vr_engine::SourceKind;
use vr_replay::{ReplaySource, SynthConfig, SynthSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Game {
    /// iRacing via its shared-memory telemetry (Windows).
    Iracing,
    /// Assetto Corsa via its shared memory (Windows), with every car when
    /// CrewChief's in-game app runs.
    Ac,
    /// Assetto Corsa's UDP telemetry from the game's PC (`--host`), from any OS.
    AcUdp,
    /// Replay an NDJSON file recorded with `vr record` / `vr serve --record`.
    Replay,
    /// A generated demo race (no game needed).
    Synthetic,
}

#[derive(Debug, Clone, Args)]
pub struct SourceArgs {
    /// Telemetry source. Without it, query commands use a running `vr serve`
    /// and fall back to a one-shot read of iRacing, then of Assetto Corsa.
    #[arg(long, value_enum, global = true)]
    pub game: Option<Game>,
    /// The game's PC for `--game ac-udp` (port 9996 unless given).
    #[arg(long, global = true, default_value = "127.0.0.1")]
    pub host: String,
    /// Recording to replay (implies `--game replay`).
    #[arg(long, global = true)]
    pub file: Option<PathBuf>,
    /// Playback speed for replay/synthetic (0 = as fast as possible).
    #[arg(long, global = true)]
    pub speed: Option<f64>,
    /// Stop replay/synthetic at this session time (seconds) for one-off queries.
    #[arg(long, global = true)]
    pub until: Option<f64>,
}

impl SourceArgs {
    pub fn explicit(&self) -> bool {
        self.game.is_some() || self.file.is_some()
    }

    pub fn game(&self) -> Game {
        match (self.game, &self.file) {
            (Some(g), _) => g,
            (None, Some(_)) => Game::Replay,
            (None, None) => Game::Iracing,
        }
    }

    /// Where the game leaves telemetry files behind, in the user's documents.
    pub fn recordings(&self, documents: Option<&std::path::Path>) -> Option<PathBuf> {
        match self.game() {
            Game::Iracing => Some(vr_iracing::telemetry_dir(documents?)),
            Game::Ac | Game::AcUdp | Game::Replay | Game::Synthetic => None,
        }
    }

    /// Whether `vr serve` keeps the sessions: those of real games, not the
    /// demo or a replay, unless told not to.
    pub fn keeps_sessions(&self, no_sessions: bool) -> bool {
        !no_sessions && matches!(self.game(), Game::Iracing | Game::Ac | Game::AcUdp)
    }

    pub fn source_kind(&self, live: bool) -> SourceKind {
        match self.game() {
            Game::Iracing | Game::Ac | Game::AcUdp if live => SourceKind::Live,
            Game::Iracing | Game::Ac | Game::AcUdp => SourceKind::Oneshot,
            Game::Replay => SourceKind::Replay,
            Game::Synthetic => SourceKind::Synthetic,
        }
    }

    pub fn game_id(&self) -> &'static str {
        match self.game() {
            Game::Iracing => "iracing",
            Game::Ac | Game::AcUdp => vr_ac::GAME,
            Game::Replay => "replay",
            Game::Synthetic => "synthetic",
        }
    }

    /// `default_speed` is used when `--speed` is not given.
    pub fn factory(&self, default_speed: f64) -> anyhow::Result<SourceFactory> {
        let speed = self.speed.unwrap_or(default_speed);
        Ok(match self.game() {
            Game::Iracing if !self.explicit() => Box::new(open_any_live),
            Game::Iracing => Box::new(|| {
                vr_iracing::open_live().map(|s| Box::new(s) as Box<dyn TelemetrySource>)
            }),
            Game::Ac => Box::new(open_ac),
            Game::AcUdp => {
                let host = self.host.clone();
                Box::new(move || {
                    vr_ac::open_udp(&host).map(|s| Box::new(s) as Box<dyn TelemetrySource>)
                })
            }
            Game::Replay => {
                let file = self.file.clone().ok_or_else(|| {
                    anyhow::anyhow!("--game replay needs --file <recording.ndjson>")
                })?;
                Box::new(move || {
                    ReplaySource::open(&file, speed)
                        .map(|s| Box::new(s) as Box<dyn TelemetrySource>)
                })
            }
            Game::Synthetic => {
                Box::new(move || -> Result<Box<dyn TelemetrySource>, SourceError> {
                    Ok(Box::new(SynthSource::new(SynthConfig::default(), speed)))
                })
            }
        })
    }
}

/// A user writes the results of their sessions in their documents.
fn open_ac() -> Result<Box<dyn TelemetrySource>, SourceError> {
    let docs = dirs::document_dir();
    vr_ac::open_live(docs.as_deref()).map(|s| Box::new(s) as Box<dyn TelemetrySource>)
}

/// Whichever game is running: iRacing, else Assetto Corsa.
fn open_any_live() -> Result<Box<dyn TelemetrySource>, SourceError> {
    let iracing = match vr_iracing::open_live() {
        Ok(s) => return Ok(Box::new(s)),
        Err(e) => e,
    };
    let ac = match open_ac() {
        Ok(s) => return Ok(s),
        Err(e) => e,
    };
    Err(either_failed(iracing, ac))
}

/// Unsupported only when neither game could ever run here.
fn either_failed(iracing: SourceError, ac: SourceError) -> SourceError {
    let both = format!("{iracing}; {ac}");
    match (&iracing, &ac) {
        (SourceError::Unsupported(_), SourceError::Unsupported(_)) => {
            SourceError::Unsupported(both)
        }
        _ => SourceError::NotRunning(both),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::path::Path;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        src: SourceArgs,
    }

    fn parse(args: &[&str]) -> SourceArgs {
        Cli::parse_from(std::iter::once("vr").chain(args.iter().copied())).src
    }

    #[test]
    fn assetto_corsa_is_a_live_game() {
        let s = parse(&["--game", "ac"]);
        assert_eq!(s.game(), Game::Ac);
        assert_eq!(s.game_id(), "ac");
        assert_eq!(s.source_kind(true), SourceKind::Live);
        assert_eq!(s.source_kind(false), SourceKind::Oneshot);
        assert_eq!(s.recordings(Some(Path::new("docs"))), None);
        assert!(s.keeps_sessions(false));
        assert!(!s.keeps_sessions(true), "--no-sessions");
        let s = parse(&["--game", "ac-udp", "--host", "gamepc:9000"]);
        assert_eq!((s.game(), s.host.as_str()), (Game::AcUdp, "gamepc:9000"));
        assert_eq!(s.source_kind(true), SourceKind::Live);
        assert!(s.keeps_sessions(false));
        let s = parse(&[]);
        assert_eq!(
            s.recordings(Some(Path::new("docs"))),
            Some(Path::new("docs").join("iRacing").join("telemetry"))
        );
        assert_eq!(s.recordings(None), None, "no documents");
        assert!(s.keeps_sessions(false));
        for demo in [&["--game", "synthetic"][..], &["--file", "x.ndjson"]] {
            let s = parse(demo);
            assert!(!s.keeps_sessions(false), "{demo:?}");
            assert_eq!(s.recordings(Some(Path::new("docs"))), None);
        }
        assert_eq!(
            (s.game(), s.host.as_str(), s.explicit()),
            (Game::Iracing, "127.0.0.1", false)
        );
    }

    #[test]
    fn neither_game_running() {
        let ns = |m: &str| SourceError::NotRunning(m.into());
        let un = |m: &str| SourceError::Unsupported(m.into());
        let e = either_failed(un("a"), un("b"));
        assert!(
            matches!(&e, SourceError::Unsupported(m) if m == "not supported on this platform: a; not supported on this platform: b"),
            "{e}"
        );
        assert!(matches!(
            either_failed(ns("a"), un("b")),
            SourceError::NotRunning(_)
        ));
        assert!(matches!(
            either_failed(un("a"), ns("b")),
            SourceError::NotRunning(_)
        ));
        #[cfg(not(windows))]
        {
            let message = |args: &[&str]| match parse(args).factory(1.0).unwrap()() {
                Err(e) => e.to_string(),
                Ok(_) => panic!("a game here"),
            };
            // Without --game both games are looked for; with it, only one.
            let any = message(&[]);
            assert!(
                any.contains("iRacing") && any.contains("Assetto Corsa"),
                "{any}"
            );
            let one = message(&["--game", "iracing"]);
            assert!(one.contains("iRacing") && !one.contains("Assetto"), "{one}");
            let one = message(&["--game", "ac"]);
            assert!(one.contains("Assetto") && !one.contains("iRacing"), "{one}");
        }
    }
}
