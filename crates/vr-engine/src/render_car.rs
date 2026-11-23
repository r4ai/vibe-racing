//! Text of the reports about the car (telemetry, setup, handling) and of
//! kept sessions.

use std::fmt::Write as _;

use vr_core::{CarTelemetry, Control, Tyre};

use crate::EventKind;
use crate::advice::{Place, fmt_value};
use crate::archive::{SessionRecord, fmt_utc};
use crate::chassis::{Chassis, Corners};
use crate::garage::{Experiment, SetupAnalysis};
use crate::handling::Balance;
use crate::live::{LIVE_KEYS, LiveReport};
use crate::render::{event_to_text, fmt_duration, fmt_lap, meta_line};
use crate::report::*;
use crate::state::{kind_label, warning_label};
use crate::strategy::{Basis, StrategyReport};

fn opt(v: Option<f64>, unit: &str, digits: usize) -> String {
    v.map(|v| format!("{v:.digits$}{unit}"))
        .unwrap_or_else(|| "--".into())
}

fn lap(v: Option<f64>) -> String {
    v.map(fmt_lap).unwrap_or_else(|| "--".into())
}

fn controls_text(c: &[Control]) -> String {
    if c.is_empty() {
        return "--".into();
    }
    c.iter()
        .map(|c| format!("{} {}", c.id, fmt_value(c.value)))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn edges(v: [Option<f64>; 3], k: f64) -> String {
    v.map(|x| {
        x.map(|x| format!("{:.0}", x * k))
            .unwrap_or_else(|| "--".into())
    })
    .join("/")
}

fn tyre_line(name: &str, t: &Tyre) -> String {
    let mut s = format!(
        "  {name}: tread {}% | {}C (outer/middle/inner)",
        edges(t.tread, 100.0),
        edges(t.temp_c, 1.0),
    );
    if let Some(p) = t.cold_kpa {
        let _ = write!(s, " | cold {p:.0}kPa");
    }
    if let Some(p) = t.hot_kpa {
        let _ = write!(s, " | hot {p:.0}kPa");
    }
    if let Some(d) = t.distance_m.filter(|d| *d > 0.0) {
        let _ = write!(s, " | {:.1}km", d / 1000.0);
    }
    if let Some(c) = t.core_c {
        let _ = write!(s, " | core {c:.0}C");
    }
    s
}

/// `body damage: front 0.12, left 0.03`; `None` when the body is whole or the
/// game does not tell.
fn body_damage(d: Option<&vr_core::Damage>) -> Option<String> {
    let hurt = d?.hurt();
    (!hurt.is_empty()).then(|| {
        let parts: Vec<String> = hurt.iter().map(|(n, v)| format!("{n} {v:.2}")).collect();
        format!("body damage {}", parts.join(", "))
    })
}

/// `brakes: LF 420C RF 430C LR 300C RR 310C`, where the game tells.
fn brakes_line(c: &CarTelemetry) -> Option<String> {
    let w = c.wheels.as_ref()?;
    let parts: Vec<String> = [("LF", &w.lf), ("RF", &w.rf), ("LR", &w.lr), ("RR", &w.rr)]
        .iter()
        .filter_map(|(n, w)| Some(format!("{n} {:.0}C", w.brake_temp_c?)))
        .collect();
    (!parts.is_empty()).then(|| format!("brakes: {}", parts.join(" ")))
}

/// One line for the brief; `None` when there is nothing to say.
pub(crate) fn car_line(c: &CarTelemetry) -> Option<String> {
    let mut parts = Vec::new();
    let damage: Vec<&str> = c
        .warnings
        .iter()
        .filter(|w| w.is_damage())
        .map(|w| warning_label(*w))
        .collect();
    if !damage.is_empty() {
        parts.push(format!("WARNING {}", damage.join(", ")));
    }
    if let Some(d) = body_damage(c.damage.as_ref()) {
        parts.push(d);
    }
    if let Some(p) = c.penalty_s.filter(|p| *p > 0.0) {
        parts.push(format!("PENALTY {p:.1}s"));
    }
    if let Some(s) = c.pit.repair_left_s.filter(|s| *s > 0.0) {
        parts.push(format!("repairs {s:.0}s"));
    }
    if let Some(s) = c.pit.tow_s.filter(|s| *s > 0.0) {
        parts.push(format!("BEING TOWED {s:.0}s"));
    }
    let tread: Vec<String> = c
        .tyres
        .all()
        .iter()
        .filter_map(|(n, t)| Some(format!("{n} {:.0}%", t.min_tread()? * 100.0)))
        .collect();
    if !tread.is_empty() {
        let name = c.tyre_compound.as_deref().unwrap_or("tyres");
        parts.push(format!("{name} {} (as of the last stop)", tread.join(" ")));
    }
    if let Some(n) = c.p2p_count {
        let active = if c.p2p_active == Some(true) {
            " ACTIVE"
        } else {
            ""
        };
        parts.push(format!("P2P/OTS {n} left{active}"));
    }
    if let Some(b) = c.battery_pct {
        parts.push(format!("battery {:.0}%", b * 100.0));
    }
    if let Some(a) = &c.alongside {
        parts.push(format!("car alongside: {a}"));
    }
    if !c.controls.is_empty() {
        parts.push(controls_text(&c.controls));
    }
    (!parts.is_empty()).then(|| format!("car: {}", parts.join(" | ")))
}

fn adjustments_text(out: &mut String, a: &[Adjustment]) {
    if a.is_empty() {
        return;
    }
    out.push_str("changed while driving:\n");
    for a in &a[a.len().saturating_sub(8)..] {
        let lap = a.lap.map(|l| format!("lap {l} ")).unwrap_or_default();
        let _ = writeln!(
            out,
            "  {lap}t={} {} {} -> {}",
            fmt_duration(a.session_time_s),
            a.control,
            fmt_value(a.from),
            fmt_value(a.to)
        );
    }
}

fn pit_line(p: &vr_core::PitState) -> String {
    let mut s = format!(
        "pit: {}",
        match p.open {
            Some(true) => "open",
            Some(false) => "CLOSED",
            None => "--",
        }
    );
    if p.in_stall == Some(true) {
        s.push_str(" | in the stall");
    }
    if let Some(status) = &p.status {
        let _ = write!(s, " | {status}");
    }
    if let Some(v) = p.speed_limit_mps {
        let _ = write!(s, " | limit {:.0}km/h", v * 3.6);
    }
    if !p.requested.is_empty() {
        let _ = write!(s, " | next stop: {}", p.requested.join(", "));
    }
    if let Some(l) = p.requested_fuel_l.filter(|l| *l > 0.0) {
        let _ = write!(s, " ({l:.1}L)");
    }
    if let Some(w) = &p.window {
        let unit = match w.unit {
            vr_core::WindowUnit::Lap => "lap",
            vr_core::WindowUnit::Minute => "min",
        };
        let _ = write!(s, " | window {unit} {:.0}-{:.0}", w.from, w.to);
    }
    match p.mandatory_done {
        Some(true) => s.push_str(" | mandatory stop done"),
        Some(false) => s.push_str(" | mandatory stop NOT done"),
        None => {}
    }
    s
}

fn damage_line(p: &vr_core::PitState) -> String {
    let fast = match (p.fast_repairs_used, p.fast_repairs_available) {
        (Some(u), Some(a)) => format!(" | fast repairs used {u}, left {a}"),
        (Some(u), None) => format!(" | fast repairs used {u}"),
        _ => String::new(),
    };
    format!(
        "damage: repairs {} (mandatory) {} (optional) | tow {}{fast}",
        opt(p.repair_left_s, "s", 0),
        opt(p.optional_repair_left_s, "s", 0),
        opt(p.tow_s, "s", 0),
    )
}

impl CarReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        let Some(c) = &self.car else {
            out.push_str("no car telemetry\n");
            return out;
        };
        if let Some(n) = &self.car_name {
            let _ = writeln!(out, "car: {n}");
        }
        let warnings: Vec<&str> = c.warnings.iter().map(|w| warning_label(*w)).collect();
        let _ = writeln!(
            out,
            "engine: water {} | oil {} {} | fuel pressure {} | voltage {} | warnings: {}",
            opt(self.water_temp_c, "C", 0),
            opt(self.oil_temp_c, "C", 0),
            opt(c.oil_press_bar, "bar", 1),
            opt(c.fuel_press_bar, "bar", 1),
            opt(c.voltage, "V", 1),
            if warnings.is_empty() {
                "none".into()
            } else {
                warnings.join(", ")
            }
        );
        let sets = match (c.tyre_sets_used, c.tyre_sets_available) {
            (Some(u), Some(a)) => format!(" | sets used {u}, left {a}"),
            (Some(u), None) => format!(" | sets used {u}"),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "tyres: {}{sets} (tread and temperatures are as of the last stop)",
            c.tyre_compound.as_deref().unwrap_or("--")
        );
        for (name, t) in c.tyres.all() {
            let _ = writeln!(out, "{}", tyre_line(name, t));
        }
        if let Some(b) = brakes_line(c) {
            let _ = writeln!(out, "{b}");
        }
        let _ = writeln!(out, "{}", pit_line(&c.pit));
        let _ = writeln!(out, "{}", damage_line(&c.pit));
        if let Some(d) = body_damage(c.damage.as_ref()) {
            let _ = writeln!(out, "{d}");
        }
        if let Some(p) = c.penalty_s.filter(|p| *p > 0.0) {
            let _ = writeln!(out, "penalty: {p:.1}s");
        }
        if let Some(n) = c.p2p_count {
            let active = if c.p2p_active == Some(true) {
                " (ACTIVE)"
            } else {
                ""
            };
            let _ = writeln!(out, "P2P/OTS: {n} left{active}");
        }
        if c.drs_active.is_some() || c.battery_pct.is_some() {
            let drs = match c.drs_active {
                Some(true) => "open",
                Some(false) => "closed",
                None => "--",
            };
            let battery = opt(c.battery_pct.map(|b| b * 100.0), "%", 0);
            let _ = writeln!(out, "DRS {drs} | battery {battery}");
        }
        let _ = writeln!(
            out,
            "delta: to session best {} | to optimal {}",
            opt(c.delta_to_session_best_s, "s", 2),
            opt(c.delta_to_optimal_s, "s", 2)
        );
        let bop = |v: Option<f64>| v.is_some_and(|v| v != 0.0);
        if bop(c.weight_penalty_kg) || bop(c.power_adjust_pct) {
            let _ = writeln!(
                out,
                "balance of performance: weight {} | power {}",
                opt(c.weight_penalty_kg, "kg", 0),
                opt(c.power_adjust_pct, "%", 1)
            );
        }
        let _ = writeln!(out, "settings in the car: {}", controls_text(&c.controls));
        adjustments_text(&mut out, &self.adjustments);
        out
    }
}

