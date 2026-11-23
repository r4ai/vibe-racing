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

/// What to do about what does not work.
fn hints(r: &Report) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if !r.server.ok {
        out.push("run `vr serve` in a separate terminal while racing for lap history, fuel averages and gap trends".into());
    }
    if !r.iracing.ok && !r.ams2.ok {
        out.push(
            "no live iRacing or Automobilista 2 session; `vr serve --game synthetic --speed 10` gives a demo race"
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
    use vr_core::{Capabilities, SourceError};

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
    fn hints_for_what_does_not_work() {
        let ok = |detail: &str| Check {
            ok: true,
            detail: detail.into(),
        };
        let down = |detail: &str| Check {
            ok: false,
            detail: detail.into(),
        };
        let mut r = Report {
            iracing: ok(""),
            ams2: down("Automobilista 2 telemetry not found (... \"Project CARS 2\"?)"),
            server: ok(""),
            skills: vec!["claude".into()],
            ..Default::default()
        };
        let starts = |r: &Report| -> Vec<String> {
            hints(r)
                .iter()
                .map(|h| h.chars().take(12).collect())
                .collect()
        };
        assert_eq!(starts(&r), ["in Automobil"], "one game is enough");
        r.ams2 = ok("");
        assert!(hints(&r).is_empty());
        r.iracing = down("");
        r.ams2 = down("unsupported");
        r.server = down("");
        r.skills.clear();
        assert_eq!(starts(&r), ["run `vr serv", "no live iRac", "install the "]);
        r.iracing = ok("");
        assert_eq!(starts(&r), ["run `vr serv", "install the "]);
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
