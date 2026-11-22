//! Text of the reports about the car (telemetry, setup, handling) and of
//! kept sessions.

use std::fmt::Write as _;

use vr_core::{CarTelemetry, Control, Tyre};

use crate::EventKind;
use crate::advice::{Place, fmt_value};
use crate::archive::{SessionRecord, fmt_utc};
use crate::render::{event_to_text, fmt_duration, fmt_lap, meta_line};
use crate::report::*;
use crate::state::{kind_label, warning_label};

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
    s
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
        let _ = writeln!(out, "{}", pit_line(&c.pit));
        let _ = writeln!(out, "{}", damage_line(&c.pit));
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
                let _ = writeln!(out, "{car}: no setup available");
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
            }
        }
        if !self.zones.is_empty() {
            out.push_str("where (share of the lap):\n");
            for z in &self.zones {
                let _ = writeln!(
                    out,
                    "  {:.0}-{:.0}%: {} {:.1}s/lap",
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
