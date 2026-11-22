//! Reads the telemetry files the game leaves behind into the kept sessions,
//! while nobody drives: they tell about the wheels, which the live telemetry
//! does not.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use vr_engine::SharedState;
use vr_server::SessionStore;

use crate::sessions::import;

/// How often it looks for new files.
const EVERY: Duration = Duration::from_secs(15);
/// A file that was not written to for so long is complete.
const SETTLED: Duration = Duration::from_secs(10);
/// Older files are left to `vr sessions import`.
const RECENT: Duration = Duration::from_secs(3 * 24 * 3600);
/// Shorter recordings hold no lap.
const MIN_BYTES: u64 = 1_000_000;
/// All samples: the dampers move fast.
const HZ: f64 = 60.0;

/// What was read before, next to the sessions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Seen(BTreeSet<String>);

impl Seen {
    fn path(store: &SessionStore) -> PathBuf {
        store.dir().join("imported").with_extension("list")
    }

    fn load(store: &SessionStore) -> Self {
        std::fs::read(Self::path(store))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, store: &SessionStore) -> anyhow::Result<()> {
        std::fs::create_dir_all(store.dir())?;
        std::fs::write(Self::path(store), serde_json::to_vec(self)?)?;
        Ok(())
    }
}

/// The complete recordings that were not read yet, oldest first.
fn waiting(dir: &Path, seen: &Seen, now: SystemTime) -> Vec<(PathBuf, String)> {
    let Ok(files) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(SystemTime, PathBuf, String)> = files
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let meta = e.metadata().ok()?;
            let written = meta.modified().ok()?;
            let age = now.duration_since(written).unwrap_or_default();
            let name = format!("{}:{}", e.file_name().to_string_lossy(), meta.len());
            let right = path.extension().is_some_and(|x| x == "ibt")
                && meta.len() >= MIN_BYTES
                && (SETTLED..RECENT).contains(&age)
                && !seen.0.contains(&name);
            right.then_some((written, path, name))
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, p, n)| (p, n)).collect()
}

/// Somebody is in the car.
fn driving(state: &SharedState) -> bool {
    state.read().is_ok_and(|st| {
        st.meta().connected
            && st
                .frame()
                .and_then(|f| f.player.as_ref())
                .is_some_and(|p| p.on_track)
    })
}

/// Reads what has come in; `stop` is asked between files.
fn read_all(dir: &Path, store: &SessionStore, seen: &mut Seen, stop: &dyn Fn() -> bool) {
    for (path, name) in waiting(dir, seen, SystemTime::now()) {
        if stop() {
            return;
        }
        match import(store, &path, HZ) {
            Ok(ids) => tracing::info!("read {}: {}", path.display(), ids.join(", ")),
            Err(e) => tracing::warn!("{}: {e:#}", path.display()),
        }
        // Also one that cannot be read: it will not get better.
        seen.0.insert(name);
        if let Err(e) = seen.save(store) {
            tracing::warn!("{e:#}");
        }
    }
}

/// Watches `dir` until `stop` is set.
pub fn spawn(dir: PathBuf, store: SessionStore, state: SharedState, stop: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new()
        .name("vr-inbox".into())
        .spawn(move || {
            let mut seen = Seen::load(&store);
            let stopped = || stop.load(Ordering::Relaxed);
            while !stopped() {
                if !driving(&state) {
                    read_all(&dir, &store, &mut seen, &|| stopped() || driving(&state));
                }
                std::thread::sleep(EVERY);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_new_recordings_are_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: u64| {
            let f = std::fs::File::create(dir.path().join(name)).unwrap();
            f.set_len(bytes).unwrap();
        };
        write("a.ibt", MIN_BYTES);
        write("short.ibt", MIN_BYTES - 1);
        write("b.txt", MIN_BYTES);
        let names = |seen: &Seen, after: Duration| -> Vec<String> {
            waiting(dir.path(), seen, SystemTime::now() + after)
                .into_iter()
                .map(|(_, n)| n)
                .collect()
        };
        let mut seen = Seen::default();
        // Still being written, perhaps.
        assert!(names(&seen, Duration::ZERO).is_empty());
        assert_eq!(names(&seen, SETTLED * 2), ["a.ibt:1000000"]);
        assert!(names(&seen, RECENT * 2).is_empty());
        seen.0.insert("a.ibt:1000000".into());
        assert!(names(&seen, SETTLED * 2).is_empty());
        // It grew: the game went on recording into it.
        write("a.ibt", MIN_BYTES + 5);
        assert_eq!(names(&seen, SETTLED * 2), ["a.ibt:1000005"]);
        assert!(waiting(Path::new("/no/such/dir"), &seen, SystemTime::now()).is_empty());
    }

    #[test]
    fn what_was_read_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("sessions"));
        assert_eq!(Seen::load(&store), Seen::default());
        let seen = Seen(["a".to_string()].into());
        seen.save(&store).unwrap();
        assert_eq!(Seen::load(&store), seen);
        // Not taken for a session.
        assert!(store.list().is_empty());
    }

    #[test]
    fn damaged_files_are_left_alone_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("sessions"));
        let file = dir.path().join("x.ibt");
        std::fs::File::create(&file)
            .unwrap()
            .set_len(MIN_BYTES)
            .unwrap();
        let old = SystemTime::now() - SETTLED * 2;
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let mut seen = Seen::default();
        read_all(dir.path(), &store, &mut seen, &|| true);
        assert!(seen.0.is_empty(), "stopped");
        read_all(dir.path(), &store, &mut seen, &|| false);
        assert_eq!(seen.0.len(), 1);
        assert_eq!(Seen::load(&store), seen);
    }
}
