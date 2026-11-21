//! End-to-end tests of the `vr` binary. No game required: they use the
//! synthetic race, recordings and a real `vr serve` child process.

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const VR: &str = env!("CARGO_BIN_EXE_vr");

/// A `vr` command isolated from any server running on this machine.
fn vr(discovery: &Path) -> Command {
    let mut c = Command::new(VR);
    c.env("VR_DISCOVERY", discovery)
        .env_remove("VR_SERVER")
        .env("VR_LOG", "warn");
    c
}

fn run(discovery: &Path, args: &[&str]) -> Output {
    vr(discovery).args(args).output().expect("run vr")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn json(o: &Output) -> serde_json::Value {
    assert!(
        o.status.success(),
        "status {:?}\nstdout {}\nstderr {}",
        o.status,
        stdout(o),
        stderr(o)
    );
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(o)))
}

fn tmp() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let disc = d.path().join("server.json");
    (d, disc)
}

const SYN: &[&str] = &["--game", "synthetic", "--until", "600"];

fn with_syn<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut v: Vec<&str> = args.to_vec();
    v.extend_from_slice(SYN);
    v
}

#[test]
fn brief_text_for_synthetic_race() {
    let (_d, disc) = tmp();
    let o = run(&disc, &with_syn(&["brief"]));
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.starts_with("[synthetic synthetic"), "{out}");
    for want in [
        "race [racing] @ Demo Raceway - GP",
        "you: P4/12",
        "ahead: P3",
        "behind: P5",
        "fuel:",
        "recent events:",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
}

