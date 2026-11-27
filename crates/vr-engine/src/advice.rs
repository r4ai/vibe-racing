//! Adjustments worth trying, from how the car behaves.
//!
//! Rules of thumb with their evidence; the engineer decides and says why.
//! Nothing is suggested that the car cannot adjust. How far to turn a control
//! comes from this car: the steps the driver turns it in, and what the changes
//! made so far did to the same tendency. Without either, one step, so that the
//! next laps tell whether it helped.

use serde::{Deserialize, Serialize};
use vr_core::{CarTelemetry, Setup};

use crate::garage::Experiment;
use crate::handling::Behaviour;
use crate::report::Adjustment;

/// What the driver changed in this car this session, and what came of it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Past<'a> {
    pub experiments: &'a [Experiment],
    pub adjustments: &'a [Adjustment],
}

/// Steps turned at most at once: past that, the car is another car.
const MAX_STEPS: f64 = 3.0;

/// The tendency a change is to take off, and how much of it is too much.
struct Aim {
    excess_s: f64,
    effect: fn(&Experiment) -> Option<f64>,
    what: &'static str,
}

impl Past<'_> {
    /// The smallest turn the driver made of a control: its step.
    fn step(&self, id: &str) -> Option<f64> {
        self.adjustments
            .iter()
            .filter(|a| a.control == id)
            .map(|a| (a.to - a.from).abs())
            .filter(|d| *d > 1e-6)
            .min_by(f64::total_cmp)
    }

    /// Changes of the control with what they did to the aim: value turned,
    /// change of the tendency (negative: less of it), lap.
    fn tried(&self, id: &str, aim: &Aim) -> Vec<(f64, f64, i32)> {
        self.experiments
            .iter()
            .filter(|e| e.control == id)
            .filter_map(|e| Some((e.to - e.from, (aim.effect)(e)?, e.lap)))
            .filter(|(d, _, _)| d.abs() > 1e-6)
            .collect()
    }
}

/// `57.25`, `2`, `0.5`.
fn fmt_fine(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Where to turn `c` to, and why that far: `dir` is the way the value goes
/// for the aim when it is known (+1 up), else what helped before tells.
fn sized(
    c: &vr_core::Control,
    dir: Option<f64>,
    aim: &Aim,
    past: &Past,
) -> Option<(String, String)> {
    let tried = past.tried(&c.id, aim);
    let helped: Vec<&(f64, f64, i32)> = tried.iter().filter(|t| t.1 < 0.0).collect();
    let dir = dir.or_else(|| helped.first().map(|t| t.0.signum()))?;
    let step = past.step(&c.id)?;
    let mut rates: Vec<(f64, &(f64, f64, i32))> = helped
        .iter()
        .filter(|t| t.0.signum() == dir)
        .map(|t| (-t.1 / t.0.abs(), *t))
        .collect();
    rates.sort_by(|a, b| a.0.total_cmp(&b.0));
    // The latest change this way decides; earlier ones that disagree with
    // each other leave one step to try, not a size to work out.
    let this_way = tried.iter().filter(|t| t.0.signum() == dir);
    let latest = this_way.clone().max_by_key(|t| t.2);
    let worse = latest.filter(|t| t.1 > 0.0);
    let mixed = this_way.clone().any(|t| t.1 > 0.0) && !rates.is_empty();
    if mixed && worse.is_none() {
        return Some((
            format!("to {}", fmt_fine(c.value + dir * step)),
            format!(
                "the changes this way so far disagree about {}: one step of {}, and compare",
                aim.what,
                fmt_fine(step)
            ),
        ));
    }
    let (steps, why) = match (rates.get(rates.len() / 2), worse) {
        (_, Some(w)) => {
            return Some((
                "leave it".into(),
                format!(
                    "turning it {} at lap {} gave {:+.1}s/lap more {}",
                    fmt_fine(w.0),
                    w.2,
                    w.1,
                    aim.what
                ),
            ));
        }
        (Some((rate, t)), _) => {
            let steps = (aim.excess_s / rate / step).round().clamp(1.0, MAX_STEPS);
            (
                steps,
                format!(
                    "turning it {} at lap {} took {:.1}s/lap of {} off; {:.1}s/lap too much is {} step(s) of {}; a first guess, check it after 2-3 laps",
                    fmt_fine(t.0),
                    t.2,
                    -t.1,
                    aim.what,
                    aim.excess_s,
                    steps,
                    fmt_fine(step)
                ),
            )
        }
        _ => (
            1.0,
            format!(
                "one step of {}, as the driver turns it; nothing measured yet",
                fmt_fine(step)
            ),
        ),
    };
    Some((
        format!("to {}", fmt_fine(c.value + dir * steps * step)),
        why,
    ))
}

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
    past: &Past,
) -> Vec<Advice> {
    let mut out = Vec::new();
    if laps >= MIN_LAPS {
        brakes(b, car, past, &mut out);
        traction(b, car, past, &mut out);
        balance(b, setup, &mut out);
    }
    tyres(car, &mut out);
    out
}

