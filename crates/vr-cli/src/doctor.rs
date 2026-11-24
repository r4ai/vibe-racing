//! `vr doctor`: explain what is (not) working.

use std::process::ExitCode;
use std::time::Duration;

use serde::Serialize;
use vr_core::{Poll, SourceError, TelemetrySource};

use crate::Format;
use crate::client;
use crate::skill;

#[derive(Debug, Default, Serialize)]
struct Report {
    version: String,
    platform: String,
    iracing: Check,
    ams2: Check,
    ac: Check,
    server: Check,
    skills: Vec<String>,
    hints: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
struct Check {
    ok: bool,
    detail: String,
}

pub fn run(server: Option<&str>, fmt: Format) -> anyhow::Result<ExitCode> {
    let mut r = Report {
        version: env!("CARGO_PKG_VERSION").into(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        ..Default::default()
    };

    r.iracing = check(vr_iracing::open_live());
    r.ams2 = check(vr_ams2::open_live());

    r.ac = match vr_ac::open_live(None) {
        Err(e) => Check {
            ok: false,
            detail: e.to_string(),
        },
        Ok(mut s) => match s.poll(Duration::from_secs(1)) {
            Ok(Poll::Frame(f)) => Check {
                ok: true,
                detail: format!(
                    "connected: {} | session {:?} {:?} | {} cars{}",
                    f.session.track.as_ref().map_or("?", |t| t.name.as_str()),
                    f.session.kind,
                    f.session.phase,
                    f.cars.len(),
                    if s.capabilities().multi_car {
                        " (CrewChief app running)"
                    } else {
                        " (start CrewChief's in-game app for the other cars)"
                    }
                ),
            },
            Ok(Poll::Idle) => Check {
                ok: false,
                detail: "shared memory present but paused, replaying or idle within 1s".into(),
            },
            Ok(Poll::Disconnected) => Check {
                ok: false,
                detail: "game running but not on track".into(),
            },
            Err(e) => Check {
                ok: false,
                detail: format!("read error: {e}"),
            },
        },
    };

    r.server = match client::server_url(server) {
        None => Check {
            ok: false,
            detail: "no server configured or discovered".into(),
        },
        Some(url) => {
            match client::HttpClient::new(url.clone(), Some(Duration::from_millis(800))).health() {
                Ok(h) => Check {
                    ok: true,
                    detail: format!(
                        "{url} pid {} game {} connected={} frames={} age={}",
                        h.pid,
                        h.game,
                        h.connected,
                        h.frames,
                        h.age_ms
                            .map(|a| format!("{a}ms"))
                            .unwrap_or_else(|| "-".into())
                    ),
                },
                Err(e) => Check {
                    ok: false,
                    detail: format!("{url} not reachable: {e}"),
                },
            }
        }
    };

    for project in [false, true] {
        for (who, d) in skill::skills_dirs(skill::Target::All, project) {
            let root = d.join(skill::SKILL_NAME);
            if root.join("SKILL.md").exists() {
                r.skills.push(format!("{who}: {}", root.display()));
            }
        }
    }

    r.hints = hints(&r);

    match fmt {
        Format::Json => println!("{}", serde_json::to_string(&r)?),
        Format::Text => {
            let mark = |c: &Check| if c.ok { "ok " } else { "-- " };
            println!("vr {} ({})", r.version, r.platform);
            println!("{}iracing: {}", mark(&r.iracing), r.iracing.detail);
            println!("{}ams2:    {}", mark(&r.ams2), r.ams2.detail);
            println!("{}ac:      {}", mark(&r.ac), r.ac.detail);
            println!("{}server:  {}", mark(&r.server), r.server.detail);
            if r.skills.is_empty() {
                println!("-- skill:   not installed");
            }
            for s in &r.skills {
                println!("ok skill:   {s}");
            }
            for h in &r.hints {
                println!("hint: {h}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// What to do about what is not working.
fn hints(r: &Report) -> Vec<String> {
    let mut out = Vec::new();
    if !r.server.ok {
        out.push("run `vr serve` in a separate terminal while racing for lap history, fuel averages and gap trends".into());
    }
    if !r.iracing.ok && !r.ams2.ok && !r.ac.ok {
        out.push(
            "no live iRacing, Automobilista 2 or Assetto Corsa session; `vr serve --game synthetic --speed 10` gives a demo race"
                .into(),
        );
    }
    if !r.ams2.ok && r.ams2.detail.contains("Project CARS 2") {
        out.push(
            "in Automobilista 2, set Options > System > Shared Memory to \"Project CARS 2\"".into(),
        );
    }
    if r.skills.is_empty() {
        out.push("install the agent skill: `vr skill install`".into());
    }
    out
}

/// Whether a live game can be read, and what it shows.
fn check<S: TelemetrySource>(opened: Result<S, SourceError>) -> Check {
    let mut s = match opened {
        Err(e) => {
            return Check {
                ok: false,
                detail: e.to_string(),
            };
        }
        Ok(s) => s,
    };
    match s.poll(Duration::from_secs(1)) {
        Ok(Poll::Frame(f)) => Check {
            ok: true,
            detail: format!(
                "connected: {} | session {:?} {:?} | {} cars | player idx {:?}",
                f.session.track.as_ref().map_or("?", |t| t.name.as_str()),
                f.session.kind,
                f.session.phase,
                f.cars.len(),
                f.player_car_idx
            ),
        },
        Ok(Poll::Idle) => Check {
            ok: false,
            detail: "shared memory present but no new data within 1s".into(),
        },
        Ok(Poll::Disconnected) => Check {
            ok: false,
            detail: "sim running but not in a session (status disconnected)".into(),
        },
        Err(e) => Check {
            ok: false,
            detail: format!("read error: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vr_core::Capabilities;

    #[test]
    fn hints_for_what_is_missing() {
        let ok = || Check {
            ok: true,
            detail: String::new(),
        };
        let all = Report {
            iracing: ok(),
            ams2: ok(),
            ac: ok(),
            server: ok(),
            skills: vec!["claude: x".into()],
            ..Default::default()
        };
        assert!(hints(&all).is_empty());
        let game = |iracing: bool, ams2: bool, ac: bool| {
            let r = Report {
                iracing: Check {
                    ok: iracing,
                    ..ok()
                },
                ams2: Check { ok: ams2, ..ok() },
                ac: Check { ok: ac, ..ok() },
                server: ok(),
                skills: vec!["x".into()],
                ..Default::default()
            };
            hints(&r)
                .iter()
                .any(|h| h.contains("no live iRacing, Automobilista 2 or Assetto Corsa"))
        };
        assert!(!game(true, false, false), "iRacing is enough");
        assert!(!game(false, true, false), "Automobilista 2 is enough");
        assert!(!game(false, false, true), "Assetto Corsa is enough");
        assert!(game(false, false, false));
        let none = hints(&Report::default());
        assert_eq!(none.len(), 3, "{none:?}");
        assert!(none[0].contains("vr serve") && none[2].contains("vr skill install"));
    }

    struct Once(Option<Result<Poll, SourceError>>);

    impl TelemetrySource for Once {
        fn game(&self) -> &str {
            "test"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn poll(&mut self, _: Duration) -> Result<Poll, SourceError> {
            self.0.take().unwrap()
        }
    }

    fn of(r: Result<Poll, SourceError>) -> Check {
        check(Ok(Once(Some(r))))
    }

    #[test]
    fn says_how_to_turn_on_the_ams2_memory() {
        let r = Report {
            ams2: Check {
                ok: false,
                detail: "Automobilista 2 telemetry not found (... \"Project CARS 2\"?)".into(),
            },
            ..Default::default()
        };
        assert!(
            hints(&r)
                .iter()
                .any(|h| h.starts_with("in Automobilista 2, set"))
        );
        let r = Report {
            ams2: Check {
                ok: false,
                detail: "only on Windows".into(),
            },
            ..Default::default()
        };
        assert!(
            !hints(&r)
                .iter()
                .any(|h| h.starts_with("in Automobilista 2"))
        );
    }

    #[test]
    fn what_each_answer_says() {
        let c = check::<Once>(Err(SourceError::NotRunning("not here".into())));
        assert!(!c.ok);
        assert_eq!(c.detail, "game is not running: not here");
        let c = of(Ok(Poll::Idle));
        assert!(!c.ok);
        assert_eq!(c.detail, "shared memory present but no new data within 1s");
        let c = of(Ok(Poll::Disconnected));
        assert!(!c.ok);
        assert_eq!(
            c.detail,
            "sim running but not in a session (status disconnected)"
        );
        let c = of(Err(SourceError::Format("torn".into())));
        assert!(!c.ok);
        assert_eq!(c.detail, "read error: telemetry format error: torn");
        let mut src = vr_replay::SynthSource::new(vr_replay::SynthConfig::default(), 0.0);
        let frame = src.poll(Duration::ZERO).unwrap();
        let c = of(Ok(frame));
        assert!(c.ok);
        assert!(c.detail.starts_with("connected: "), "{}", c.detail);
        assert!(c.detail.contains("| session Race "), "{}", c.detail);
    }
}
