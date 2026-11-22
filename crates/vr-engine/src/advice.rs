//! Adjustments worth trying, from how the car behaves.
//!
//! Rules of thumb with their evidence; the engineer decides and says why.
//! Nothing is suggested that the car cannot adjust, and only one step at a
//! time, so that the next laps tell whether it helped.

use serde::{Deserialize, Serialize};
use vr_core::{CarTelemetry, Setup};

use crate::handling::Behaviour;

/// Seconds per lap from which a tendency is worth a change. Some of each is
/// normal in a car driven at the limit.
const UNDERSTEER_S: f64 = 2.0;
const OVERSTEER_S: f64 = 1.2;
const ENTRY_UNDERSTEER_S: f64 = 0.5;
const ENTRY_OVERSTEER_S: f64 = 0.3;
const EXIT_OVERSTEER_S: f64 = 0.8;
const WHEELSPIN_S: f64 = 0.8;
const REAR_LOCK_S: f64 = 0.4;
const ABS_S: f64 = 3.0;
/// Difference across a tyre, in °C.
const CAMBER_C: f64 = 15.0;
const PRESSURE_C: f64 = 6.0;
/// Laps before the numbers mean something.
pub const MIN_LAPS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// From the cockpit, while driving.
    InCar,
    /// At a pit stop.
    Pit,
    /// In the garage, between sessions.
    Garage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Advice {
    pub place: Place,
    /// Control id or setup item, e.g. `"brake_bias"`.
    pub target: String,
    /// What it is set to now.
    pub current: Option<String>,
    pub change: String,
    /// The numbers it rests on.
    pub reason: String,
    /// What gets worse.
    pub risk: String,
}

fn control<'a>(c: &'a CarTelemetry, ids: &[&str]) -> Option<&'a vr_core::Control> {
    ids.iter()
        .find_map(|id| c.controls.iter().find(|k| k.id == *id))
}

/// Garage items whose path mentions all the words.
fn item<'a>(setup: Option<&'a Setup>, words: &[&str]) -> Option<&'a vr_core::SetupItem> {
    setup?.items.iter().find(|i| {
        let path = i.path.to_ascii_lowercase();
        words.iter().all(|w| path.contains(w))
    })
}

