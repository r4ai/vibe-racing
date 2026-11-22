//! VOICEVOX engine client (`audio_query` → `synthesis`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::child::{Retry, Supervised};
use crate::http;

pub trait Synth: Send + Sync + 'static {
    /// Returns a WAV file for `text`. Blocking.
    fn synthesize(&self, text: &str, speaker: u32) -> Result<Vec<u8>>;

    /// Gets a voice ready, so that its first sentence is not slower. Blocking.
    fn warm_up(&self, _speaker: u32) -> Result<()> {
        Ok(())
    }

    /// Speaking rate from the next sentence on (1.0 = engine default).
    fn set_speed(&self, _speed: f64) {}
}

/// One selectable voice: a character and one of its styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    pub id: u32,
    /// The character, e.g. ずんだもん.
    pub speaker: String,
    /// The style of the character, e.g. ノーマル.
    pub style: String,
}

impl std::fmt::Display for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（{}）", self.speaker, self.style)
    }
}

#[derive(Deserialize)]
struct Speaker {
    name: String,
    styles: Vec<Style>,
}

#[derive(Deserialize)]
struct Style {
    name: String,
    id: u32,
}

pub struct Voicevox {
    base_url: String,
    /// Multiplies the speaking rate (1.0 = engine default); bits of an `f64`.
    speed: AtomicU64,
    agent: ureq::Agent,
}

impl Voicevox {
    pub fn new(base_url: impl Into<String>, speed: f64) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            speed: AtomicU64::new(speed.to_bits()),
            agent: http::agent(Duration::from_secs(20)),
        }
    }

    fn unreachable(&self) -> String {
        format!("VOICEVOX unreachable at {}", self.base_url)
    }

    pub fn voices(&self) -> Result<Vec<Voice>> {
        let resp = self
            .agent
            .get(format!("{}/speakers", self.base_url))
            .call()
            .with_context(|| self.unreachable())?;
        let mut resp = http::check(resp, "VOICEVOX /speakers")?;
        let speakers: Vec<Speaker> = resp.body_mut().read_json()?;
        Ok(speakers
            .into_iter()
            .flat_map(|s| {
                s.styles.into_iter().map(move |st| Voice {
                    id: st.id,
                    speaker: s.name.clone(),
                    style: st.name,
                })
            })
            .collect())
    }
}

impl Synth for Voicevox {
    fn set_speed(&self, speed: f64) {
        self.speed.store(speed.to_bits(), Ordering::Relaxed);
    }

    /// The engine loads a voice's model on its first use (about a second on
    /// a CPU); `skip_reinit` makes it a no-op for a loaded one.
    fn warm_up(&self, speaker: u32) -> Result<()> {
        let resp = self
            .agent
            .post(format!("{}/initialize_speaker", self.base_url))
            .query("speaker", speaker.to_string())
            .query("skip_reinit", "true")
            .send_empty()
            .with_context(|| self.unreachable())?;
        http::check(resp, "VOICEVOX initialize_speaker")?;
        Ok(())
    }

    fn synthesize(&self, text: &str, speaker: u32) -> Result<Vec<u8>> {
        let speaker = speaker.to_string();
        let resp = self
            .agent
            .post(format!("{}/audio_query", self.base_url))
            .query("text", crate::aloud::aloud(text))
            .query("speaker", &speaker)
            .send_empty()
            .with_context(|| self.unreachable())?;
        let mut resp = http::check(resp, "VOICEVOX audio_query")?;
        let mut query: Value = resp.body_mut().read_json()?;
        query["speedScale"] = f64::from_bits(self.speed.load(Ordering::Relaxed)).into();
        // Shorter silences at both ends: every sentence is a separate clip.
        query["prePhonemeLength"] = 0.05.into();
        query["postPhonemeLength"] = 0.05.into();
        let resp = self
            .agent
            .post(format!("{}/synthesis", self.base_url))
            .query("speaker", &speaker)
            .send_json(&query)
            .with_context(|| self.unreachable())?;
        let mut resp = http::check(resp, "VOICEVOX synthesis")?;
        Ok(resp
            .body_mut()
            .with_config()
            .limit(64 << 20)
            .read_to_vec()?)
    }
}