impl ChannelsReport {
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "{}\n{} of {} channels\n",
            meta_line(&self.meta),
            self.channels.len(),
            self.total
        );
        for c in &self.channels {
            let _ = write!(out, "{} = {}", c.name, c.value);
            if !c.unit.is_empty() {
                let _ = write!(out, " {}", c.unit);
            }
            if !c.description.is_empty() {
                let _ = write!(out, "  # {}", c.description);
            }
            out.push('\n');
        }
        out
    }
}

impl InfoReport {
    pub fn to_text(&self) -> String {
        let body = match &self.value {
            Some(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
            None => "no session info".into(),
        };
        format!("{}\n{body}\n", meta_line(&self.meta))
    }
}

impl SetupReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        let car = self.car_name.as_deref().unwrap_or("--");
        match &self.setup {
            None => {
                let _ = writeln!(
                    out,
                    "{car}: no setup available (the game tells it for your own car, once you have been in it)"
                );
            }
            Some(s) => {
                let _ = writeln!(
                    out,
                    "{car} | setup {}{}{}",
                    s.name.as_deref().unwrap_or("--"),
                    if s.modified == Some(true) {
                        " (modified since loaded)"
                    } else {
                        ""
                    },
                    if s.passed_tech == Some(false) {
                        " | FAILS TECH"
                    } else {
                        ""
                    }
                );
                let mut section = "";
                for i in &s.items {
                    let (dir, key) = i.path.rsplit_once('/').unwrap_or(("", &i.path));
                    if dir != section {
                        section = dir;
                        let _ = writeln!(out, "{dir}:");
                    }
                    let _ = writeln!(out, "  {key}: {}", i.value);
                }
                if !s.unavailable.is_empty() {
                    let _ = writeln!(out, "not told by the game: {}", s.unavailable.join(", "));
                }
            }
        }
        let _ = writeln!(
            out,
            "adjustable while driving: {}",
            controls_text(&self.controls)
        );
        adjustments_text(&mut out, &self.adjustments);
        out
    }
}

