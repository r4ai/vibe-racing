//! Game adapter registry. Adding a game = adding a variant here plus a
//! `TelemetrySource` implementation crate.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, ValueEnum};
use vr_core::{Capabilities, Extras, Frame, Poll, SourceError, SourceFactory, TelemetrySource};
use vr_engine::SourceKind;
use vr_replay::{ReplaySource, SynthConfig, SynthSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Game {
    /// iRacing via its shared-memory telemetry (Windows).
    Iracing,
    /// Automobilista 2 via its shared memory in "Project CARS 2" mode
    /// (Windows).
    Ams2,
    /// Whichever live game is in a session (the default).
    #[value(skip)]
    Auto,
    /// Replay an NDJSON file recorded with `vr record` / `vr serve --record`.
    Replay,
    /// A generated demo race (no game needed).
    Synthetic,
}

#[derive(Debug, Clone, Args)]
pub struct SourceArgs {
    /// Telemetry source. Without it, query commands use a running `vr serve`
    /// and fall back to a one-shot read of whichever live game is running.
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
            (None, None) => Game::Auto,
        }
    }

    /// Where the game leaves telemetry files behind.
    pub fn recordings(&self) -> Option<PathBuf> {
        match self.game() {
            Game::Iracing | Game::Auto => Some(vr_iracing::telemetry_dir(&dirs::document_dir()?)),
            // Automobilista 2 leaves no telemetry files.
            Game::Ams2 | Game::Replay | Game::Synthetic => None,
        }
    }

    pub fn source_kind(&self, live: bool) -> SourceKind {
        match self.game() {
            Game::Iracing | Game::Ams2 | Game::Auto if live => SourceKind::Live,
            Game::Iracing | Game::Ams2 | Game::Auto => SourceKind::Oneshot,
            Game::Replay => SourceKind::Replay,
            Game::Synthetic => SourceKind::Synthetic,
        }
    }

    pub fn game_id(&self) -> &'static str {
        match self.game() {
            Game::Iracing => "iracing",
            Game::Ams2 => "ams2",
            Game::Auto => "auto",
            Game::Replay => "replay",
            Game::Synthetic => "synthetic",
        }
    }

    /// `default_speed` is used when `--speed` is not given.
    pub fn factory(&self, default_speed: f64) -> anyhow::Result<SourceFactory> {
        let speed = self.speed.unwrap_or(default_speed);
        Ok(match self.game() {
            Game::Iracing => Box::new(open_iracing),
            Game::Ams2 => Box::new(open_ams2),
            Game::Auto => Box::new(open_any),
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

    /// A live game, as opposed to a recording or a demo race.
    pub fn live_game(&self) -> bool {
        matches!(self.game(), Game::Iracing | Game::Ams2 | Game::Auto)
    }
}

fn open_iracing() -> Result<Box<dyn TelemetrySource>, SourceError> {
    vr_iracing::open_live().map(|s| Box::new(s) as Box<dyn TelemetrySource>)
}

fn open_ams2() -> Result<Box<dyn TelemetrySource>, SourceError> {
    vr_ams2::open_live().map(|s| Box::new(s) as Box<dyn TelemetrySource>)
}

/// How long each game gets to show a sample when looking for the one in a
/// session.
const PROBE: Duration = Duration::from_millis(100);

/// The first live game that is in a session; else one that is running; else
/// why none could be opened.
fn open_any() -> Result<Box<dyn TelemetrySource>, SourceError> {
    first_live(&mut [&mut open_iracing, &mut open_ams2], PROBE)
}

type Opener<'a> = &'a mut dyn FnMut() -> Result<Box<dyn TelemetrySource>, SourceError>;

pub(crate) fn first_live(
    openers: &mut [Opener<'_>],
    probe: Duration,
) -> Result<Box<dyn TelemetrySource>, SourceError> {
    let mut waiting: Option<Box<dyn TelemetrySource>> = None;
    let mut idle: Option<Box<dyn TelemetrySource>> = None;
    let mut errors = Vec::new();
    for open in openers.iter_mut() {
        match open() {
            Err(e) => errors.push(e),
            Ok(mut s) => match s.poll(probe) {
                Ok(Poll::Frame(f)) => {
                    return Ok(Box::new(Primed {
                        inner: s,
                        first: Some(f),
                    }));
                }
                Ok(Poll::Idle) => {
                    idle.get_or_insert(s);
                }
                Ok(Poll::Disconnected) => {
                    waiting.get_or_insert(s);
                }
                Err(e) => errors.push(e),
            },
        }
    }
    if let Some(s) = idle.or(waiting) {
        return Ok(s);
    }
    let text = errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    Err(
        if errors
            .iter()
            .all(|e| matches!(e, SourceError::Unsupported(_)))
        {
            SourceError::Unsupported(text)
        } else {
            SourceError::NotRunning(text)
        },
    )
}

/// A source with the sample it was chosen by still to give.
struct Primed {
    inner: Box<dyn TelemetrySource>,
    first: Option<Box<Frame>>,
}

impl TelemetrySource for Primed {
    fn game(&self) -> &str {
        self.inner.game()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn poll(&mut self, timeout: Duration) -> Result<Poll, SourceError> {
        match self.first.take() {
            Some(f) => Ok(Poll::Frame(f)),
            None => self.inner.poll(timeout),
        }
    }

    fn extras(&mut self) -> Option<Extras> {
        self.inner.extras()
    }

    fn reconnectable(&self) -> bool {
        self.inner.reconnectable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game that answers the first poll with `first`.
    struct Fake {
        name: &'static str,
        first: Option<Result<Poll, SourceError>>,
    }

    impl TelemetrySource for Fake {
        fn game(&self) -> &str {
            self.name
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                incidents: self.name == "a",
                ..Capabilities::default()
            }
        }
        fn poll(&mut self, _timeout: Duration) -> Result<Poll, SourceError> {
            self.first.take().unwrap_or(Ok(Poll::Idle))
        }
        fn extras(&mut self) -> Option<Extras> {
            Some(Extras::default())
        }
        fn reconnectable(&self) -> bool {
            self.name != "finite"
        }
    }

    fn frame() -> Box<Frame> {
        let mut src = SynthSource::new(SynthConfig::default(), 0.0);
        match src.poll(Duration::ZERO).unwrap() {
            Poll::Frame(f) => f,
            other => panic!("{other:?}"),
        }
    }

    fn game(
        name: &'static str,
        first: Result<Poll, SourceError>,
    ) -> impl FnMut() -> Result<Box<dyn TelemetrySource>, SourceError> {
        let mut first = Some(first);
        move || {
            Ok(Box::new(Fake {
                name,
                first: first.take(),
            }))
        }
    }

    fn missing(e: SourceError) -> impl FnMut() -> Result<Box<dyn TelemetrySource>, SourceError> {
        let mut e = Some(e);
        move || Err(e.take().unwrap())
    }

    fn pick(openers: &mut [Opener<'_>]) -> Result<Box<dyn TelemetrySource>, SourceError> {
        first_live(openers, Duration::ZERO)
    }

    #[test]
    fn the_game_in_a_session_wins() {
        let (mut a, mut b) = (
            game("a", Ok(Poll::Disconnected)),
            game("b", Ok(Poll::Frame(frame()))),
        );
        let mut s = pick(&mut [&mut a, &mut b]).unwrap();
        assert_eq!(s.game(), "b");
        assert!(!s.capabilities().incidents);
        assert!(
            matches!(s.poll(Duration::ZERO).unwrap(), Poll::Frame(_)),
            "the probe's sample"
        );
        assert!(matches!(s.poll(Duration::ZERO).unwrap(), Poll::Idle));
        assert_eq!(s.extras(), Some(Extras::default()));
        assert!(s.reconnectable());

        let (mut a, mut b) = (
            game("a", Ok(Poll::Frame(frame()))),
            game("b", Ok(Poll::Frame(frame()))),
        );
        let s = pick(&mut [&mut a, &mut b]).unwrap();
        assert_eq!(s.game(), "a", "in order");
        assert!(s.capabilities().incidents);
        let mut f = game("finite", Ok(Poll::Frame(frame())));
        assert!(!pick(&mut [&mut f]).unwrap().reconnectable());
    }

    #[test]
    fn running_beats_not_in_a_session() {
        let (mut a, mut b) = (game("a", Ok(Poll::Disconnected)), game("b", Ok(Poll::Idle)));
        assert_eq!(pick(&mut [&mut a, &mut b]).unwrap().game(), "b");
        let (mut a, mut b) = (
            game("a", Ok(Poll::Disconnected)),
            missing(SourceError::NotRunning("b".into())),
        );
        assert_eq!(pick(&mut [&mut a, &mut b]).unwrap().game(), "a");
        let (mut a, mut b) = (
            game("a", Ok(Poll::Disconnected)),
            game("b", Ok(Poll::Disconnected)),
        );
        assert_eq!(pick(&mut [&mut a, &mut b]).unwrap().game(), "a");
    }

    #[test]
    fn says_why_nothing_opened() {
        let (mut a, mut b) = (
            missing(SourceError::Unsupported("x".into())),
            missing(SourceError::Unsupported("y".into())),
        );
        let e = pick(&mut [&mut a, &mut b]).err().unwrap();
        assert!(
            matches!(&e, SourceError::Unsupported(m) if m.contains("x") && m.contains("; ")),
            "{e}"
        );
        let (mut a, mut b) = (
            missing(SourceError::Unsupported("x".into())),
            game("b", Err(SourceError::Format("torn".into()))),
        );
        let e = pick(&mut [&mut a, &mut b]).err().unwrap();
        assert!(
            matches!(&e, SourceError::NotRunning(m) if m.contains("torn")),
            "{e}"
        );
    }

    #[test]
    fn games_and_kinds() {
        let args = |game: Option<Game>| SourceArgs {
            game,
            file: None,
            speed: None,
            until: None,
        };
        assert_eq!(args(None).game(), Game::Auto);
        assert_eq!(args(None).game_id(), "auto");
        assert_eq!(args(Some(Game::Ams2)).game_id(), "ams2");
        assert_eq!(args(Some(Game::Ams2)).source_kind(true), SourceKind::Live);
        assert_eq!(
            args(Some(Game::Ams2)).source_kind(false),
            SourceKind::Oneshot
        );
        assert_eq!(args(None).source_kind(true), SourceKind::Live);
        assert_eq!(
            args(Some(Game::Ams2)).recordings(),
            None,
            "AMS2 leaves no files"
        );
        assert!(args(Some(Game::Ams2)).live_game());
        assert!(args(None).live_game());
        assert!(!args(Some(Game::Synthetic)).live_game());
        assert!(!args(Some(Game::Replay)).live_game());
        assert!(Game::from_str("ams2", true).is_ok());
        assert!(Game::from_str("auto", true).is_err(), "not offered");
    }
}