#[test]
fn every_query_command_emits_json() {
    let (_d, disc) = tmp();
    let b = json(&run(&disc, &with_syn(&["brief", "-f", "json"])));
    assert_eq!(b["meta"]["source"], "synthetic");
    assert_eq!(b["player"]["car"]["position"], 4);

    let s = json(&run(
        &disc,
        &with_syn(&["standings", "--around", "1", "-f", "json"]),
    ));
    let rows = s["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1]["is_player"], true);
    let s = json(&run(
        &disc,
        &with_syn(&["standings", "--top", "2", "--overall", "-f", "json"]),
    ));
    assert_eq!(s["rows"].as_array().unwrap().len(), 2);
    assert_eq!(s["class_only"], false);

    let leader = s["rows"][0]["car"]["number"].as_str().unwrap().to_string();
    let g = json(&run(
        &disc,
        &with_syn(&["gap", "--car", &leader, "--window", "5", "-f", "json"]),
    ));
    assert_eq!(g["target_is_ahead"], true);

    let f = json(&run(&disc, &with_syn(&["fuel", "-f", "json"])));
    assert_eq!(f["fuel"]["basis"], "history");

    let l = json(&run(
        &disc,
        &with_syn(&["laps", "--last", "3", "-f", "json"]),
    ));
    assert_eq!(l["laps"].as_array().unwrap().len(), 3);

    let e = json(&run(
        &disc,
        &with_syn(&["events", "--since", "2", "--limit", "4", "-f", "json"]),
    ));
    let evs = e["events"].as_array().unwrap();
    assert_eq!(evs.len(), 4);
    assert!(evs.iter().all(|x| x["seq"].as_u64().unwrap() > 2));

    let snap = json(&run(&disc, &with_syn(&["snapshot", "-f", "json"])));
    assert_eq!(snap["frame"]["cars"].as_array().unwrap().len(), 12);
}

#[test]
fn text_forms_of_other_commands() {
    let (_d, disc) = tmp();
    let checks: &[(&[&str], &str)] = &[
        (&["standings", "--top", "3"], "standings:"),
        (&["gap"], "ahead: P3"),
        (&["fuel"], "L/lap (avg"),
        (&["laps", "--last", "2"], "laps for #23 You"),
        (&["events", "--limit", "3"], "last_seq="),
        (&["snapshot"], "\"frame\""),
    ];
    for (args, want) in checks {
        let o = run(&disc, &with_syn(args));
        assert!(o.status.success(), "{args:?}: {}", stderr(&o));
        assert!(
            stdout(&o).contains(want),
            "{args:?} missing {want:?}:\n{}",
            stdout(&o)
        );
    }
}

#[test]
fn exit_codes() {
    let (_d, disc) = tmp();
    let o = run(&disc, &with_syn(&["gap", "--car", "999"]));
    assert_eq!(o.status.code(), Some(4), "{}", stderr(&o));
    assert!(stderr(&o).contains("not found: car #999"));

    let o = run(&disc, &with_syn(&["laps", "--car", "999"]));
    assert_eq!(o.status.code(), Some(4));

    let o = run(&disc, &["brief", "--game", "replay"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("--file"));

    let o = run(&disc, &["brief", "--server", "http://127.0.0.1:9"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("not reachable"), "{}", stderr(&o));

    // No server and no live iRacing session (CI, or sim not in a session).
    let o = run(&disc, &["brief"]);
    if o.status.success() {
        eprintln!("live iRacing session detected; skipping no-data assertion");
    } else {
        assert_eq!(o.status.code(), Some(3), "{}", stderr(&o));
        assert!(!stderr(&o).is_empty());
    }

    let o = run(&disc, &["bogus"]);
    assert_eq!(o.status.code(), Some(2), "clap usage error");
}

#[test]
fn record_then_query_replay() {
    let (d, disc) = tmp();
    let file = d.path().join("race.ndjson");
    let f = file.to_str().unwrap();
    let o = run(
        &disc,
        &[
            "record",
            "--out",
            f,
            "--hz",
            "2",
            "--game",
            "synthetic",
            "--speed",
            "0",
        ],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stderr(&o).contains("recorded"), "{}", stderr(&o));
    let lines = std::fs::read_to_string(&file).unwrap().lines().count();
    assert!(lines > 3000, "{lines} frames at 2 Hz for a ~31 min race");

    // --file implies --game replay; the recorded game id is kept.
    let b = json(&run(
        &disc,
        &["brief", "--file", f, "--until", "600", "-f", "json"],
    ));
    assert_eq!(b["meta"]["source"], "replay");
    assert_eq!(b["meta"]["game"], "synthetic");
    assert_eq!(b["player"]["car"]["position"], 4);
    // Whole replay: the race is over.
    let b = json(&run(&disc, &["brief", "--file", f, "-f", "json"]));
    assert_eq!(b["session"]["phase"], "checkered");
    assert_eq!(b["meta"]["connected"], false);
}

#[test]
fn watch_in_process_streams_events() {
    let (_d, disc) = tmp();
    let o = run(&disc, &["watch", "--game", "synthetic", "--speed", "0"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.lines().count() > 20, "{out}");
    assert!(
        out.contains("pit in: you") && out.contains("chequered flag"),
        "{out}"
    );
    let o = run(
        &disc,
        &["watch", "--game", "synthetic", "--speed", "0", "-f", "json"],
    );
    let first: serde_json::Value =
        serde_json::from_str(stdout(&o).lines().next().unwrap()).unwrap();
    assert_eq!(first["type"], "session_changed");
}

struct ServeChild(Child);

impl Drop for ServeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn serve_then_query_through_discovery() {
    let (_d, disc) = tmp();
    let port = free_port().to_string();
    let child = vr(&disc)
        .args([
            "serve",
            "--game",
            "synthetic",
            "--speed",
            "0",
            "--port",
            &port,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _guard = ServeChild(child);

    // Wait for the discovery file and for the (fast) race to finish.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "server did not come up");
        if disc.exists() {
            let o = run(&disc, &["brief", "-f", "json"]);
            if o.status.success() {
                let b: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
                if b["session"]["phase"] == "checkered" && b["meta"]["connected"] == false {
                    assert_eq!(b["meta"]["history"], true);
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let info: serde_json::Value = serde_json::from_slice(&std::fs::read(&disc).unwrap()).unwrap();
    assert_eq!(info["url"], format!("http://127.0.0.1:{port}"));
    assert_eq!(info["game"], "synthetic");

    // Explicit --server, every endpoint through the HTTP client.
    let url = format!("http://127.0.0.1:{port}");
    let srv = |args: &[&str]| {
        let mut v = vec!["--server", url.as_str()];
        v.extend_from_slice(args);
        run(&disc, &v)
    };
    let e = json(&srv(&["events", "--limit", "500", "-f", "json"]));
    assert!(e["events"].as_array().unwrap().len() > 20);
    assert!(
        json(&srv(&["standings", "--around", "1", "-f", "json"]))["rows"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    assert!(
        json(&srv(&["laps", "-f", "json"]))["laps"]
            .as_array()
            .unwrap()
            .len()
            == 10
    );
    assert!(json(&srv(&["fuel", "-f", "json"]))["fuel"].is_object());
    assert!(json(&srv(&["gap", "-f", "json"]))["nearby"].is_array());
    assert!(json(&srv(&["snapshot", "-f", "json"]))["frame"].is_object());
    let o = srv(&["gap", "--car", "999"]);
    assert_eq!(o.status.code(), Some(4), "404 maps to not-found exit code");

    // VR_SERVER env var works too.
    let o = vr(&disc)
        .env("VR_SERVER", &url)
        .args(["brief"])
        .output()
        .unwrap();
    assert!(
        stdout(&o).starts_with("[synthetic synthetic"),
        "{}",
        stdout(&o)
    );

    // Doctor sees the server.
    let d = json(&run(&disc, &["doctor", "-f", "json"]));
    assert_eq!(d["server"]["ok"], true, "{d}");
    assert!(
        d["server"]["detail"]
            .as_str()
            .unwrap()
            .contains("game synthetic")
    );
}

#[test]
fn doctor_without_server() {
    let (_d, disc) = tmp();
    let d = json(&run(&disc, &["doctor", "-f", "json"]));
    assert_eq!(d["server"]["ok"], false);
    assert!(
        d["hints"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h.as_str().unwrap().contains("vr serve"))
    );
    let o = run(&disc, &["doctor"]);
    assert!(stdout(&o).contains("-- server:"), "{}", stdout(&o));
    let o = vr(&disc)
        .env("VR_SERVER", "http://127.0.0.1:9")
        .args(["doctor"])
        .output()
        .unwrap();
    assert!(stdout(&o).contains("not reachable"), "{}", stdout(&o));
}

#[test]
fn skill_install_matches_repository_files() {
    let (d, disc) = tmp();
    let target = d.path().join("skills");
    let o = run(
        &disc,
        &["skill", "install", "--dir", target.to_str().unwrap()],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/race-engineer");
    for rel in [
        "SKILL.md",
        "references/commands.md",
        "references/radio-style.md",
        "references/glossary.md",
    ] {
        let installed = std::fs::read_to_string(target.join("race-engineer").join(rel)).unwrap();
        let source = std::fs::read_to_string(repo.join(rel)).unwrap();
        assert_eq!(installed, source, "{rel}");
    }
    let o = run(&disc, &["skill", "show"]);
    assert!(
        stdout(&o).starts_with("---\nname: race-engineer\n"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn skill_install_project_targets() {
    let (d, disc) = tmp();
    let o = vr(&disc)
        .current_dir(d.path())
        .args(["skill", "paths", "--project"])
        .output()
        .unwrap();
    assert!(stdout(&o).contains("not installed"));
    let o = vr(&disc)
        .current_dir(d.path())
        .args(["skill", "install", "--project", "--target", "codex"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        d.path()
            .join(".agents/skills/race-engineer/SKILL.md")
            .exists()
    );
    assert!(!d.path().join(".claude").exists());
    let o = vr(&disc)
        .current_dir(d.path())
        .args(["skill", "install", "--project"])
        .output()
        .unwrap();
    assert!(stdout(&o).contains("for claude") && stdout(&o).contains("for codex"));
    let o = vr(&disc)
        .current_dir(d.path())
        .args(["skill", "paths", "--project"])
        .output()
        .unwrap();
    assert_eq!(
        stdout(&o).matches("(installed)").count(),
        2,
        "{}",
        stdout(&o)
    );
}
