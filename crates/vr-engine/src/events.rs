use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use vr_core::{Flag, SessionKind, SessionPhase};

use crate::strategy::StrategyCall;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic sequence number; pass the last seen value as `since` to page.
    pub seq: u64,
    pub session_time_s: f64,
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub kind: EventKind,
    /// Short human readable description.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    SessionChanged {
        session_num: i32,
        kind: SessionKind,
    },
    PhaseChanged {
        from: SessionPhase,
        to: SessionPhase,
    },
    FlagRaised {
        flag: Flag,
    },
    FlagCleared {
        flag: Flag,
    },
    PositionChanged {
        from: u32,
        to: u32,
    },
    LapCompleted {
        lap: i32,
        time_s: Option<f64>,
        position: Option<u32>,
    },
    PersonalBest {
        lap: i32,
        time_s: f64,
    },
    PitEntry {
        car_idx: usize,
        number: String,
        driver: String,
        is_player: bool,
    },
    PitExit {
        car_idx: usize,
        number: String,
        driver: String,
        is_player: bool,
    },
    Incident {
        added: i32,
        total: i32,
    },
    /// The fuel lasts for about this many more laps, and not to the finish.
    FuelLow {
        laps: f64,
    },
    /// The track got wetter or drier.
    WeatherChanged {
        from: String,
        to: String,
    },
    /// The car needs repairs, or runs too hot or short of pressure.
    Damage {
        warning: vr_core::Warning,
    },
    /// The stops: something to act on.
    Strategy {
        advice: StrategyCall,
    },
    /// The driver changed an in-car setting.
    Adjustment {
        control: String,
        from: f64,
        to: f64,
    },
}

#[derive(Debug, Clone)]
pub struct EventLog {
    next_seq: u64,
    buf: VecDeque<Event>,
    cap: usize,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::with_capacity(1000)
    }
}

impl EventLog {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            next_seq: 1,
            buf: VecDeque::new(),
            cap,
        }
    }

    pub fn push(&mut self, session_time_s: f64, timestamp_ms: i64, kind: EventKind, text: String) {
        let ev = Event {
            seq: self.next_seq,
            session_time_s,
            timestamp_ms,
            kind,
            text,
        };
        self.next_seq += 1;
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(ev);
    }

    /// Events with `seq > since`, oldest first, at most `limit` (newest kept).
    pub fn since(&self, since: u64, limit: usize) -> Vec<Event> {
        let all: Vec<&Event> = self.buf.iter().filter(|e| e.seq > since).collect();
        let skip = all.len().saturating_sub(limit);
        all.into_iter().skip(skip).cloned().collect()
    }

    pub fn last(&self, n: usize) -> Vec<Event> {
        self.since(0, n)
    }

    /// Sequence number of the newest event (0 when empty).
    pub fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }
}
