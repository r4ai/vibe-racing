//! VOICEVOX engine client (`audio_query` → `synthesis`).

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::http;

pub trait Synth: Send + Sync + 'static {
    /// Returns a WAV file for `text`. Blocking.
    fn synthesize(&self, text: &str, speaker: u32) -> Result<Vec<u8>>;
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
    /// Multiplies the speaking rate (1.0 = engine default).
    pub speed: f64,
    agent: ureq::Agent,
}

impl Voicevox {
    pub fn new(base_url: impl Into<String>, speed: f64) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            speed,
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
    fn synthesize(&self, text: &str, speaker: u32) -> Result<Vec<u8>> {
        let speaker = speaker.to_string();
        let resp = self
            .agent
            .post(format!("{}/audio_query", self.base_url))
            .query("text", text)
            .query("speaker", &speaker)
            .send_empty()
            .with_context(|| self.unreachable())?;
        let mut resp = http::check(resp, "VOICEVOX audio_query")?;
        let mut query: Value = resp.body_mut().read_json()?;
        query["speedScale"] = self.speed.into();
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
        assert_eq!(v.synthesize("前 0.6", 3).unwrap(), b"RIFFwav");

        let reqs = server.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(
            reqs[0].target,
            "/audio_query?text=%E5%89%8D%200.6&speaker=3"
        );
        assert_eq!(reqs[1].target, "/synthesis?speaker=3");
        let sent: Value = serde_json::from_slice(&reqs[1].body).unwrap();
        assert_eq!(sent["speedScale"], 1.25);
        assert_eq!(sent["prePhonemeLength"], 0.05);
        assert_eq!(sent["postPhonemeLength"], 0.05);
        assert_eq!(sent["accent_phrases"], Value::Array(vec![]));
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
