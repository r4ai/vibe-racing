//! Choices made in the settings sheet, kept between runs.
//!
//! Command-line options and environment variables win over the file; the
//! file wins over the built-in defaults. API keys are never stored here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{AgentOptions, Config};
use crate::directive::MAX_VOLUME;
use crate::rules::Rules;
use crate::stt::SttBackend;
use crate::watch::Proactive;
use crate::wheel::Binding;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Model, effort and so on, by the command of the agent.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_options: BTreeMap<String, AgentOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<u32>,
    /// Percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// Speech rate (1.0 = the engine's own).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt: Option<SttBackend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    /// Wheel or controller button that also works as push-to-talk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wheel: Option<Binding>,
    /// Closing the window keeps the radio running in the tray.
    pub close_to_tray: bool,
    /// Which events the engineer calls about by itself.
    pub proactive: Proactive,
    /// When to be quiet, what to look out for, what to keep in mind.
    pub rules: Rules,
}

/// `settings.json` in the user's configuration directory.
pub fn default_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("vr-radio").join("settings.json"))
}

impl Settings {
    /// A missing file is the default settings; a broken one is an error.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).with_context(|| format!("{}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("{}", path.display())),
        }
    }

    /// Replaces the file atomically, so a crash cannot leave half of it.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path).with_context(|| format!("{}", path.display()))
    }

    /// Fills the options of `config` the user did not give explicitly.
    pub fn apply(&self, config: &mut Config, explicit: impl Fn(&str) -> bool) {
        fn set<T: Clone>(target: &mut T, value: &Option<T>, explicit: bool) {
            if let Some(v) = value
                && !explicit
            {
                *target = v.clone();
            }
        }
        set(&mut config.agent, &self.agent, explicit("agent"));
        config.saved_options.clone_from(&self.agent_options);
        set(&mut config.speaker, &self.speaker, explicit("speaker"));
        // A file edited by hand must not deafen the driver.
        let volume = self.volume.map(|v| v.min(MAX_VOLUME));
        set(&mut config.volume, &volume, explicit("volume"));
        set(&mut config.speed, &self.speed, explicit("speed"));
        set(&mut config.stt, &self.stt, explicit("stt"));
        set(
            &mut config.stt_model,
            &self.stt_model,
            // A model belongs to its service: another URL has other models.
            explicit("stt_model") || explicit("stt_url"),
        );
        set(&mut config.hotkey, &self.hotkey, explicit("hotkey"));
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    fn full() -> Settings {
        Settings {
            agent: Some("codex".into()),
            agent_options: BTreeMap::from([(
                "codex".into(),
                AgentOptions {
                    radio: vec![("model".into(), "m".into())],
                    desk: vec![("reasoning_effort".into(), "high".into())],
                },
            )]),
            speaker: Some(8),
            volume: Some(60),
            speed: Some(1.5),
            stt: Some(SttBackend::Windows),
            stt_model: Some("m".into()),
            hotkey: Some("F13".into()),
            wheel: Some(Binding {
                device: "VID_346E&PID_0006".into(),
                name: "R9".into(),
                button: 40,
            }),
            close_to_tray: true,
            proactive: Proactive {
                lap: true,
                flag: false,
                ..Default::default()
            },
            rules: Rules {
                interval_s: 60,
                hold_calls: None,
                notes: vec!["短く".into()],
                ..Default::default()
            },
        }
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        full().save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), full());
        // Saving again replaces the file and leaves no temporary behind.
        Settings::default().save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        let files = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(files, 1);
    }

    #[test]
    fn broken_or_unreadable_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{").unwrap();
        assert!(Settings::load(&path).is_err());
        assert!(Settings::load(dir.path()).is_err());
        // Unknown and missing fields are fine.
        std::fs::write(&path, r#"{"speaker":2,"future":1}"#).unwrap();
        assert_eq!(Settings::load(&path).unwrap().speaker, Some(2));
        assert_eq!(
            Settings::load(&path).unwrap().proactive,
            Proactive::default()
        );
    }

    #[test]
    fn fills_only_what_was_not_given() {
        let before = Config::parse_from(["vr-radio"]);
        let applied = |explicit: &str| {
            let mut c = Config::parse_from(["vr-radio"]);
            full().apply(&mut c, |i| i == explicit);
            [
                c.agent == before.agent,
                c.speaker == before.speaker,
                c.stt == before.stt,
                c.stt_model == before.stt_model,
                c.hotkey == before.hotkey,
                c.volume == before.volume,
                c.speed == before.speed,
            ]
        };
        assert_eq!(applied(""), [false; 7]);
        assert_eq!(
            applied("agent"),
            [true, false, false, false, false, false, false]
        );
        assert_eq!(
            applied("speaker"),
            [false, true, false, false, false, false, false]
        );
        assert_eq!(
            applied("stt"),
            [false, false, true, false, false, false, false]
        );
        assert_eq!(
            applied("stt_model"),
            [false, false, false, true, false, false, false]
        );
        assert_eq!(
            applied("stt_url"),
            [false, false, false, true, false, false, false]
        );
        assert_eq!(
            applied("hotkey"),
            [false, false, false, false, true, false, false]
        );

        let mut c = Config::parse_from(["vr-radio"]);
        full().apply(&mut c, |_| false);
        assert_eq!(c.agent, "codex");
        assert_eq!(c.speaker, 8);
        assert_eq!(c.stt, SttBackend::Windows);
        assert_eq!(c.hotkey, "F13");
        assert_eq!((c.volume, c.speed), (60, 1.5));
        Settings::default().apply(&mut c, |_| false);
        assert_eq!(c.agent, "codex");
        assert_eq!(c.volume, 60);
        let loud = Settings {
            volume: Some(255),
            ..Default::default()
        };
        loud.apply(&mut c, |_| false);
        assert_eq!(c.volume, MAX_VOLUME);
    }

    #[test]
    fn default_path_is_per_user() {
        let path = default_path().expect("a home directory");
        assert!(path.ends_with("vr-radio/settings.json"));
    }
}
