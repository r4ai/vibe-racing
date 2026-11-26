//! Whether the radio tells what a call is about, so that the data goes along
//! with it.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Deserialize;
use vr_radio::decide::Decide;
use vr_radio::topic::{self, Found, Topic};

use crate::report::percentile;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Labelled {
    pub say: String,
    #[serde(default)]
    pub topics: Vec<Name>,
    #[serde(default)]
    pub long: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Name {
    Handling,
    Car,
    Past,
    Strategy,
}

impl From<Name> for Topic {
    fn from(n: Name) -> Self {
        match n {
            Name::Handling => Topic::Handling,
            Name::Car => Topic::Car,
            Name::Past => Topic::Past,
            Name::Strategy => Topic::Strategy,
        }
    }
}

pub fn load(path: &Path) -> Result<Vec<Labelled>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("{}", path.display()))
}

/// How a set of calls was told apart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Score {
    pub calls: usize,
    /// Topics the agent would have to look up.
    pub missed: Vec<String>,
    /// Data nobody needs.
    pub extra: Vec<String>,
    /// Announced though short, or the other way round.
    pub long_wrong: Vec<String>,
    /// Calls the model was asked about, and how long each took.
    pub asked_ms: Vec<f64>,
    pub errors: usize,
}

impl Score {
    fn add(&mut self, l: &Labelled, f: &Found) {
        self.calls += 1;
        let wanted: Vec<Topic> = l.topics.iter().map(|n| Topic::from(*n)).collect();
        // The sets label no driving questions: corner data along with a
        // question of handling costs a little text, nothing more.
        for t in Topic::ALL
            .into_iter()
            .filter(|t| !matches!(t, Topic::Driving | Topic::Race))
        {
            let at = || format!("{} [{t:?}]", l.say);
            match (wanted.contains(&t), f.topics.contains(&t)) {
                (true, false) => self.missed.push(at()),
                (false, true) => self.extra.push(at()),
                _ => {}
            }
        }
        if l.long != f.long {
            self.long_wrong.push(l.say.clone());
        }
    }

    pub fn line(&self) -> String {
        let ms = |p| percentile(&self.asked_ms, p).map_or("--".into(), |v| format!("{v:.0}ms"));
        format!(
            "{} calls | missed {} | extra {} | long wrong {} | asked {} (p50 {}, p90 {}) | errors {}",
            self.calls,
            self.missed.len(),
            self.extra.len(),
            self.long_wrong.len(),
            self.asked_ms.len(),
            ms(0.5),
            ms(0.9),
            self.errors
        )
    }
}

/// As the radio does it: the words first, then the model. `words`: without
/// them the model is asked about every call.
pub fn score(set: &[Labelled], words: bool, model: Option<&dyn Decide>) -> Score {
    let mut s = Score::default();
    if let Some(m) = model {
        m.warm_up();
    }
    for l in set {
        let read = words.then(|| topic::read(&l.say)).flatten();
        let found = read.unwrap_or_else(|| {
            let Some(m) = model else {
                return Found::default();
            };
            let began = Instant::now();
            let answer = m.decide(&l.say);
            s.asked_ms.push(began.elapsed().as_secs_f64() * 1000.0);
            answer.unwrap_or_else(|_| {
                s.errors += 1;
                Found::default()
            })
        });
        s.add(l, &found);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(name: &str) -> Vec<Labelled> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
        load(&dir.join(name)).unwrap()
    }

    /// The words alone rarely fetch what nobody needs.
    #[test]
    fn words_find_most_topics() {
        let s = score(&set("topics.yaml"), true, None);
        assert_eq!(s.calls, 69);
        assert!(s.missed.len() <= 11, "{:?}", s.missed);
        assert!(s.extra.is_empty(), "{:?}", s.extra);
        assert!(s.asked_ms.is_empty());
    }

    #[test]
    fn held_out_calls_are_new() {
        let known = set("topics.yaml");
        let held_out = set("topics-heldout.yaml");
        assert!(held_out.len() >= 30);
        for h in held_out {
            assert!(known.iter().all(|k| k.say != h.say), "{}", h.say);
        }
    }

    struct Always(Found);
    impl Decide for Always {
        fn decide(&self, _: &str) -> Result<Found> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn the_model_is_asked_only_when_the_words_do_not_tell() {
        let call = |say: &str, topics| Labelled {
            say: say.into(),
            topics,
            long: false,
        };
        let set = [
            call("前との差は", vec![]),
            call("ケツが出る", vec![Name::Handling]),
        ];
        let car = Always(Found {
            topics: vec![Topic::Car],
            long: true,
        });
        let s = score(&set, true, Some(&car));
        assert_eq!(s.asked_ms.len(), 1);
        assert_eq!(s.missed, ["ケツが出る [Handling]"]);
        assert_eq!(s.extra, ["ケツが出る [Car]"]);
        assert_eq!(s.long_wrong, ["ケツが出る"]);
        let s = score(&set, false, Some(&car));
        assert_eq!((s.asked_ms.len(), s.extra.len()), (2, 2));
        assert!(s.line().contains("asked 2"));
    }
}
