//! Compact plain-text rendering of reports.
//!
//! The text form is meant to be read by an LLM agent answering a driver on the
//! radio: dense, unambiguous, no decoration, one fact group per line.

use std::fmt::Write as _;

use crate::Event;
use crate::report::*;
use crate::state::{flag_label, kind_label, phase_label};

pub fn fmt_lap(s: f64) -> String {
    if !s.is_finite() || s <= 0.0 {
        return "--".into();
    }
    let m = (s / 60.0).floor() as i64;
    let rest = s - m as f64 * 60.0;
    if m > 0 {
        format!("{m}:{rest:06.3}")
    } else {
        format!("{rest:.3}")
    }
}

pub fn fmt_duration(s: f64) -> String {
    let s = s.max(0.0).round() as i64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}h{m:02}m{sec:02}s")
    } else {
        format!("{m}m{sec:02}s")
    }
}

fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> String) -> String {
    v.map(f).unwrap_or_else(|| "--".into())
}

fn car_label(c: &CarRef) -> String {
    format!("#{} {}", c.number, c.driver)
}

fn pos_label(c: &CarRef, class: bool) -> String {
    let p = if class {
        c.class_position.or(c.position)
    } else {
        c.position
    };
    opt(p, |p| format!("P{p}"))
}

pub fn meta_line(m: &Meta) -> String {
    let src = match m.source {
        SourceKind::Live => "live",
        SourceKind::Oneshot => "oneshot",
        SourceKind::Replay => "replay",
        SourceKind::Synthetic => "synthetic",
    };
    let mut s = format!("[{} {}", m.game, src);
    if !m.connected {
        s.push_str(" DISCONNECTED");
    }
    if let Some(a) = m.age_ms {
        let _ = write!(s, " age={}ms", a);
    }
    if !m.history {
        s.push_str(" history=unavailable(start `vr serve` for lap history/trends)");
    }
    s.push(']');
    s
}

fn gap_line(label: &str, g: &GapInfo, ahead: bool) -> String {
    let sign = if ahead { "+" } else { "-" };
    let gap = match (g.laps, g.gap_s) {
        (l, _) if l >= 1 => format!("{sign}{l}L"),
        (_, Some(s)) => format!("{sign}{s:.2}s"),
        _ => "--".into(),
    };
    let mode = match g.mode {
        GapMode::RaceTime => "",
        GapMode::TrackProgress => " (est)",
        GapMode::BestLap => " (best-lap delta)",
    };
    let mut s = format!(
        "{label}: {} {}{mode} {}",
        pos_label(&g.car, true),
        car_label(&g.car),
        gap
    );
    if let Some(t) = g.trend_s_per_lap {
        let what = if t < -0.05 {
            format!("closing {:.2}s/lap", -t)
        } else if t > 0.05 {
            format!("growing {:.2}s/lap", t)
        } else {
            "stable".into()
        };
        let _ = write!(s, " | gap {what}");
    }
    if let Some(d) = g.last_lap_delta_s {
        let _ = write!(
            s,
            " | their last {} ({}{:.2}s vs you)",
            opt(g.their_last_lap_s, fmt_lap),
            if d >= 0.0 { "+" } else { "" },
            d
        );
    }
    if g.on_pit_road {
        s.push_str(" | ON PIT ROAD");
    }
    s
}

fn fuel_line(f: &FuelInfo) -> String {
    let mut s = format!("fuel: {}", opt(f.fuel_l, |v| format!("{v:.1}L")));
    if let Some(c) = f.capacity_l {
        let _ = write!(s, "/{c:.0}L");
    }
    match (f.per_lap_l, f.basis) {
        (Some(pl), FuelBasis::History) => {
            let _ = write!(s, " | {pl:.2}L/lap (avg {} laps)", f.laps_sampled);
        }
        (Some(pl), FuelBasis::Instant) => {
            let _ = write!(s, " | ~{pl:.2}L/lap (instant estimate)");
        }
        _ => s.push_str(" | per-lap usage unknown"),
    }
    if let Some(l) = f.laps_of_fuel {
        let _ = write!(s, " | lasts {l:.1} laps");
    }
    if let Some(l) = f.last_lap_on_fuel {
        let _ = write!(s, " (to lap {l})");
    }
    if let (Some(l), Some(n)) = (f.laps_to_finish, f.to_finish_l) {
        if n > 0.0 {
            let _ = write!(
                s,
                " | {l:.1} laps to go: SHORT {n:.1}L, add {}",
                opt(f.add_with_margin_l, |v| format!("{v:.1}L incl. margin"))
            );
        } else {
            let _ = write!(s, " | {l:.1} laps to go: enough, surplus {:.1}L", 0.0 - n);
        }
    }
    s
}

