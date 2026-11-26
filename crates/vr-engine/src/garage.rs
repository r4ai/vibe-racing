//! What to change in the garage, and what the changes before did.
//!
//! Every finding names its evidence and the items of this car's setup it is
//! about, with their values. Rules of thumb decide the direction; how far is
//! left to one step at a time and the comparison afterwards.

use serde::{Deserialize, Serialize};
use vr_core::{Setup, SetupItem};

use crate::SessionRecord;
use crate::chassis::{Chassis, MIN_LAPS};
use crate::handling::{Balance, Behaviour, HandlingLap};
use crate::report::{Adjustment, LapRecord};
use crate::util::mean;

/// Seconds per lap a wheel locks or spins before it is worth a change.
const LOCK_S: f64 = 0.15;
const SPIN_S: f64 = 0.6;
/// Seconds per lap the floor may touch.
const TOUCH_S: f64 = 0.4;
/// Seconds per lap more of one side than is usual of it, per speed class.
const UNDERSTEER_S: f64 = 0.9;
const OVERSTEER_S: f64 = 0.5;
/// One side of the balance outweighs the other by this factor.
const CLEARLY: f64 = 1.5;
/// Locking this many times as long on one axle: a full step of bias, not half.
const FAR_APART: f64 = 3.0;
const FAR_APART_S: f64 = 0.5;
/// Across a loaded tyre, in °C: the inner edge should be a little hotter.
const INNER_MORE_C: f64 = 9.0;
const INNER_LESS_C: f64 = -1.0;
const MIDDLE_C: f64 = 3.0;
/// Share of the time a damper works fast; above, the car is upset by bumps.
const FAST_DAMPER: f64 = 0.25;
/// Bump and rebound times that differ by this factor.
const LOPSIDED: f64 = 1.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    Brakes,
    Traction,
    SlowBalance,
    FastBalance,
    Balance,
    RideHeight,
    Camber,
    Pressure,
    Dampers,
}

impl Area {
    pub fn label(self) -> &'static str {
        match self {
            Self::Brakes => "brakes",
            Self::Traction => "traction",
            Self::SlowBalance => "balance in slow corners",
            Self::FastBalance => "balance in fast corners",
            Self::Balance => "balance",
            Self::RideHeight => "ride height",
            Self::Camber => "camber",
            Self::Pressure => "tyre pressure",
            Self::Dampers => "dampers",
        }
    }
}

/// Something the data shows, and what to do about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub area: Area,
    /// The numbers it rests on.
    pub evidence: String,
    /// Ways to change it, the gentlest first.
    pub options: Vec<Change>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// What to do, for this car if its setup is known.
    pub change: String,
    /// The items of the setup, as they are now.
    pub items: Vec<SetupItem>,
    /// What gets worse.
    pub risk: String,
}