/// The host and port of a VOICEVOX on this machine.
fn local(url: &str) -> Option<(&str, u16)> {
    let rest = url.strip_prefix("http://")?;
    let authority = rest.split('/').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (authority, 80),
    };
    matches!(host, "127.0.0.1" | "localhost").then_some((host, port))
}

/// Where the installers put the engine.
fn installed() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(local) = dirs::data_local_dir() {
        dirs.push(local.join("Programs"));
    }
    if let Some(p) = std::env::var_os("ProgramFiles") {
        dirs.push(p.into());
    }
    dirs.into_iter()
        .map(|d| d.join("VOICEVOX").join("vv-engine").join("run.exe"))
        .collect()
}

/// The command that starts the engine at `url`: `given` or, when there is
/// none, the first of `candidates` that exists. `None`: not to be started.
fn engine_command(url: &str, given: Option<&str>, candidates: &[PathBuf]) -> Option<String> {
    let (host, port) = local(url)?;
    let command = match given {
        Some(c) => c.trim().to_string(),
        None => {
            let exe = candidates.iter().find(|c| c.is_file())?;
            format!("\"{}\"", exe.display())
        }
    };
    (!command.is_empty()).then(|| format!("{command} --host {host} --port {port}"))
}

/// Starts the engine and keeps it running until the returned value is dropped.
pub fn start_engine(url: &str, given: Option<&str>) -> Option<Supervised> {
    let command = engine_command(url, given, &installed())?;
    tracing::info!("starting VOICEVOX: {command}");
    let retry = Retry {
        initial: Duration::ZERO,
        first: Duration::from_secs(15),
        max: Duration::from_secs(300),
    };
    Supervised::spawn(&command, std::env::temp_dir(), retry, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake;

    #[test]
    fn synthesize_queries_then_synthesizes_with_speed() {
        let server = fake::serve(|r| {
            if r.target.starts_with("/audio_query") {
                fake::Response::json(
                    r#"{"speedScale":1.0,"prePhonemeLength":0.1,"accent_phrases":[]}"#,
                )
            } else {
                fake::Response {
                    status: 200,
                    content_type: "audio/wav",
                    body: b"RIFFwav".to_vec(),
                }
            }
        });
        let v = Voicevox::new(format!("{}/", server.url), 1.25);
        assert_eq!(v.synthesize("前0.6", 3).unwrap(), b"RIFFwav");
        v.set_speed(1.5);
        v.synthesize("後", 3).unwrap();

        let reqs = server.requests.lock().unwrap();
        assert_eq!(reqs.len(), 4);
        let faster: Value = serde_json::from_slice(&reqs[3].body).unwrap();
        assert_eq!(faster["speedScale"], 1.5);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(
            reqs[0].target,
            "/audio_query?text=%E5%89%8D%E3%82%B3%E3%83%B3%E3%83%9E6&speaker=3"
        );
        assert_eq!(reqs[1].target, "/synthesis?speaker=3");
        let sent: Value = serde_json::from_slice(&reqs[1].body).unwrap();
        assert_eq!(sent["speedScale"], 1.25);
        assert_eq!(sent["prePhonemeLength"], 0.05);
        assert_eq!(sent["postPhonemeLength"], 0.05);
        assert_eq!(sent["accent_phrases"], Value::Array(vec![]));
    }

    #[test]
    fn only_a_local_engine_is_started() {
        assert_eq!(local("http://127.0.0.1:50021"), Some(("127.0.0.1", 50021)));
        assert_eq!(local("http://localhost:5/"), Some(("localhost", 5)));
        assert_eq!(local("http://localhost"), Some(("localhost", 80)));
        for url in [
            "http://192.168.0.2:50021",
            "https://127.0.0.1:50021",
            "http://127.0.0.1:x",
            "127.0.0.1:50021",
        ] {
            assert_eq!(local(url), None, "{url}");
        }
    }

    #[test]
    fn the_installed_engine_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("run.exe");
        let missing = dir.path().join("none.exe");
        let url = "http://127.0.0.1:50021";
        let candidates = [missing.clone(), exe.clone()];
        assert_eq!(engine_command(url, None, &candidates), None);
        std::fs::write(&exe, "").unwrap();
        assert_eq!(
            engine_command(url, None, &candidates),
            Some(format!(
                "\"{}\" --host 127.0.0.1 --port 50021",
                exe.display()
            ))
        );
        assert_eq!(
            engine_command(url, Some("my-engine --x"), &candidates).as_deref(),
            Some("my-engine --x --host 127.0.0.1 --port 50021")
        );
        assert_eq!(engine_command(url, Some(" "), &candidates), None);
        assert_eq!(engine_command("http://nas:50021", None, &candidates), None);
    }

    #[test]
    fn warm_up_loads_the_voice_once() {
        let server = fake::serve(|_| fake::Response {
            status: 204,
            content_type: "text/plain",
            body: vec![],
        });
        Voicevox::new(server.url.clone(), 1.0).warm_up(13).unwrap();
        let reqs = server.requests.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(
            reqs[0].target,
            "/initialize_speaker?speaker=13&skip_reinit=true"
        );
        let err = format!(
            "{:#}",
            Voicevox::new("http://127.0.0.1:9", 1.0)
                .warm_up(3)
                .unwrap_err()
        );
        assert!(err.starts_with("VOICEVOX unreachable"), "{err}");
    }

    #[test]
    fn voices_flatten_styles() {
        let server = fake::serve(|_| {
            fake::Response::json(
                r#"[{"name":"ずんだもん","speaker_uuid":"x","styles":[{"name":"ノーマル","id":3},{"name":"あまあま","id":1}]},
                    {"name":"四国めたん","styles":[{"name":"ノーマル","id":2}]}]"#,
            )
        });
        let voices = Voicevox::new(server.url.clone(), 1.0).voices().unwrap();
        assert_eq!(
            voices,
            vec![
                Voice {
                    id: 3,
                    speaker: "ずんだもん".into(),
                    style: "ノーマル".into()
                },
                Voice {
                    id: 1,
                    speaker: "ずんだもん".into(),
                    style: "あまあま".into()
                },
                Voice {
                    id: 2,
                    speaker: "四国めたん".into(),
                    style: "ノーマル".into()
                },
            ]
        );
    }

    #[test]
    fn voice_displays_its_label() {
        let v = Voice {
            id: 3,
            speaker: "ずんだもん".into(),
            style: "ノーマル".into(),
        };
        assert_eq!(v.to_string(), "ずんだもん（ノーマル）");
    }

    #[test]
    fn unreachable_engine_names_the_url() {
        // Port 9 (discard) is closed on test machines: the connection is refused.
        let v = Voicevox::new("http://127.0.0.1:9", 1.0);
        let err = format!("{:#}", v.synthesize("a", 3).unwrap_err());
        assert!(
            err.starts_with("VOICEVOX unreachable at http://127.0.0.1:9: "),
            "{err}"
        );
        let err = format!("{:#}", v.voices().unwrap_err());
        assert!(
            err.starts_with("VOICEVOX unreachable at http://127.0.0.1:9: "),
            "{err}"
        );
    }

    #[test]
    fn engine_error_is_reported() {
        let server = fake::serve(|_| fake::Response {
            status: 422,
            content_type: "application/json",
            body: br#"{"detail":"bad speaker"}"#.to_vec(),
        });
        let err = Voicevox::new(server.url.clone(), 1.0)
            .synthesize("a", 999)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            r#"VOICEVOX audio_query: HTTP 422 Unprocessable Entity: {"detail":"bad speaker"}"#
        );
    }
}