fn event_line(e: &Event) -> String {
    format!(
        "  #{} t={} {}",
        e.seq,
        fmt_duration(e.session_time_s),
        e.text
    )
}

/// `weather: lightly wet (rain 20%) | air 21.4C | track 29.4C`; `None`
/// when the game says nothing about it.
fn weather_line(w: &vr_core::Weather, trend: Option<&RainTrend>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(wet) = &w.wetness {
        let mut p = wet.clone();
        let rain = w.precipitation.filter(|r| *r > 0.0);
        let trend = trend.map(|t| match t.direction {
            RainDirection::Rising => format!("rising {:.0}%/min", t.per_min * 100.0),
            RainDirection::Easing => format!("easing {:.0}%/min", -t.per_min * 100.0),
            RainDirection::Steady => "steady".into(),
        });
        match (rain, trend) {
            (Some(r), Some(t)) => {
                let _ = write!(p, " (rain {:.0}%, {t})", r * 100.0);
            }
            (Some(r), None) => {
                let _ = write!(p, " (rain {:.0}%)", r * 100.0);
            }
            (None, Some(t)) if !t.starts_with("steady") => {
                let _ = write!(p, " (rain {t})");
            }
            _ => {}
        }
        parts.push(p);
    }
    if let Some(t) = w.air_temp_c {
        parts.push(format!("air {t:.1}C"));
    }
    if let Some(t) = w.track_temp_c {
        parts.push(format!("track {t:.1}C"));
    }
    (!parts.is_empty()).then(|| format!("weather: {}", parts.join(" | ")))
}

impl Brief {
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{}", meta_line(&self.meta));
        let Some(sess) = &self.session else {
            out.push_str("no telemetry yet (game not running or not in a session)\n");
            return out;
        };
        let mut line = format!("{} [{}]", kind_label(sess.kind), phase_label(sess.phase));
        if let Some(t) = &sess.track {
            let _ = write!(line, " @ {t}");
        }
        match (sess.lap, sess.laps_total) {
            (Some(l), Some(t)) if t > 0 && t < 10_000 => {
                let _ = write!(line, " | lap {l}/{t}");
            }
            (Some(l), _) => {
                let _ = write!(line, " | lap {l}");
            }
            _ => {}
        }
        if let Some(r) = sess.laps_remaining.filter(|r| (0..10_000).contains(r)) {
            let _ = write!(line, " | {r} laps left");
        }
        if let Some(t) = sess
            .time_remaining_s
            .filter(|t| *t >= 0.0 && *t < 7.0 * 86400.0)
        {
            let _ = write!(line, " | {} left", fmt_duration(t));
        }
        let flags: Vec<&str> = sess.flags.iter().map(|f| flag_label(*f)).collect();
        let _ = write!(
            line,
            " | flags: {}",
            if flags.is_empty() {
                "none".into()
            } else {
                flags.join(",")
            }
        );
        let _ = writeln!(out, "{line}");

