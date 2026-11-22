//! Race events as they happen (`vr watch`), and when to tell the driver
//! about them without being asked.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::child::{Retry, Supervised};

/// Events switched on and off together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Flag,
    Position,
    Lap,
    Pit,
    Incident,
    Fuel,
    Weather,
    Session,
    Adjustment,
    Strategy,
}

impl Group {
    pub const ALL: [Self; 10] = [
        Self::Flag,
        Self::Position,
        Self::Incident,
        Self::Fuel,
        Self::Weather,
        Self::Strategy,
        Self::Pit,
        Self::Session,
        Self::Lap,
        Self::Adjustment,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Flag => "旗",
            Self::Position => "順位",
            Self::Lap => "ラップ",
            Self::Pit => "ピット",
            Self::Incident => "インシデント・損傷",
            Self::Fuel => "燃料",
            Self::Weather => "天候",
            Self::Session => "セッション",
            Self::Adjustment => "車内調整",
            Self::Strategy => "作戦",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Position => "position",
            Self::Lap => "lap",
            Self::Pit => "pit",
            Self::Incident => "incident",
            Self::Fuel => "fuel",
            Self::Weather => "weather",
            Self::Session => "session",
            Self::Adjustment => "adjustment",
            Self::Strategy => "strategy",
        }
    }

    /// The group of an event type.
    pub fn of(kind: &str) -> Option<Self> {
        Some(match kind {
            "flag_raised" | "flag_cleared" => Self::Flag,
            "position_changed" => Self::Position,
            "lap_completed" | "personal_best" => Self::Lap,
            "pit_entry" | "pit_exit" => Self::Pit,
            "incident" | "damage" => Self::Incident,
            "fuel_low" => Self::Fuel,
            "weather_changed" => Self::Weather,
            "phase_changed" | "session_changed" => Self::Session,
            "adjustment" => Self::Adjustment,
            "strategy" => Self::Strategy,
            _ => return None,
        })
    }

    /// A group by its key or by one of its event types.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|g| g.key() == s)
            .or_else(|| Self::of(s))
    }
}

/// Which events the engineer calls about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Proactive {
    pub enabled: bool,
    pub flag: bool,
    pub position: bool,
    pub lap: bool,
    pub pit: bool,
    pub incident: bool,
    pub fuel: bool,
    pub weather: bool,
    pub session: bool,
    pub adjustment: bool,
    pub strategy: bool,
}

impl Default for Proactive {
    fn default() -> Self {
        Self {
            enabled: true,
            flag: true,
            position: true,
            lap: false,
            pit: true,
            incident: true,
            fuel: true,
            weather: true,
            session: true,
            adjustment: false,
            strategy: true,
        }
    }
}

impl Proactive {
    fn slot(&mut self, g: Group) -> &mut bool {
        match g {
            Group::Flag => &mut self.flag,
            Group::Position => &mut self.position,
            Group::Lap => &mut self.lap,
            Group::Pit => &mut self.pit,
            Group::Incident => &mut self.incident,
            Group::Fuel => &mut self.fuel,
            Group::Weather => &mut self.weather,
            Group::Session => &mut self.session,
            Group::Adjustment => &mut self.adjustment,
            Group::Strategy => &mut self.strategy,
        }
    }

    pub fn get(mut self, g: Group) -> bool {
        *self.slot(g)
    }

    pub fn set(&mut self, g: Group, on: bool) {
        *self.slot(g) = on;
    }

    /// Whether the driver wants to hear about `e`.
    pub fn wants(self, e: &RaceEvent) -> bool {
        // What the driver asked to be told is told.
        e.kind == WATCH || self.enabled && e.group().is_some_and(|g| self.get(g))
    }
}

/// Kind of the events the driver's own watches make.
pub const WATCH: &str = "watch";

/// What a strategy event is about.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Advice {
    pub call: String,
}

/// One line of `vr watch -f json`; other fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RaceEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub flag: Option<String>,
    #[serde(default)]
    pub advice: Option<Advice>,
}

impl RaceEvent {
    pub fn parse(line: &str) -> Option<Self> {
        serde_json::from_str(line).ok()
    }

