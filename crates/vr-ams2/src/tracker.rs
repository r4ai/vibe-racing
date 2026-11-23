//! What the game does not tell but follows from what it told before: the
//! session clock, which session this is, the gaps at the line, where the
//! sectors begin, the fuel flow and the wheel radius.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::layout::Snapshot;

// (Type#1) GameState
pub const GAME_INGAME_PLAYING: u32 = 2;
pub const GAME_INGAME_PAUSED: u32 = 3;
pub const GAME_INGAME_INMENU_TIME_TICKING: u32 = 4;
pub const GAME_INGAME_RESTARTING: u32 = 5;

/// What makes one session differ from the next.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    kind: u32,
    track: Option<String>,
    variation: Option<String>,
    car: Option<String>,
    laps: Option<u32>,
    minutes: Option<u32>,
}

impl Key {
    fn of(s: &Snapshot<'_>) -> Option<Self> {
        let kind = match s.u32("mSessionState")? {
            0 => return None,
            1 | 2 => 1,
            // Formation lap and race are one session.
            4 | 5 => 5,
            k => k,
        };
        Some(Self {
            kind,
            track: s.str("mTrackLocation"),
            variation: s.str("mTrackVariation"),
            car: s.str("mCarName"),
            laps: s.u32("mLapsInEvent"),
            minutes: s.f64("mSessionDuration").map(|m| m.round() as u32),
        })
    }

    /// Stable across processes, so a collector that starts again keeps the
    /// session.
    fn hash(&self) -> i64 {
        let text = format!("{self:?}");
        // FNV-1a, 31 bits.
        let mut h: u32 = 0x811c_9dc5;
        for b in text.bytes() {
            h ^= u32::from(b);
            h = h.wrapping_mul(0x0100_0193);
        }
        i64::from(h & 0x7fff_ffff)
    }
}

/// The session as of one sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moment {
    pub time_s: f64,
    pub session_num: i32,
    pub uid: i64,
    pub player: Option<usize>,
}

/// Seconds of the session left and its scheduled length, when timed.
fn timed(s: &Snapshot<'_>) -> Option<(f64, f64)> {
    let total = s.f64("mSessionDuration").filter(|m| *m > 0.0)? * 60.0;
    let left = s.f64("mEventTimeRemaining").filter(|t| *t >= 0.0)?;
    Some((left, total))
}

/// A gap after which the clock does not count the time between samples
/// (the game was in its menus or not sending).
const MAX_STEP_S: f64 = 2.0;
/// The remaining time going up by more than this is a restart.
const RESTART_S: f64 = 5.0;
/// Fuel flow over this window.
const FUEL_WINDOW_S: f64 = 3.0;
const FUEL_MIN_S: f64 = 1.5;
/// Samples before the wheel radius is trusted.
const RADIUS_SAMPLES: u32 = 30;
/// Boundary samples kept per sector.
const SECTOR_SAMPLES: usize = 31;

#[derive(Debug, Default)]
struct Line {
    laps: u32,
    /// When the car last crossed the line, and on which lap count.
    crossed: Option<(u32, f64)>,
}

#[derive(Debug, Default)]
pub struct Tracker {
    clock_s: f64,
    last_now: Option<f64>,
    session_num: i32,
    key: Option<Key>,
    restarts: i64,
    restarting: bool,
    last_left: Option<f64>,
    player_laps: Option<u32>,
    player: Option<usize>,
    /// Per car.
    lines: HashMap<usize, Line>,
    /// Lap count → when the first car reached it.
    first: BTreeMap<u32, f64>,
    sector_of: HashMap<usize, i32>,
    /// Sector index → where it began, as lap fractions.
    starts: BTreeMap<i32, Vec<f64>>,
    fuel: VecDeque<(f64, f64)>,
    fuel_rate: Option<f64>,
    /// Metres per unit of `mTyreRPS`, per wheel.
    radius: [(f64, u32); 4],
}

