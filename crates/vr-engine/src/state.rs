use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use vr_core::{
    Capabilities, CarState, Control, Extras, Flag, Frame, SessionKind, SessionPhase, Setup, Warning,
};

use crate::archive::SessionRecord;
use crate::chassis::{Chassis, ChassisTracker};
use crate::handling::HandlingTracker;
use crate::report::*;
use crate::strategy::{self, StrategyCall, StrategyReport};
use crate::timing::Timing;
use crate::trace::{LapTrace, TraceRecorder};
use crate::util::{mean, now_ms, positive, wrap_half};
use crate::{Event, EventKind, EventLog};

const MAX_LAPS_PER_CAR: usize = 200;
const MAX_GAP_SAMPLES: usize = 20;
const FUEL_LAPS_WINDOW: usize = 5;
const PACE_LAPS_WINDOW: usize = 3;
const TREND_LAPS_WINDOW: usize = 3;
/// How long to wait for the game to publish a lap time after the line crossing.
const PENDING_LAP_TIMEOUT_S: f64 = 3.0;
/// A lap timed by the crossings this close to the game's old time is the
/// same lap time.
const SAME_LAP_S: f64 = 0.2;
const NEARBY_WINDOW_S: f64 = 3.0;
/// Slower than this, the instant fuel consumption is not that of a lap.
const MIN_INSTANT_SPEED_MPS: f64 = 15.0;
/// A setting counts as changed once it rests for this long.
const ADJUSTMENT_REST_S: f64 = 1.5;
const MAX_ADJUSTMENTS: usize = 200;
/// Remaining laps of fuel at which to warn (when it does not reach the end).
const FUEL_WARN_LAPS: [u32; 3] = [5, 3, 1];
/// How often the fuel is checked, in session time.
const FUEL_CHECK_S: f64 = 1.0;
/// The game shows no laps for a moment while a car is towed.
const LAP_DROP_S: f64 = 1.0;
/// Session time going back by more than this is a restart.
const RESTART_S: f64 = 5.0;

/// How often the strategy is looked at without anything having happened.
const STRATEGY_EVERY_S: f64 = 5.0;
/// A strategy call is a new one when it did not hold for so long.
const STRATEGY_FORGET_S: f64 = 60.0;

#[derive(Debug, Clone)]
struct PendingLap {
    lap: i32,
    prev_last_lap: Option<f64>,
    since_s: f64,
    fuel_used_l: Option<f64>,
    pit: bool,
    caution: bool,
    position: Option<u32>,
}

#[derive(Debug, Clone, Default)]
struct CarTracker {
    laps: VecDeque<LapRecord>,
    last_completed: Option<i32>,
    last_seen_last_lap: Option<f64>,
    pending: Option<PendingLap>,
    pit_this_lap: bool,
    caution_this_lap: bool,
    fuel_at_lap_start: Option<f64>,
    prev_on_pit_road: Option<bool>,
    /// Since when fewer laps are shown than before.
    dropped_since: Option<f64>,
    /// Gap samples taken when the player crosses the line:
    /// (player lap, seconds this car is ahead of the player).
    gap_samples: VecDeque<(i32, f64)>,
}

impl CarTracker {
    fn clean_laps(&self) -> impl DoubleEndedIterator<Item = &LapRecord> {
        self.laps.iter().filter(|l| l.is_clean())
    }

    fn avg_clean_lap(&self, n: usize) -> Option<f64> {
        let times: Vec<f64> = self
            .clean_laps()
            .rev()
            .take(n)
            .filter_map(|l| l.time_s)
            .collect();
        mean(&times)
    }

    fn push_lap(&mut self, lap: LapRecord) {
        if self.laps.len() == MAX_LAPS_PER_CAR {
            self.laps.pop_front();
        }
        self.laps.push_back(lap);
    }
}

/// Accumulated race state. Feed frames with [`RaceState::ingest`]; query with
/// the methods in `api.rs` (via [`crate::RaceApi`]).
#[derive(Debug, Clone)]
pub struct RaceState {
    game: String,
    source: SourceKind,
    capabilities: Capabilities,
    connected: bool,
    frame: Option<Frame>,
    ingested_at_ms: Option<i64>,
    frames_ingested: u64,
    session_num: Option<i32>,
    cars: HashMap<usize, CarTracker>,
    events: EventLog,
    prev_flags: BTreeSet<Flag>,
    prev_phase: Option<SessionPhase>,
    prev_position: Option<u32>,
    prev_incidents: Option<i32>,
    best_lap_s: Option<f64>,
    /// Rain over the last minutes: session time and precipitation (0..1).
    rain: VecDeque<(f64, f64)>,
    session_uid: Option<i64>,
    /// Sequence number of the last event before this session.
    session_start_seq: u64,
    extras: Option<Extras>,
    handling: HandlingTracker,
    traces: TraceRecorder,
    timing: Timing,
    chassis: ChassisTracker,
    /// Pit loss of earlier sessions on the track.
    prior_pit_loss: Option<f64>,
    /// When a strategy call last held, by key.
    strategy_calls: BTreeMap<String, f64>,
    /// When the strategy was looked at.
    strategy_at: f64,
    plan: Option<Vec<i32>>,
    adjustments: Vec<Adjustment>,
    /// In-car settings as of the last resting value.
    controls: BTreeMap<String, f64>,
    /// Settings being turned: value and since when.
    turning: BTreeMap<String, (f64, f64)>,
    damage: BTreeSet<Warning>,
    wetness: Option<String>,
    fuel_warned: Option<u32>,
    fuel_checked_s: Option<f64>,
    /// Sessions that ended and were not taken away yet.
    finished: Vec<SessionRecord>,
}

/// How far back the rain trend looks, in seconds of session time.
const RAIN_WINDOW_S: f64 = 120.0;
/// Changes smaller than this per minute are steady rain.
const RAIN_STEADY_PER_MIN: f64 = 0.01;