        if let Some(p) = &self.player {
            let mut l = if p.cars_in_class != p.cars_total {
                format!(
                    "you: class {} {}/{} (overall {}/{}",
                    opt(p.car.class_name.clone(), |c| c),
                    pos_label(&p.car, true),
                    p.cars_in_class,
                    opt(p.car.position, |v| format!("P{v}")),
                    p.cars_total
                )
            } else {
                format!("you: {}/{} | ", pos_label(&p.car, false), p.cars_total)
            };
            let _ = write!(
                l,
                "{}last {} | best {} | avg {}",
                if p.cars_in_class != p.cars_total {
                    ") | "
                } else {
                    ""
                },
                opt(p.last_lap_s, fmt_lap),
                opt(p.best_lap_s, fmt_lap),
                opt(p.avg_lap_s, fmt_lap)
            );
            if let Some(i) = p.incidents {
                let _ = write!(l, " | {i}x");
            }
            if p.on_pit_road {
                l.push_str(" | ON PIT ROAD");
            }
            let _ = writeln!(out, "{l}");
        }
        match &self.ahead {
            Some(g) => {
                let _ = writeln!(out, "{}", gap_line("ahead", g, true));
            }
            None if self.player.is_some() => out.push_str("ahead: none (leading)\n"),
            None => {}
        }
        if let Some(g) = &self.behind {
            let _ = writeln!(out, "{}", gap_line("behind", g, false));
        }
        if !self.nearby.is_empty() {
            let parts: Vec<String> = self.nearby.iter().map(relative_label).collect();
            let _ = writeln!(out, "on track nearby: {}", parts.join("; "));
        }
        if let Some(f) = &self.fuel {
            let _ = writeln!(out, "{}", fuel_line(f));
        }
        if let Some(w) = self
            .weather
            .as_ref()
            .and_then(|w| weather_line(w, self.rain_trend.as_ref()))
        {
            let _ = writeln!(out, "{w}");
        }
        if !self.recent_events.is_empty() {
            out.push_str("recent events:\n");
            for e in &self.recent_events {
                let _ = writeln!(out, "{}", event_line(e));
            }
        }
        out
    }
}

fn relative_label(r: &RelativeCar) -> String {
    let dir = if r.rel_s >= 0.0 { "ahead" } else { "behind" };
    let mut s = format!("{} {:.1}s {dir}", car_label(&r.car), r.rel_s.abs());
    if !r.same_class {
        let _ = write!(
            s,
            " [other class {}]",
            r.car.class_name.clone().unwrap_or_default()
        );
    }
    let d = r.lap_diff;
    match d.cmp(&0) {
        std::cmp::Ordering::Greater => {
            let _ = write!(s, " [lapping you +{d}L]");
        }
        std::cmp::Ordering::Less => {
            let _ = write!(s, " [lapped car {d}L]");
        }
        std::cmp::Ordering::Equal => {}
    }
    s
}

impl StandingsReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        if self.rows.is_empty() {
            out.push_str("no standings\n");
            return out;
        }
        let _ = writeln!(
            out,
            "{}",
            if self.class_only {
                "class standings:"
            } else {
                "standings:"
            }
        );
        for r in &self.rows {
            let me = if r.is_player { ">" } else { " " };
            let gap = opt(r.gap_to_leader_s, |g| {
                if g.abs() < 1e-6 {
                    "leader".into()
                } else {
                    format!("{g:+.1}s")
                }
            });
            let int = opt(r.interval_s, |i| format!("{i:+.1}s"));
            let _ = writeln!(
                out,
                "{me}{:>4} {:<28} laps {:>3} gap {:>9} int {:>8} last {:>9} best {:>9}{}",
                pos_label(&r.car, self.class_only),
                car_label(&r.car),
                opt(r.laps_completed, |v| v.to_string()),
                gap,
                int,
                opt(r.last_lap_s, fmt_lap),
                opt(r.best_lap_s, fmt_lap),
                if r.on_pit_road { " PIT" } else { "" }
            );
        }
        out
    }
}

impl GapReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        if let (Some(t), Some(ahead)) = (&self.target, self.target_is_ahead) {
            let _ = writeln!(out, "{}", gap_line("target", t, ahead));
        }
        match &self.ahead {
            Some(g) => {
                let _ = writeln!(out, "{}", gap_line("ahead", g, true));
            }
            None => out.push_str("ahead: none\n"),
        }
        match &self.behind {
            Some(g) => {
                let _ = writeln!(out, "{}", gap_line("behind", g, false));
            }
            None => out.push_str("behind: none\n"),
        }
        if self.nearby.is_empty() {
            out.push_str("on track nearby: nobody close\n");
        } else {
            for r in &self.nearby {
                let _ = writeln!(out, "nearby: {}", relative_label(r));
            }
        }
        out
    }
}

impl FuelReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        match &self.fuel {
            Some(f) => {
                let _ = writeln!(out, "{}", fuel_line(f));
            }
            None => out.push_str("fuel: unavailable\n"),
        }
        out
    }
}

