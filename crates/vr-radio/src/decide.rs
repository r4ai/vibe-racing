//! Asks a decision model what a question is about, when its words alone do
//! not tell (`topic`). Such a model writes no text: it answers yes/no
//! questions with probabilities, in about 0.3 s.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::http;
use crate::topic::{Found, Topic};

/// Later than this the agent is asked without an answer: it looks the data
/// up itself.
const TIMEOUT: Duration = Duration::from_millis(800);
/// The connection is kept between questions: opening it costs another 0.2 s.
const KEEP: Duration = Duration::from_secs(600);
/// Told apart all but one of 60 labelled calls (docs/architecture.md).
const LIKELY: f64 = 0.5;

/// Tells what the driver's words are about.
pub trait Decide: Send + Sync + 'static {
    /// Blocking.
    fn decide(&self, text: &str) -> Result<Found>;

    /// Opens the connection, so that the next question does not. Blocking.
    fn warm_up(&self) {}
}

/// OpenRouter's Decisions API (alpha).
pub struct Decisions {
    agent: ureq::Agent,
    base_url: String,
    key: String,
    model: String,
}

impl Decisions {
    pub fn new(
        base_url: impl Into<String>,
        key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            agent: http::agent_keeping(TIMEOUT, KEEP),
            base_url: base_url.into().trim_end_matches('/').into(),
            key: key.into(),
            model: model.into(),
        }
    }
}

/// English: the models are trained on it, the calls may be in any language.
fn questions() -> Value {
    let noul = |instructions: &str, yes: &str, no: &str| {
        json!({
            "type": "noul",
            "instructions": instructions,
            "criteria": { "true": yes, "false": no },
        })
    };
    json!({
        "handling": noul(
            "Is the driver talking about how their car handles (understeer, oversteer, sliding, locking, wheelspin, grip, stability, braking feel), its garage setup, or in-car settings such as brake bias, traction control, ABS or anti-roll bars?",
            "Car balance, grip, setup or in-car adjustment is a topic of the call",
            "Gaps, positions, laps, fuel, pit timing, weather, flags, other cars, radio volume or small talk only",
        ),
        "car": noul(
            "Is the driver asking about the physical state of their own car: tyre wear, tyre pressure or temperature, damage, repairs, engine, water or oil temperature, warnings?",
            "Tyre state, damage, repairs, temperatures or warnings are a topic of the call",
            "Anything else, also pit timing, fuel amount, undercuts, or handling without tyres",
        ),
        "past": noul(
            "Does the call need data of an earlier session (a previous race, practice, qualifying, yesterday, last time), not of the current one?",
            "An earlier session or day is asked about or compared with",
            "Only the current session, also its earlier laps",
        ),
        "strategy": noul(
            "Is the driver asking about race strategy: when to make a pit stop, how long the tyres last or how lap times fall off, undercut or overcut, where they would rejoin, or what rivals' stops mean?",
            "Pit timing, tyre life, lap time fade, undercuts, overcuts or rivals' stops are a topic of the call",
            "Gaps, positions, fuel amount, handling, flags, weather, radio settings or small talk only",
        ),
        "long": noul(
            "Will answering take the race engineer a long look-up: searching the web, reading or writing files or issues, or analysing many laps or several sessions in depth?",
            "Research, writing something down, or a deep analysis of much data is asked for",
            "A question answered at once from live telemetry, a setting of the radio, or small talk",
        ),
    })
}

fn found(v: &Value) -> Result<Found> {
    let answers = v["answers"].as_object().context("no answers")?;
    let likely = |name: &str| answers.get(name).and_then(|a| a["noul"].as_f64()) >= Some(LIKELY);
    Ok(Found {
        topics: [
            (Topic::Handling, "handling"),
            (Topic::Car, "car"),
            (Topic::Past, "past"),
            (Topic::Strategy, "strategy"),
        ]
        .into_iter()
        .filter(|(_, name)| likely(name))
        .map(|(t, _)| t)
        .collect(),
        long: likely("long"),
    })
}

impl Decide for Decisions {
    fn decide(&self, text: &str) -> Result<Found> {
        let resp = self
            .agent
            .post(format!("{}/decisions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.key))
            .send_json(json!({
                "model": self.model,
                "state": { "radio_call": text },
                "questions": questions(),
            }))?;
        found(&http::check(resp, "decisions")?.body_mut().read_json()?)
    }

    fn warm_up(&self) {
        // Any answer will do, it only has to be read to the end.
        if let Ok(mut resp) = self.agent.head(&self.base_url).call() {
            let _ = resp.body_mut().read_to_vec();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake;

    fn answers(handling: f64, car: f64, past: f64, long: f64) -> Value {
        let a = |p: f64| json!({"type": "noul", "noul": p});
        json!({"answers": {"handling": a(handling), "car": a(car), "past": a(past), "long": a(long)}})
    }

    #[test]
    fn likely_topics_are_found() {
        let f = found(&answers(0.5, 0.49, 0.9, 0.1)).unwrap();
        assert_eq!(f.topics, [Topic::Handling, Topic::Past]);
        assert!(!f.long);
        let f = found(&answers(0.0, 0.5, 0.0, 0.5)).unwrap();
        assert_eq!(f.topics, [Topic::Car]);
        assert!(f.long);
    }

    #[test]
    fn asks_about_the_call() {
        let server = fake::serve(|_| {
            fake::Response::json(
                r#"{"answers":{"handling":{"type":"noul","noul":0.9}},"usage":{"cost":1e-5}}"#,
            )
        });
        let d = Decisions::new(format!("{}/api/alpha/", server.url), "sk-test", "m");
        assert_eq!(d.decide("ケツが出る").unwrap().topics, [Topic::Handling]);
        let reqs = server.requests.lock().unwrap();
        assert_eq!(reqs[0].target, "/api/alpha/decisions");
        assert_eq!(reqs[0].header("authorization"), Some("Bearer sk-test"));
        let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(body["model"], "m");
        assert_eq!(body["state"]["radio_call"], "ケツが出る");
        let names: Vec<_> = body["questions"].as_object().unwrap().keys().collect();
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn missing_answers_are_no() {
        assert_eq!(found(&json!({"answers": {}})).unwrap(), Found::default());
        assert!(found(&json!({"error": {}})).is_err());
    }
}