impl RaceState {
    pub fn new(game: impl Into<String>, source: SourceKind, capabilities: Capabilities) -> Self {
        Self {
            game: game.into(),
            source,
            capabilities,
            connected: false,
            frame: None,
            ingested_at_ms: None,
            frames_ingested: 0,
            session_num: None,
            cars: HashMap::new(),
            events: EventLog::default(),
            prev_flags: BTreeSet::new(),
            prev_phase: None,
            prev_position: None,
            prev_incidents: None,
            best_lap_s: None,
            rain: VecDeque::new(),
            session_uid: None,
            session_start_seq: 0,
            extras: None,
            handling: HandlingTracker::default(),
            traces: TraceRecorder::default(),
            timing: Timing::default(),
            chassis: ChassisTracker::default(),
            prior_pit_loss: None,
            strategy_calls: BTreeMap::new(),
            strategy_at: 0.0,
            plan: None,
            adjustments: Vec::new(),
            controls: BTreeMap::new(),
            turning: BTreeMap::new(),
            damage: BTreeSet::new(),
            wetness: None,
            fuel_warned: None,
            fuel_checked_s: None,
            finished: Vec::new(),
        }
    }

    pub fn set_extras(&mut self, extras: Extras) {
        self.extras = Some(extras);
    }

    pub fn extras(&self) -> Option<&Extras> {
        self.extras.as_ref()
    }

    pub fn setup_now(&self) -> Option<&Setup> {
        self.extras.as_ref()?.setup.as_ref()
    }

    pub fn adjustments(&self) -> &[Adjustment] {
        &self.adjustments
    }

    pub(crate) fn controls(&self) -> Vec<Control> {
        self.frame
            .as_ref()
            .and_then(|f| f.player.as_ref())
            .map(|p| p.car.controls.clone())
            .unwrap_or_default()
    }

    pub(crate) fn handling_tracker(&self) -> &HandlingTracker {
        &self.handling
    }

    /// Events of the session in progress.
    pub(crate) fn session_events(&self) -> Vec<Event> {
        self.events.since(self.session_start_seq, usize::MAX)
    }

    pub(crate) fn timing(&self) -> &Timing {
        &self.timing
    }

    /// Laps completed since the last call, place by place.
    pub fn take_traces(&mut self) -> Vec<LapTrace> {
        self.traces.take()
    }

    /// Sessions that ended since the last call.
    pub fn take_finished(&mut self) -> Vec<SessionRecord> {
        std::mem::take(&mut self.finished)
    }

    /// Rain per minute over the last two minutes; `None` until there are
    /// two minutes of it.
    pub fn rain_trend(&self) -> Option<RainTrend> {
        let &(t1, r1) = self.rain.back()?;
        let &(t0, r0) = self.rain.iter().find(|(t, _)| t1 - t <= RAIN_WINDOW_S)?;
        if t1 - t0 < RAIN_WINDOW_S * 0.9 {
            return None;
        }
        let per_min = (r1 - r0) / (t1 - t0) * 60.0;
        Some(RainTrend {
            per_min,
            direction: if per_min > RAIN_STEADY_PER_MIN {
                RainDirection::Rising
            } else if per_min < -RAIN_STEADY_PER_MIN {
                RainDirection::Easing
            } else {
                RainDirection::Steady
            },
        })
    }

    fn track_rain(&mut self, frame: &Frame) {
        let Some(r) = frame.weather.as_ref().and_then(|w| w.precipitation) else {
            return;
        };
        let t = frame.session.time_s;
        // One sample every few seconds is plenty.
        if self
            .rain
            .back()
            .is_some_and(|(last, _)| t - last < 5.0 && t >= *last)
        {
            return;
        }
        if self.rain.back().is_some_and(|(last, _)| t < *last) {
            self.rain.clear();
        }
        self.rain.push_back((t, r));
        while self
            .rain
            .front()
            .is_some_and(|(old, _)| t - old > RAIN_WINDOW_S + 10.0)
        {
            self.rain.pop_front();
        }
    }

    /// Update the adapter identity once the real source is opened.
    pub fn set_source(&mut self, game: impl Into<String>, capabilities: Capabilities) {
        self.game = game.into();
        self.capabilities = capabilities;
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }

    pub fn event_log(&self) -> &EventLog {
        &self.events
    }

    pub fn frames_ingested(&self) -> u64 {
        self.frames_ingested
    }

    pub fn ingest(&mut self, frame: Frame) {
        self.ingest_at(frame, now_ms());
    }

    /// Like [`RaceState::ingest`] with an explicit wall clock (for tests).
    pub fn ingest_at(&mut self, frame: Frame, now_ms: i64) {
        self.connected = true;
        let t = frame.session.time_s;
        let ts = frame.timestamp_ms;

        let before = self.events.last_seq();
        // A restart keeps the session number.
        let restarted = self.session_uid != frame.session.uid
            || self
                .frame
                .as_ref()
                .is_some_and(|f| t < f.session.time_s - RESTART_S);
        if self.session_num != Some(frame.session.session_num) || restarted {
            if self.session_num.is_some() {
                let ended = SessionRecord::of(self);
                self.finished.extend(ended);
                self.reset_session();
            }
            self.session_num = Some(frame.session.session_num);
            self.session_uid = frame.session.uid;
            self.events.push(
                t,
                ts,
                EventKind::SessionChanged {
                    session_num: frame.session.session_num,
                    kind: frame.session.kind,
                },
                format!("session: {}", kind_label(frame.session.kind)),
            );
        }

        self.detect_session_events(&frame);
        self.track_laps(&frame);
        self.detect_player_events(&frame);
        self.track_rain(&frame);
        self.handling.ingest(&frame);
        self.chassis.ingest(&frame);
        self.traces.ingest(&frame);
        self.timing.ingest(&frame);
        self.detect_car_events(&frame);

        self.frame = Some(frame);
        self.ingested_at_ms = Some(now_ms);
        self.frames_ingested += 1;
        self.warn_fuel();
        // Laps and stops are what changes the strategy.
        let moved = self.events.since(before, usize::MAX).iter().any(|e| {
            matches!(
                e.kind,
                EventKind::LapCompleted { .. }
                    | EventKind::PitEntry { .. }
                    | EventKind::PitExit { .. }
                    | EventKind::FlagRaised { .. }
            )
        });
        // Others' stops show without an event.
        let t = self.frame.as_ref().map_or(0.0, |f| f.session.time_s);
        if moved || !(0.0..STRATEGY_EVERY_S).contains(&(t - self.strategy_at)) {
            self.strategy_at = t;
            self.watch_strategy();
        }
    }