fn basis(b: Basis) -> &'static str {
    match b {
        Basis::Own => "own",
        Basis::Field => "field",
        Basis::Both => "own+field",
        Basis::Past => "earlier sessions",
        Basis::Unknown => "unknown",
    }
}

impl StrategyReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        if !self.missing.is_empty() {
            let _ = writeln!(
                out,
                "{}missing: {}",
                if self.plans.is_empty() {
                    "no plan yet, "
                } else {
                    ""
                },
                self.missing.join(", ")
            );
        }
        let Some(lap) = self.lap else { return out };
        let _ = writeln!(
            out,
            "lap {lap}/{} | stops done {} | tyres {} laps old{}",
            self.laps_total.map_or("--".into(), |l| l.to_string()),
            self.stops_done,
            self.tyre_age,
            match self.fuel_to_end {
                Some(true) => " | fuel reaches the end",
                Some(false) => " | fuel does not reach the end",
                None => "",
            }
        );
        if let Some(f) = &self.fade {
            let s = |v: Option<f64>| v.map_or("--".into(), |v| format!("{v:+.2}s"));
            let _ = writeln!(
                out,
                "fade per lap (tyres less fuel): {:+.2}s used ({}) | own {} over {} laps | field {} | new tyres {} at {} stops",
                f.used_s_per_lap,
                basis(f.basis),
                s(f.own_s_per_lap),
                f.own_laps,
                s(f.field_s_per_lap),
                s(f.fresh_gain_s),
                f.fresh_gain_stops,
            );
        }
        if let Some(c) = &self.tyre_call {
            let mut line = format!(
                "other tyres: {} {:.2}s/lap faster than yours",
                c.compound, c.gain_s_per_lap
            );
            if let Some(p) = c.payback_laps {
                let _ = write!(line, "; the stop pays back in {p:.1} laps");
            }
            if let Some(l) = c.laps_left {
                let _ = write!(line, "; {l} laps left");
            }
            match c.worth {
                Some(true) => line.push_str(": worth changing now"),
                Some(false) => line.push_str(": not worth it before the end"),
                None => {}
            }
            let _ = writeln!(out, "{line}");
        }
        if let Some(l) = self.pit_loss.seconds {
            let _ = writeln!(
                out,
                "a stop costs {l:.1}s ({}, {} stops)",
                basis(self.pit_loss.basis),
                self.pit_loss.stops
            );
        }
        if let Some((a, b)) = self.fuel_window {
            let _ = writeln!(out, "fuel window for a single stop: end of lap {a} to {b}");
        }
        if !self.plans.is_empty() {
            out.push_str("plans (stop at the end of lap):\n");
            for p in &self.plans {
                let laps: Vec<String> = p.stops.iter().map(i32::to_string).collect();
                let _ = writeln!(
                    out,
                    "  {}: +{:.1}s",
                    if laps.is_empty() {
                        "no stop".into()
                    } else {
                        laps.join(", ")
                    },
                    p.behind_best_s
                );
            }
        }
        if let Some(r) = &self.rejoin_now {
            let car = |c: &Option<CarRef>, s: Option<f64>| match (c, s) {
                (Some(c), Some(s)) => format!("#{} {} {s:.1}s", c.number, c.driver),
                _ => "nobody".into(),
            };
            let _ = writeln!(
                out,
                "stopping now rejoins P{} | ahead {} | behind {}{}",
                r.position,
                car(&r.ahead, r.ahead_s),
                car(&r.behind, r.behind_s),
                if r.traffic { " | TRAFFIC" } else { "" }
            );
        }
        if !self.rivals.is_empty() {
            out.push_str("rivals (gap + is ahead; net: once both made as many stops):\n");
        }
        for r in &self.rivals {
            let sum = |v: &[f64]| {
                v.iter()
                    .map(|g| format!("{g:+.1}"))
                    .collect::<Vec<_>>()
                    .join("/")
            };
            let mut line = format!(
                "  #{} {} {:+.1}s | net {:+.1}s | stops {} | tyres {} laps",
                r.car.number, r.car.driver, r.gap_s, r.net_gap_s, r.stops, r.tyre_age
            );
            if let Some(f) = r.fade_s_per_lap {
                let _ = write!(line, " | fade {f:+.2}s");
            }
            if let Some(p) = r.pace_delta_s {
                let _ = write!(line, " | pace {p:+.2}s vs you");
            }
            if r.in_pit {
                line.push_str(" | IN PIT");
            } else if r.just_stopped {
                line.push_str(" | just stopped");
            }
            if !r.undercut_gain_s.is_empty() {
                let _ = write!(
                    line,
                    " | undercut gains {}s in 1-4 laps{}",
                    sum(&r.undercut_gain_s),
                    r.undercut_laps
                        .map_or(", not enough".into(), |n| format!(", enough after {n}"))
                );
            }
            if let Some(n) = r.overcut_laps {
                let _ = write!(line, " | overcut after {n} laps");
            }
            if !r.threat_gain_s.is_empty() {
                let _ = write!(
                    line,
                    " | its stop gains {}s in 1-4 laps{}",
                    sum(&r.threat_gain_s),
                    r.threat_laps
                        .map_or(", no danger".into(), |n| format!(", passes after {n}"))
                );
            }
            let _ = writeln!(out, "{line}");
        }
        if !self.calls.is_empty() {
            out.push_str("calls:\n");
            for c in &self.calls {
                let _ = writeln!(out, "  {}", c.text());
            }
        }
        out
    }
}