impl LapsReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{}\n", meta_line(&self.meta));
        let Some(car) = &self.car else {
            out.push_str("no car\n");
            return out;
        };
        let _ = writeln!(
            out,
            "laps for {} | best {} | avg clean {}",
            car_label(car),
            opt(self.best_lap_s, fmt_lap),
            opt(self.avg_clean_lap_s, fmt_lap)
        );
        if self.laps.is_empty() {
            out.push_str("no completed laps recorded yet\n");
        }
        for l in &self.laps {
            let mut tags = Vec::new();
            if l.pit {
                tags.push("pit");
            }
            if l.caution {
                tags.push("caution");
            }
            let _ = writeln!(
                out,
                "  L{:<3} {:>9} fuel {:>6} pos {:>4} {}",
                l.lap,
                opt(l.time_s, fmt_lap),
                opt(l.fuel_used_l, |v| format!("{v:.2}L")),
                opt(l.position, |p| format!("P{p}")),
                tags.join(",")
            );
        }
        out
    }
}

impl EventsReport {
    pub fn to_text(&self) -> String {
        let mut out = format!("{} last_seq={}\n", meta_line(&self.meta), self.last_seq);
        if self.events.is_empty() {
            out.push_str("no events\n");
        }
        for e in &self.events {
            let _ = writeln!(out, "{}", event_line(e));
        }
        out
    }
}

