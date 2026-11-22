//! Keeps the crate dependency graph in the shape `docs/architecture.md`
//! describes. A new crate or a new edge fails here until the rule (and the
//! document) is updated.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde_json::Value;

/// Workspace crates each crate may depend on (normal and build
/// dependencies). Dev-dependencies are free: tests may use any crate.
const ALLOWED: &[(&str, &[&str])] = &[
    ("vr-core", &[]),
    ("vr-iracing", &["vr-core"]),
    ("vr-replay", &["vr-core"]),
    // The analysis layer never depends on input adapters.
    ("vr-engine", &["vr-core"]),
    ("vr-server", &["vr-core", "vr-engine", "vr-replay"]),
    (
        "vr-cli",
        &[
            "vr-core",
            "vr-engine",
            "vr-iracing",
            "vr-replay",
            "vr-server",
        ],
    ),
    // The voice radio talks to the agent only; the agent runs `vr`.
    ("vr-radio", &[]),
    // The evaluation puts the radio into a simulated race.
    (
        "vr-eval",
        &["vr-core", "vr-engine", "vr-radio", "vr-replay", "vr-server"],
    ),
];

/// Workspace crate → its workspace dependencies that are not dev-only.
fn workspace_graph() -> BTreeMap<String, BTreeSet<String>> {
    let out = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: Value = serde_json::from_slice(&out.stdout).unwrap();
    let packages = meta["packages"].as_array().unwrap();
    let members: BTreeSet<String> = packages
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    packages
        .iter()
        .map(|p| {
            let deps = p["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d["kind"].as_str() != Some("dev"))
                .map(|d| d["name"].as_str().unwrap().to_string())
                .filter(|n| members.contains(n))
                .collect();
            (p["name"].as_str().unwrap().to_string(), deps)
        })
        .collect()
}

#[test]
fn every_crate_has_a_rule() {
    let graph = workspace_graph();
    let ruled: BTreeSet<&str> = ALLOWED.iter().map(|(c, _)| *c).collect();
    let crates: BTreeSet<&str> = graph.keys().map(String::as_str).collect();
    assert_eq!(
        crates, ruled,
        "add new crates to ALLOWED and docs/architecture.md"
    );
}

#[test]
fn dependencies_point_the_documented_way() {
    let graph = workspace_graph();
    let mut violations = Vec::new();
    for (krate, allowed) in ALLOWED {
        for dep in graph.get(*krate).into_iter().flatten() {
            if !allowed.contains(&dep.as_str()) {
                violations.push(format!("{krate} -> {dep}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "forbidden dependencies: {violations:?}"
    );
}

#[test]
fn rules_are_acyclic() {
    // Kahn's algorithm over the allowed edges themselves.
    let mut indegree: BTreeMap<&str, usize> = ALLOWED.iter().map(|(c, _)| (*c, 0)).collect();
    for (_, deps) in ALLOWED {
        for d in *deps {
            *indegree.get_mut(d).expect("rule names a known crate") += 1;
        }
    }
    let mut ready: Vec<&str> = indegree
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(c, _)| *c)
        .collect();
    let mut seen = 0;
    while let Some(c) = ready.pop() {
        seen += 1;
        let deps = ALLOWED.iter().find(|(k, _)| *k == c).unwrap().1;
        for d in deps {
            let n = indegree.get_mut(d).unwrap();
            *n -= 1;
            if *n == 0 {
                ready.push(d);
            }
        }
    }
    assert_eq!(seen, ALLOWED.len(), "the allowed graph has a cycle");
}