impl LiveReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        for (key, unit) in LIVE_KEYS {
            let value = self.values.get(*key);
            let _ = writeln!(
                out,
                "{key} {}{}{unit}",
                value.map_or("--".into(), f64::to_string),
                if unit.is_empty() { "" } else { " | " }
            );
        }
        out
    }
}

fn signed(v: Option<f64>) -> String {
    v.map_or("--".into(), |v| format!("{v:+.1}s"))
}

fn experiments_text(out: &mut String, all: &[Experiment]) {
    if all.is_empty() {
        return;
    }
    out.push_str("what the changes did (up to 3 laps after against before):\n");
    for e in all {
        let _ = writeln!(
            out,
            "  lap {} {} {} -> {}: {}",
            e.lap,
            e.control,
            fmt_value(e.from),
            fmt_value(e.to),
            if e.laps_after == 0 || e.laps_before == 0 {
                "no clean laps on both sides yet".to_string()
            } else {
                format!(
                    "lap time {} | understeer {} | oversteer {} | wheelspin {} | rear locking {} ({} laps after)",
                    e.lap_time_s.map_or("--".into(), |v| format!("{v:+.2}s")),
                    signed(e.understeer_s),
                    signed(e.oversteer_s),
                    signed(e.wheelspin_s),
                    signed(e.rear_lock_s),
                    e.laps_after
                )
            }
        );
    }
}

