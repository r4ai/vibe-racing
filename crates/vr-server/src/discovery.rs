use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Written by `vr serve` so the CLI can find a running server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub url: String,
    pub pid: u32,
    pub game: String,
    pub started_ms: i64,
}

/// `$VR_DISCOVERY` overrides the location (useful for tests and for running
/// several servers side by side).
pub fn discovery_path() -> PathBuf {
    if let Some(p) = std::env::var_os("VR_DISCOVERY").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("vibe-racing")
        .join("server.json")
}

pub fn write_discovery(info: &ServerInfo) -> std::io::Result<PathBuf> {
    let p = discovery_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, serde_json::to_vec_pretty(info)?)?;
    Ok(p)
}

pub fn read_discovery() -> Option<ServerInfo> {
    serde_json::from_slice(&std::fs::read(discovery_path()).ok()?).ok()
}

/// Remove the discovery file if it still points at this process.
pub fn clear_discovery(pid: u32) {
    if read_discovery().is_some_and(|i| i.pid == pid) {
        let _ = std::fs::remove_file(discovery_path());
    }
}