pub fn event_to_text(e: &Event) -> String {
    event_line(e).trim_start().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vr_core::{Capabilities, Flag, SessionKind, SessionPhase};

    fn car(num: &str, pos: u32) -> CarRef {
        CarRef {
            idx: 0,
            number: num.into(),
            driver: "D".into(),
            class_name: Some("GT3".into()),
            position: Some(pos),
            class_position: Some(pos),
        }
    }

    fn gap(trend: Option<f64>) -> GapInfo {
        GapInfo {
            car: car("7", 3),
            gap_s: Some(1.234),
            laps: 0,
            mode: GapMode::RaceTime,
            trend_s_per_lap: trend,
            their_last_lap_s: None,
            last_lap_delta_s: None,
            on_pit_road: false,
        }
    }

    fn meta() -> Meta {
        Meta {
            game: "g".into(),
            source: SourceKind::Live,
            connected: true,
            age_ms: Some(5),
            history: true,
            session_time_s: Some(1.0),
            capabilities: Capabilities::ALL,
        }
    }

    fn session() -> SessionSummary {
        SessionSummary {
            kind: SessionKind::Race,
            phase: SessionPhase::Racing,
            track: None,
            lap: Some(3),
            laps_total: None,
            laps_remaining: None,
            time_remaining_s: None,
            flags: vec![Flag::Green, Flag::Blue],
        }
    }

    fn brief() -> Brief {
        Brief {
            meta: meta(),
            session: Some(session()),
            player: None,
            ahead: None,
            behind: None,
            nearby: Vec::new(),
            fuel: None,
            recent_events: Vec::new(),
            weather: None,
            rain_trend: None,
        }
    }

    #[test]
    fn weather_line_says_only_what_is_known() {
        let w = |wetness: Option<&str>, rain: Option<f64>, air: Option<f64>, track: Option<f64>| {
            weather_line(
                &vr_core::Weather {
                    air_temp_c: air,
                    track_temp_c: track,
                    wetness: wetness.map(String::from),
                    precipitation: rain,
                },
                None,
            )
        };
        let trend = |per_min, direction| RainTrend { per_min, direction };
        let wet = vr_core::Weather {
            wetness: Some("lightly wet".into()),
            precipitation: Some(0.3),
            ..Default::default()
        };
        assert_eq!(
            weather_line(&wet, Some(&trend(0.044, RainDirection::Rising))).as_deref(),
            Some("weather: lightly wet (rain 30%, rising 4%/min)")
        );
        assert_eq!(
            weather_line(&wet, Some(&trend(-0.05, RainDirection::Easing))).as_deref(),
            Some("weather: lightly wet (rain 30%, easing 5%/min)")
        );
        let dry = vr_core::Weather {
            wetness: Some("dry".into()),
            precipitation: Some(0.0),
            ..Default::default()
        };
        assert_eq!(
            weather_line(&dry, Some(&trend(0.0, RainDirection::Steady))).as_deref(),
            Some("weather: dry")
        );
        assert_eq!(
            weather_line(&dry, Some(&trend(-0.02, RainDirection::Easing))).as_deref(),
            Some("weather: dry (rain easing 2%/min)")
        );
        assert_eq!(
            w(Some("lightly wet"), Some(0.2), Some(21.44), Some(29.4)).as_deref(),
            Some("weather: lightly wet (rain 20%) | air 21.4C | track 29.4C")
        );
        assert_eq!(
            w(Some("dry"), Some(0.0), None, None).as_deref(),
            Some("weather: dry")
        );
        assert_eq!(w(None, Some(0.5), None, None), None);
        let mut b = brief();
        assert!(!b.to_text().contains("weather"));
        b.weather = Some(vr_core::Weather {
            track_temp_c: Some(31.0),
            ..Default::default()
        });
        assert!(b.to_text().contains("\nweather: track 31.0C\n"));
    }

    #[test]
    fn gap_trend_wording_and_thresholds() {
        let cases = [
            (None, None),
            (Some(-0.3), Some("gap closing 0.30s/lap")),
            (Some(-0.05), Some("gap stable")),
            (Some(-0.0501), Some("gap closing 0.05s/lap")),
            (Some(0.0), Some("gap stable")),
            (Some(0.05), Some("gap stable")),
            (Some(0.0501), Some("gap growing 0.05s/lap")),
            (Some(0.7), Some("gap growing 0.70s/lap")),
        ];
        for (t, want) in cases {
            let line = gap_line("ahead", &gap(t), true);
            match want {
                Some(w) => assert!(line.ends_with(&format!(" | {w}")), "{t:?}: {line}"),
                None => assert_eq!(line, "ahead: P3 #7 D +1.23s"),
            }
        }
    }

    #[test]
    fn gap_line_variants() {
        let mut g = gap(None);
        assert_eq!(gap_line("behind", &g, false), "behind: P3 #7 D -1.23s");
        g.laps = 1;
        assert_eq!(gap_line("ahead", &g, true), "ahead: P3 #7 D +1L");
        g.laps = 0;
        g.gap_s = None;
        g.mode = GapMode::TrackProgress;
        assert_eq!(gap_line("ahead", &g, true), "ahead: P3 #7 D (est) --");
        g.mode = GapMode::BestLap;
        g.gap_s = Some(0.5);
        g.their_last_lap_s = Some(90.0);
        g.last_lap_delta_s = Some(0.0);
        g.on_pit_road = true;
        assert_eq!(
            gap_line("ahead", &g, true),
            "ahead: P3 #7 D (best-lap delta) +0.50s | their last 1:30.000 (+0.00s vs you) | ON PIT ROAD"
        );
        g.last_lap_delta_s = Some(-0.25);
        assert!(gap_line("ahead", &g, true).contains("(-0.25s vs you)"));
    }

    fn fuel() -> FuelInfo {
        FuelInfo {
            fuel_l: Some(10.0),
            capacity_l: None,
            per_lap_l: Some(2.0),
            basis: FuelBasis::History,
            laps_sampled: 4,
            laps_of_fuel: None,
            laps_to_finish: Some(5.0),
            to_finish_l: Some(0.0),
            add_with_margin_l: Some(1.0),
            last_lap_on_fuel: None,
        }
    }

    #[test]
    fn fuel_line_variants() {
        let f = fuel();
        assert_eq!(
            fuel_line(&f),
            "fuel: 10.0L | 2.00L/lap (avg 4 laps) | 5.0 laps to go: enough, surplus 0.0L"
        );
        let mut short = fuel();
        short.to_finish_l = Some(0.01);
        assert!(
            fuel_line(&short).ends_with("SHORT 0.0L, add 1.0L incl. margin"),
            "{}",
            fuel_line(&short)
        );
        short.add_with_margin_l = None;
        assert!(fuel_line(&short).ends_with("add --"));
        let mut none = fuel();
        none.per_lap_l = None;
        assert!(fuel_line(&none).contains("per-lap usage unknown"));
        let mut hist_no_rate = fuel();
        hist_no_rate.per_lap_l = None;
        hist_no_rate.basis = FuelBasis::Instant;
        assert!(fuel_line(&hist_no_rate).contains("per-lap usage unknown"));
        let mut partial = fuel();
        partial.to_finish_l = None;
        assert_eq!(fuel_line(&partial), "fuel: 10.0L | 2.00L/lap (avg 4 laps)");
        partial.fuel_l = None;
        assert!(fuel_line(&partial).starts_with("fuel: -- |"));
    }

    #[test]
    fn brief_session_line_guards() {
        let line = |s: SessionSummary| {
            let mut b = brief();
            b.session = Some(s);
            b.to_text().lines().nth(1).unwrap().to_string()
        };
        assert_eq!(line(session()), "race [racing] | lap 3 | flags: green,blue");
        let mut s = session();
        s.laps_total = Some(20);
        assert_eq!(
            line(s.clone()),
            "race [racing] | lap 3/20 | flags: green,blue"
        );
        for bogus in [0, 10_000, 32_767, -1] {
            s.laps_total = Some(bogus);
            assert_eq!(
                line(s.clone()),
                "race [racing] | lap 3 | flags: green,blue",
                "laps_total {bogus}"
            );
        }
        s.laps_total = Some(9_999);
        assert!(line(s.clone()).contains("lap 3/9999"));
        let mut s = session();
        s.lap = None;
        s.laps_total = Some(20);
        assert_eq!(line(s), "race [racing] | flags: green,blue");
        let mut s = session();
        for (r, shown) in [(0, true), (9_999, true), (10_000, false), (-1, false)] {
            s.laps_remaining = Some(r);
            assert_eq!(
                line(s.clone()).contains("laps left"),
                shown,
                "laps_remaining {r}"
            );
        }
        let mut s = session();
        let week = 7.0 * 86400.0;
        for (t, shown) in [
            (0.0, true),
            (week - 1.0, true),
            (week, false),
            (week + 86400.0 * 7.0, false),
            (-1.0, false),
        ] {
            s.time_remaining_s = Some(t);
            assert_eq!(
                line(s.clone()).contains(" left"),
                shown,
                "time_remaining {t}"
            );
        }
    }

    #[test]
    fn brief_ahead_none_only_with_player() {
        let b = brief();
        assert!(!b.to_text().contains("ahead:"));
        let mut b = brief();
        b.player = Some(PlayerSummary {
            car: car("1", 1),
            cars_total: 2,
            cars_in_class: 2,
            last_lap_s: None,
            best_lap_s: None,
            avg_lap_s: None,
            incidents: None,
            on_pit_road: false,
            on_track: true,
        });
        let t = b.to_text();
        assert!(
            t.contains("\nyou: P1/2 | last -- | best -- | avg --\n"),
            "{t}"
        );
        assert!(t.contains("ahead: none (leading)"));
        assert!(!t.contains("nearby") && !t.contains("recent events") && !t.contains("fuel:"));
    }

    #[test]
    fn relative_label_direction_and_laps() {
        let mut r = RelativeCar {
            car: car("5", 2),
            rel_s: 0.0,
            same_class: true,
            lap_diff: 0,
        };
        assert_eq!(relative_label(&r), "#5 D 0.0s ahead");
        r.rel_s = -0.04;
        assert_eq!(relative_label(&r), "#5 D 0.0s behind");
        r.lap_diff = 1;
        assert!(relative_label(&r).ends_with("[lapping you +1L]"));
        r.lap_diff = -2;
        assert!(relative_label(&r).ends_with("[lapped car -2L]"));
    }

    #[test]
    fn standings_leader_label() {
        let row = |g: Option<f64>| StandingRow {
            car: car("1", 1),
            is_player: false,
            laps_completed: Some(1),
            gap_to_leader_s: g,
            interval_s: None,
            last_lap_s: None,
            best_lap_s: None,
            on_pit_road: false,
        };
        let r = StandingsReport {
            meta: meta(),
            class_only: false,
            rows: vec![
                row(Some(0.0)),
                row(Some(-0.0000001)),
                row(Some(0.01)),
                row(Some(-0.5)),
                row(Some(1e-6)),
            ],
        };
        let t = r.to_text();
        let gaps: Vec<&str> = t
            .lines()
            .skip(2)
            .map(|l| {
                l.split(" gap ")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(gaps, vec!["leader", "leader", "+0.0s", "-0.5s", "+0.0s"]);
    }

    #[test]
    fn formats_lap_times() {
        assert_eq!(fmt_lap(92.481), "1:32.481");
        assert_eq!(fmt_lap(59.9), "59.900");
        assert_eq!(fmt_lap(-1.0), "--");
        assert_eq!(fmt_duration(3725.0), "1h02m05s");
        assert_eq!(fmt_duration(65.0), "1m05s");
    }
}