fn by_speed_text(out: &mut String, b: &[Balance; 3], sign: bool) {
    let one = |v: f64| {
        if sign {
            format!("{v:+.1}")
        } else {
            format!("{v:.1}")
        }
    };
    let parts: Vec<String> = ["slow", "medium", "fast"]
        .iter()
        .zip(b)
        .map(|(n, b)| {
            format!(
                "{n} under {} over {}",
                one(b.understeer_s),
                one(b.oversteer_s)
            )
        })
        .collect();
    let _ = writeln!(
        out,
        "  by corner speed (<120, 120-180, >180 km/h): {}",
        parts.join(" | ")
    );
}

impl Chassis {
    fn to_text(&self) -> String {
        let mut out = format!("measured at the wheels over {} laps:\n", self.laps);
        let four = |c: &Corners<f64>, scale: f64, digits: usize| {
            c.all()
                .iter()
                .map(|(n, v)| format!("{n} {:.*}", digits, **v * scale))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let _ = writeln!(
            out,
            "  locking s/lap: {} (deepest slip %: {})",
            four(&self.lock_s, 1.0, 2),
            four(&self.deepest_lock, 100.0, 0)
        );
        let _ = writeln!(out, "  wheelspin s/lap: {}", four(&self.spin_s, 1.0, 2));
        if let Some(f) = self.front_brake {
            let _ = writeln!(out, "  brake pressure at the front: {:.1}%", f * 100.0);
        }
        for r in &self.ride {
            let _ = writeln!(
                out,
                "  ride height {:.0}{} km/h: front {:.1} rear {:.1} mm (rake {:+.1})",
                r.from_kmh,
                r.to_kmh.map_or("+".into(), |t| format!("-{t:.0}")),
                r.front_mm,
                r.rear_mm,
                r.rear_mm - r.front_mm
            );
        }
        let _ = writeln!(
            out,
            "  touching the ground s/lap: front {:.1} rear {:.1}",
            self.touch_front_s, self.touch_rear_s
        );
        for ((n, t), (_, p)) in self.loaded_c.all().iter().zip(self.pressure_kpa.all()) {
            if let Some([o, m, i]) = t {
                let _ = writeln!(
                    out,
                    "  {n} loaded: outer {o:.0} middle {m:.0} inner {i:.0} C{}",
                    p.map_or(String::new(), |p| format!(" | {p:.0} kPa hot"))
                );
            }
        }
        for (n, d) in self.dampers.all() {
            if let Some(d) = d {
                let _ = writeln!(
                    out,
                    "  {n} damper %: rebound slow {:.0} fast {:.0} | bump slow {:.0} fast {:.0}",
                    d.rebound_low * 100.0,
                    d.rebound_high * 100.0,
                    d.bump_low * 100.0,
                    d.bump_high * 100.0
                );
            }
        }
        out
    }
}

impl SetupAnalysis {
    pub fn to_text(&self) -> String {
        let dash = |s: &Option<String>| s.clone().unwrap_or_else(|| "--".into());
        let mut out = format!(
            "{} | {} | {} | setup {}\n",
            self.session,
            dash(&self.track),
            dash(&self.car),
            dash(&self.setup)
        );
        match &self.behaviour {
            None => out.push_str("no laps at pace to judge the balance\n"),
            Some(b) => {
                let _ = writeln!(
                    out,
                    "balance, mean of {} laps at pace, seconds per lap:",
                    self.laps
                );
                let _ = writeln!(
                    out,
                    "  understeer: entry {:.1} mid {:.1} exit {:.1} | oversteer: entry {:.1} mid {:.1} exit {:.1}",
                    b.understeer.entry,
                    b.understeer.mid,
                    b.understeer.exit,
                    b.oversteer.entry,
                    b.oversteer.mid,
                    b.oversteer.exit
                );
                by_speed_text(&mut out, &b.by_speed, false);
            }
        }
        match &self.chassis {
            Some(c) => out.push_str(&c.to_text()),
            None => out.push_str(
                "nothing measured at the wheels (import the game's telemetry file of this run)\n",
            ),
        }
        if self.findings.is_empty() {
            out.push_str("findings: none\n");
        }
        for (i, f) in self.findings.iter().enumerate() {
            let _ = writeln!(out, "{}. {}: {}", i + 1, f.area.label(), f.evidence);
            for o in &f.options {
                let items: Vec<String> = o
                    .items
                    .iter()
                    .map(|i| format!("{} = {}", i.path, i.value))
                    .collect();
                let _ = writeln!(
                    out,
                    "   - {}{} | risk: {}",
                    o.change,
                    if items.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", items.join(", "))
                    },
                    o.risk
                );
            }
        }
        experiments_text(&mut out, &self.experiments);
        for c in &self.comparisons {
            let s = |v: Option<f64>| v.map_or("--".into(), |v| format!("{v:+.2}s"));
            let _ = writeln!(
                out,
                "against {}: best {} | clean laps {}",
                c.session,
                s(c.best_s),
                s(c.clean_s)
            );
            by_speed_text(&mut out, &c.by_speed, true);
            for (path, there, here) in &c.changed {
                let _ = writeln!(out, "  {path}: {there} -> {here}");
            }
        }
        out
    }
}

