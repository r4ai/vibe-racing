use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Extras, Frame};

/// What a game adapter is able to provide. Consumers use this to tell the
/// user "not available in this game" rather than guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Capabilities {
    pub multi_car: bool,
    pub gaps: bool,
    pub fuel: bool,
    pub incidents: bool,
    pub weather: bool,
    pub flags: bool,
}

impl Capabilities {
    pub const ALL: Self = Self {
        multi_car: true,
        gaps: true,
        fuel: true,
        incidents: true,
        weather: true,
        flags: true,
    };
}

#[derive(Debug)]
pub enum Poll {
    /// A new sample.
    Frame(Box<Frame>),
    /// Connected but nothing new arrived within the timeout.
    Idle,
    /// The game is not running / not in a session, or the stream ended.
    Disconnected,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("game is not running: {0}")]
    NotRunning(String),
    #[error("not supported on this platform: {0}")]
    Unsupported(String),
    #[error("telemetry format error: {0}")]
    Format(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A live (or replayed) stream of telemetry from one game.
pub trait TelemetrySource: Send {
    /// Stable adapter id, e.g. `"iracing"`.
    fn game(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    /// Wait up to `timeout` for the next sample.
    fn poll(&mut self, timeout: Duration) -> Result<Poll, SourceError>;
    /// What is too big or too slow for every frame, as of the last sample.
    /// Called about once a second.
    fn extras(&mut self) -> Option<Extras> {
        None
    }
    /// Whether reopening after `Disconnected` makes sense (true for live games,
    /// false for finite replays).
    fn reconnectable(&self) -> bool {
        true
    }
}

/// Opens a fresh [`TelemetrySource`]. Used by long-running collectors to
/// reconnect after the game restarts.
pub type SourceFactory = Box<dyn FnMut() -> Result<Box<dyn TelemetrySource>, SourceError> + Send>;