    /// Track and car, once both are known.
    pub fn track_and_car(&self) -> Option<(String, String)> {
        let f = self.frame.as_ref()?;
        Some((
            self.session_summary(f).track?,
            f.player_car()?.car_name.clone()?,
        ))
    }

    /// What the wheels did, if the game tells.
    pub fn chassis(&self) -> Option<Chassis> {
        self.chassis.get()
    }

    pub fn set_prior_pit_loss(&mut self, seconds: Option<f64>) {
        self.prior_pit_loss = seconds;
    }

    /// The lap the plan stops at the end of, if one is to come.
    pub(crate) fn next_stop(&self) -> Option<i32> {
        let lap = self.frame.as_ref()?.player_car()?.lap?;
        self.plan.as_ref()?.iter().copied().find(|s| *s >= lap)
    }

    pub(crate) fn strategy_report(&self) -> StrategyReport {
        strategy::strategy(self, self.prior_pit_loss)
    }

    /// Tells about strategy calls once, when they begin to hold.
    fn watch_strategy(&mut self) {
        let Some((t, ts)) = self
            .frame
            .as_ref()
            .filter(|f| f.session.phase == SessionPhase::Racing)
            .map(|f| (f.session.time_s, f.timestamp_ms))
        else {
            return;
        };
        let r = self.strategy_report();
        let mut advice: Vec<StrategyCall> = r
            .calls
            .iter()
            .filter(|c| !self.strategy_calls.contains_key(&c.key()))
            .cloned()
            .collect();
        // One that comes and goes is told once.
        for c in &r.calls {
            self.strategy_calls.insert(c.key(), t);
        }
        self.strategy_calls
            .retain(|_, last| (0.0..STRATEGY_FORGET_S).contains(&(t - *last)));
        if let Some(stops) = strategy::new_plan(self.plan.as_deref(), &r.plans) {
            self.plan = Some(stops.clone());
            advice.push(StrategyCall::Plan { stops });
        }
        for a in advice {
            let text = a.text();
            self.events
                .push(t, ts, EventKind::Strategy { advice: a }, text);
        }
    }

    /// Settings, damage, weather: what the driver should hear about.
    fn detect_car_events(&mut self, frame: &Frame) {
        let t = frame.session.time_s;
        let ts = frame.timestamp_ms;
        let wet = frame.weather.as_ref().and_then(|w| w.wetness.clone());
        if let (Some(from), Some(to)) = (&self.wetness, &wet)
            && from != to
        {
            let (from, to) = (from.clone(), to.clone());
            let text = format!("track now {to} (was {from})");
            self.events
                .push(t, ts, EventKind::WeatherChanged { from, to }, text);
        }
        if wet.is_some() {
            self.wetness = wet;
        }
        let Some(p) = &frame.player else { return };
        let lap = frame.player_car().and_then(|c| c.lap);
        for c in &p.car.controls {
            self.track_control(c, t, ts, lap);
        }

        // With the engine off, pressures are low without any damage.
        let stalled = p.car.warnings.contains(&Warning::Stalled);
        let damage: BTreeSet<Warning> = p
            .car
            .warnings
            .iter()
            .copied()
            .filter(|w| w.is_damage() && (!stalled || *w == Warning::Damage))
            .collect();
        for w in damage.difference(&self.damage) {
            self.events.push(
                t,
                ts,
                EventKind::Damage { warning: *w },
                format!("car warning: {}", warning_label(*w)),
            );
        }
        self.damage = damage;
    }

    /// Settings are turned through several steps: only where they come to
    /// rest counts.
    fn track_control(&mut self, c: &Control, t: f64, ts: i64, lap: Option<i32>) {
        let Some(was) = self.controls.get(&c.id).copied() else {
            self.controls.insert(c.id.clone(), c.value);
            return;
        };
        if was == c.value {
            self.turning.remove(&c.id);
            return;
        }
        let resting = self.turning.entry(c.id.clone()).or_insert((c.value, t));
        if resting.0 != c.value {
            *resting = (c.value, t);
        }
        if t - resting.1 < ADJUSTMENT_REST_S {
            return;
        }
        self.turning.remove(&c.id);
        self.controls.insert(c.id.clone(), c.value);
        if self.adjustments.len() == MAX_ADJUSTMENTS {
            self.adjustments.remove(0);
        }
        self.adjustments.push(Adjustment {
            session_time_s: t,
            lap,
            control: c.id.clone(),
            from: was,
            to: c.value,
        });
        let fmt = crate::advice::fmt_value;
        self.events.push(
            t,
            ts,
            EventKind::Adjustment {
                control: c.id.clone(),
                from: was,
                to: c.value,
            },
            format!("{} {} -> {}", c.id, fmt(was), fmt(c.value)),
        );
    }

