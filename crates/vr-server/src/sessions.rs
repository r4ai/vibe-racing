//! Past sessions, one JSON file each.

use std::path::{Path, PathBuf};

use vr_core::{Poll, SourceError, TelemetrySource};
use vr_engine::{RaceState, SessionRecord};

/// The directory sessions are kept in: `$VR_SESSIONS`, or next to the
/// discovery file.
pub fn sessions_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("VR_SESSIONS").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    crate::discovery_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("sessions")
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    dir: PathBuf,
}

/// Why a session could not be found.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FindError {
    #[error("no session matches {0:?}")]
    None(String),
    #[error("{0:?} matches several sessions: {1}")]
    Several(String, String),
}

impl SessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Ids come from file names, and go back into them.
    fn path(&self, id: &str) -> PathBuf {
        let safe: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .collect();
        self.dir.join(format!("{safe}.json"))
    }

    /// Replaces the file atomically, so a crash cannot leave half of it.
    fn write(&self, r: &SessionRecord) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(&r.id);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(r)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn read(path: &Path) -> Option<SessionRecord> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// Saves a session, joined with what was kept of it before. Returns its
    /// id.
    pub fn save(&self, mut r: SessionRecord) -> anyhow::Result<String> {
        // Only the same day's files: this runs while driving.
        let day: String = r.id.chars().take(8).collect();
        let old = self
            .read_all(&day)
            .into_iter()
            .find(|o| o.id == r.id || r.same_session(o));
        if let Some(old) = old {
            r.merge_older(&old);
        }
        self.write(&r)?;
        Ok(r.id)
    }

    /// All sessions, newest first. Files that cannot be read are left out.
    pub fn list(&self) -> Vec<SessionRecord> {
        let mut all = self.read_all("");
        all.sort_by(|a, b| b.started_ms.cmp(&a.started_ms).then(b.id.cmp(&a.id)));
        all
    }

    /// Sessions whose file name starts with `prefix`.
    fn read_all(&self, prefix: &str) -> Vec<SessionRecord> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        dir.flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .filter_map(|p| Self::read(&p))
            .collect()
    }

    /// By id, the beginning of one, `last`, or a number (1 is the newest).
    pub fn find(&self, key: &str) -> Result<SessionRecord, FindError> {
        let all = self.list();
        if key == "last" {
            return all.into_iter().next().ok_or(FindError::None(key.into()));
        }
        // An id starts with a date: eight digits are never a position.
        if key.len() < 8
            && let Ok(n) = key.parse::<usize>()
        {
            return n
                .checked_sub(1)
                .and_then(|i| all.into_iter().nth(i))
                .ok_or(FindError::None(key.into()));
        }
        if let Some(exact) = all.iter().find(|r| r.id == key) {
            return Ok(exact.clone());
        }
        let mut hits: Vec<SessionRecord> =
            all.into_iter().filter(|r| r.id.starts_with(key)).collect();
        match hits.len() {
            0 => Err(FindError::None(key.into())),
            1 => Ok(hits.remove(0)),
            _ => {
                let ids: Vec<&str> = hits.iter().map(|r| r.id.as_str()).collect();
                Err(FindError::Several(key.into(), ids.join(", ")))
            }
        }
    }

    /// Sets the note (empty removes it).
    pub fn note(&self, key: &str, note: &str) -> anyhow::Result<SessionRecord> {
        let mut r = self.find(key)?;
        r.note = Some(note.trim().to_string()).filter(|n| !n.is_empty());
        self.write(&r)?;
        Ok(r)
    }

    pub fn delete(&self, key: &str) -> anyhow::Result<SessionRecord> {
        let r = self.find(key)?;
        std::fs::remove_file(self.path(&r.id))?;
        Ok(r)
    }
}

/// Runs a finite source to its end and returns the sessions in it.
pub fn read_sessions(
    source: &mut dyn TelemetrySource,
    state: &mut RaceState,
) -> Result<Vec<SessionRecord>, SourceError> {
    let mut out = Vec::new();
    let mut extras_at: Option<f64> = None;
    loop {
        match source.poll(std::time::Duration::ZERO)? {
            Poll::Frame(f) => {
                let t = f.session.time_s;
                state.ingest(*f);
                out.extend(state.take_finished());
                // Once a minute is enough for what changes rarely.
                if extras_at.is_none_or(|e| !(0.0..60.0).contains(&(t - e))) {
                    extras_at = Some(t);
                    if let Some(e) = source.extras() {
                        state.set_extras(e);
                    }
                }
            }
            Poll::Idle => {}
            Poll::Disconnected => break,
        }
    }
    out.extend(SessionRecord::of(state));
    Ok(out)
}