fn in_car(c: &vr_core::Control, change: &str, reason: String, risk: &str) -> Advice {
    Advice {
        place: Place::InCar,
        target: c.id.clone(),
        current: Some(fmt_fine(c.value)),
        change: change.into(),
        reason,
        risk: risk.into(),
    }
}

/// The same, sized from the car where it can be.
fn in_car_sized(
    c: &vr_core::Control,
    change: &str,
    reason: String,
    risk: &str,
    sizing: (Option<f64>, Aim, &Past),
) -> Advice {
    let (dir, aim, past) = sizing;
    match sized(c, dir, &aim, past) {
        Some((to, why)) => in_car(
            c,
            &format!("{change}: {to}"),
            format!("{reason}; {why}"),
            risk,
        ),
        None => in_car(c, &format!("one step {change}"), reason, risk),
    }
}

fn brakes(b: &Behaviour, car: &CarTelemetry, past: &Past, out: &mut Vec<Advice>) {
    let rear_locks = b.rear_lock_s >= REAR_LOCK_S;
    // Entry oversteer with no locking is the engine holding the rear back
    // where the car lets that be turned: one cause, one change at a time.
    let engine = !rear_locks
        && b.oversteer.entry >= ENTRY_OVERSTEER_S
        && control(car, &["engine_brake"]).is_some();
    if let Some(c) = control(car, &["brake_bias", "brake_bias_fine"]) {
        // Braking moves the load forward: the rear has the less of it the
        // harder the car stops, and locks first when the bias is too far back.
        if rear_locks || (b.oversteer.entry >= ENTRY_OVERSTEER_S && !engine) {
            let aim = if rear_locks {
                Aim {
                    excess_s: b.rear_lock_s - REAR_LOCK_S * 0.5,
                    effect: |e| e.rear_lock_s,
                    what: "rear locking",
                }
            } else {
                Aim {
                    excess_s: b.oversteer.entry - ENTRY_OVERSTEER_S * 0.5,
                    effect: |e| e.entry_oversteer_s,
                    what: "entry oversteer",
                }
            };
            out.push(in_car_sized(
                c,
                "forward (higher number)",
                format!(
                    "rear locking {:.1}s and entry oversteer {:.1}s per lap",
                    b.rear_lock_s, b.oversteer.entry
                ),
                "more entry understeer, front locking",
                (Some(1.0), aim, past),
            ));
        } else if b.understeer.entry >= ENTRY_UNDERSTEER_S {
            out.push(in_car_sized(
                c,
                "rearward (lower number)",
                format!(
                    "entry understeer {:.1}s per lap, no rear locking",
                    b.understeer.entry
                ),
                "rear instability on the brakes",
                (
                    Some(-1.0),
                    Aim {
                        excess_s: b.understeer.entry - ENTRY_UNDERSTEER_S * 0.5,
                        effect: |e| e.entry_understeer_s,
                        what: "entry understeer",
                    },
                    past,
                ),
            ));
        }
    }
    if engine && let Some(c) = control(car, &["engine_brake"]) {
        out.push(in_car_sized(
            c,
            "less engine braking",
            format!(
                "entry oversteer {:.1}s per lap without rear locking; a test, not a diagnosis: changes down, trail braking and the rear suspension do the same",
                b.oversteer.entry
            ),
            "less help slowing down, the brakes work harder",
            (
                None,
                Aim {
                    excess_s: b.oversteer.entry - ENTRY_OVERSTEER_S * 0.5,
                    effect: |e| e.entry_oversteer_s,
                    what: "entry oversteer",
                },
                past,
            ),
        ));
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

fn traction(b: &Behaviour, car: &CarTelemetry, past: &Past, out: &mut Vec<Advice>) {
    // Traction control acts on wheelspin: a slide with the wheels gripping is
    // the throttle or the balance, not a matter for it.
    let spins = b.wheelspin_s >= WHEELSPIN_S
        || b.oversteer.exit >= EXIT_OVERSTEER_S && b.wheelspin_s >= WHEELSPIN_S * 0.5;
    let tc = control(car, &["traction_control", "traction_control2"]);
    if spins {
        let reason = format!(
            "wheelspin {:.1}s and exit oversteer {:.1}s per lap",
            b.wheelspin_s, b.oversteer.exit
        );
        match (tc, control(car, &["throttle_shape"])) {
            (Some(c), _) => out.push(in_car_sized(
                c,
                "more intervention",
                reason,
                "slower exits when the grip is there",
                (
                    None,
                    // What is too much decides what the change is measured by.
                    if b.wheelspin_s >= WHEELSPIN_S {
                        Aim {
                            excess_s: b.wheelspin_s - WHEELSPIN_S * 0.5,
                            effect: |e| e.wheelspin_s,
                            what: "wheelspin",
                        }
                    } else {
                        Aim {
                            excess_s: b.oversteer.exit - EXIT_OVERSTEER_S * 0.5,
                            effect: |e| e.exit_oversteer_s,
                            what: "exit oversteer",
                        }
                    },
                    past,
                ),
            )),
            (None, Some(c)) => out.push(in_car(
                c,
                "one step softer pedal response",
                reason,
                "less direct throttle",
            )),
            (None, None) => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use vr_core::Control;

    fn tried(control: &str, from: f64, to: f64, lap: i32, rear_lock_s: f64) -> Experiment {
        Experiment {
            control: control.into(),
            from,
            to,
            lap,
            laps_before: 3,
            laps_after: 3,
            lap_time_s: None,
            understeer_s: None,
            oversteer_s: Some(0.0),
            wheelspin_s: None,
            rear_lock_s: Some(rear_lock_s),
            entry_oversteer_s: Some(0.0),
            entry_understeer_s: None,
            exit_oversteer_s: None,
        }
    }

    fn turned(control: &str, from: f64, to: f64) -> Adjustment {
        Adjustment {
            session_time_s: 0.0,
            lap: Some(3),
            control: control.into(),
            from,
            to,
        }
    }

    fn bias_advice(past: &Past) -> Advice {
        let b = Behaviour {
            rear_lock_s: 1.0,
            ..Default::default()
        };
        let car = CarTelemetry {
            controls: vec![Control {
                id: "brake_bias".into(),
                value: 56.0,
            }],
            ..Default::default()
        };
        advise(&b, 3, &car, None, past).remove(0)
    }

    #[test]
    fn how_far_comes_from_what_the_car_did() {
        // Nothing turned yet: one step, of a size nobody knows.
        let a = bias_advice(&Past::default());
        assert_eq!(a.change, "one step forward (higher number)");
        // The driver turns it in quarters: one quarter, nothing measured.
        let adjustments = [
            turned("brake_bias", 55.5, 55.75),
            turned("brake_bias", 55.75, 56.0),
        ];
        let past = Past {
            experiments: &[],
            adjustments: &adjustments,
        };
        let a = bias_advice(&past);
        assert_eq!(a.change, "forward (higher number): to 56.25");
        assert!(
            a.reason
                .ends_with("one step of 0.25, as the driver turns it; nothing measured yet")
        );
        // A quarter took 0.1 s of locking off: 0.8 s too much is three steps at most.
        let experiments = [tried("brake_bias", 55.75, 56.0, 4, -0.1)];
        let past = Past {
            experiments: &experiments,
            adjustments: &adjustments,
        };
        let a = bias_advice(&past);
        assert_eq!(a.change, "forward (higher number): to 56.75");
        assert!(
            a.reason.ends_with(
                "turning it 0.25 at lap 4 took 0.1s/lap of rear locking off; 0.8s/lap too much is 3 step(s) of 0.25; a first guess, check it after 2-3 laps"
            ),
            "{}",
            a.reason
        );
        // A half that took 0.8 s off: 0.8 s is two quarters.
        let experiments = [tried("brake_bias", 55.5, 56.0, 4, -0.8)];
        let past = Past {
            experiments: &experiments,
            adjustments: &adjustments,
        };
        assert_eq!(
            bias_advice(&past).change,
            "forward (higher number): to 56.5"
        );
        // One helped, a later one made it worse: the later one decides.
        let experiments = [
            tried("brake_bias", 55.5, 55.75, 4, -0.3),
            tried("brake_bias", 55.75, 56.0, 7, 0.2),
        ];
        let past = Past {
            experiments: &experiments,
            adjustments: &adjustments,
        };
        assert_eq!(
            bias_advice(&past).change,
            "forward (higher number): leave it"
        );
        // A later one helped after one that did not: one step, and compare.
        let experiments = [
            tried("brake_bias", 55.5, 55.75, 4, 0.2),
            tried("brake_bias", 55.75, 56.0, 7, -0.3),
        ];
        let past = Past {
            experiments: &experiments,
            adjustments: &adjustments,
        };
        let a = bias_advice(&past);
        assert_eq!(a.change, "forward (higher number): to 56.25");
        assert!(a.reason.ends_with("the changes this way so far disagree about rear locking: one step of 0.25, and compare"), "{}", a.reason);
        // Forward made it worse: not again.
        let experiments = [tried("brake_bias", 55.5, 56.0, 4, 0.3)];
        let past = Past {
            experiments: &experiments,
            adjustments: &adjustments,
        };
        let a = bias_advice(&past);
        assert_eq!(a.change, "forward (higher number): leave it");
        assert!(
            a.reason
                .ends_with("turning it 0.5 at lap 4 gave +0.3s/lap more rear locking")
        );
    }

    #[test]
    fn entry_oversteer_without_locking_is_one_change() {
        let b = Behaviour {
            oversteer: crate::handling::Phases {
                entry: 0.6,
                mid: 0.0,
                exit: 0.0,
            },
            ..Default::default()
        };
        let control = |id: &str| Control {
            id: id.into(),
            value: 3.0,
        };
        let mut car = CarTelemetry {
            controls: vec![control("brake_bias"), control("engine_brake")],
            ..Default::default()
        };
        let a = advise(&b, 3, &car, None, &Past::default());
        let targets: Vec<&str> = a.iter().map(|a| a.target.as_str()).collect();
        assert_eq!(targets, ["engine_brake"]);
        assert_eq!(a[0].change, "one step less engine braking");
        // Without engine braking to turn, the bias.
        car.controls.pop();
        let a = advise(&b, 3, &car, None, &Past::default());
        let targets: Vec<&str> = a.iter().map(|a| a.target.as_str()).collect();
        assert_eq!(targets, ["brake_bias"]);
    }

    #[test]
    fn traction_control_is_for_wheelspin() {
        let tc = |wheelspin_s: f64, exit: f64| {
            let b = Behaviour {
                wheelspin_s,
                oversteer: crate::handling::Phases {
                    entry: 0.0,
                    mid: 0.0,
                    exit,
                },
                ..Default::default()
            };
            let car = CarTelemetry {
                controls: vec![Control {
                    id: "traction_control".into(),
                    value: 2.0,
                }],
                ..Default::default()
            };
            advise(&b, 3, &car, None, &Past::default())
                .into_iter()
                .map(|a| a.change)
                .collect::<Vec<_>>()
        };
        assert_eq!(tc(0.8, 0.0), ["one step more intervention"]);
        // A slide with some spin is the spin's.
        assert_eq!(tc(0.4, 0.8), ["one step more intervention"]);
        // A slide with the wheels gripping is not.
        assert!(tc(0.39, 2.0).is_empty());
        assert!(tc(0.79, 0.0).is_empty());
    }

    /// A change of `control` from `from` to `to` at `lap` that did `set`.
    fn measured(
        control: &str,
        from: f64,
        to: f64,
        lap: i32,
        set: fn(&mut Experiment),
    ) -> Experiment {
        let mut e = Experiment {
            control: control.into(),
            from,
            to,
            lap,
            laps_before: 3,
            laps_after: 3,
            lap_time_s: None,
            understeer_s: None,
            oversteer_s: None,
            wheelspin_s: None,
            rear_lock_s: None,
            entry_oversteer_s: None,
            entry_understeer_s: None,
            exit_oversteer_s: None,
        };
        set(&mut e);
        e
    }

    /// The change advised for `target`, with whole steps of the controls and
    /// one change measured.
    fn sized_change(b: &Behaviour, controls: &[&str], target: &str, e: Experiment) -> String {
        let car = CarTelemetry {
            controls: controls
                .iter()
                .map(|id| Control {
                    id: (*id).into(),
                    value: 3.0,
                })
                .collect(),
            ..Default::default()
        };
        let adjustments: Vec<Adjustment> = controls.iter().map(|id| turned(id, 2.0, 3.0)).collect();
        let experiments = [e];
        let past = Past {
            experiments: &experiments,
            adjustments: &adjustments,
        };
        advise(b, 3, &car, None, &past)
            .into_iter()
            .find(|a| a.target == target)
            .unwrap()
            .change
    }

    #[test]
    fn each_tendency_sizes_its_own_change() {
        let phases = |entry: f64, exit: f64| crate::handling::Phases {
            entry,
            mid: 0.0,
            exit,
        };
        // Entry oversteer 0.32 s, 0.17 s over half its threshold; a step took
        // 0.1 s off: two steps.
        let b = Behaviour {
            oversteer: phases(0.32, 0.0),
            ..Default::default()
        };
        let e = measured("brake_bias", 2.0, 3.0, 4, |e| {
            e.entry_oversteer_s = Some(-0.1)
        });
        assert_eq!(
            sized_change(&b, &["brake_bias"], "brake_bias", e),
            "forward (higher number): to 5"
        );
        // With engine braking to turn, that instead; down helped.
        let e = measured("engine_brake", 4.0, 3.0, 4, |e| {
            e.entry_oversteer_s = Some(-0.1)
        });
        assert_eq!(
            sized_change(&b, &["brake_bias", "engine_brake"], "engine_brake", e),
            "less engine braking: to 1"
        );
        // Entry understeer 0.6 s, 0.35 s over; a step back took 0.2 s off.
        let b = Behaviour {
            understeer: phases(0.6, 0.0),
            ..Default::default()
        };
        let e = measured("brake_bias", 4.0, 3.0, 4, |e| {
            e.entry_understeer_s = Some(-0.2)
        });
        assert_eq!(
            sized_change(&b, &["brake_bias"], "brake_bias", e),
            "rearward (lower number): to 1"
        );
        // Wheelspin 0.85 s, 0.45 s over; a step took 0.2 s off.
        let b = Behaviour {
            wheelspin_s: 0.85,
            ..Default::default()
        };
        let e = measured("traction_control", 1.0, 2.0, 4, |e| {
            e.wheelspin_s = Some(-0.2)
        });
        assert_eq!(
            sized_change(&b, &["traction_control"], "traction_control", e),
            "more intervention: to 5"
        );
        // Some spin and a slide on the way out: the slide is measured.
        let b = Behaviour {
            wheelspin_s: 0.5,
            oversteer: phases(0.0, 0.85),
            ..Default::default()
        };
        let e = measured("traction_control", 1.0, 2.0, 4, |e| {
            e.exit_oversteer_s = Some(-0.2)
        });
        assert_eq!(
            sized_change(&b, &["traction_control"], "traction_control", e),
            "more intervention: to 5"
        );
    }

    #[test]
    fn a_change_that_did_nothing_neither_helped_nor_hurt() {
        let aim = Aim {
            excess_s: 0.3,
            effect: |e| e.rear_lock_s,
            what: "rear locking",
        };
        // A turn too small to be one is no step and no change.
        let adjustments = [turned("abs", 0.0, 1e-6), turned("abs", 0.0, 0.25)];
        let experiments = [
            measured("abs", 0.0, 1e-6, 2, |e| e.rear_lock_s = Some(-0.5)),
            measured("abs", 0.0, 0.25, 3, |e| e.rear_lock_s = Some(-0.1)),
        ];
        let past = Past {
            experiments: &experiments,
            adjustments: &adjustments,
        };
        assert_eq!(past.step("abs"), Some(0.25));
        assert_eq!(past.tried("abs", &aim), [(0.25, -0.1, 3)]);
        let c = Control {
            id: "abs".into(),
            value: 1.0,
        };
        // Nothing changed: no way to go is known.
        let experiments = [measured("abs", 0.0, 0.25, 3, |e| e.rear_lock_s = Some(0.0))];
        let past = Past {
            experiments: &experiments,
            adjustments: &adjustments,
        };
        assert_eq!(sized(&c, None, &aim, &past), None);
        // Helped, then a later one did nothing: not worse, nothing to disagree.
        let experiments = [
            measured("abs", 0.0, 0.25, 3, |e| e.rear_lock_s = Some(-0.1)),
            measured("abs", 0.25, 0.5, 5, |e| e.rear_lock_s = Some(0.0)),
        ];
        let past = Past {
            experiments: &experiments,
            adjustments: &adjustments,
        };
        let (to, why) = sized(&c, Some(1.0), &aim, &past).unwrap();
        assert_eq!(to, "to 1.75");
        assert!(
            why.starts_with("turning it 0.25 at lap 3 took 0.1s/lap"),
            "{why}"
        );
    }
}