    fn warn_fuel(&mut self) {
        let Some(f) = &self.frame else { return };
        let t = f.session.time_s;
        if f.session.kind != SessionKind::Race
            || !f.player.as_ref().is_some_and(|p| p.on_track)
            || self
                .fuel_checked_s
                .is_some_and(|last| (0.0..FUEL_CHECK_S).contains(&(t - last)))
        {
            return;
        }
        let Some(fuel) = self.fuel_info(f) else {
            return;
        };
        let ts = f.timestamp_ms;
        self.fuel_checked_s = Some(t);
        let (Some(laps), FuelBasis::History) = (fuel.laps_of_fuel, fuel.basis) else {
            return;
        };
        let short = fuel.to_finish_l.is_some_and(|l| l > 0.0);
        // The lowest threshold reached.
        let reached = FUEL_WARN_LAPS
            .into_iter()
            .filter(|n| short && laps < f64::from(*n) + 0.5)
            .min();
        match (reached, self.fuel_warned) {
            (Some(n), warned) if warned.is_none_or(|w| n < w) => {
                self.fuel_warned = Some(n);
                self.events.push(
                    t,
                    ts,
                    EventKind::FuelLow { laps },
                    format!("fuel for {laps:.1} more laps, short of the finish"),
                );
            }
            // Refuelled, or the race got shorter.
            (None, Some(_)) => self.fuel_warned = None,
            _ => {}
        }
    }

    fn reset_session(&mut self) {
        self.cars.clear();
        self.prev_flags.clear();
        self.prev_phase = None;
        self.prev_position = None;
        self.prev_incidents = None;
        self.best_lap_s = None;
        self.rain.clear();
        self.session_start_seq = self.events.last_seq();
        self.handling = HandlingTracker::default();
        self.traces.restart();
        self.timing.restart();
        self.chassis = ChassisTracker::default();
        self.strategy_calls.clear();
        self.plan = None;
        self.adjustments.clear();
        self.controls.clear();
        self.turning.clear();
        self.damage.clear();
        self.wetness = None;
        self.fuel_warned = None;
        self.fuel_checked_s = None;
    }

    fn detect_session_events(&mut self, frame: &Frame) {
        let t = frame.session.time_s;
        let ts = frame.timestamp_ms;
        let phase = frame.session.phase;
        if let Some(prev) = self.prev_phase
            && prev != phase
        {
            self.events.push(
                t,
                ts,
                EventKind::PhaseChanged {
                    from: prev,
                    to: phase,
                },
                format!("session phase: {}", phase_label(phase)),
            );
        }
        self.prev_phase = Some(phase);

        let flags: BTreeSet<Flag> = frame.session.flags.iter().copied().collect();
        for f in flags.difference(&self.prev_flags) {
            self.events.push(
                t,
                ts,
                EventKind::FlagRaised { flag: *f },
                format!("{} flag", flag_label(*f)),
            );
        }
        for f in self.prev_flags.difference(&flags) {
            if *f == Flag::Green {
                // "Green cleared" is implied by whatever flag replaced it.
                continue;
            }
            self.events.push(
                t,
                ts,
                EventKind::FlagCleared { flag: *f },
                format!("{} flag cleared", flag_label(*f)),
            );
        }
        self.prev_flags = flags;
    }

