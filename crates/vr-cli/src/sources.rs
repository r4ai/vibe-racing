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
    /// Replay an NDJSON file recorded with `vr record` / `vr serve --record`.
    Replay,
    /// A generated demo race (no game needed).
    Synthetic,
}

#[derive(Debug, Clone, Args)]
pub struct SourceArgs {
    /// Telemetry source. Without it, query commands use a running `vr serve`
    /// and fall back to a one-shot iRacing read.
    #[arg(long, value_enum, global = true)]
    pub game: Option<Game>,
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

    /// Where the game leaves telemetry files behind.
    pub fn recordings(&self) -> Option<PathBuf> {
        match self.game() {
            Game::Iracing => Some(vr_iracing::telemetry_dir(&dirs::document_dir()?)),
            Game::Replay | Game::Synthetic => None,
        }
    }

    pub fn source_kind(&self, live: bool) -> SourceKind {
        match self.game() {
            Game::Iracing if live => SourceKind::Live,
            Game::Iracing => SourceKind::Oneshot,
            Game::Replay => SourceKind::Replay,
            Game::Synthetic => SourceKind::Synthetic,
        }
    }

    pub fn game_id(&self) -> &'static str {
        match self.game() {
            Game::Iracing => "iracing",
            Game::Replay => "replay",
            Game::Synthetic => "synthetic",
        }
    }

    /// `default_speed` is used when `--speed` is not given.
    pub fn factory(&self, default_speed: f64) -> anyhow::Result<SourceFactory> {
        let speed = self.speed.unwrap_or(default_speed);
        Ok(match self.game() {
            Game::Iracing => Box::new(|| {
                vr_iracing::open_live().map(|s| Box::new(s) as Box<dyn TelemetrySource>)
            }),
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
