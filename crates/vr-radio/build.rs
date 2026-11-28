use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");

    let pkg = env!("CARGO_PKG_VERSION");
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let label = match hash {
        Some(h) => format!("v{pkg} ({h})"),
        None => format!("v{pkg}"),
    };
    println!("cargo:rustc-env=VR_RADIO_VERSION={label}");
}