impl HandlingReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        if !self.ready {
            out.push_str("not enough driving yet to judge the balance\n");
        }
        match &self.mean {
            None => out.push_str("no complete lap yet\n"),
            Some(b) => {
                let _ = writeln!(
                    out,
                    "mean of {} laps, seconds per lap (tendencies, not measurements):",
                    self.laps_sampled
                );
                let _ = writeln!(
                    out,
                    "  understeer: entry {:.1} | mid {:.1} | exit {:.1}",
                    b.understeer.entry, b.understeer.mid, b.understeer.exit
                );
                let _ = writeln!(
                    out,
                    "  oversteer: entry {:.1} | mid {:.1} | exit {:.1}",
                    b.oversteer.entry, b.oversteer.mid, b.oversteer.exit
                );
                let _ = writeln!(
                    out,
                    "  wheelspin {:.1} | rear locking {:.1} | ABS {:.1} | largest slip angle {:.0}deg",
                    b.wheelspin_s, b.rear_lock_s, b.abs_s, b.max_slip_deg
                );
                by_speed_text(&mut out, &b.by_speed, false);
            }
        }
        if !self.zones.is_empty() {
            out.push_str("where (share of the lap):\n");
            for z in &self.zones {
                let place = z.place.as_ref().map_or(String::new(), |p| format!(" {p}"));
                let _ = writeln!(
                    out,
                    "  {:.0}-{:.0}%{place}: {} {:.1}s/lap",
                    z.from_pct * 100.0,
                    z.to_pct * 100.0,
                    z.tendency.label(),
                    z.seconds
                );
            }
        }
        let _ = writeln!(
            out,
            "settings in the car: {}",
            controls_text(&self.controls)
        );
        adjustments_text(&mut out, &self.adjustments);
        experiments_text(&mut out, &self.experiments);
        if self.advice.is_empty() {
            out.push_str("suggestions: none from the data\n");
            return out;
        }
        out.push_str("suggestions (one step at a time):\n");
        for a in &self.advice {
            let place = match a.place {
                Place::InCar => "in the car",
                Place::Pit => "at the stop",
                Place::Garage => "in the garage",
            };
            let now = a
                .current
                .as_ref()
                .map(|c| format!(" (now {c})"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  [{place}] {}{now}: {} | because {} | risk: {}",
                a.target, a.change, a.reason, a.risk
            );
        }
        out
    }
}