pub(crate) fn fmt_value(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// `laps`: how many the behaviour is the mean of.
pub fn advise(
    b: &Behaviour,
    laps: usize,
    car: &CarTelemetry,
    setup: Option<&Setup>,
) -> Vec<Advice> {
    let mut out = Vec::new();
    if laps >= MIN_LAPS {
        brakes(b, car, &mut out);
        traction(b, car, &mut out);
        balance(b, setup, &mut out);
    }
    tyres(car, &mut out);
    out
}

fn in_car(c: &vr_core::Control, change: &str, reason: String, risk: &str) -> Advice {
    Advice {
        place: Place::InCar,
        target: c.id.clone(),
        current: Some(fmt_value(c.value)),
        change: change.into(),
        reason,
        risk: risk.into(),
    }
}

fn brakes(b: &Behaviour, car: &CarTelemetry, out: &mut Vec<Advice>) {
    if let Some(c) = control(car, &["brake_bias", "brake_bias_fine"]) {
        let locks = b.rear_lock_s >= REAR_LOCK_S || b.oversteer.entry >= ENTRY_OVERSTEER_S;
        if locks {
            out.push(in_car(
                c,
                "forward 0.5-1.0 % (higher number)",
                format!(
                    "rear locking {:.1}s and entry oversteer {:.1}s per lap",
                    b.rear_lock_s, b.oversteer.entry
                ),
                "more entry understeer, front locking",
            ));
        } else if b.understeer.entry >= ENTRY_UNDERSTEER_S {
            out.push(in_car(
                c,
                "rearward 0.5 % (lower number)",
                format!(
                    "entry understeer {:.1}s per lap, no rear locking",
                    b.understeer.entry
                ),
                "rear instability on the brakes",
            ));
        }
    }
    if b.abs_s >= ABS_S
        && let Some(c) = control(car, &["abs"])
    {
        out.push(in_car(
            c,
            "one step less intervention",
            format!("ABS working {:.1}s per lap", b.abs_s),
            "locking when braking late",
        ));
    }
}

fn traction(b: &Behaviour, car: &CarTelemetry, out: &mut Vec<Advice>) {
    let spins = b.wheelspin_s >= WHEELSPIN_S || b.oversteer.exit >= EXIT_OVERSTEER_S;
    let tc = control(car, &["traction_control", "traction_control2"]);
    if spins {
        let reason = format!(
            "wheelspin {:.1}s and exit oversteer {:.1}s per lap",
            b.wheelspin_s, b.oversteer.exit
        );
        match (tc, control(car, &["throttle_shape"])) {
            (Some(c), _) => out.push(in_car(
                c,
                "one step more intervention",
                reason,
                "slower exits when the grip is there",
            )),
            (None, Some(c)) => out.push(in_car(
                c,
                "one step softer pedal response",
                reason,
                "less direct throttle",
            )),
            (None, None) => {}
        }
    } else if b.understeer.exit >= UNDERSTEER_S
        && b.wheelspin_s < 0.1
        && let Some(c) = tc
    {
        out.push(in_car(
            c,
            "one step less intervention",
            format!(
                "exit understeer {:.1}s per lap with almost no wheelspin",
                b.understeer.exit
            ),
            "wheelspin as the tyres wear",
        ));
    }
    // The anti-roll bars some cars adjust from the cockpit.
    let under = b.understeer.mid + b.understeer.exit;
    let over = b.oversteer.mid + b.oversteer.exit;
    if let Some(c) = control(car, &["anti_roll_front"])
        && tight(under, over)
    {
        out.push(in_car(
            c,
            "one step softer",
            format!("understeer {under:.1}s per lap from mid-corner on"),
            "lazier turn-in, looser rear",
        ));
    }
    if let Some(c) = control(car, &["anti_roll_rear"])
        && loose(under, over)
    {
        out.push(in_car(
            c,
            "one step softer",
            format!("oversteer {over:.1}s per lap from mid-corner on"),
            "more understeer",
        ));
    }
}

/// The car mostly understeers / oversteers: each against what is normal.
fn tight(under: f64, over: f64) -> bool {
    under >= UNDERSTEER_S && under / UNDERSTEER_S > over / OVERSTEER_S
}

fn loose(under: f64, over: f64) -> bool {
    over >= OVERSTEER_S && over / OVERSTEER_S >= under / UNDERSTEER_S
}

fn balance(b: &Behaviour, setup: Option<&Setup>, out: &mut Vec<Advice>) {
    let under = b.understeer.mid + b.understeer.exit;
    let over = b.oversteer.mid + b.oversteer.exit;
    let (words, change, reason, risk): (&[&[&str]], _, _, _) = if tight(under, over) {
        (
            &[&["front", "arb"], &["front", "flap"], &["front", "wing"]],
            "softer front bar / one step more front wing",
            format!("understeer {under:.1}s per lap from mid-corner on"),
            "looser rear, and in fast corners first",
        )
    } else if loose(under, over) {
        (
            &[&["rear", "arb"], &["rear", "wing"]],
            "softer rear bar / one step more rear wing",
            format!("oversteer {over:.1}s per lap from mid-corner on"),
            "more understeer, more drag with the wing",
        )
    } else {
        return;
    };
    for w in words {
        if let Some(i) = item(setup, w) {
            out.push(Advice {
                place: Place::Garage,
                target: i.path.clone(),
                current: Some(i.value.clone()),
                change: change.into(),
                reason: reason.clone(),
                risk: risk.into(),
            });
        }
    }
}

fn tyres(car: &CarTelemetry, out: &mut Vec<Advice>) {
    for (name, t) in car.tyres.all() {
        let [Some(outer), Some(mid), Some(inner)] = t.temp_c else {
            continue;
        };
        let pressure = mid - (outer + inner) / 2.0;
        let current = t.cold_kpa.map(|p| format!("{p:.0} kPa cold"));
        if pressure.abs() >= PRESSURE_C {
            let (change, reason) = if pressure > 0.0 {
                ("3-5 kPa lower", "hotter in the middle")
            } else {
                ("3-5 kPa higher", "colder in the middle")
            };
            out.push(Advice {
                place: Place::Pit,
                target: format!("{name} pressure"),
                current: current.clone(),
                change: change.into(),
                reason: format!("{reason} than at the edges by {:.0}C", pressure.abs()),
                risk: "compare at the same point of a stint".into(),
            });
        }
        let camber = inner - outer;
        if camber.abs() >= CAMBER_C {
            let change = if camber > 0.0 {
                "less negative camber"
            } else {
                "more negative camber"
            };
            out.push(Advice {
                place: Place::Garage,
                target: format!("{name} camber"),
                current: None,
                change: change.into(),
                reason: format!(
                    "inner edge {:.0}C {} than the outer",
                    camber.abs(),
                    if camber > 0.0 { "hotter" } else { "colder" }
                ),
                risk: "grip in the corner against tyre life".into(),
            });
        }
    }
}