    /// `None` for events nobody is called about.
    pub fn group(&self) -> Option<Group> {
        let flag = self.flag.as_deref().unwrap_or_default();
        // The end of a blue or a white flag is not worth a word.
        if self.kind == "flag_cleared" && !matches!(flag, "yellow" | "yellow_waving" | "caution") {
            return None;
        }
        Group::of(&self.kind)
    }

    pub fn advice(&self) -> &str {
        self.advice.as_ref().map_or("", |a| a.call.as_str())
    }

    /// Something the driver asked to be told about.
    pub fn watch(text: String) -> Self {
        Self {
            kind: WATCH.into(),
            text,
            flag: None,
            advice: None,
        }
    }

    /// Something the engineer's own plan looked out for.
    pub fn planned(text: String) -> Self {
        Self {
            kind: "strategy".into(),
            ..Self::watch(text)
        }
    }

    /// Worth telling for so long.
    fn lasts(&self) -> Duration {
        match self.kind.as_str() {
            // A lap's work.
            "strategy" => Duration::from_secs(45),
            _ => Duration::ZERO,
        }
    }

    /// Does not wait for its turn.
    pub fn urgent(&self) -> bool {
        match self.kind.as_str() {
            "incident" | "damage" | "fuel_low" | WATCH => true,
            "strategy" => matches!(self.advice(), "box_this_lap" | "answer" | "fuel_stop"),
            "flag_raised" => matches!(
                self.flag.as_deref(),
                Some(
                    "yellow"
                        | "yellow_waving"
                        | "caution"
                        | "red"
                        | "black"
                        | "repair"
                        | "disqualify"
                )
            ),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// Events this close together are one call.
    pub coalesce: Duration,
    /// Older news is no news.
    pub max_age: Duration,
    /// Between calls that can wait.
    pub min_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            coalesce: Duration::from_millis(1500),
            max_age: Duration::from_secs(10),
            min_interval: Duration::from_secs(8),
        }
    }
}

/// An event and when it arrived.
pub type Dated = (Instant, RaceEvent);

/// Events waiting to be told.
#[derive(Debug, Default)]
pub struct Queue {
    timing: Timing,
    waiting: Vec<Dated>,
    last_call: Option<Instant>,
}

impl Queue {
    pub fn new(timing: Timing) -> Self {
        Self {
            timing,
            ..Default::default()
        }
    }

    pub fn push(&mut self, now: Instant, e: RaceEvent) {
        self.waiting.push((now, e));
    }

    /// Drops what the driver no longer wants to hear about.
    pub fn retain(&mut self, wanted: Proactive) {
        self.waiting.retain(|(_, e)| wanted.wants(e));
    }

    pub fn set_interval(&mut self, d: Duration) {
        self.timing.min_interval = d;
    }

    /// Drops what can wait.
    pub fn keep_urgent(&mut self) {
        self.waiting.retain(|(_, e)| e.urgent());
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Puts back what could not be told after all.
    pub fn restore(&mut self, mut events: Vec<Dated>) {
        events.append(&mut self.waiting);
        self.waiting = events;
    }

    /// The events to tell now. `idle`: nobody is talking.
    pub fn due(&mut self, now: Instant, idle: bool) -> Option<Vec<Dated>> {
        let Timing {
            coalesce,
            max_age,
            min_interval,
        } = self.timing;
        let since = |t: Instant| now.saturating_duration_since(t);
        self.waiting
            .retain(|(t, e)| since(*t) <= max_age.max(e.lasts()));
        let (first, _) = self.waiting.first()?;
        let ripe = since(*first) >= coalesce;
        let urgent = self.waiting.iter().any(|(_, e)| e.urgent());
        let spaced = self.last_call.is_none_or(|t| since(t) >= min_interval);
        if !idle || !ripe || !(urgent || spaced) {
            return None;
        }
        self.last_call = Some(now);
        Some(std::mem::take(&mut self.waiting))
    }
}

/// What happened, as one line.
pub fn summary(events: &[Dated]) -> String {
    let texts: Vec<&str> = events.iter().map(|(_, e)| e.text.as_str()).collect();
    texts.join(" / ")
}

/// Events as they happen, and the process that tells them.
pub struct Follow {
    pub events: UnboundedReceiver<RaceEvent>,
    child: Option<Supervised>,
}

impl Follow {
    /// Follows `command`; `None` when it is empty.
    pub fn start(command: &str, cwd: PathBuf) -> Option<Self> {
        let (tx, events) = unbounded_channel();
        let retry = Retry {
            // `vr serve` is being started at the same time.
            initial: Duration::from_secs(2),
            first: Duration::from_secs(1),
            max: Duration::from_secs(30),
        };
        let child = Supervised::spawn(
            command,
            cwd,
            retry,
            Some(Box::new(move |line| match RaceEvent::parse(line) {
                Some(e) => tx.send(e).is_ok(),
                None => !tx.is_closed(),
            })),
        )?;
        Some(Self {
            events,
            child: Some(child),
        })
    }