impl Tracker {
    fn new_session(&mut self, s: &Snapshot<'_>) {
        self.session_num += 1;
        self.clock_s = timed(s).map_or(0.0, |(left, total)| (total - left).max(0.0));
        self.last_now = None;
        self.lines.clear();
        self.first.clear();
        self.sector_of.clear();
        self.fuel.clear();
        self.fuel_rate = None;
        self.player_laps = None;
    }

    /// Advance to the sample read at `now_s` (any monotonic clock, seconds).
    pub fn begin(&mut self, s: &Snapshot<'_>, now_s: f64) -> Moment {
        let state = s.u32("mGameState").unwrap_or(0);
        let first_sample = self.key.is_none();
        if state == GAME_INGAME_RESTARTING {
            self.restarting = true;
        }
        let key = Key::of(s);
        let player = self.guess_player(s, state);
        let player_laps = player.and_then(|p| s.part_u32(p, "mLapsCompleted"));
        let left = timed(s).map(|(l, _)| l);
        if let Some(k) = key.filter(|k| Some(k) != self.key.as_ref()) {
            if self.key.as_ref().is_some_and(|old| old.track != k.track) {
                self.starts.clear();
            }
            self.key = Some(k);
            self.restarts = 0;
            self.restarting = false;
            self.new_session(s);
        } else if state != GAME_INGAME_RESTARTING && self.key.is_some() {
            let went_back = left
                .zip(self.last_left)
                .is_some_and(|(now, before)| now > before + RESTART_S);
            let laps_back = player_laps
                .zip(self.player_laps)
                .is_some_and(|(now, before)| now < before);
            if self.restarting || went_back || laps_back {
                self.restarting = false;
                self.restarts += 1;
                self.new_session(s);
            }
        }
        if first_sample && self.key.is_none() {
            // Not in a session yet; keep counting from zero.
            self.session_num = self.session_num.max(1);
        }
        let running = matches!(state, GAME_INGAME_PLAYING | GAME_INGAME_INMENU_TIME_TICKING);
        if let Some(before) = self.last_now {
            let step = now_s - before;
            if running && (0.0..=MAX_STEP_S).contains(&step) {
                self.clock_s += step;
            }
        }
        self.last_now = Some(now_s);
        self.last_left = left.or(self.last_left);
        self.player_laps = player_laps.or(self.player_laps);
        Moment {
            time_s: self.clock_s,
            session_num: self.session_num,
            uid: self.key.as_ref().map_or(0, Key::hash) + self.restarts,
            player,
        }
    }

