//! Scenario files: races, and what the driver says during them.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use vr_replay::{Balance, SynthConfig};

use crate::facts::Fact;

/// One race and one agent session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stint {
    pub id: String,
    pub world: WorldSpec,
    /// The engineer calls by itself about race events.
    #[serde(default)]
    pub proactive: bool,
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
    /// Realism of the race (see `SynthConfig`).
    pub tyre_deg_s: f64,
    pub lap_noise_s: f64,
    pub mistake_one_in: u32,
    pub stops: Vec<(usize, i32)>,
    pub rain: Option<(f64, f64)>,
    pub rain_loss_s: f64,
    pub incident_at: Option<f64>,
    /// How the player's car misbehaves.
    pub balance: Option<BalanceSpec>,
    /// Session time the player's car is damaged, and the seconds of repairs.
    pub damage: Option<(f64, f64)>,
    /// Sessions driven before, the latest one first.
    pub past: Vec<PastSpec>,
}

/// Seconds per lap (see `vr_replay::Balance`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BalanceSpec {
    pub understeer_mid_s: f64,
    pub oversteer_exit_s: f64,
    pub rear_lock_s: f64,
    pub wheelspin_s: f64,
}

/// A race of an earlier day.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PastSpec {
    pub days_ago: u32,
    pub laps: i32,
    /// Starting place: 0 is the pole.
    pub player_idx: usize,
    pub fuel_per_lap_l: f64,
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
            tyre_deg_s: c.tyre_deg_s,
            lap_noise_s: c.lap_noise_s,
            mistake_one_in: c.mistake_one_in,
            stops: c.stops,
            rain: c.rain,
            rain_loss_s: c.rain_loss_s,
            incident_at: c.incident_at,
            balance: None,
            damage: None,
            past: Vec::new(),
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
            tyre_deg_s: self.tyre_deg_s,
            lap_noise_s: self.lap_noise_s,
            mistake_one_in: self.mistake_one_in,
            stops: self.stops.clone(),
            rain: self.rain,
            rain_loss_s: self.rain_loss_s,
            incident_at: self.incident_at,
            balance: self.balance.map(|b| Balance {
                understeer_mid_s: b.understeer_mid_s,
                oversteer_exit_s: b.oversteer_exit_s,
                rear_lock_s: b.rear_lock_s,
                wheelspin_s: b.wheelspin_s,
            }),
            damage: self.damage,
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
    /// The driver is upset: steadies them in a few words, then answers
    /// with facts.
    Calm,
    /// Changes a setting of the radio.
    Setting,
    /// The driver says nothing and something happened: the engineer calls.
    Call,
    /// Nothing worth a call happened.
    Quiet,
}

impl Behavior {
    /// Nobody asked anything.
    pub fn listening(self) -> bool {
        matches!(self, Self::Call | Self::Quiet)
    }
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
    /// What the driver means; the judges read this.
    #[serde(default)]
    pub say: String,
    /// What a transcriber made of it (cut, misheard, repeated words): typed
    /// in text mode instead of `say`.
    #[serde(default)]
    pub typed: Option<String>,
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
    /// Settings of the radio after the call, e.g. `volume<100`. Any other
    /// change counts as wrong.
    #[serde(default)]
    pub settings: Vec<String>,
    /// The driver says nothing until this session time.
    #[serde(default)]
    pub listen_until: Option<f64>,
    /// Session time of what the engineer should call about: times are
    /// measured from it.
    #[serde(default)]
    pub event_at: Option<f64>,
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

    /// What text mode types.
    pub fn typed_words(&self) -> String {
        self.typed.clone().unwrap_or_else(|| self.words())
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
                c.behavior.listening() == c.listen_until.is_some(),
                "{}/{}: `call` and `quiet` need listen_until, others must not have it",
                s.id,
                c.id
            );
            ensure!(
                !c.behavior.listening() || s.proactive,
                "{}/{}: the engineer of this stint does not call",
                s.id,
                c.id
            );
            ensure!(
                !c.say.is_empty() || c.duration_s > 0.0 || c.listen_until.is_some(),
                "{}/{} is empty",
                s.id,
                c.id
            );
            crate::setting::check_syntax(&c.settings)
                .with_context(|| format!("{}/{}", s.id, c.id))?;
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
        for name in [
            "dev.yaml",
            "heldout.yaml",
            "hard.yaml",
            "features.yaml",
            "features-heldout.yaml",
            "rules.yaml",
            "strategy.yaml",
            "complex.yaml",
            "complex-heldout.yaml",
            "pressure.yaml",
            "real-radio.yaml",
            "real-radio-heldout.yaml",
        ] {
            let stints = load(&dir().join(name)).unwrap();
            assert!(stints.len() >= 3, "{name}");
        }
    }

    /// The held-out sets must not repeat what the radio was tuned on.
    #[test]
    fn held_out_words_and_voices_are_new() {
        new_to("dev.yaml", "heldout.yaml");
        new_to("features.yaml", "features-heldout.yaml");
        new_to("complex.yaml", "complex-heldout.yaml");
        new_to("real-radio.yaml", "real-radio-heldout.yaml");
    }

    fn new_to(known: &str, held_out: &str) {
        let calls = |name: &str| -> Vec<Call> {
            load(&dir().join(name))
                .unwrap()
                .into_iter()
                .flat_map(|s| s.calls)
                .collect()
        };
        let dev = calls(known);
        let said: BTreeSet<_> = dev.iter().map(|c| c.words()).collect();
        let voices: BTreeSet<_> = dev.iter().map(|c| c.voice.clone()).collect();
        for c in calls(held_out).iter().filter(|c| !c.say.is_empty()) {
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
        let one = |c: &str| check(&format!("[{{id: s, world: {{}}, calls: [{c}]}}]"));
        let call = one("{id: a, behavior: call, listen_until: 9, points: [p]}");
        assert!(call.unwrap_err().contains("does not call"));
        let call = one("{id: a, behavior: call, points: [p]}");
        assert!(call.unwrap_err().contains("listen_until"));
        let call = one("{id: a, say: x, listen_until: 9, points: [p]}");
        assert!(call.unwrap_err().contains("listen_until"));
        let call = one("{id: a, say: x, settings: [volume], points: [p]}");
        assert!(call.unwrap_err().contains("s/a"));
        assert!(
            check(
                "[{id: s, world: {}, proactive: true, \
                 calls: [{id: a, behavior: quiet, listen_until: 9, points: [p]}]}]"
            )
            .is_ok()
        );
    }
}