impl SessionRecord {
    /// One line of the list.
    pub fn line(&self) -> String {
        let p = self.player.as_ref();
        let pos = p
            .and_then(|p| Some(format!("P{}/{}", p.car.position?, p.cars_total)))
            .unwrap_or_else(|| "--".into());
        let note = self
            .note
            .as_ref()
            .map(|n| format!(" | note: {n}"))
            .unwrap_or_default();
        format!(
            "{} | {} | {} | {} | {pos} | {} laps | best {} | avg {} | {}x{note}",
            self.id,
            fmt_utc(self.started_ms),
            kind_label(self.kind),
            self.track.as_deref().unwrap_or("--"),
            self.laps_completed,
            lap(self.best_lap_s()),
            lap(self.avg_clean_lap_s()),
            p.and_then(|p| p.incidents).unwrap_or(0),
        )
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", self.line());
        let _ = writeln!(
            out,
            "car: {} | duration {} | fuel {}",
            self.car.as_deref().unwrap_or("--"),
            fmt_duration(self.duration_s),
            opt(self.fuel_per_lap_l(), "L/lap", 2)
        );
        if let Some(s) = &self.setup {
            let modified = if s.modified == Some(true) {
                " (modified)"
            } else {
                ""
            };
            let name = s.name.as_deref().unwrap_or("--");
            let _ = writeln!(out, "setup: {name}{modified}");
        }
        if !self.standings.is_empty() {
            out.push_str("result:\n");
            for (i, r) in self.standings.iter().enumerate() {
                if i >= 10 && !r.is_player {
                    continue;
                }
                let _ = writeln!(
                    out,
                    "  {} #{} {} | {} laps | best {}{}",
                    r.car
                        .position
                        .map(|p| format!("P{p}"))
                        .unwrap_or_else(|| "--".into()),
                    r.car.number,
                    r.car.driver,
                    r.laps_completed.unwrap_or(0),
                    lap(r.best_lap_s),
                    if r.is_player { " <- you" } else { "" }
                );
            }
        }
        if !self.laps.is_empty() {
            out.push_str("laps:\n");
            for l in &self.laps {
                let fuel = l
                    .fuel_used_l
                    .map(|f| format!(" | {f:.2}L"))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "  {}: {}{fuel}{}{}",
                    l.lap,
                    lap(l.time_s),
                    if l.pit { " | pit" } else { "" },
                    if l.caution { " | caution" } else { "" }
                );
            }
        }
        adjustments_text(&mut out, &self.adjustments);
        let notable: Vec<String> = self
            .events
            .iter()
            .filter(|e| !matches!(e.kind, EventKind::LapCompleted { .. }))
            .map(|e| event_to_text(e).trim().to_string())
            .collect();
        if !notable.is_empty() {
            out.push_str("events:\n");
            for e in &notable[notable.len().saturating_sub(30)..] {
                let _ = writeln!(out, "  {e}");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceKind;
    use vr_core::{Capabilities, Damage, PitState, PitWindow, Wheel, Wheels, WindowUnit};

    fn report(c: CarTelemetry) -> CarReport {
        CarReport {
            meta: Meta {
                game: "assetto".into(),
                source: SourceKind::Live,
                connected: true,
                age_ms: Some(0),
                history: true,
                session_time_s: Some(10.0),
                capabilities: Capabilities::ALL,
            },
            car_name: Some("ks_bmw_m3_e30".into()),
            car: Some(c),
            water_temp_c: None,
            oil_temp_c: None,
            incidents: None,
            adjustments: Vec::new(),
        }
    }

    #[test]
    fn brakes_body_damage_penalty_and_pit_rules_are_told() {
        let brake = |t: Option<f64>| Wheel {
            brake_temp_c: t,
            ..Default::default()
        };
        let mut c = CarTelemetry {
            wheels: Some(Wheels {
                lf: brake(Some(420.4)),
                rf: brake(Some(431.0)),
                lr: brake(None),
                rr: brake(Some(300.0)),
            }),
            damage: Some(Damage {
                front: Some(0.05),
                rear: Some(0.0),
                left: Some(0.125),
                right: None,
                centre: None,
            }),
            penalty_s: Some(5.0),
            pit: PitState {
                window: Some(PitWindow {
                    from: 10.0,
                    to: 20.0,
                    unit: WindowUnit::Lap,
                }),
                mandatory_done: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        c.tyres.lf.core_c = Some(85.4);
        let text = report(c.clone()).to_text();
        assert!(text.contains("brakes: LF 420C RF 431C RR 300C\n"), "{text}");
        assert!(
            text.contains("body damage left 0.12, front 0.05\n"),
            "{text}"
        );
        assert!(text.contains("penalty: 5.0s\n"), "{text}");
        assert!(
            text.contains("| window lap 10-20 | mandatory stop NOT done\n"),
            "{text}"
        );
        assert!(text.contains(" | core 85C\n"), "{text}");
        let line = car_line(&c).unwrap();
        assert!(
            line.contains("body damage left 0.12, front 0.05 | PENALTY 5.0s"),
            "{line}"
        );

        // Whole, clean, no rules: nothing said.
        c.damage = Some(Damage::default());
        c.penalty_s = Some(0.0);
        c.wheels = None;
        c.pit.window = Some(PitWindow {
            from: 5.0,
            to: 25.0,
            unit: WindowUnit::Minute,
        });
        c.pit.mandatory_done = Some(true);
        let text = report(c.clone()).to_text();
        assert!(!text.contains("brakes:"), "{text}");
        assert!(!text.contains("body damage"), "{text}");
        assert!(!text.contains("penalty"), "{text}");
        assert!(
            text.contains("| window min 5-25 | mandatory stop done\n"),
            "{text}"
        );
        assert_eq!(car_line(&c), None);
    }
}