    /// The viewed car while the player drives; the last such car while the
    /// camera looks at others (CrewChief does the same).
    fn guess_player(&mut self, s: &Snapshot<'_>, state: u32) -> Option<usize> {
        let viewed = s
            .i32("mViewedParticipantIndex")
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| *i < crate::layout::PARTICIPANTS);
        if state == GAME_INGAME_PLAYING && viewed.is_some() {
            self.player = viewed;
        }
        self.player.or(viewed)
    }

    /// Note where car `idx` is at `time_s`; returns its time behind the
    /// first car to complete as many laps.
    pub fn gap(&mut self, idx: usize, laps: u32, time_s: f64) -> Option<f64> {
        let line = self.lines.entry(idx).or_insert(Line {
            laps,
            crossed: None,
        });
        if laps > line.laps {
            line.crossed = Some((laps, time_s));
            self.first.entry(laps).or_insert(time_s);
            while self.first.len() > 400 {
                self.first.pop_first();
            }
        }
        line.laps = laps;
        let (lap, at) = line.crossed?;
        Some(at - self.first.get(&lap)?).filter(|g| *g >= 0.0)
    }

    /// Note that car `idx` is in `sector` at lap fraction `pct`.
    pub fn sector(&mut self, idx: usize, sector: i32, pct: f64) {
        let before = self.sector_of.insert(idx, sector);
        if before.is_some_and(|b| sector == b + 1) && (0.01..0.99).contains(&pct) {
            let v = self.starts.entry(sector).or_default();
            v.push(pct);
            if v.len() > SECTOR_SAMPLES {
                v.remove(0);
            }
        }
    }

    /// Where the sectors begin, once every one of `count` has been seen
    /// twice; the first is 0.
    pub fn sector_starts(&self, count: usize) -> Vec<f64> {
        if count < 2 || self.starts.len() != count - 1 || self.starts.values().any(|v| v.len() < 2)
        {
            return Vec::new();
        }
        let mut out = vec![0.0];
        for v in self.starts.values() {
            let mut v = v.clone();
            v.sort_by(f64::total_cmp);
            out.push(v[v.len() / 2]);
        }
        out.sort_by(f64::total_cmp);
        out
    }

    /// Litres per hour, from how the fuel went down.
    pub fn fuel_flow(&mut self, time_s: f64, fuel_l: Option<f64>) -> Option<f64> {
        let Some(fuel) = fuel_l else {
            return self.fuel_rate;
        };
        if self
            .fuel
            .back()
            .is_some_and(|(t, f)| fuel > f + 0.05 || time_s < *t)
        {
            // Refuelled or reset.
            self.fuel.clear();
            self.fuel_rate = None;
        }
        if self.fuel.back().is_none_or(|(t, _)| time_s > *t) {
            self.fuel.push_back((time_s, fuel));
        }
        while self
            .fuel
            .front()
            .is_some_and(|(t, _)| time_s - t > FUEL_WINDOW_S)
        {
            self.fuel.pop_front();
        }
        if let Some((t0, f0)) = self.fuel.front().copied()
            && time_s - t0 >= FUEL_MIN_S
        {
            self.fuel_rate = Some(((f0 - fuel) / (time_s - t0) * 3600.0).max(0.0));
        }
        self.fuel_rate
    }

    /// Learn the radius from a wheel that rolls freely (`free`), and tell
    /// the speed of its tread.
    pub fn wheel_speed(&mut self, wheel: usize, rps: f64, car_mps: f64, free: bool) -> Option<f64> {
        let (k, n) = self.radius.get_mut(wheel)?;
        let spin = rps.abs();
        if free && spin > 1.0 && car_mps > 15.0 {
            let r = car_mps / spin;
            *k = if *n == 0 { r } else { *k + (r - *k) * 0.05 };
            *n = n.saturating_add(1);
        }
        (*n >= RADIUS_SAMPLES).then_some(spin * *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::testutil::Image;

    fn session(img: &mut Image, kind: u32) {
        img.set_u32("mGameState", 0, GAME_INGAME_PLAYING);
        img.set_u32("mSessionState", 0, kind);
        img.set_str("mTrackLocation", 0, "Interlagos");
        img.set_str("mCarName", 0, "Formula Ultimate");
        img.set_i32("mViewedParticipantIndex", 0, 2);
    }

    #[test]
    fn clock_runs_while_playing_only() {
        let mut img = Image::new();
        session(&mut img, 1);
        let mut t = Tracker::default();
        let m = t.begin(&img.snapshot(), 100.0);
        assert_eq!(m.time_s, 0.0);
        assert_eq!(m.session_num, 1);
        assert_eq!(m.player, Some(2));
        assert_eq!(t.begin(&img.snapshot(), 100.5).time_s, 0.5);
        img.set_u32("mGameState", 0, GAME_INGAME_PAUSED);
        img.set_i32("mViewedParticipantIndex", 0, 5);
        let m = t.begin(&img.snapshot(), 110.0);
        assert_eq!(m.time_s, 0.5, "paused");
        assert_eq!(m.player, Some(2), "looking at another car");
        img.set_u32("mGameState", 0, GAME_INGAME_INMENU_TIME_TICKING);
        assert_eq!(t.begin(&img.snapshot(), 111.0).time_s, 1.5);
        // A long silence is not counted.
        assert_eq!(t.begin(&img.snapshot(), 113.1).time_s, 1.5);
        assert_eq!(t.begin(&img.snapshot(), 113.0).time_s, 1.5, "backwards");
        assert_eq!(t.begin(&img.snapshot(), 115.0).time_s, 3.5, "2 s counts");
        assert_eq!(t.begin(&img.snapshot(), 115.0).session_num, 1);
    }

    #[test]
    fn sessions_change_with_kind_restart_and_time() {
        let mut img = Image::new();
        session(&mut img, 1);
        let mut t = Tracker::default();
        let first = t.begin(&img.snapshot(), 0.0);
        assert_eq!(t.begin(&img.snapshot(), 1.0).time_s, 1.0);
        // Not a session (loading): nothing changes.
        img.set_u32("mSessionState", 0, 0);
        assert_eq!(t.begin(&img.snapshot(), 1.5).session_num, 1);
        // Formation lap, then the race: one session.
        img.set_u32("mSessionState", 0, 4);
        let race = t.begin(&img.snapshot(), 2.0);
        assert_eq!(race.session_num, 2);
        assert_eq!(race.time_s, 0.0);
        assert_ne!(race.uid, first.uid);
        img.set_u32("mSessionState", 0, 5);
        assert_eq!(t.begin(&img.snapshot(), 3.0).session_num, 2);
        // Restart.
        img.set_u32("mGameState", 0, GAME_INGAME_RESTARTING);
        assert_eq!(t.begin(&img.snapshot(), 4.0).session_num, 2);
        img.set_u32("mGameState", 0, GAME_INGAME_PLAYING);
        let again = t.begin(&img.snapshot(), 5.0);
        assert_eq!(again.session_num, 3);
        assert_eq!(again.uid, race.uid + 1);
        assert_eq!(again.time_s, 0.0);

        // Timed: the clock starts where the session is.
        img.set_u32("mSessionState", 0, 3);
        img.set_f32("mSessionDuration", 0, 10.0);
        img.set_f32("mEventTimeRemaining", 0, 540.0);
        let q = t.begin(&img.snapshot(), 6.0);
        assert_eq!((q.session_num, q.time_s), (4, 60.0));
        img.set_f32("mEventTimeRemaining", 0, 539.0);
        assert_eq!(t.begin(&img.snapshot(), 7.0).time_s, 61.0);
        // The remaining time jumps back up: restarted.
        img.set_f32("mEventTimeRemaining", 0, 599.0);
        let r = t.begin(&img.snapshot(), 8.0);
        assert_eq!((r.session_num, r.time_s), (5, 1.0));
        // After the time is up the game says -1; the clock goes on.
        img.set_f32("mEventTimeRemaining", 0, -1.0);
        assert_eq!(t.begin(&img.snapshot(), 9.0).time_s, 2.0);

        // The player's laps going down is a restart too.
        img.part_u32(2, "mLapsCompleted", 3);
        assert_eq!(t.begin(&img.snapshot(), 10.0).session_num, 5);
        img.part_u32(2, "mLapsCompleted", 0);
        assert_eq!(t.begin(&img.snapshot(), 11.0).session_num, 6);
    }

    #[test]
    fn same_session_same_uid_in_a_new_process() {
        let mut img = Image::new();
        session(&mut img, 5);
        let a = Tracker::default().begin(&img.snapshot(), 0.0);
        let b = Tracker::default().begin(&img.snapshot(), 50.0);
        assert_eq!(a.uid, b.uid);
        img.set_str("mTrackVariation", 0, "Historic");
        let c = Tracker::default().begin(&img.snapshot(), 0.0);
        assert_ne!(a.uid, c.uid);
        assert!(a.uid >= 0 && a.uid <= i64::from(i32::MAX));
    }

    #[test]
    fn no_session_yet() {
        let img = Image::new();
        let mut t = Tracker::default();
        let m = t.begin(&img.snapshot(), 0.0);
        assert_eq!((m.session_num, m.uid, m.player), (1, 0, Some(0)));
    }

    #[test]
    fn gaps_at_the_line() {
        let mut t = Tracker::default();
        assert_eq!(t.gap(0, 0, 0.0), None, "first sight");
        assert_eq!(t.gap(1, 0, 0.0), None);
        assert_eq!(t.gap(0, 1, 90.0), Some(0.0), "leader");
        assert_eq!(t.gap(1, 0, 91.0), None, "not over the line yet");
        assert_eq!(t.gap(1, 1, 91.5), Some(1.5));
        assert_eq!(t.gap(1, 1, 92.0), Some(1.5), "held until the next line");
        assert_eq!(t.gap(0, 2, 180.0), Some(0.0));
        assert_eq!(t.gap(1, 2, 182.25), Some(2.25));
        // Seen for the first time already on lap 5: unknown until it crosses.
        assert_eq!(t.gap(2, 5, 200.0), None);
        assert_eq!(t.gap(2, 6, 300.0), Some(0.0));
    }

    #[test]
    fn sectors_from_where_cars_cross() {
        let mut t = Tracker::default();
        assert!(t.sector_starts(3).is_empty());
        for (car, at) in [(0, 0.33), (1, 0.35), (2, 0.34)] {
            t.sector(car, 0, 0.2);
            t.sector(car, 1, at);
        }
        assert!(t.sector_starts(3).is_empty(), "sector 3 not seen");
        t.sector(0, 2, 0.70);
        assert!(t.sector_starts(3).is_empty(), "seen once");
        t.sector(1, 2, 0.72);
        assert_eq!(t.sector_starts(3), vec![0.0, 0.34, 0.72]);
        assert!(t.sector_starts(1).is_empty());
        assert!(t.sector_starts(2).is_empty(), "count disagrees");
        // A wrap or a jump is not a boundary.
        t.sector(0, 0, 0.0);
        t.sector(0, 2, 0.5);
        assert_eq!(t.sector_starts(3), vec![0.0, 0.34, 0.72]);
        // Bounded.
        for i in 0..40 {
            t.sector(9, 0, 0.1);
            t.sector(9, 1, 0.40 + f64::from(i) * 1e-4);
        }
        assert_eq!(t.starts[&1].len(), SECTOR_SAMPLES);
    }

    #[test]
    fn fuel_flow_from_the_level() {
        let mut t = Tracker::default();
        assert_eq!(t.fuel_flow(0.0, Some(50.0)), None);
        assert_eq!(t.fuel_flow(1.0, Some(49.99)), None, "too short");
        let r = t.fuel_flow(2.0, Some(49.98)).unwrap();
        assert!((r - 36.0).abs() < 1e-6, "{r}");
        assert_eq!(t.fuel_flow(2.0, None), Some(r));
        assert_eq!(t.fuel_flow(2.0, Some(49.98)), Some(r), "time stands");
        // Window of 3 s: the sample at 0 s drops, the one at 1 s is the base.
        let r = t.fuel_flow(4.0, Some(49.94)).unwrap();
        assert!((r - 60.0).abs() < 1e-6, "{r}");
        // Refuelling starts over.
        assert_eq!(t.fuel_flow(5.0, Some(60.0)), None);
        assert_eq!(t.fuel_flow(7.0, Some(60.1)), None, "refuelling");
        let r = t.fuel_flow(9.0, Some(60.08)).unwrap();
        assert!((r - 36.0).abs() < 1e-6, "{r}");
        // A rising level within a stop never reads as negative.
        let mut t = Tracker::default();
        t.fuel_flow(0.0, Some(10.0));
        assert_eq!(t.fuel_flow(2.0, Some(10.04)), Some(0.0));
    }

    #[test]
    fn wheel_radius_from_free_rolling() {
        let mut t = Tracker::default();
        for _ in 0..RADIUS_SAMPLES - 1 {
            assert_eq!(t.wheel_speed(0, -100.0, 33.0, true), None);
        }
        assert_eq!(t.wheel_speed(0, -100.0, 33.0, true), Some(33.0));
        // Locked: not learnt, speed of the tread is 0.
        assert_eq!(t.wheel_speed(0, 0.0, 33.0, false), Some(0.0));
        assert_eq!(t.wheel_speed(0, -50.0, 33.0, false), Some(16.5));
        // Too slow to learn from.
        assert_eq!(t.wheel_speed(1, -10.0, 3.0, true), None);
        assert_eq!(t.radius[1].1, 0);
        assert_eq!(t.wheel_speed(4, 1.0, 20.0, true), None);
    }
}