/// Parts of a setup by what they do, as games call them.
#[derive(Debug, Clone, Copy)]
enum Part {
    FrontWing,
    RearWing,
    FrontBar,
    RearBar,
    FrontHeight,
    RearHeight,
    FrontSpring,
    RearSpring,
    BrakeBias,
    Preload,
    DriveAngle,
    CoastAngle,
    Camber(&'static str),
    Pressure(&'static str),
    Bump(&'static str),
    Rebound(&'static str),
}

/// Words the folder and the name of an item must contain, all of one of the
/// alternatives.
fn words(p: Part) -> Vec<Vec<&'static str>> {
    match p {
        Part::FrontWing => vec![
            vec!["front", "flap"],
            vec!["front", "wing"],
            vec!["splitter"],
        ],
        Part::RearWing => vec![
            vec!["rear", "wingangle"],
            vec!["rear", "wing"],
            vec!["wing"],
        ],
        Part::FrontBar => vec![
            vec!["front", "arbposition"],
            vec!["front", "arbblade"],
            vec!["front", "arb"],
        ],
        Part::RearBar => vec![
            vec!["rear", "arbposition"],
            vec!["rear", "arbblade"],
            vec!["rear", "arb"],
        ],
        Part::FrontHeight => vec![
            vec!["front", "pushrod"],
            vec!["front", "springperch"],
            vec!["front", "rideheight"],
        ],
        Part::RearHeight => vec![
            vec!["rear", "pushrod"],
            vec!["rear", "springperch"],
            vec!["rear", "rideheight"],
        ],
        Part::FrontSpring => vec![
            vec!["front", "heavespringrate"],
            vec!["front", "springrate"],
            vec!["front", "torsionbar"],
        ],
        Part::RearSpring => vec![
            vec!["rear", "rdspringrate"],
            vec!["rear", "thirdspring"],
            vec!["rear", "springrate"],
        ],
        Part::BrakeBias => vec![
            vec!["brakepressurebias"],
            vec!["brakebias"],
            vec!["frontbrake"],
        ],
        Part::Preload => vec![vec!["preload"]],
        Part::DriveAngle => vec![vec!["driveangle"], vec!["diff", "ramp"]],
        Part::CoastAngle => vec![vec!["coastangle"]],
        Part::Camber(c) => vec![vec![c, "/camber"]],
        Part::Pressure(c) => vec![vec![c, "coldpressure"], vec![c, "startingpressure"]],
        Part::Bump(c) => vec![vec![c, "bumpstiff"], vec![c, "lsbump"], vec![c, "bump"]],
        Part::Rebound(c) => vec![
            vec![c, "reboundstiff"],
            vec![c, "lsrebound"],
            vec![c, "rebound"],
        ],
    }
}

/// The items of one part; both sides of an axle count once.
fn items(setup: Option<&Setup>, p: Part) -> Vec<SetupItem> {
    let Some(setup) = setup else {
        return Vec::new();
    };
    for alternative in words(p) {
        let mut found: Vec<SetupItem> = Vec::new();
        for i in &setup.items {
            let path = i.path.to_ascii_lowercase();
            // What the game works out from the others cannot be set.
            let shown = path.contains("aerocalculator") || path.contains("last");
            if !shown
                && alternative.iter().all(|w| path.contains(w))
                && !found.iter().any(|f| {
                    let leaf = |p: &str| p.rsplit('/').next().unwrap_or_default().to_string();
                    leaf(&f.path) == leaf(&i.path) && f.value == i.value
                })
            {
                found.push(i.clone());
            }
        }
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

fn change(setup: Option<&Setup>, parts: &[Part], what: &str, risk: &str) -> Option<Change> {
    let items: Vec<SetupItem> = parts.iter().flat_map(|p| items(setup, *p)).collect();
    // A part the car does not have cannot be changed.
    (setup.is_none() || !items.is_empty()).then(|| Change {
        change: what.into(),
        items,
        risk: risk.into(),
    })
}

fn finding(area: Area, evidence: String, options: Vec<Option<Change>>) -> Option<Finding> {
    let options: Vec<Change> = options.into_iter().flatten().collect();
    (!options.is_empty()).then_some(Finding {
        area,
        evidence,
        options,
    })
}

/// Everything the data shows about the setup, the clearest first.
pub fn findings(
    b: Option<&Behaviour>,
    laps: usize,
    chassis: Option<&Chassis>,
    setup: Option<&Setup>,
) -> Vec<Finding> {
    let mut out: Vec<Option<Finding>> = Vec::new();
    let chassis = chassis.filter(|c| c.laps >= MIN_LAPS);
    if let Some(c) = chassis {
        out.push(brakes(c, setup));
        out.push(traction(c, setup));
    }
    if let Some(b) = b.filter(|_| laps >= crate::advice::MIN_LAPS) {
        out.extend(balance(b, setup));
    }
    if let Some(c) = chassis {
        out.extend(ride(c, setup));
        out.extend(tyres(c, setup));
        out.extend(dampers(c, setup));
    }
    out.into_iter().flatten().collect()
}

fn brakes(c: &Chassis, setup: Option<&Setup>) -> Option<Finding> {
    let front = c.lock_s.lf + c.lock_s.rf;
    let rear = c.lock_s.lr + c.lock_s.rr;
    if front.max(rear) < LOCK_S {
        return None;
    }
    let deepest = c.deepest_lock;
    let evidence = format!(
        "locking per lap: LF {:.2}s RF {:.2}s LR {:.2}s RR {:.2}s, deepest slip {:.0}/{:.0}/{:.0}/{:.0}%{}",
        c.lock_s.lf,
        c.lock_s.rf,
        c.lock_s.lr,
        c.lock_s.rr,
        deepest.lf * 100.0,
        deepest.rf * 100.0,
        deepest.lr * 100.0,
        deepest.rr * 100.0,
        c.front_brake.map_or(String::new(), |s| format!(
            ", {:.1}% of the pressure at the front",
            s * 100.0
        ))
    );
    let (front_first, other) = (front >= rear * CLEARLY, rear >= front * CLEARLY);
    // The axle that locks has more brake than grip; the further apart the
    // two axles lock, the further the share of the pressure is off.
    let by = |more: f64, less: f64| {
        // Far apart, and by enough to be more than a moment of locking.
        if more >= less * FAR_APART && more - less >= FAR_APART_S {
            "1.0"
        } else {
            "0.5"
        }
    };
    let now = c.front_brake.map_or(String::new(), |s| {
        format!(" from {:.1}% at the front", s * 100.0)
    });
    let options = if front_first {
        vec![change(
            setup,
            &[Part::BrakeBias],
            &format!(
                "bias rearward {} %{now} (the fronts lock first)",
                by(front, rear)
            ),
            "rear instability on the brakes",
        )]
    } else if other {
        vec![
            change(
                setup,
                &[Part::BrakeBias],
                &format!(
                    "bias forward {} %{now} (the rears lock first)",
                    by(rear, front)
                ),
                "front locking, entry understeer",
            ),
            change(
                setup,
                &[Part::CoastAngle, Part::Preload],
                "more locking off the throttle (smaller coast angle, more preload)",
                "entry understeer",
            ),
        ]
    } else {
        vec![change(
            setup,
            &[Part::BrakeBias],
            "bias is right, both axles lock alike: less peak pressure, release earlier",
            "longer braking",
        )]
    };
    finding(Area::Brakes, evidence, options)
}

fn traction(c: &Chassis, setup: Option<&Setup>) -> Option<Finding> {
    let s = c.spin_s;
    let front = s.lf + s.rf;
    let rear = s.lr + s.rr;
    if front.max(rear) < SPIN_S {
        return None;
    }
    let (axle, l, r) = if rear >= front {
        ("rear", s.lr, s.rr)
    } else {
        ("front", s.lf, s.rf)
    };
    let one = l.max(r) >= l.min(r) * 2.0;
    let evidence = format!("{axle} wheelspin per lap: left {l:.2}s right {r:.2}s");
    let options = if one {
        vec![
            change(
                setup,
                &[Part::Preload],
                "more preload: one wheel spins alone",
                "power understeer, snappier when both let go",
            ),
            change(
                setup,
                &[Part::DriveAngle],
                "smaller drive angle (more locking on power)",
                "power understeer",
            ),
            change(
                setup,
                &[Part::RearBar],
                "softer rear bar",
                "more understeer mid-corner",
            ),
        ]
    } else {
        vec![
            change(
                setup,
                &[Part::RearBar],
                "softer rear bar: both wheels spin",
                "more understeer mid-corner",
            ),
            change(
                setup,
                &[Part::RearWing],
                "one step more rear wing",
                "drag, understeer in fast corners",
            ),
            change(
                setup,
                &[Part::RearSpring],
                "softer rear springs",
                "more squat, slower reactions",
            ),
        ]
    };
    finding(Area::Traction, evidence, options)
}

/// Understeers (`Some(true)`), oversteers, or neither clearly.
fn side(b: &Balance) -> Option<bool> {
    let (u, o) = (b.understeer_s / UNDERSTEER_S, b.oversteer_s / OVERSTEER_S);
    if u >= 1.0 && u >= o * CLEARLY {
        Some(true)
    } else if o >= 1.0 && o >= u * CLEARLY {
        Some(false)
    } else {
        None
    }
}

fn balance(b: &Behaviour, setup: Option<&Setup>) -> Vec<Option<Finding>> {
    let [slow, medium, fast] = b.by_speed;
    let text = |name: &str, x: &Balance| {
        format!(
            "{name}: understeer {:.1}s oversteer {:.1}s per lap",
            x.understeer_s, x.oversteer_s
        )
    };
    let mechanical = |tight: bool| {
        if tight {
            vec![
                change(
                    setup,
                    &[Part::FrontBar],
                    "softer front bar",
                    "lazier turn-in",
                ),
                change(
                    setup,
                    &[Part::RearBar],
                    "stiffer rear bar",
                    "less traction, looser on kerbs",
                ),
                change(
                    setup,
                    &[Part::Preload],
                    "less preload",
                    "less stable on entry",
                ),
            ]
        } else {
            vec![
                change(
                    setup,
                    &[Part::RearBar],
                    "softer rear bar",
                    "more understeer",
                ),
                change(
                    setup,
                    &[Part::FrontBar],
                    "stiffer front bar",
                    "more understeer, front tyres work harder",
                ),
                change(
                    setup,
                    &[Part::RearSpring],
                    "softer rear springs",
                    "more roll, slower reactions",
                ),
            ]
        }
    };
    let aero = |tight: bool| {
        if tight {
            vec![
                change(
                    setup,
                    &[Part::FrontWing],
                    "one step more front wing",
                    "looser in fast corners",
                ),
                change(
                    setup,
                    &[Part::RearHeight],
                    "rear 1 mm higher (more rake)",
                    "looser on the brakes",
                ),
                change(
                    setup,
                    &[Part::RearWing],
                    "one step less rear wing",
                    "less rear grip, more top speed",
                ),
            ]
        } else {
            vec![
                change(
                    setup,
                    &[Part::RearWing],
                    "one step more rear wing",
                    "drag, understeer in fast corners",
                ),
                change(
                    setup,
                    &[Part::FrontWing],
                    "one step less front wing",
                    "understeer in fast corners",
                ),
                change(
                    setup,
                    &[Part::RearHeight],
                    "rear 1 mm lower (less rake)",
                    "less front grip",
                ),
            ]
        }
    };
    let (s, f) = (side(&slow), side(&fast));
    match (s, f) {
        // The same everywhere.
        (Some(a), Some(b)) if a == b => vec![finding(
            Area::Balance,
            format!(
                "{}, {}, {}",
                text("slow", &slow),
                text("medium", &medium),
                text("fast", &fast)
            ),
            mechanical(a).into_iter().take(2).chain(aero(a)).collect(),
        )],
        _ => vec![
            s.and_then(|t| {
                finding(
                    Area::SlowBalance,
                    format!("{}, {}", text("slow", &slow), text("fast", &fast)),
                    mechanical(t),
                )
            }),
            f.and_then(|t| {
                finding(
                    Area::FastBalance,
                    format!("{}, {}", text("fast", &fast), text("slow", &slow)),
                    aero(t),
                )
            }),
            // Neither slow nor fast, but in between.
            side(&medium)
                .filter(|_| s.is_none() && f.is_none())
                .and_then(|t| {
                    let options = mechanical(t).into_iter().take(1).chain(aero(t)).collect();
                    finding(Area::Balance, text("medium", &medium), options)
                }),
        ],
    }
}

fn ride(c: &Chassis, setup: Option<&Setup>) -> Vec<Option<Finding>> {
    let heights = c
        .ride
        .last()
        .map(|r| {
            format!(
                ", at top speed front {:.1} mm rear {:.1} mm",
                r.front_mm, r.rear_mm
            )
        })
        .unwrap_or_default();
    let touch = |s: f64, name: &str, height, spring| {
        (s >= TOUCH_S).then(|| {
            finding(
                Area::RideHeight,
                format!("the {name} touches the ground {s:.1}s per lap{heights}"),
                vec![
                    change(
                        setup,
                        &[height],
                        &format!("{name} 1-2 mm higher"),
                        "less downforce, the balance shifts to the other axle",
                    ),
                    change(
                        setup,
                        &[spring],
                        &format!("stiffer {name} springs (heave / third spring first)"),
                        "harsher over bumps and kerbs",
                    ),
                ],
            )
        })?
    };
    vec![
        touch(
            c.touch_front_s,
            "front",
            Part::FrontHeight,
            Part::FrontSpring,
        ),
        touch(c.touch_rear_s, "rear", Part::RearHeight, Part::RearSpring),
    ]
}

fn corner_words(name: &str) -> &'static str {
    match name {
        "LF" => "leftfront",
        "RF" => "rightfront",
        "LR" => "leftrear",
        _ => "rightrear",
    }
}

fn tyres(c: &Chassis, setup: Option<&Setup>) -> Vec<Option<Finding>> {
    let mut out = Vec::new();
    for (name, t) in c.loaded_c.all() {
        let Some([outer, middle, inner]) = *t else {
            continue;
        };
        let evidence = format!(
            "{name} loaded: outer {outer:.0} middle {middle:.0} inner {inner:.0} degrees C{}",
            match name {
                "LF" => c.pressure_kpa.lf,
                "RF" => c.pressure_kpa.rf,
                "LR" => c.pressure_kpa.lr,
                _ => c.pressure_kpa.rr,
            }
            .map_or(String::new(), |p| format!(", {p:.0} kPa hot"))
        );
        let corner = corner_words(name);
        let across = inner - outer;
        let camber = if across > INNER_MORE_C {
            Some("less negative camber (0.2-0.3)")
        } else if across < INNER_LESS_C {
            Some("more negative camber (0.2-0.3)")
        } else {
            None
        };
        out.push(camber.and_then(|what| {
            finding(
                Area::Camber,
                evidence.clone(),
                vec![change(
                    setup,
                    &[Part::Camber(corner)],
                    what,
                    "cornering grip against braking and tyre life",
                )],
            )
        }));
        let crown = middle - (inner + outer) / 2.0;
        let pressure = if crown > MIDDLE_C {
            Some("2-4 kPa lower")
        } else if crown < -MIDDLE_C {
            Some("2-4 kPa higher")
        } else {
            None
        };
        out.push(pressure.and_then(|what| {
            finding(
                Area::Pressure,
                evidence,
                vec![change(
                    setup,
                    &[Part::Pressure(corner)],
                    what,
                    "response and wear change, look at it warm again",
                )],
            )
        }));
    }
    out
}

fn dampers(c: &Chassis, setup: Option<&Setup>) -> Vec<Option<Finding>> {
    let mut out = Vec::new();
    for (name, d) in c.dampers.all() {
        let Some(d) = d else { continue };
        let corner = corner_words(name);
        let fast = d.bump_high + d.rebound_high;
        let evidence = format!(
            "{name}: rebound {:.0}/{:.0}% bump {:.0}/{:.0}% (slow/fast)",
            d.rebound_low * 100.0,
            d.rebound_high * 100.0,
            d.bump_low * 100.0,
            d.bump_high * 100.0
        );
        if fast >= FAST_DAMPER {
            out.push(finding(
                Area::Dampers,
                evidence,
                vec![change(
                    setup,
                    &[Part::Bump(corner), Part::Rebound(corner)],
                    "softer dampers: bumps and kerbs move the wheel fast",
                    "less support in transitions",
                )],
            ));
            continue;
        }
        let (bump, rebound) = (d.bump_low + d.bump_high, d.rebound_low + d.rebound_high);
        // The wheel spends longer on the side that is damped more.
        let what = if bump >= rebound * LOPSIDED {
            "less bump or more rebound damping"
        } else if rebound >= bump * LOPSIDED {
            "less rebound or more bump damping"
        } else {
            continue;
        };
        out.push(finding(
            Area::Dampers,
            evidence,
            vec![change(
                setup,
                &[Part::Bump(corner), Part::Rebound(corner)],
                what,
                "changes how the platform moves in every transition",
            )],
        ));
    }
    out
}

/// What a change in the car did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Experiment {
    pub control: String,
    pub from: f64,
    pub to: f64,
    pub lap: i32,
    /// Laps compared.
    pub laps_before: usize,
    pub laps_after: usize,
    /// Mean lap time after against before, less the fade; `None` without
    /// clean laps.
    pub lap_time_s: Option<f64>,
    /// After against before, seconds per lap.
    pub understeer_s: Option<f64>,
    pub oversteer_s: Option<f64>,
    pub wheelspin_s: Option<f64>,
    pub rear_lock_s: Option<f64>,
    /// Oversteer and understeer on the brakes only.
    #[serde(default)]
    pub entry_oversteer_s: Option<f64>,
    #[serde(default)]
    pub entry_understeer_s: Option<f64>,
    /// Oversteer on the throttle only.
    #[serde(default)]
    pub exit_oversteer_s: Option<f64>,
}

/// Laps compared on either side of a change.
const AROUND: usize = 3;

/// The changes made while driving and what came of them. Only laps between
/// two changes count for either.
pub fn experiments(
    adjustments: &[Adjustment],
    laps: &[LapRecord],
    handling: &[HandlingLap],
    fade_s: f64,
) -> Vec<Experiment> {
    // Several turns of a dial on one lap are one change.
    let mut changes: Vec<(i32, Adjustment)> = Vec::new();
    for a in adjustments {
        let Some(lap) = a.lap else { continue };
        match changes
            .iter_mut()
            .find(|(l, c)| *l == lap && c.control == a.control)
        {
            Some((_, c)) => c.to = a.to,
            None => changes.push((lap, a.clone())),
        }
    }
    changes.retain(|(_, c)| (c.from - c.to).abs() > 1e-9);
    let mut out = Vec::new();
    for (lap, c) in &changes {
        // The lap of the change itself belongs to neither side.
        let from = changes
            .iter()
            .map(|(l, _)| *l)
            .filter(|l| l < lap)
            .max()
            .unwrap_or(0);
        let to = changes
            .iter()
            .map(|(l, _)| *l)
            .filter(|l| l > lap)
            .min()
            .unwrap_or(i32::MAX);
        let before = |l: i32| l > from && l < *lap;
        let after = |l: i32| l > *lap && l < to;
        let times = |keep: &dyn Fn(i32) -> bool, last: bool| -> Vec<(i32, f64)> {
            let mut v: Vec<(i32, f64)> = laps
                .iter()
                .filter(|l| l.is_clean() && l.lap > 1 && keep(l.lap))
                .filter_map(|l| Some((l.lap, l.time_s?)))
                .collect();
            if last {
                v.reverse();
            }
            v.truncate(AROUND);
            v
        };
        let (b, a) = (times(&before, true), times(&after, false));
        let middle = |v: &[(i32, f64)]| {
            Some((
                mean(&v.iter().map(|x| f64::from(x.0)).collect::<Vec<_>>())?,
                mean(&v.iter().map(|x| x.1).collect::<Vec<_>>())?,
            ))
        };
        let lap_time_s = middle(&b)
            .zip(middle(&a))
            .map(|((lb, tb), (la, ta))| ta - tb - fade_s * (la - lb));
        let of = |v: &[(i32, f64)], f: &dyn Fn(&Behaviour) -> f64| {
            let x: Vec<f64> = handling
                .iter()
                .filter(|h| v.iter().any(|l| l.0 == h.lap))
                .map(|h| f(&h.behaviour))
                .collect();
            mean(&x)
        };
        let delta = |f: &dyn Fn(&Behaviour) -> f64| of(&b, f).zip(of(&a, f)).map(|(b, a)| a - b);
        out.push(Experiment {
            control: c.control.clone(),
            from: c.from,
            to: c.to,
            lap: *lap,
            laps_before: b.len(),
            laps_after: a.len(),
            lap_time_s,
            understeer_s: delta(&|b| b.understeer.total()),
            oversteer_s: delta(&|b| b.oversteer.total()),
            wheelspin_s: delta(&|b| b.wheelspin_s),
            rear_lock_s: delta(&|b| b.rear_lock_s),
            entry_oversteer_s: delta(&|b| b.oversteer.entry),
            entry_understeer_s: delta(&|b| b.understeer.entry),
            exit_oversteer_s: delta(&|b| b.oversteer.exit),
        });
    }
    out
}

/// Two runs of the same car on the same track with different setups.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// The other run.
    pub session: String,
    /// Path, value there, value here.
    pub changed: Vec<(String, String, String)>,
    /// This run against the other (negative: this one is faster).
    pub best_s: Option<f64>,
    pub clean_s: Option<f64>,
    /// Seconds per lap more understeer and oversteer here.
    pub by_speed: [Balance; 3],
}

/// What a run tells about the setup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupAnalysis {
    pub session: String,
    pub setup: Option<String>,
    pub track: Option<String>,
    pub car: Option<String>,
    /// Laps the behaviour is the mean of.
    pub laps: usize,
    pub behaviour: Option<Behaviour>,
    pub chassis: Option<Chassis>,
    pub findings: Vec<Finding>,
    pub experiments: Vec<Experiment>,
    pub comparisons: Vec<Comparison>,
}

/// What differs from run to run without anybody changing it.
const NOT_SET: [&str; 6] = [
    "last",
    "tread",
    "fuellevel",
    "cornerweight",
    "aerocalculator",
    "displaypage",
];

fn differences(there: &Setup, here: &Setup) -> Vec<(String, String, String)> {
    here.items
        .iter()
        .filter(|i| {
            let path = i.path.to_ascii_lowercase();
            !NOT_SET.iter().any(|n| path.contains(n))
        })
        .filter_map(|i| {
            let was = there.items.iter().find(|o| o.path == i.path)?;
            (was.value != i.value).then(|| (i.path.clone(), was.value.clone(), i.value.clone()))
        })
        .collect()
}

/// The laps driven at pace, without those that were wrecked.
fn at_pace(r: &SessionRecord) -> Vec<&HandlingLap> {
    let best = r.best_lap_s();
    r.handling
        .iter()
        .filter(|h| {
            let lap = r.laps.iter().find(|l| l.lap == h.lap);
            lap.is_some_and(|l| {
                l.is_clean() && l.lap > 1 && best.zip(l.time_s).is_some_and(|(b, t)| t < b * 1.04)
            })
        })
        .collect()
}

fn behaviour(r: &SessionRecord) -> (usize, Option<Behaviour>) {
    let laps = at_pace(r);
    let each: Vec<Behaviour> = laps.iter().map(|h| h.behaviour).collect();
    (each.len(), Behaviour::mean(&each))
}

/// Runs compared with.
const COMPARED: usize = 3;

/// Everything `r` tells about its setup. `others`: kept sessions, newest
/// first.
pub fn analyse(r: &SessionRecord, others: &[SessionRecord]) -> SetupAnalysis {
    let (laps, b) = behaviour(r);
    let comparisons = others
        .iter()
        .filter(|o| o.id != r.id && o.track == r.track && o.car == r.car)
        .filter_map(|o| {
            let changed = differences(o.setup.as_ref()?, r.setup.as_ref()?);
            let delta = |f: fn(&SessionRecord) -> Option<f64>| f(r).zip(f(o)).map(|(x, y)| x - y);
            let there = behaviour(o).1.zip(b);
            (!changed.is_empty()).then(|| Comparison {
                session: o.id.clone(),
                changed,
                best_s: delta(SessionRecord::best_lap_s),
                clean_s: delta(SessionRecord::avg_clean_lap_s),
                by_speed: there.map_or_else(Default::default, |(x, y)| {
                    std::array::from_fn(|i| Balance {
                        understeer_s: y.by_speed[i].understeer_s - x.by_speed[i].understeer_s,
                        oversteer_s: y.by_speed[i].oversteer_s - x.by_speed[i].oversteer_s,
                    })
                }),
            })
        })
        .take(COMPARED)
        .collect();
    SetupAnalysis {
        session: r.id.clone(),
        setup: r.setup.as_ref().and_then(|s| s.name.clone()),
        track: r.track.clone(),
        car: r.car.clone(),
        laps,
        behaviour: b,
        findings: findings(b.as_ref(), laps, r.chassis.as_ref(), r.setup.as_ref()),
        experiments: experiments(
            &r.adjustments,
            &r.laps,
            &r.handling,
            crate::strategy::fade_of(&r.laps).unwrap_or(0.0),
        ),
        chassis: r.chassis.clone(),
        comparisons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chassis::{Corners, Damper, Ride};

    fn setup() -> Setup {
        let item = |path: &str, value: &str| SetupItem {
            path: path.into(),
            value: value.into(),
        };
        Setup {
            name: Some("x".into()),
            items: vec![
                item("TiresAero/LeftFront/ColdPressure", "138 kPa"),
                item("TiresAero/LeftFront/LastHotPressure", "150 kPa"),
                item("TiresAero/FrontAero/FlapAngle", "26 deg"),
                item("TiresAero/RearAero/WingAngle", "23.0 deg"),
                item("TiresAero/AeroCalculator/FrontRhAtSpeed", "6.0 mm"),
                item("Chassis/Front/ArbPosition", "P2"),
                item("Chassis/Front/BrakePressureBias", "57.3%"),
                item("Chassis/LeftFront/PushrodLength", "636.4 mm"),
                item("Chassis/RightFront/PushrodLength", "636.4 mm"),
                item("Chassis/LeftFront/Camber", "-2.69 deg"),
                item("Chassis/LeftRear/SpringRate", "140 N/mm"),
                item("Chassis/LeftFront/BumpStiffness", "+4 clicks"),
                item("Chassis/LeftFront/ReboundStiffness", "+4 clicks"),
                item("Chassis/Rear/ArbPosition", "P2"),
                item("Drivetrain/Differential/Preload", "25 Nm"),
                item("Drivetrain/Differential/CoastAngle", "60 deg"),
            ],
            ..Setup::default()
        }
    }

    fn chassis() -> Chassis {
        Chassis {
            laps: 5,
            lock_s: Corners::default(),
            deepest_lock: Corners::default(),
            spin_s: Corners::default(),
            front_brake: Some(0.573),
            pressure_kpa: Corners::default(),
            loaded_c: Corners::default(),
            ride: vec![Ride {
                from_kmh: 180.0,
                to_kmh: None,
                front_mm: 9.0,
                rear_mm: 10.7,
            }],
            touch_front_s: 0.0,
            touch_rear_s: 0.0,
            dampers: Corners::default(),
        }
    }

    fn paths(c: &Change) -> Vec<&str> {
        c.items.iter().map(|i| i.path.as_str()).collect()
    }

    fn of(c: &Chassis) -> Vec<Finding> {
        findings(None, 0, Some(c), Some(&setup()))
    }

    #[test]
    fn the_axle_that_locks_decides_the_bias() {
        let mut c = chassis();
        assert!(of(&c).is_empty());
        c.lock_s.rf = 0.149;
        assert!(of(&c).is_empty());
        c.lock_s.rf = 0.15;
        let f = of(&c);
        assert_eq!((f.len(), f[0].area), (1, Area::Brakes));
        assert_eq!(paths(&f[0].options[0]), ["Chassis/Front/BrakePressureBias"]);
        assert!(f[0].options[0].change.contains("rearward"));
        assert!(f[0].evidence.contains("57.3%"), "{}", f[0].evidence);
        c.lock_s.lr = 0.3;
        let f = of(&c);
        assert!(f[0].options[0].change.contains("forward"));
        assert_eq!(
            paths(&f[0].options[1]),
            [
                "Drivetrain/Differential/CoastAngle",
                "Drivetrain/Differential/Preload"
            ]
        );
        c.lock_s.lf = 0.1;
        assert!(of(&c)[0].options[0].change.contains("bias is right"));
        // Too little driven to tell.
        c.laps = 1;
        assert!(of(&c).is_empty());
    }

    #[test]
    fn one_wheel_spinning_is_a_matter_of_the_differential() {
        let mut c = chassis();
        (c.spin_s.lr, c.spin_s.rr) = (0.2, 0.4);
        let f = of(&c);
        assert_eq!(f[0].area, Area::Traction);
        assert_eq!(paths(&f[0].options[0]), ["Drivetrain/Differential/Preload"]);
        // No such item in this car.
        assert!(f[0].options.iter().all(|o| !o.change.contains("drive")));
        c.spin_s.lr = 0.3;
        assert_eq!(paths(&of(&c)[0].options[0]), ["Chassis/Rear/ArbPosition"]);
        c.spin_s.lr = 0.19;
        assert!(of(&c).is_empty());
    }

    fn balanced(slow: (f64, f64), medium: (f64, f64), fast: (f64, f64)) -> Vec<Finding> {
        let b = |(understeer_s, oversteer_s)| Balance {
            understeer_s,
            oversteer_s,
        };
        let behaviour = Behaviour {
            by_speed: [b(slow), b(medium), b(fast)],
            ..Behaviour::default()
        };
        findings(Some(&behaviour), 2, None, Some(&setup()))
    }

    #[test]
    fn slow_corners_are_mechanical_and_fast_ones_aerodynamic() {
        let f = balanced((0.9, 0.0), (0.0, 0.0), (0.0, 0.0));
        assert_eq!((f.len(), f[0].area), (1, Area::SlowBalance));
        assert_eq!(paths(&f[0].options[0]), ["Chassis/Front/ArbPosition"]);
        let f = balanced((0.0, 0.0), (0.0, 0.0), (0.0, 0.5));
        assert_eq!((f.len(), f[0].area), (1, Area::FastBalance));
        assert_eq!(paths(&f[0].options[0]), ["TiresAero/RearAero/WingAngle"]);
        let f = balanced((0.0, 0.6), (0.0, 0.0), (1.0, 0.0));
        assert_eq!(
            f.iter().map(|f| f.area).collect::<Vec<_>>(),
            [Area::SlowBalance, Area::FastBalance]
        );
        assert!(f[1].options[0].change.contains("more front wing"));
        let f = balanced((1.0, 0.0), (1.0, 0.0), (1.0, 0.0));
        assert_eq!((f.len(), f[0].area), (1, Area::Balance));
        assert!(f[0].options.len() >= 4);
        let f = balanced((0.0, 0.0), (0.0, 0.5), (0.0, 0.0));
        assert_eq!((f.len(), f[0].area), (1, Area::Balance));
        assert!(balanced((0.89, 0.0), (0.0, 0.0), (0.0, 0.49)).is_empty());
        // Both at once is the limit, not a side.
        assert!(balanced((0.9, 0.5), (0.0, 0.0), (0.0, 0.0)).is_empty());
    }

    #[test]
    fn a_car_that_touches_the_ground_is_raised() {
        let mut c = chassis();
        c.touch_front_s = 0.39;
        assert!(of(&c).is_empty());
        c.touch_front_s = 0.4;
        let f = of(&c);
        assert_eq!(f[0].area, Area::RideHeight);
        assert_eq!(paths(&f[0].options[0]), ["Chassis/LeftFront/PushrodLength"]);
        assert!(f[0].evidence.contains("front 9.0 mm rear 10.7 mm"));
        // No front springs in this setup: one way left.
        assert_eq!(f[0].options.len(), 1);
    }

    #[test]
    fn temperatures_across_a_tyre_tell_camber_and_pressure() {
        let mut c = chassis();
        let mut with = |t: [f64; 3]| {
            c.loaded_c.lf = Some(t);
            of(&c)
                .into_iter()
                .map(|f| (f.area, f.options[0].change.clone()))
                .collect::<Vec<_>>()
        };
        assert!(with([70.0, 74.0, 79.0]).is_empty());
        assert_eq!(
            with([70.0, 75.0, 79.1]),
            [(Area::Camber, "less negative camber (0.2-0.3)".into())]
        );
        assert!(with([71.0, 70.5, 70.0]).is_empty());
        assert_eq!(with([72.0, 71.0, 70.0])[0].0, Area::Camber);
        assert!(with([70.0, 74.5, 73.0]).is_empty());
        assert_eq!(
            with([70.0, 75.6, 75.0]),
            [(Area::Pressure, "2-4 kPa lower".into())]
        );
        assert_eq!(
            with([75.0, 71.0, 75.0]),
            [(Area::Pressure, "2-4 kPa higher".into())]
        );
        let f = of(&c);
        assert_eq!(
            paths(&f[0].options[0]),
            ["TiresAero/LeftFront/ColdPressure"]
        );
    }

    #[test]
    fn dampers_are_judged_by_what_they_do() {
        let mut c = chassis();
        let mut with = |bl, bh, rl, rh| {
            c.dampers.lf = Some(Damper {
                bump_low: bl,
                bump_high: bh,
                rebound_low: rl,
                rebound_high: rh,
            });
            let f = of(&c);
            f.first().map(|f| f.options[0].change.clone())
        };
        assert_eq!(with(0.45, 0.04, 0.47, 0.04), None);
        assert!(with(0.375, 0.125, 0.375, 0.125).unwrap().contains("softer"));
        assert!(with(0.6, 0.0, 0.4, 0.0).unwrap().contains("less bump"));
        assert!(with(0.4, 0.0, 0.6, 0.0).unwrap().contains("less rebound"));
        assert_eq!(with(0.58, 0.0, 0.42, 0.0), None);
    }

    #[test]
    fn without_a_setup_the_direction_remains() {
        let mut c = chassis();
        c.lock_s.rf = 0.5;
        let f = findings(None, 0, Some(&c), None);
        assert_eq!((f.len(), f[0].options[0].items.len()), (1, 0));
    }

    fn lap(n: i32, time: f64) -> LapRecord {
        LapRecord {
            lap: n,
            time_s: Some(time),
            fuel_used_l: None,
            pit: false,
            caution: false,
            position: None,
            session_time_s: 0.0,
        }
    }

    fn adjusted(lap: i32, control: &str, from: f64, to: f64) -> Adjustment {
        Adjustment {
            session_time_s: 0.0,
            lap: Some(lap),
            control: control.into(),
            from,
            to,
        }
    }

    #[test]
    fn a_change_is_judged_by_the_laps_around_it() {
        let laps: Vec<LapRecord> = (1..=12)
            .map(|n| lap(n, if n <= 6 { 90.0 } else { 89.6 } + 0.1 * f64::from(n)))
            .collect();
        let handling: Vec<HandlingLap> = (1..=12)
            .map(|n| HandlingLap {
                lap: n,
                behaviour: Behaviour {
                    wheelspin_s: if n <= 6 { 2.0 } else { 1.5 },
                    ..Behaviour::default()
                },
            })
            .collect();
        let a = [
            adjusted(6, "brake_bias", 57.0, 56.5),
            adjusted(6, "brake_bias", 56.5, 56.0),
            adjusted(11, "tc", 2.0, 3.0),
            adjusted(12, "abs", 2.0, 2.0),
        ];
        let e = experiments(&a, &laps, &handling, 0.1);
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].from, e[0].to, e[0].lap), (57.0, 56.0, 6));
        assert_eq!((e[0].laps_before, e[0].laps_after), (3, 3));
        assert!((e[0].lap_time_s.unwrap() + 0.4).abs() < 1e-9);
        assert!((e[0].wheelspin_s.unwrap() + 0.5).abs() < 1e-9);
        assert_eq!(e[0].understeer_s, Some(0.0));
        // One lap after the second change, and that counts.
        assert_eq!((e[1].laps_before, e[1].laps_after), (3, 1));
        // None yet.
        let e = experiments(&a[..1], &laps[..6], &handling, 0.1);
        assert_eq!((e[0].laps_after, e[0].lap_time_s), (0, None));
        assert!(experiments(&[], &laps, &handling, 0.0).is_empty());
    }
}
