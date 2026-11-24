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
    /// and fall back to a one-shot read of the game in a session: iRacing,
    /// Automobilista 2 or Assetto Corsa.
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
            // Automobilista 2 and Assetto Corsa leave no telemetry files.
            Game::Ams2 | Game::Ac | Game::AcUdp | Game::Replay | Game::Synthetic => None,
        }
    }

    /// The game's recordings that `vr serve` reads in after driving: only
    /// when it keeps the sessions and is told to import.
    pub fn inbox(
        &self,
        keep: bool,
        import: bool,
        documents: Option<&std::path::Path>,
    ) -> Option<PathBuf> {
        self.recordings(documents).filter(|_| keep && import)
    }

    /// Whether `vr serve` keeps the sessions: those of real games, not the
    /// demo or a replay, unless told not to.
    pub fn keeps_sessions(&self, no_sessions: bool) -> bool {
        !no_sessions && self.live_game()
    }

    /// A game being driven, as opposed to a recording or a demo race.
    pub fn live_game(&self) -> bool {
        matches!(
            self.game(),
            Game::Iracing | Game::Ams2 | Game::Ac | Game::AcUdp
        )
    }

    pub fn source_kind(&self, live: bool) -> SourceKind {
        match self.game() {
            Game::Iracing | Game::Ams2 | Game::Ac | Game::AcUdp if live => SourceKind::Live,
            Game::Iracing | Game::Ams2 | Game::Ac | Game::AcUdp => SourceKind::Oneshot,
            Game::Replay => SourceKind::Replay,
            Game::Synthetic => SourceKind::Synthetic,
        }
    }

    pub fn game_id(&self) -> &'static str {
        match self.game() {
            Game::Iracing => "iracing",
            Game::Ams2 => "ams2",
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
            Game::Iracing => Box::new(open_iracing),
            Game::Ams2 => Box::new(open_ams2),
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

fn open_iracing() -> Result<Box<dyn TelemetrySource>, SourceError> {
    vr_iracing::open_live().map(|s| Box::new(s) as Box<dyn TelemetrySource>)
}

fn open_ams2() -> Result<Box<dyn TelemetrySource>, SourceError> {
    vr_ams2::open_live().map(|s| Box::new(s) as Box<dyn TelemetrySource>)
}

/// How long each game gets to show a sample when looking for the one in a
/// session.
const PROBE: Duration = Duration::from_millis(100);

/// The first game that is in a session (iRacing, Automobilista 2, Assetto
/// Corsa); else one that is running; else why none could be opened.
fn open_any_live() -> Result<Box<dyn TelemetrySource>, SourceError> {
    first_live(
        &mut [&mut open_iracing, &mut open_ams2, &mut open_ac],
        PROBE,
    )
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
        let docs = Some(Path::new("docs"));
        assert!(s.inbox(true, true, docs).is_some());
        assert_eq!(s.inbox(false, true, docs), None, "sessions not kept");
        assert_eq!(s.inbox(true, false, docs), None, "--no-import");
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
        #[cfg(not(windows))]
        {
            let message = |args: &[&str]| match parse(args).factory(1.0).unwrap()() {
                Err(e) => e.to_string(),
                Ok(_) => panic!("a game here"),
            };
            // Without --game every game is looked for; with it, only one.
            let any = message(&[]);
            assert!(
                any.contains("iRacing")
                    && any.contains("Automobilista 2")
                    && any.contains("Assetto Corsa"),
                "{any}"
            );
            let one = message(&["--game", "ams2"]);
            assert!(
                one.contains("Automobilista 2") && !one.contains("iRacing"),
                "{one}"
            );
            let one = message(&["--game", "iracing"]);
            assert!(one.contains("iRacing") && !one.contains("Assetto"), "{one}");
            let one = message(&["--game", "ac"]);
            assert!(one.contains("Assetto") && !one.contains("iRacing"), "{one}");
        }
    }

    #[test]
    fn automobilista_2_is_a_live_game() {
        let s = parse(&["--game", "ams2"]);
        assert_eq!(s.game(), Game::Ams2);
        assert_eq!(s.game_id(), "ams2");
        assert_eq!(s.source_kind(true), SourceKind::Live);
        assert_eq!(s.source_kind(false), SourceKind::Oneshot);
        assert_eq!(s.recordings(Some(Path::new("docs"))), None, "no files");
        assert!(s.keeps_sessions(false));
        assert!(!s.keeps_sessions(true));
        assert!(s.live_game());
        assert!(!parse(&["--game", "synthetic"]).live_game());
    }

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
            Some(Extras {
                info: Some(self.name.into()),
                ..Extras::default()
            })
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
        assert_eq!(s.extras().unwrap().info, Some("b".into()));
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
}
