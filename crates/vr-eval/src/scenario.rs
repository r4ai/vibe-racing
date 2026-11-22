//! Scenario files: races, and what the driver says during them.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use vr_replay::SynthConfig;

use crate::facts::Fact;

/// One race and one agent session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stint {
    pub id: String,
    pub world: WorldSpec,
    pub calls: Vec<Call>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WorldSpec {
    /// Session time of the first call.
    pub start_s: f64,
    pub cars: usize,
    pub player_idx: usize,
    pub laps: i32,
    pub fuel_start_l: f64,
    pub fuel_per_lap_l: f64,
    pub player_pit_lap: Option<i32>,
    pub rival_pit_lap: Option<i32>,
    pub yellow: Option<(f64, f64)>,
    /// The game disconnects at `start_s`; the last data ages.
    pub stale_after_start: bool,
}

impl Default for WorldSpec {
    fn default() -> Self {
        let c = SynthConfig::default();
        Self {
            start_s: 0.0,
            cars: c.cars,
            player_idx: c.player_idx,
            laps: c.laps,
            fuel_start_l: c.fuel_start_l,
            fuel_per_lap_l: c.fuel_per_lap_l,
            player_pit_lap: c.player_pit_lap,
            rival_pit_lap: c.rival_pit_lap,
            yellow: c.yellow,
            stale_after_start: false,
        }
    }
}

impl WorldSpec {
    pub fn synth(&self) -> SynthConfig {
        SynthConfig {
            cars: self.cars,
            laps: self.laps,
            player_idx: self.player_idx,
            fuel_start_l: self.fuel_start_l,
            fuel_per_lap_l: self.fuel_per_lap_l,
            player_pit_lap: self.player_pit_lap,
            rival_pit_lap: self.rival_pit_lap,
            yellow: self.yellow,
            ..SynthConfig::default()
        }
    }
}

/// What a good engineer does with the call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Behavior {
    /// Answers with every point.
    #[default]
    Answer,
    /// Answers, or asks briefly because the call was hard to hear.
    AnswerOrAsk,
    /// Acknowledges; nothing was asked.
    Ack,
    /// Nothing was said: no race information out of nowhere.
    Silent,
    /// Says that the data is missing or old.
    AdmitUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Noise {
    /// Speech to engine/wind noise ratio.
    pub snr_db: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub id: String,
    /// `<pause 1.6>` is a silence of 1.6 s. Empty: the button alone.
    pub say: String,
    /// `vv:<style id>`, `gemini:<voice>`, `eleven:<voice>` or `aura:<voice>`.
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default)]
    pub noise: Option<Noise>,
    /// Pressed late / released early: speech cut away.
    #[serde(default)]
    pub cut_start_s: f32,
    #[serde(default)]
    pub cut_end_s: f32,
    /// Length of a call without speech.
    #[serde(default)]
    pub duration_s: f32,
    #[serde(default)]
    pub behavior: Behavior,
    /// Checked by number and keyword; the first one marks the moment the
    /// driver is informed.
    #[serde(default)]
    pub facts: Vec<Fact>,
    /// What the judges look for, in words.
    pub points: Vec<String>,
    #[serde(default)]
    pub barge_in: Option<BargeIn>,
}

fn default_voice() -> String {
    "vv:13".into()
}

/// The driver talks again before the answer is over; only the second
/// question is graded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BargeIn {
    /// Seconds after the end of the first question.
    pub after_s: f32,
    pub say: String,
    #[serde(default)]
    pub facts: Vec<Fact>,
    pub points: Vec<String>,
}

impl Call {
    /// The words without pause marks, as the judges see them.
    pub fn words(&self) -> String {
        strip_pauses(&self.say)
    }
}

/// A piece of a call: words, or a silence in seconds.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Words(String),
    Pause(f32),
}