    /// Events from somewhere else.
    pub fn of(events: UnboundedReceiver<RaceEvent>) -> Self {
        Self {
            events,
            child: None,
        }
    }

    /// It has had something to watch for at least `d`.
    pub fn running(&self, d: Duration) -> bool {
        self.child.as_ref().is_some_and(|c| c.running(d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, flag: Option<&str>) -> RaceEvent {
        RaceEvent {
            kind: kind.into(),
            text: kind.into(),
            flag: flag.map(str::to_string),
            advice: None,
        }
    }

    #[test]
    fn lines_of_vr_watch_are_read() {
        let e = RaceEvent::parse(
            r#"{"seq":3,"session_time_s":12.5,"timestamp_ms":1,"type":"flag_raised","flag":"yellow","text":"yellow flag"}"#,
        );
        assert_eq!(
            e,
            Some(RaceEvent {
                kind: "flag_raised".into(),
                text: "yellow flag".into(),
                flag: Some("yellow".into()),
                advice: None,
            })
        );
        let e = RaceEvent::parse(
            r#"{"seq":4,"type":"position_changed","from":5,"to":4,"text":"gained"}"#,
        );
        assert_eq!(e.unwrap().kind, "position_changed");
        assert_eq!(RaceEvent::parse("watching events from"), None);
        let e = RaceEvent::parse(
            r#"{"seq":5,"type":"strategy","advice":{"call":"answer","car":{},"within_laps":2},"text":"x"}"#,
        )
        .unwrap();
        assert_eq!((e.advice(), e.group()), ("answer", Some(Group::Strategy)));
        assert!(e.urgent());
        let e = RaceEvent::parse(r#"{"type":"strategy","advice":{"call":"plan"},"text":"x"}"#);
        assert!(!e.unwrap().urgent());
    }

    #[test]
    fn kinds_belong_to_groups() {
        let of = |k| event(k, None).group();
        for (k, g) in [
            ("flag_raised", Group::Flag),
            ("position_changed", Group::Position),
            ("lap_completed", Group::Lap),
            ("personal_best", Group::Lap),
            ("pit_entry", Group::Pit),
            ("pit_exit", Group::Pit),
            ("incident", Group::Incident),
            ("damage", Group::Incident),
            ("fuel_low", Group::Fuel),
            ("weather_changed", Group::Weather),
            ("phase_changed", Group::Session),
            ("session_changed", Group::Session),
            ("adjustment", Group::Adjustment),
        ] {
            assert_eq!(of(k), Some(g), "{k}");
            assert_eq!(Group::parse(k), Some(g));
            assert_eq!(Group::parse(g.key()), Some(g));
        }
        assert_eq!(of("teleported"), None);
        assert_eq!(Group::parse("teleported"), None);
        let cleared = |f| event("flag_cleared", f).group();
        for f in ["yellow", "yellow_waving", "caution"] {
            assert_eq!(cleared(Some(f)), Some(Group::Flag));
        }
        assert_eq!(cleared(Some("blue")), None);
        assert_eq!(cleared(None), None);
    }

    #[test]
    fn urgent_events() {
        for f in [
            "yellow",
            "yellow_waving",
            "caution",
            "red",
            "black",
            "repair",
            "disqualify",
        ] {
            assert!(event("flag_raised", Some(f)).urgent(), "{f}");
            assert!(!event("flag_cleared", Some(f)).urgent(), "{f}");
        }
        for f in ["green", "blue", "white", "checkered", "debris"] {
            assert!(!event("flag_raised", Some(f)).urgent(), "{f}");
        }
        for k in ["incident", "damage", "fuel_low"] {
            assert!(event(k, None).urgent());
        }
        for k in ["position_changed", "pit_entry", "weather_changed"] {
            assert!(!event(k, None).urgent());
        }
    }

    #[test]
    fn defaults_and_switches() {
        let mut p = Proactive::default();
        let on: Vec<&str> = Group::ALL
            .into_iter()
            .filter(|g| p.get(*g))
            .map(Group::key)
            .collect();
        assert_eq!(
            on,
            [
                "flag", "position", "incident", "fuel", "weather", "strategy", "pit", "session"
            ]
        );
        assert!(p.enabled);
        let lap = event("lap_completed", None);
        assert!(!p.wants(&lap));
        for g in Group::ALL {
            p.set(g, true);
            assert!(p.get(g));
            p.set(g, false);
            assert!(!p.get(g));
        }
        p.set(Group::Lap, true);
        assert!(p.wants(&lap));
        p.enabled = false;
        assert!(!p.wants(&lap));
        // Older settings files miss fields.
        let p: Proactive = serde_json::from_str(r#"{"lap":true}"#).unwrap();
        assert!(p.lap && p.enabled && p.flag && !p.adjustment);
    }

    fn texts(events: Option<Vec<Dated>>) -> Option<String> {
        events.map(|e| summary(&e))
    }

    fn clock() -> impl Fn(u64) -> Instant {
        let t = Instant::now();
        move |ms| t + Duration::from_millis(ms)
    }

    #[test]
    fn events_close_together_are_one_call() {
        let mut q = Queue::default();
        let at = clock();
        assert_eq!(q.due(at(0), true), None);
        q.push(at(0), event("position_changed", None));
        q.push(at(1400), event("pit_entry", None));
        assert_eq!(q.due(at(1499), true), None);
        // Somebody is talking.
        assert_eq!(q.due(at(1500), false), None);
        assert_eq!(
            texts(q.due(at(1500), true)).as_deref(),
            Some("position_changed / pit_entry")
        );
        assert_eq!(q.due(at(1500), true), None);
    }

    #[test]
    fn calls_that_can_wait_are_spaced_and_urgent_ones_are_not() {
        let mut q = Queue::default();
        let at = clock();
        q.push(at(0), event("pit_entry", None));
        assert!(q.due(at(1500), true).is_some());
        q.push(at(2000), event("pit_exit", None));
        assert_eq!(q.due(at(9499), true), None);
        assert!(q.due(at(9500), true).is_some());
        q.push(at(10_000), event("incident", None));
        assert_eq!(texts(q.due(at(11_500), true)).as_deref(), Some("incident"));
    }

    #[test]
    fn old_news_is_dropped() {
        let mut q = Queue::new(Timing::default());
        let at = clock();
        q.push(at(0), event("pit_entry", None));
        q.push(at(5000), event("pit_exit", None));
        assert_eq!(texts(q.due(at(10_001), true)).as_deref(), Some("pit_exit"));
        q.push(at(20_000), event("pit_entry", None));
        assert!(q.due(at(30_000), true).is_some());
        // Put back, an event keeps its age, and its place before newer ones.
        q.push(at(52_000), event("pit_exit", None));
        q.restore(vec![(at(51_000), event("incident", None))]);
        assert_eq!(q.due(at(52_000), false), None);
        assert_eq!(
            texts(q.due(at(61_000), true)).as_deref(),
            Some("incident / pit_exit")
        );
        q.push(at(70_000), event("lap_completed", None));
        q.retain(Proactive::default());
        assert_eq!(q.due(at(72_000), true), None);
    }

    #[test]
    fn following_nothing_gives_no_events() {
        assert!(Follow::start("", std::env::temp_dir()).is_none());
        let mut f = Follow::start("rustc --version", std::env::temp_dir()).unwrap();
        assert!(!f.running(Duration::ZERO));
        assert!(f.events.try_recv().is_err());
        assert!(!Follow::of(unbounded_channel().1).running(Duration::ZERO));
    }
}