    fn track_laps(&mut self, frame: &Frame) {
        let t = frame.session.time_s;
        let ts = frame.timestamp_ms;
        let caution = frame.session.flags.iter().any(|f| {
            matches!(
                f,
                Flag::Yellow | Flag::Caution | Flag::YellowWaving | Flag::Red
            )
        });
        let player_idx = frame.player_car_idx;
        let player_fuel = frame.player.as_ref().and_then(|p| p.fuel_l);
        let neighbours = self.neighbour_idxs(frame);
        let mut new_events = Vec::new();
        let mut player_completed: Option<i32> = None;

        for car in &frame.cars {
            let is_player = Some(car.idx) == player_idx;
            let tr = self.cars.entry(car.idx).or_default();

            // Pit road transitions.
            if let Some(prev) = tr.prev_on_pit_road
                && prev != car.on_pit_road
                && (is_player || neighbours.contains(&car.idx))
            {
                let (number, driver) = (car.number.clone(), car.driver.clone());
                let who = if is_player {
                    "you".to_string()
                } else {
                    format!("#{} {}", number, driver)
                };
                if car.on_pit_road {
                    new_events.push((
                        EventKind::PitEntry {
                            car_idx: car.idx,
                            number,
                            driver,
                            is_player,
                        },
                        format!("pit in: {who}"),
                    ));
                } else {
                    new_events.push((
                        EventKind::PitExit {
                            car_idx: car.idx,
                            number,
                            driver,
                            is_player,
                        },
                        format!("pit out: {who}"),
                    ));
                }
            }
            tr.prev_on_pit_road = Some(car.on_pit_road);
            tr.pit_this_lap |= car.on_pit_road;
            tr.caution_this_lap |= caution;

            // Line crossing.
            let done = car.laps_completed.filter(|done| {
                if tr.last_completed.is_none_or(|prev| *done >= prev) {
                    tr.dropped_since = None;
                    return true;
                }
                // Believed once it lasts.
                t - *tr.dropped_since.get_or_insert(t) >= LAP_DROP_S
            });
            if let Some(done) = done {
                match tr.last_completed {
                    Some(prev) if done > prev => {
                        let fuel_used_l = if is_player {
                            match (tr.fuel_at_lap_start, player_fuel) {
                                (Some(start), Some(now)) if start >= now => Some(start - now),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        tr.pending = Some(PendingLap {
                            lap: done,
                            prev_last_lap: tr.last_seen_last_lap,
                            since_s: t,
                            fuel_used_l,
                            pit: tr.pit_this_lap,
                            caution: tr.caution_this_lap,
                            position: car.position,
                        });
                        tr.pit_this_lap = car.on_pit_road;
                        tr.caution_this_lap = caution;
                        if is_player {
                            tr.fuel_at_lap_start = player_fuel;
                            player_completed = Some(done);
                        }
                    }
                    Some(prev) if done < prev => {
                        // Back to the pits: the laps driven so far stay.
                        tr.dropped_since = None;
                        tr.pending = None;
                        tr.pit_this_lap = car.on_pit_road;
                        tr.caution_this_lap = caution;
                        tr.gap_samples.clear();
                        tr.fuel_at_lap_start = player_fuel.filter(|_| is_player);
                    }
                    None if is_player => tr.fuel_at_lap_start = player_fuel,
                    _ => {}
                }
                tr.last_completed = Some(done);
            }

            // Resolve a pending lap once the game publishes the new lap time.
            if let Some(p) = tr.pending.clone() {
                let last = positive(car.last_lap_s);
                let changed = last.is_some() && last != p.prev_last_lap;
                let timed_out = t - p.since_s > PENDING_LAP_TIMEOUT_S;
                if changed || timed_out {
                    // Still the old time: the game did not tell this lap's
                    // (after the flag it never does). The crossings tell it,
                    // to a frame; a lap within a frame of the last one is the
                    // game's.
                    let crossed = tr
                        .laps
                        .back()
                        .map(|l| p.since_s - l.session_time_s)
                        .filter(|d| *d > 0.0);
                    let time_s = match (changed, last, crossed) {
                        (false, Some(l), Some(c)) if (c - l).abs() > SAME_LAP_S => Some(c),
                        _ => last,
                    };
                    tr.push_lap(LapRecord {
                        lap: p.lap,
                        time_s,
                        fuel_used_l: p.fuel_used_l,
                        pit: p.pit,
                        caution: p.caution,
                        position: p.position,
                        session_time_s: p.since_s,
                    });
                    tr.pending = None;
                    if is_player {
                        new_events.push((
                            EventKind::LapCompleted {
                                lap: p.lap,
                                time_s,
                                position: p.position,
                            },
                            format!(
                                "lap {} {}",
                                p.lap,
                                time_s
                                    .map(crate::render::fmt_lap)
                                    .unwrap_or_else(|| "--".into())
                            ),
                        ));
                        if let Some(lt) = time_s
                            && self.best_lap_s.is_none_or(|b| lt < b - 0.0005)
                        {
                            if self.best_lap_s.is_some() {
                                new_events.push((
                                    EventKind::PersonalBest {
                                        lap: p.lap,
                                        time_s: lt,
                                    },
                                    format!("personal best {}", crate::render::fmt_lap(lt)),
                                ));
                            }
                            self.best_lap_s = Some(lt);
                        }
                    }
                }
            }
            if let Some(l) = positive(car.last_lap_s) {
                tr.last_seen_last_lap = Some(l);
            }
        }

        if let Some(lap) = player_completed
            && frame.session.kind == SessionKind::Race
        {
            self.sample_gaps(frame, lap);
        }

        for (kind, text) in new_events {
            self.events.push(t, ts, kind, text);
        }
    }

    fn sample_gaps(&mut self, frame: &Frame, lap: i32) {
        let Some(player) = frame.player_car() else {
            return;
        };
        let Some(pg) = player.gap_to_leader_s else {
            return;
        };
        for car in &frame.cars {
            if car.idx == player.idx {
                continue;
            }
            if let Some(cg) = car.gap_to_leader_s {
                let tr = self.cars.entry(car.idx).or_default();
                if tr.gap_samples.len() == MAX_GAP_SAMPLES {
                    tr.gap_samples.pop_front();
                }
                tr.gap_samples.push_back((lap, pg - cg));
            }
        }
    }

    fn detect_player_events(&mut self, frame: &Frame) {
        let t = frame.session.time_s;
        let ts = frame.timestamp_ms;
        if let Some(car) = frame.player_car() {
            let multi_class = is_multi_class(frame);
            let pos = if multi_class {
                car.class_position
            } else {
                car.position
            };
            let dir = match (self.prev_position, pos) {
                (Some(prev), Some(now)) => match now.cmp(&prev) {
                    std::cmp::Ordering::Less => Some((prev, now, "gained")),
                    std::cmp::Ordering::Greater => Some((prev, now, "lost")),
                    std::cmp::Ordering::Equal => None,
                },
                _ => None,
            };
            if let Some((prev, now, dir)) = dir
                && frame.session.kind == SessionKind::Race
            {
                self.events.push(
                    t,
                    ts,
                    EventKind::PositionChanged {
                        from: prev,
                        to: now,
                    },
                    format!("{dir} position: P{prev} -> P{now}"),
                );
            }
            if pos.is_some() {
                self.prev_position = pos;
            }
        }
        if let Some(inc) = frame.player.as_ref().and_then(|p| p.incidents) {
            if let Some(prev) = self.prev_incidents
                && inc > prev
            {
                self.events.push(
                    t,
                    ts,
                    EventKind::Incident {
                        added: inc - prev,
                        total: inc,
                    },
                    format!("incident +{}x (total {}x)", inc - prev, inc),
                );
            }
            self.prev_incidents = Some(inc);
        }
    }

    /// Cars directly ahead/behind (±2 positions) of the player in scope.
    fn neighbour_idxs(&self, frame: &Frame) -> Vec<usize> {
        let Some(player) = frame.player_car() else {
            return Vec::new();
        };
        let multi = is_multi_class(frame);
        let ppos = if multi {
            player.class_position
        } else {
            player.position
        };
        let Some(ppos) = ppos else { return Vec::new() };
        frame
            .cars
            .iter()
            .filter(|c| c.idx != player.idx && (!multi || c.class_id == player.class_id))
            .filter(|c| {
                let pos = if multi { c.class_position } else { c.position };
                pos.is_some_and(|p| p.abs_diff(ppos) <= 2)
            })
            .map(|c| c.idx)
            .collect()
    }

    // ---------------------------------------------------------------------
    // Derived queries
    // ---------------------------------------------------------------------

    pub fn meta(&self) -> Meta {
        Meta {
            game: self.game.clone(),
            source: self.source,
            connected: self.connected,
            age_ms: self.ingested_at_ms.map(|t| (now_ms() - t).max(0)),
            history: self.source != SourceKind::Oneshot,
            session_time_s: self.frame.as_ref().map(|f| f.session.time_s),
            capabilities: self.capabilities,
        }
    }

    pub(crate) fn car_laps(&self, idx: usize) -> Vec<LapRecord> {
        self.cars
            .get(&idx)
            .map(|t| t.laps.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn avg_clean_lap(&self, idx: usize) -> Option<f64> {
        self.cars
            .get(&idx)
            .and_then(|t| t.avg_clean_lap(PACE_LAPS_WINDOW))
    }

    /// Best estimate of the player's current lap time.
    pub(crate) fn reference_lap(&self, frame: &Frame) -> Option<f64> {
        let car = frame.player_car()?;
        self.avg_clean_lap(car.idx)
            .or(positive(car.last_lap_s))
            .or(positive(car.best_lap_s))
            .or(positive(car.est_lap_s))
    }

    pub(crate) fn session_summary(&self, frame: &Frame) -> SessionSummary {
        SessionSummary {
            kind: frame.session.kind,
            phase: frame.session.phase,
            track: frame.session.track.as_ref().map(|t| match &t.config {
                Some(c) if !c.is_empty() => format!("{} - {}", t.name, c),
                _ => t.name.clone(),
            }),
            lap: frame.player_car().and_then(|c| c.lap),
            laps_total: frame.session.laps_total,
            laps_remaining: frame.session.laps_remaining,
            time_remaining_s: frame.session.time_remaining_s,
            flags: frame.session.flags.clone(),
        }
    }

    pub(crate) fn player_summary(&self, frame: &Frame) -> Option<PlayerSummary> {
        let car = frame.player_car()?;
        let p = frame.player.clone().unwrap_or_default();
        Some(PlayerSummary {
            car: car_ref(car),
            cars_total: frame.cars.len(),
            cars_in_class: frame
                .cars
                .iter()
                .filter(|c| c.class_id == car.class_id)
                .count(),
            last_lap_s: positive(car.last_lap_s),
            best_lap_s: positive(car.best_lap_s).or(self.best_lap_s),
            avg_lap_s: self.avg_clean_lap(car.idx),
            incidents: p.incidents,
            on_pit_road: car.on_pit_road,
            on_track: p.on_track,
            live_position: self.live_position(frame),
            lap_pct: car.lap_dist_pct.filter(|p| (0.0..=1.0).contains(p)),
            corner_now: frame
                .session
                .track
                .as_ref()
                .and_then(crate::tracks::of_track)
                .zip(car.lap_dist_pct)
                .and_then(|(b, at)| crate::tracks::name_at(b, at))
                .map(String::from),
            corner_before: frame
                .session
                .track
                .as_ref()
                .and_then(crate::tracks::of_track)
                .zip(car.lap_dist_pct)
                .and_then(|(b, at)| crate::tracks::last_before(b, at))
                .map(String::from),
        })
    }

    pub(crate) fn gap_between(
        &self,
        frame: &Frame,
        player: &CarState,
        other: &CarState,
    ) -> GapInfo {
        let progress = |c: &CarState| {
            c.laps_completed
                .map(|l| l as f64 + c.lap_dist_pct.unwrap_or(0.0))
        };
        let ahead_by_progress = match (progress(other), progress(player)) {
            (Some(o), Some(p)) => Some(o - p),
            _ => None,
        };
        let laps = ahead_by_progress
            .map(|d| d.abs().floor() as i32)
            .unwrap_or(0);

        let (gap_s, mode) = if frame.session.kind == SessionKind::Race {
            match (player.gap_to_leader_s, other.gap_to_leader_s) {
                (Some(pg), Some(og)) if pg >= 0.0 && og >= 0.0 => {
                    (Some((pg - og).abs()), GapMode::RaceTime)
                }
                _ => (
                    ahead_by_progress
                        .zip(self.reference_lap(frame))
                        .map(|(d, lap)| d.abs() * lap),
                    GapMode::TrackProgress,
                ),
            }
        } else {
            (
                positive(player.best_lap_s)
                    .zip(positive(other.best_lap_s))
                    .map(|(p, o)| (p - o).abs()),
                GapMode::BestLap,
            )
        };

        // Gap trend from samples taken at the line (seconds `other` is ahead).
        let trend_s_per_lap = self.cars.get(&other.idx).and_then(|t| {
            let s: Vec<&(i32, f64)> = t
                .gap_samples
                .iter()
                .rev()
                .take(TREND_LAPS_WINDOW + 1)
                .collect();
            let (newest, oldest) = (s.first()?, s.last()?);
            let dl = newest.0 - oldest.0;
            if dl <= 0 {
                return None;
            }
            // Magnitude change: positive = gap growing.
            Some((newest.1.abs() - oldest.1.abs()) / dl as f64)
        });

        // As it is now, where the two are racing each other on track.
        let live = ahead_by_progress
            .filter(|d| frame.session.kind == SessionKind::Race && d.abs() < 1.0)
            .and_then(|d| {
                let (a, b) = if d >= 0.0 {
                    (other.idx, player.idx)
                } else {
                    (player.idx, other.idx)
                };
                let (gap, change) = self.timing.gap(a, b)?;
                Some((gap, change, d >= 0.0))
            });

        let their_last = positive(other.last_lap_s);
        GapInfo {
            car: car_ref(other),
            gap_s,
            laps,
            mode,
            trend_s_per_lap,
            their_last_lap_s: their_last,
            last_lap_delta_s: their_last
                .zip(positive(player.last_lap_s))
                .map(|(o, p)| o - p),
            on_pit_road: other.on_pit_road,
            live_s: live.map(|l| l.0),
            live_change_s: live.and_then(|l| l.1),
            live_ahead: live.map(|l| l.2),
            p2p_left: other.extra.p2p_count,
            p2p_active: other.extra.p2p_active,
        }
    }

    /// The cars of the player's class as they run on track now; races only.
    /// Official places change at the line, these at once.
    fn running_order<'a>(&self, frame: &'a Frame) -> Vec<&'a CarState> {
        let Some(player) = frame.player_car() else {
            return Vec::new();
        };
        if frame.session.kind != SessionKind::Race || frame.session.phase != SessionPhase::Racing {
            return Vec::new();
        }
        let progress = |c: &CarState| {
            c.laps_completed
                .map(f64::from)
                .zip(c.lap_dist_pct)
                .map(|(l, p)| l + p)
        };
        let mut cars: Vec<(f64, &CarState)> = frame
            .cars
            .iter()
            .filter(|c| c.class_id == player.class_id)
            .filter_map(|c| Some((progress(c)?, c)))
            .collect();
        cars.sort_by(|a, b| b.0.total_cmp(&a.0));
        cars.into_iter().map(|(_, c)| c).collect()
    }

    /// The player's place as the cars run now.
    pub(crate) fn live_position(&self, frame: &Frame) -> Option<u32> {
        let i = frame.player_car_idx?;
        let at = self.running_order(frame).iter().position(|c| c.idx == i)?;
        Some(at as u32 + 1)
    }

    /// The car so many places ahead (negative) or behind as they run now.
    pub(crate) fn running(&self, frame: &Frame, offset: i32) -> Option<GapInfo> {
        let player = frame.player_car()?;
        let order = self.running_order(frame);
        let at = order.iter().position(|c| c.idx == player.idx)? as i32 + offset;
        let other = order.get(usize::try_from(at).ok()?)?;
        Some(self.gap_between(frame, player, other))
    }

    /// Car one position ahead/behind the player (class scope in multi-class).
    pub(crate) fn adjacent(&self, frame: &Frame, offset: i32) -> Option<GapInfo> {
        let player = frame.player_car()?;
        let multi = is_multi_class(frame);
        let pos_of = |c: &CarState| if multi { c.class_position } else { c.position };
        let ppos = pos_of(player)? as i32;
        let target = ppos + offset;
        if target < 1 {
            return None;
        }
        let other = frame
            .cars
            .iter()
            .filter(|c| !multi || c.class_id == player.class_id)
            .find(|c| pos_of(c) == Some(target as u32))?;
        Some(self.gap_between(frame, player, other))
    }

    /// The leader of the class, unless it is the player or the car ahead.
    pub(crate) fn leader(&self, frame: &Frame) -> Option<GapInfo> {
        let player = frame.player_car()?;
        let multi = is_multi_class(frame);
        let pos_of = |c: &CarState| if multi { c.class_position } else { c.position };
        if pos_of(player)? <= 2 {
            return None;
        }
        let first = frame
            .cars
            .iter()
            .filter(|c| !multi || c.class_id == player.class_id)
            .find(|c| pos_of(c) == Some(1))?;
        Some(self.gap_between(frame, player, first))
    }

    pub(crate) fn nearby(&self, frame: &Frame, window_s: Option<f64>) -> Vec<RelativeCar> {
        let Some(player) = frame.player_car() else {
            return Vec::new();
        };
        let (Some(ppct), Some(reference)) = (player.lap_dist_pct, self.reference_lap(frame)) else {
            return Vec::new();
        };
        let window = window_s.unwrap_or(NEARBY_WINDOW_S);
        let pprog = player.laps_completed.map(|l| l as f64 + ppct);
        let mut out: Vec<RelativeCar> = frame
            .cars
            .iter()
            .filter(|c| c.idx != player.idx && c.surface != vr_core::TrackSurface::NotInWorld)
            .filter(|c| !c.on_pit_road || player.on_pit_road)
            .filter_map(|c| {
                let pct = c.lap_dist_pct?;
                let d = wrap_half(pct - ppct);
                let rel_s = d * reference;
                if rel_s.abs() > window {
                    return None;
                }
                let lap_diff = match (c.laps_completed.map(|l| l as f64 + pct), pprog) {
                    (Some(cp), Some(pp)) => (cp - pp - d).round() as i32,
                    _ => 0,
                };
                Some(RelativeCar {
                    car: car_ref(c),
                    rel_s,
                    same_class: c.class_id == player.class_id,
                    lap_diff,
                })
            })
            .collect();
        out.sort_by(|a, b| a.rel_s.abs().total_cmp(&b.rel_s.abs()));
        out
    }

    /// Metres along the track to the next car ahead and behind, whoever they
    /// are. From the middle of one car to that of the other.
    pub(crate) fn near_m(&self, frame: &Frame) -> (Option<f64>, Option<f64>) {
        let length = frame.session.track.as_ref().and_then(|t| t.length_m);
        let at = frame
            .player_car()
            .and_then(|p| Some((p, p.lap_dist_pct?, length?)));
        let Some((player, pct, length)) = at else {
            return (None, None);
        };
        let all = frame
            .cars
            .iter()
            .filter(|c| c.idx != player.idx && c.surface != vr_core::TrackSurface::NotInWorld)
            .filter(|c| !c.on_pit_road || player.on_pit_road)
            .filter_map(|c| Some(wrap_half(c.lap_dist_pct? - pct) * length));
        all.fold((None, None), |(ahead, behind), m| {
            let nearer = |was: Option<f64>| Some(was.map_or(m.abs(), |w: f64| w.min(m.abs())));
            if m >= 0.0 {
                (nearer(ahead), behind)
            } else {
                (ahead, nearer(behind))
            }
        })
    }

    pub(crate) fn fuel_info(&self, frame: &Frame) -> Option<FuelInfo> {
        let p = frame.player.as_ref()?;
        let car = frame.player_car();
        let fuel = p.fuel_l;
        let samples: Vec<f64> = car
            .and_then(|c| self.cars.get(&c.idx))
            .map(|t| {
                t.laps
                    .iter()
                    .rev()
                    .filter(|l| !l.pit && !l.caution)
                    .filter_map(|l| positive(l.fuel_used_l))
                    .take(FUEL_LAPS_WINDOW)
                    .collect()
            })
            .unwrap_or_default();
        let reference = self.reference_lap(frame);

        let (per_lap, basis) = match mean(&samples) {
            Some(m) => (Some(m), FuelBasis::History),
            // In the pits or crawling, the instant value says nothing.
            None => match (
                positive(p.fuel_use_lph).filter(|_| {
                    !car.is_some_and(|c| c.on_pit_road)
                        && p.speed_mps.is_none_or(|v| v >= MIN_INSTANT_SPEED_MPS)
                }),
                reference,
            ) {
                (Some(lph), Some(lap)) => (Some(lph * lap / 3600.0), FuelBasis::Instant),
                _ => (None, FuelBasis::None),
            },
        };

        let laps_of_fuel = fuel.zip(per_lap).map(|(f, pl)| f / pl);
        let pct = car.and_then(|c| c.lap_dist_pct).unwrap_or(0.0);
        let laps_to_finish = laps_to_finish(frame, pct, reference);
        let to_finish_l = laps_to_finish
            .zip(per_lap)
            .zip(fuel)
            .map(|((l, pl), f)| l * pl - f);
        let add_with_margin_l = to_finish_l
            .zip(per_lap)
            .map(|(n, pl)| (n + 0.5 * pl).max(0.0));
        let last_lap_on_fuel = car
            .and_then(|c| c.laps_completed)
            .zip(laps_of_fuel)
            .map(|(done, lf)| done + (pct + lf).floor() as i32);

        Some(FuelInfo {
            fuel_l: fuel,
            capacity_l: p.fuel_capacity_l,
            per_lap_l: per_lap,
            basis,
            laps_sampled: samples.len(),
            laps_of_fuel,
            laps_to_finish,
            to_finish_l,
            add_with_margin_l,
            last_lap_on_fuel,
        })
    }
}

/// Laps (fractional, from the current position) the player still has to drive.
fn laps_to_finish(frame: &Frame, pct: f64, reference: Option<f64>) -> Option<f64> {
    if frame.session.phase == SessionPhase::Checkered
        || frame.session.phase == SessionPhase::CoolDown
    {
        return Some(0.0);
    }
    if let Some(n) = frame
        .session
        .laps_remaining
        .filter(|n| (0..10_000).contains(n))
    {
        // The game counts the lap in progress as remaining.
        return Some(((n as f64) - pct).max(0.0));
    }
    let remain = frame
        .session
        .time_remaining_s
        .filter(|t| *t >= 0.0 && *t < 7.0 * 24.0 * 3600.0)?;
    let lap = reference?;
    // Timed race: the lap in progress when the clock hits zero is also driven.
    Some((pct + remain / lap).ceil() - pct)
}

pub(crate) fn is_multi_class(frame: &Frame) -> bool {
    let mut ids = frame.cars.iter().filter_map(|c| c.class_id);
    match ids.next() {
        Some(first) => ids.any(|i| i != first),
        None => false,
    }
}

pub(crate) fn car_ref(c: &CarState) -> CarRef {
    CarRef {
        idx: c.idx,
        number: c.number.clone(),
        driver: c.driver.clone(),
        class_name: c.class_name.clone(),
        position: c.position,
        class_position: c.class_position,
    }
}

pub(crate) fn flag_label(f: Flag) -> &'static str {
    match f {
        Flag::Green => "green",
        Flag::Yellow => "yellow",
        Flag::YellowWaving => "local yellow",
        Flag::Caution => "full course caution",
        Flag::Red => "red",
        Flag::Blue => "blue",
        Flag::White => "white",
        Flag::Checkered => "chequered",
        Flag::Black => "black",
        Flag::Repair => "meatball (repair)",
        Flag::Disqualify => "disqualify",
        Flag::Debris => "debris",
        Flag::OneLapToGreen => "one lap to green",
    }
}

pub(crate) fn warning_label(w: Warning) -> &'static str {
    match w {
        Warning::WaterTemp => "water temperature",
        Warning::FuelPressure => "fuel pressure",
        Warning::OilPressure => "oil pressure",
        Warning::OilTemp => "oil temperature",
        Warning::Stalled => "engine stalled",
        Warning::PitLimiter => "pit limiter",
        Warning::RevLimiter => "rev limiter",
        Warning::Damage => "repairs needed",
    }
}

pub(crate) fn kind_label(k: SessionKind) -> &'static str {
    match k {
        SessionKind::Practice => "practice",
        SessionKind::Qualify => "qualify",
        SessionKind::Warmup => "warmup",
        SessionKind::Race => "race",
        SessionKind::Other => "other",
    }
}

pub(crate) fn phase_label(p: SessionPhase) -> &'static str {
    match p {
        SessionPhase::Unknown => "unknown",
        SessionPhase::GetInCar => "get in car",
        SessionPhase::Warmup => "warmup",
        SessionPhase::ParadeLaps => "parade laps",
        SessionPhase::Racing => "racing",
        SessionPhase::Checkered => "chequered",
        SessionPhase::CoolDown => "cool down",
    }
}
