//! `vr doctor`: explain what is (not) working.

use std::process::ExitCode;
use std::time::Duration;

use serde::Serialize;
use vr_core::{Poll, TelemetrySource as _};

use crate::Format;
use crate::client;
use crate::skill;

#[derive(Debug, Default, Serialize)]
struct Report {
    version: String,
    platform: String,
    iracing: Check,
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

    r.iracing = match vr_iracing::open_live() {
        Err(e) => Check {
            ok: false,
            detail: e.to_string(),
        },
        Ok(mut s) => match s.poll(Duration::from_secs(1)) {
            Ok(Poll::Frame(f)) => {
                let si = s.session_info();
                Check {
                    ok: true,
                    detail: format!(
                        "connected: {} | session {:?} {:?} | {} cars | player idx {:?}",
                        si.track_name.as_deref().unwrap_or("?"),
                        f.session.kind,
                        f.session.phase,
                        f.cars.len(),
                        f.player_car_idx
                    ),
                }
            }
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
        },
    };

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
    if !r.iracing.ok && !r.ac.ok {
        out.push(
            "no live iRacing or Assetto Corsa session; `vr serve --game synthetic --speed 10` gives a demo race"
                .into(),
        );
    }
    if r.skills.is_empty() {
        out.push("install the agent skill: `vr skill install`".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_for_what_is_missing() {
        let ok = || Check {
            ok: true,
            detail: String::new(),
        };
        let all = Report {
            iracing: ok(),
            ac: ok(),
            server: ok(),
            skills: vec!["claude: x".into()],
            ..Default::default()
        };
        assert!(hints(&all).is_empty());
        let game = |iracing: bool, ac: bool| {
            let r = Report {
                iracing: Check {
                    ok: iracing,
                    ..ok()
                },
                ac: Check { ok: ac, ..ok() },
                server: ok(),
                skills: vec!["x".into()],
                ..Default::default()
            };
            hints(&r)
                .iter()
                .any(|h| h.contains("no live iRacing or Assetto Corsa"))
        };
        assert!(!game(true, false), "iRacing is enough");
        assert!(!game(false, true), "Assetto Corsa is enough");
        assert!(game(false, false));
        let none = hints(&Report::default());
        assert_eq!(none.len(), 3, "{none:?}");
        assert!(none[0].contains("vr serve") && none[2].contains("vr skill install"));
    }
}