pub fn pieces(say: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut rest = say;
    while let Some(i) = rest.find("<pause ") {
        push_words(&mut out, &rest[..i]);
        let Some(end) = rest[i..].find('>') else {
            rest = "";
            break;
        };
        let secs = rest[i + "<pause ".len()..i + end].trim();
        out.push(Piece::Pause(secs.parse().unwrap_or(0.0)));
        rest = &rest[i + end + 1..];
    }
    push_words(&mut out, rest);
    out
}

fn push_words(out: &mut Vec<Piece>, words: &str) {
    let words = words.trim_matches(['…', ' ']);
    if !words.is_empty() {
        out.push(Piece::Words(words.into()));
    }
}

pub fn strip_pauses(say: &str) -> String {
    let words: Vec<String> = pieces(say)
        .into_iter()
        .filter_map(|p| match p {
            Piece::Words(w) => Some(w),
            Piece::Pause(_) => None,
        })
        .collect();
    words.join("、")
}

pub fn load(path: &Path) -> Result<Vec<Stint>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
    let stints: Vec<Stint> =
        serde_yaml::from_str(&text).with_context(|| format!("{}", path.display()))?;
    let mut ids = BTreeSet::new();
    for s in &stints {
        for c in &s.calls {
            ensure!(
                ids.insert((s.id.clone(), c.id.clone())),
                "duplicate call {}/{}",
                s.id,
                c.id
            );
            ensure!(!c.points.is_empty(), "{}/{} has no points", s.id, c.id);
            ensure!(
                !c.say.is_empty() || c.duration_s > 0.0,
                "{}/{} is empty",
                s.id,
                c.id
            );
        }
    }
    Ok(stints)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios")
    }

    #[test]
    fn both_sets_load() {
        for name in ["dev.yaml", "heldout.yaml"] {
            let stints = load(&dir().join(name)).unwrap();
            assert!(stints.len() >= 5, "{name}");
        }
    }

    /// The held-out set must not repeat what the radio was tuned on.
    #[test]
    fn held_out_words_and_voices_are_new() {
        let calls = |name: &str| -> Vec<Call> {
            load(&dir().join(name))
                .unwrap()
                .into_iter()
                .flat_map(|s| s.calls)
                .collect()
        };
        let dev = calls("dev.yaml");
        let said: BTreeSet<_> = dev.iter().map(|c| c.words()).collect();
        let voices: BTreeSet<_> = dev.iter().map(|c| c.voice.clone()).collect();
        for c in calls("heldout.yaml").iter().filter(|c| !c.say.is_empty()) {
            assert!(!said.contains(&c.words()), "{}", c.id);
            assert!(!voices.contains(&c.voice), "{}", c.id);
        }
    }

    #[test]
    fn pauses() {
        assert_eq!(
            pieces("前の車……<pause 1.6>……何秒差？"),
            vec![
                Piece::Words("前の車".into()),
                Piece::Pause(1.6),
                Piece::Words("何秒差？".into())
            ]
        );
        assert_eq!(
            strip_pauses("あのさ……<pause 2>……燃料は？"),
            "あのさ、燃料は？"
        );
        assert_eq!(pieces(""), vec![]);
    }

    #[test]
    fn mistakes_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.yaml");
        let check = |y: &str| {
            std::fs::write(&p, y).unwrap();
            load(&p).map_err(|e| format!("{e:#}"))
        };
        let call = "{id: a, say: x, points: [p]}";
        assert!(check(&format!("[{{id: s, world: {{}}, calls: [{call}]}}]")).is_ok());
        let dup = check(&format!(
            "[{{id: s, world: {{}}, calls: [{call}, {call}]}}]"
        ));
        assert!(dup.unwrap_err().contains("duplicate call s/a"));
        let typo = check("[{id: s, world: {strat_s: 1}, calls: []}]");
        assert!(typo.is_err());
        let empty = check("[{id: s, world: {}, calls: [{id: a, say: '', points: [p]}]}]");
        assert!(empty.unwrap_err().contains("is empty"));
    }
}
