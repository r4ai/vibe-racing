//! Answers as the engineer says them, worked out without a model: the verdict,
//! then what to do. For the questions asked again and again, where a model
//! only reads the same numbers aloud.
//!
//! A driver at speed cannot think: no reasons, no list of numbers, and a
//! number only where one was asked for. Japanese, as short as on the radio.
//! Names are left out: the voice cannot read them.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use vr_core::{Flag, SessionKind, SessionPhase, Warning};

use crate::api::{ApiResult, LapsParams, RaceApi, SectorsParams};
use crate::report::*;
use crate::trace::CornersReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Topic {
    /// How things are: also for a question nobody understood.
    Status,
    Position,
    Ahead,
    Behind,
    Leader,
    /// Own lap times.
    Pace,
    AheadLap,
    LeaderLap,
    /// After every lap: the lap, the gaps, where to find time.
    Lap,
    Sectors,
    Fuel,
    /// What is left of the race.
    Left,
    Flags,
    Weather,
    Damage,
    Tyres,
    /// Where the race ends at this pace.
    Forecast,
    /// Where time is lost, and to be found.
    Losing,
    /// Laps of fuel, as a number.
    FuelLaps,
    TyreTemps,
}

impl Topic {
    pub const ALL: [Self; 20] = [
        Self::Status,
        Self::Position,
        Self::Ahead,
        Self::Behind,
        Self::Leader,
        Self::Pace,
        Self::AheadLap,
        Self::LeaderLap,
        Self::Lap,
        Self::Sectors,
        Self::Fuel,
        Self::Left,
        Self::Flags,
        Self::Weather,
        Self::Damage,
        Self::Tyres,
        Self::Forecast,
        Self::Losing,
        Self::FuelLaps,
        Self::TyreTemps,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Position => "position",
            Self::Ahead => "ahead",
            Self::Behind => "behind",
            Self::Leader => "leader",
            Self::Pace => "pace",
            Self::AheadLap => "ahead-lap",
            Self::LeaderLap => "leader-lap",
            Self::Lap => "lap",
            Self::Sectors => "sectors",
            Self::Fuel => "fuel",
            Self::Left => "left",
            Self::Flags => "flags",
            Self::Weather => "weather",
            Self::Damage => "damage",
            Self::Tyres => "tyres",
            Self::Forecast => "forecast",
            Self::Losing => "losing",
            Self::FuelLaps => "fuel-laps",
            Self::TyreTemps => "tyre-temps",
        }
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

impl FromStr for Topic {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|t| t.key() == s).ok_or_else(|| {
            let all: Vec<&str> = Self::ALL.into_iter().map(Self::key).collect();
            format!("one of {}", all.join(", "))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Speech {
    pub meta: Meta,
    pub topic: Topic,
    pub text: String,
}

impl Speech {
    pub fn to_text(&self) -> String {
        format!("{}\n", self.text)
    }
}

/// Older than this, the data is not how things are.
const STALE_MS: i64 = 3000;
/// Changes per lap smaller than this are none.
const STEADY_S: f64 = 0.05;
/// A car this close is fought with.
const CLOSE_S: f64 = 1.5;
/// Within this distance one thinks of passing.
const FIGHT_S: f64 = 3.0;
/// A sector differs by at least this to be worth a word.
const SECTOR_S: f64 = 0.1;
/// A lap this close to the best one is a good one.
const GOOD_S: f64 = 0.3;
/// Further away than this many laps, nobody catches anybody.
const FAR_LAPS: f64 = 30.0;
/// A car that comes within as many laps is thought of now.
const SOON_LAPS: f64 = 3.0;
/// Litres left at the flag that are no reserve.
const TIGHT_L: f64 = 1.0;
/// Laps slower than the best by more come from the damage.
const HURT_S: f64 = 1.0;
/// Tread left: think of the tyre, stop for it.
const WORN: f64 = 0.4;
const GONE: f64 = 0.2;
/// A tyre this much hotter than the mean of all works too hard.
const HOT_C: f64 = 8.0;
/// Seconds a lap a driver can find when told to.
const MORE_S: f64 = 0.5;

/// A corner that loses less is not worth the words.
const CORNER_S: f64 = 0.05;
/// Metres and km/h a driver can act on.
const CORNER_M: f64 = 5.0;
const CORNER_KMH: f64 = 3.0;

/// Where the most time goes, and what to do there: what the laps usually lose
/// when there are some, else this lap.
fn corner_advice(r: &CornersReport) -> Option<String> {
    let usual = r.usual_laps >= 2 && r.usual_s.len() == r.corners.len();
    let loss = |i: usize| {
        if usual {
            r.usual_s[i]
        } else {
            r.corners[i].loss_s
        }
    };
    let i = (0..r.corners.len())
        .filter(|&i| loss(i) >= CORNER_S)
        .max_by(|&a, &b| loss(a).total_cmp(&loss(b)))?;
    let c = &r.corners[i];
    let name = if let Some(n) = &c.name {
        n.clone()
    } else if i + 1 == r.corners.len() && i > 0 {
        "最終コーナー".to_string()
    } else {
        format!("{}コーナー", c.number)
    };
    // Five metres at a time: nobody brakes at a metre.
    let metres = |m: f64| ((m.abs() / CORNER_M).round() * CORNER_M).max(CORNER_M);
    let todo = if let Some(m) = c.brake_m.filter(|m| *m <= -CORNER_M) {
        format!("ブレーキをあと{:.0}m遅らせられる。", metres(m))
    } else if c.min_kmh <= -CORNER_KMH {
        format!("ボトムスピードをあと{:.0}キロ上げられる。", c.min_kmh.abs())
    } else if let Some(m) = c.throttle_m.filter(|m| *m >= CORNER_M) {
        format!("アクセルをあと{:.0}m早く開けられる。", metres(m))
    } else if let Some(m) = c.brake_m.filter(|m| *m >= CORNER_M) {
        format!("ブレーキが{:.0}m奥すぎる、少し手前で。", metres(m))
    } else {
        "ラインを見直そう。".to_string()
    };
    Some(format!("{name}で{}失ってる。{todo}", sec(loss(i))))
}

const PIT_LAP: &str = "ピットか黄旗の周は比べられない、次の周で伝える。";
const NO_SECTORS: &str = "区間タイムは1周走り切ってから出せる。";

pub fn speak(api: &dyn RaceApi, topic: Topic) -> ApiResult<Speech> {
    speak_with(api, topic, None)
}

/// With the corners of the laps of this session, where they were recorded.
pub fn speak_with(
    api: &dyn RaceApi,
    topic: Topic,
    corners: Option<&CornersReport>,
) -> ApiResult<Speech> {
    let b = api.brief()?;
    let text = if b.session.is_none() {
        "テレメトリが来ていない。コースに出たら答える。".to_string()
    } else if !b.meta.connected || b.meta.age_ms.is_some_and(|a| a > STALE_MS) {
        "テレメトリが途切れてる。戻ったら伝える。".to_string()
    } else {
        Voice {
            api,
            b: &b,
            corners,
        }
        .say(topic)?
    };
    Ok(Speech {
        meta: b.meta,
        topic,
        text,
    })
}

/// Sentences about one moment.
struct Voice<'a> {
    api: &'a dyn RaceApi,
    b: &'a Brief,
    corners: Option<&'a CornersReport>,
}

/// `0.6秒`; what is closer than a tenth counts in hundredths.
fn sec(s: f64) -> String {
    if s.abs() < 0.095 {
        format!("{:.2}秒", s.abs())
    } else {
        format!("{:.1}秒", s.abs())
    }
}

/// `1:55.6`
fn lap_time(s: f64) -> String {
    let tenths = (s * 10.0).round() as i64;
    let (m, rest) = (tenths / 600, tenths % 600);
    if m > 0 {
        format!("{m}:{:02}.{}", rest / 10, rest % 10)
    } else {
        format!("{}.{}秒", rest / 10, rest % 10)
    }
}

/// `3分05秒`
fn duration(s: f64) -> String {
    let s = s as i64;
    format!("{}分{:02}秒", s / 60, s % 60)
}

/// `この周` or `あと2周`
fn laps(l: f64) -> String {
    if l < 1.0 {
        "この周".into()
    } else {
        format!("あと{l:.0}周")
    }
}

fn place(c: &CarRef) -> Option<u32> {
    c.class_position.or(c.position)
}

fn now(g: &GapInfo) -> Option<f64> {
    g.live_s.or(g.gap_s)
}

fn distance(g: &GapInfo) -> Option<String> {
    if g.laps >= 1 && g.live_s.is_none() {
        return Some(format!("{}周", g.laps));
    }
    now(g).map(sec)
}

/// Seconds per lap, negative: shrinking.
fn change(g: &GapInfo) -> Option<f64> {
    g.trend_s_per_lap.or(g.live_change_s)
}

/// What the laps will bring.
#[derive(Clone, Copy)]
enum Chase {
    Catches(f64),
    /// Not before the end.
    TooLate,
    Steady,
    Drops,
    Unknown,
}

fn chase(g: &GapInfo, left: Option<f64>) -> Chase {
    let (Some(gap), Some(d)) = (now(g), change(g)) else {
        return Chase::Unknown;
    };
    if d >= STEADY_S {
        return Chase::Drops;
    }
    if d > -STEADY_S {
        return Chase::Steady;
    }
    let laps = gap / -d;
    if laps > left.unwrap_or(FAR_LAPS).min(FAR_LAPS) {
        Chase::TooLate
    } else {
        Chase::Catches(laps)
    }
}

fn join(parts: impl IntoIterator<Item = Option<String>>) -> String {
    parts.into_iter().flatten().collect()
}

fn or(text: String, none: &str) -> String {
    if text.is_empty() { none.into() } else { text }
}

impl Voice<'_> {
    fn session(&self) -> &SessionSummary {
        self.b.session.as_ref().expect("looked at before")
    }

    fn race(&self) -> bool {
        self.session().kind == SessionKind::Race
    }

    /// Laps to the flag.
    fn left(&self) -> Option<f64> {
        self.b.fuel.as_ref()?.laps_to_finish
    }

    /// The car raced against, as they run now.
    fn rival_of(&self, ahead: bool) -> Option<&GapInfo> {
        if ahead {
            self.b.running_ahead.as_ref().or(self.b.ahead.as_ref())
        } else {
            self.b.running_behind.as_ref().or(self.b.behind.as_ref())
        }
    }

    /// Official places lag behind what happens on track.
    fn place(&self) -> Option<u32> {
        let p = self.b.player.as_ref()?;
        p.live_position.or_else(|| place(&p.car))
    }

    fn leader(&self) -> Option<&GapInfo> {
        let ahead = || self.b.ahead.as_ref().filter(|g| place(&g.car) == Some(1));
        self.b.leader.as_ref().or_else(ahead)
    }

    fn close(&self, ahead: bool) -> bool {
        let g = self.rival_of(ahead).filter(|_| self.race());
        g.and_then(now).is_some_and(|s| s < CLOSE_S)
    }

    fn say(&self, topic: Topic) -> ApiResult<String> {
        Ok(match topic {
            Topic::Status => self.status(),
            Topic::Position => self.position(),
            Topic::Ahead => self.ahead(),
            Topic::Behind => self.behind(),
            Topic::Leader => self.to_leader(),
            Topic::LeaderLap => self.leader_lap(),
            Topic::AheadLap => self.ahead_lap(),
            Topic::Pace => self.pace(),
            Topic::Lap => self.lap(),
            Topic::Sectors => self.sectors()?,
            Topic::Losing => self.losing()?,
            Topic::Fuel => self.fuel(),
            Topic::FuelLaps => self.fuel_laps(),
            Topic::Left => self.what_is_left(),
            Topic::Flags => self.flags(),
            Topic::Weather => self.weather(),
            Topic::Damage => self.damage()?,
            Topic::Tyres => self.tyres()?,
            Topic::TyreTemps => self.tyre_temps()?,
            Topic::Forecast => self.forecast()?,
        })
    }

    fn left_text(&self) -> Option<String> {
        let s = self.session();
        if matches!(s.phase, SessionPhase::Checkered | SessionPhase::CoolDown) {
            return Some("チェッカーは出てる。".into());
        }
        if let Some(l) = self.left().filter(|_| self.race()) {
            return Some(if l < 1.0 {
                "この周で終わり。".into()
            } else {
                format!("残り{}周。", l.ceil())
            });
        }
        let t = s.time_remaining_s.filter(|t| (0.0..86400.0).contains(t))?;
        Some(format!("残り{}。", duration(t)))
    }

    fn short_of_fuel(&self) -> bool {
        let f = self.b.fuel.as_ref().filter(|_| self.race());
        f.is_some_and(|f| f.basis == FuelBasis::History && f.to_finish_l.is_some_and(|n| n > 0.0))
    }

    /// The one thing to look out for, if there is one.
    fn concern(&self) -> Option<String> {
        let flags = &self.session().flags;
        let hot = [
            Flag::Red,
            Flag::Caution,
            Flag::YellowWaving,
            Flag::Yellow,
            Flag::Repair,
            Flag::Black,
            Flag::BlackWhite,
        ];
        if let Some(f) = hot.into_iter().find(|f| flags.contains(f)) {
            return Some(format!("{}。", act_on(f)));
        }
        self.short_of_fuel()
            .then(|| "燃料が足りない、ピットが要る。".into())
    }

    /// What to do now, one thing.
    fn advice(&self) -> Option<String> {
        if let Some(c) = self.concern() {
            return Some(c);
        }
        if !self.race() {
            return None;
        }
        let of = |ahead| {
            self.rival_of(ahead)
                .map_or(Chase::Unknown, |g| chase(g, self.left()))
        };
        let text = if self.close(false) && !matches!(of(false), Chase::Drops) {
            "後ろが近い、守ろう。"
        } else if self.close(true) {
            "前は射程内、仕掛けよう。"
        } else {
            match (of(true), of(false)) {
                (_, Chase::Catches(l)) if l <= SOON_LAPS => "後ろが来る、守ろう。",
                (Chase::Catches(_), _) => "前に追いつける、攻めよう。",
                (_, Chase::Catches(_)) => "後ろが来る、ペースを上げよう。",
                (Chase::Unknown, Chase::Unknown) => return None,
                _ => "このペースを守ろう。",
            }
        };
        Some(text.into())
    }

    fn status(&self) -> String {
        let place = self.place().map(|p| format!("{p}番手。"));
        or(
            join([place, self.advice(), self.left_text()]),
            "まだ順位も差も出ていない。",
        )
    }

    fn position(&self) -> String {
        let place = match self.place() {
            Some(p) => format!("{p}番手。"),
            None => "まだ順位が出ていない。".into(),
        };
        join([Some(place), self.advice()])
    }

    /// Against a car: the sector won most and the one lost most.
    fn sectors_against(&self, g: &GapInfo) -> (Option<usize>, Option<usize>) {
        // Not worth losing the answer over.
        let r = self.api.sectors(&SectorsParams {
            car: Some(g.car.number.clone()),
        });
        let delta = r.map(|r| r.delta).unwrap_or_default();
        let most = |sign: f64| {
            delta
                .iter()
                .enumerate()
                .filter_map(|(i, d)| Some((i + 1, (*d)? * sign)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|(_, d)| *d >= SECTOR_S)
                .map(|(i, _)| i)
        };
        (most(-1.0), most(1.0))
    }

    /// Where to pass, or to find time.
    fn attack(&self, g: &GapInfo) -> Option<String> {
        match self.sectors_against(g) {
            (Some(w), _) if now(g).is_some_and(|s| s < FIGHT_S) => {
                Some(format!("狙いはセクター{w}。"))
            }
            (_, Some(l)) => Some(format!("セクター{l}を詰めよう。")),
            _ => None,
        }
    }

    fn ahead(&self) -> String {
        if !self.race() {
            return self.traffic(true);
        }
        if self.place() == Some(1) {
            return join([Some("トップだ、前はいない。".into()), self.advice()]);
        }
        let Some(g) = self.rival_of(true) else {
            return "前の車は取れていない。".into();
        };
        let gap = distance(g).map(|d| format!("前と{d}。"));
        if g.on_pit_road {
            return join([gap, Some("前はピットだ、今のうちに飛ばそう。".into())]);
        }
        let verdict = match chase(g, self.left()) {
            Chase::Catches(l) if self.close(true) => format!("{}で追いつく、仕掛けよう。", laps(l)),
            _ if self.close(true) => "仕掛けられる。".to_string(),
            Chase::Catches(l) => format!("{}で追いつく。", laps(l)),
            Chase::TooLate => "終わりまでには届かない。".into(),
            Chase::Steady => "差は変わらない。".into(),
            Chase::Drops => "離されてる。".into(),
            Chase::Unknown => String::new(),
        };
        or(
            join([gap, Some(verdict), self.attack(g)]),
            "前との差はまだ出ていない。",
        )
    }

    fn behind(&self) -> String {
        if !self.race() {
            return self.traffic(false);
        }
        let Some(g) = self.rival_of(false) else {
            return "後ろは気にしなくていい。".into();
        };
        let gap = distance(g).map(|d| format!("後ろと{d}。"));
        if g.on_pit_road {
            return join([gap, Some("後ろはピットだ。".into())]);
        }
        let guard = || {
            let (_, lost) = self.sectors_against(g);
            let where_to = lost.map_or("内側".into(), |l| format!("セクター{l}"));
            Some(format!("{where_to}を守ろう。"))
        };
        let (verdict, hint) = match chase(g, self.left()) {
            Chase::Drops => ("離してる、心配ない。".to_string(), None),
            _ if self.close(false) => ("近い。".into(), guard()),
            Chase::Catches(l) => (format!("{}で来る。", laps(l)), guard()),
            Chase::TooLate => ("逃げ切れる。".into(), None),
            Chase::Steady => ("差は変わらない。".into(), None),
            Chase::Unknown => (String::new(), None),
        };
        or(
            join([gap, Some(verdict), hint]),
            "後ろとの差はまだ出ていない。",
        )
    }

    /// Outside races the cars around matter, not the places.
    fn traffic(&self, ahead: bool) -> String {
        let side = if ahead { "前" } else { "後ろ" };
        let near = self.b.nearby.iter().find(|c| (c.rel_s > 0.0) == ahead);
        match near {
            Some(c) => format!("すぐ{side}に車、{}。", sec(c.rel_s)),
            None => format!("{side}は空いてる。"),
        }
    }

    fn to_leader(&self) -> String {
        if self.place() == Some(1) && self.race() {
            return join([Some("トップはこっちだ。".into()), self.advice()]);
        }
        let none = "トップとの差はまだ出ていない。";
        let Some(g) = self.leader() else {
            return none.into();
        };
        if !self.race() {
            let gap = g.gap_s.map(|d| format!("トップとはベストで{}差。", sec(d)));
            return or(gap.unwrap_or_default(), none);
        }
        let gap = distance(g).map(|d| format!("トップと{d}。"));
        let end = match chase(g, self.left()) {
            Chase::Catches(l) => Some(format!("{}で追いつく、攻めよう。", laps(l))),
            Chase::TooLate => Some("縮めてるけど届かない。".into()),
            Chase::Drops => Some("離されてる。".into()),
            Chase::Steady => Some("差は変わらない。".into()),
            Chase::Unknown => None,
        };
        or(join([gap, end]), none)
    }

    /// `トップは1:54.3、こっちが0.7秒速い。`
    fn their_lap(&self, who: &str, g: &GapInfo) -> Option<String> {
        let l = lap_time(g.their_last_lap_s?);
        let d = match g.last_lap_delta_s {
            Some(d) if d >= STEADY_S => format!("、こっちが{}速い", sec(d)),
            Some(d) if d <= -STEADY_S => format!("、こっちが{}遅い", sec(d)),
            Some(_) => "、同じペース".into(),
            None => String::new(),
        };
        Some(format!("{who}は{l}{d}。"))
    }

    fn leader_lap(&self) -> String {
        if self.place() == Some(1) && self.race() {
            return join([Some("トップはこっちだ。".into()), self.last_lap()]);
        }
        let lap = self.leader().and_then(|g| self.their_lap("トップ", g));
        or(lap.unwrap_or_default(), "トップのラップはまだ出ていない。")
    }

    fn ahead_lap(&self) -> String {
        let g = self.rival_of(true);
        let lap = g.and_then(|g| self.their_lap("前", g));
        or(lap.unwrap_or_default(), "前の車のラップはまだ出ていない。")
    }

    /// The last lap, and whether it was a good one.
    fn last_lap(&self) -> Option<String> {
        let p = self.b.player.as_ref()?;
        let last = p.last_lap_s?;
        let how = match p.best_lap_s {
            Some(b) if last - b < 0.0005 => "、ベストだ".to_string(),
            Some(b) if last - b <= GOOD_S => "、いいペース".into(),
            Some(b) => format!("、ベストから{}落ち", sec(last - b)),
            None => String::new(),
        };
        Some(format!("{}{how}。", lap_time(last)))
    }

    fn pace(&self) -> String {
        let Some(own) = self.last_lap() else {
            return "まだ1周の記録が無い。".into();
        };
        let ahead = self.rival_of(true).filter(|_| self.race());
        let lost = ahead.and_then(|g| self.sectors_against(g).1);
        join([Some(own), lost.map(|l| format!("セクター{l}を詰めよう。"))])
    }

    fn lap(&self) -> String {
        let gap = self.leader().filter(|_| self.race()).and_then(|g| {
            let c = match g.live_change_s.or(g.trend_s_per_lap) {
                Some(c) if c <= -STEADY_S => "、縮めた",
                Some(c) if c >= STEADY_S => "、離された",
                _ => "",
            };
            Some(format!("トップと{}{c}。", distance(g)?))
        });
        let hint = if self.close(false) {
            Some("後ろが近い。".into())
        } else {
            let ahead = self.rival_of(true).filter(|_| self.race());
            ahead.and_then(|g| self.attack(g))
        };
        or(join([self.last_lap(), gap, hint]), "まだ1周の記録が無い。")
    }

    fn sectors(&self) -> ApiResult<String> {
        let r = self.api.sectors(&SectorsParams::default())?;
        let me = r.rows.first().filter(|_| r.basis != SectorBasis::None);
        let Some(me) = me.filter(|m| m.last.iter().any(Option::is_some)) else {
            return Ok(NO_SECTORS.into());
        };
        // The verdict, not the times: nobody keeps six numbers in mind.
        let lost: Option<f64> = me
            .last
            .iter()
            .zip(&me.best)
            .map(|(a, b)| Some((*a)? - (*b)?))
            .sum();
        let own = lost.map(|l| {
            if l >= STEADY_S {
                format!("前の周は自己ベストから{}落ち。", sec(l))
            } else {
                "前の周は自己ベスト並み。".to_string()
            }
        });
        let named = r.losses.first().filter(|l| l.lost_s >= STEADY_S);
        let place = match named.and_then(|l| Some((l.name.as_ref()?, l.lost_s))) {
            Some((name, s)) => Some(format!("{name}で前に{}負けてる。", sec(s))),
            None => self.worst_sector(&r),
        };
        Ok(or(join([own, place]), "区間の差は無い、いいペースだ。"))
    }

    /// The sector to work on.
    fn worst_sector(&self, r: &SectorsReport) -> Option<String> {
        let me = r.rows.first()?;
        let worst = |d: Vec<Option<f64>>| {
            d.into_iter()
                .enumerate()
                .filter_map(|(i, d)| Some((i + 1, d?)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|(_, d)| *d >= STEADY_S)
        };
        if r.versus.is_some() {
            return worst(r.delta.clone())
                .map(|(i, d)| format!("負けてるのはセクター{i}、{}。", sec(d)));
        }
        let own = me.last.iter().zip(&me.best);
        worst(own.map(|(a, b)| Some((*a)? - (*b)?)).collect())
            .map(|(i, d)| format!("セクター{i}が自己ベストから{}落ち。", sec(d)))
    }

    fn losing(&self) -> ApiResult<String> {
        if let Some(text) = self.corners.and_then(corner_advice) {
            return Ok(text);
        }
        let r = self.api.sectors(&SectorsParams::default())?;
        if r.basis == SectorBasis::None || r.rows.is_empty() {
            return Ok(NO_SECTORS.into());
        }
        // A stop or a caution is no time lost on the track.
        let off = |car: Option<&CarRef>| {
            let last = self.api.laps(&LapsParams {
                car: car.map(|c| c.number.clone()),
                last: Some(1),
            });
            last.ok()
                .and_then(|l| l.laps.last().cloned())
                .is_some_and(|l| !l.is_clean())
        };
        if off(None) || r.versus.as_ref().is_some_and(|c| off(Some(c))) {
            return Ok(PIT_LAP.into());
        }
        if let Some(l) = r.losses.first().filter(|l| l.lost_s >= STEADY_S) {
            return Ok(format!(
                "{}で{}負けてる、そこを詰めよう。",
                spot(&r.starts, l),
                sec(l.lost_s)
            ));
        }
        Ok(or(
            self.worst_sector(&r).unwrap_or_default(),
            "負けてる区間は無い、いいペースだ。",
        ))
    }

    /// The lap at whose end the car has to stop at the latest.
    fn box_by(&self) -> Option<i32> {
        let n = self.b.fuel.as_ref()?.laps_of_fuel?;
        let lap = self.session().lap?;
        Some(lap + (n.floor() as i32 - 1).max(0))
    }

    fn fuel(&self) -> String {
        let f = self.b.fuel.as_ref().filter(|f| f.fuel_l.is_some());
        let f = f.filter(|f| self.race() && f.laps_of_fuel.is_some());
        let Some((f, short)) = f.and_then(|f| Some((f, f.to_finish_l?))) else {
            return self.fuel_laps();
        };
        if short > 0.0 {
            let by = self.box_by().map(|l| format!("{l}周目までにピット"));
            let add = f
                .add_with_margin_l
                .map(|a| format!("{}リッター入れる", a.ceil()));
            let todo: Vec<String> = by.into_iter().chain(add).collect();
            let todo = (!todo.is_empty()).then(|| format!("{}。", todo.join("、")));
            join([Some("燃料が足りない。".into()), todo])
        } else if -short < TIGHT_L {
            "燃料はぎりぎりだ、少し抑えて完走しよう。".into()
        } else {
            "燃料は大丈夫だ、このまま完走する。".into()
        }
    }

    fn fuel_laps(&self) -> String {
        let Some((f, l)) = self.b.fuel.as_ref().and_then(|f| Some((f, f.fuel_l?))) else {
            return "燃料は取れていない。".into();
        };
        let Some(n) = f.laps_of_fuel else {
            return format!("燃料は{l:.0}リッター、1周走れば何周持つか出せる。");
        };
        let end = match f.to_finish_l.filter(|_| self.race()) {
            Some(short) if short > 0.0 => "ピットが要る。",
            Some(_) => "完走できる。",
            None => "",
        };
        format!("燃料はあと{}周分。{end}", n.floor())
    }

    fn what_is_left(&self) -> String {
        let Some(left) = self.left_text() else {
            return "残りはまだ分からない。".into();
        };
        let fuel = self
            .short_of_fuel()
            .then(|| "燃料が足りない、ピットが要る。".to_string());
        join([Some(left), fuel])
    }

    fn flags(&self) -> String {
        let shown: Vec<&str> = self
            .session()
            .flags
            .iter()
            .filter(|f| **f != Flag::Green)
            .map(|f| act_on(*f))
            .collect();
        if shown.is_empty() {
            "旗は無い、全開でいい。".into()
        } else {
            format!("{}。", shown.join("。"))
        }
    }

    fn weather(&self) -> String {
        let none = "天気は取れていない。";
        let Some(w) = &self.b.weather else {
            return none.into();
        };
        let wet = w.wetness.as_deref();
        let track = w.track_temp_c.map(|t| format!("路面{t:.0}度。"));
        if wet == Some("dry") && w.precipitation.is_some_and(|p| p <= 0.0) {
            return join([Some("ドライ、雨は無い。".into()), track]);
        }
        let road = wet.map(|w| format!("路面は{}。", wetness(w)));
        let rain = w.precipitation.map(|p| {
            let trend = self.b.rain_trend;
            let text = match (p > 0.0, trend.map(|t| t.direction)) {
                (false, _) => "雨は無い。",
                (true, Some(RainDirection::Rising)) => "雨が強くなる、備えよう。",
                // At the rate it eases: the game tells no forecast.
                (true, Some(RainDirection::Easing)) => match trend.filter(|t| t.per_min < 0.0) {
                    Some(t) => {
                        return format!(
                            "雨は弱まってる、{:.0}分ほどで止む見込み。",
                            (p / -t.per_min).ceil()
                        );
                    }
                    None => "雨は弱まる。",
                },
                (true, _) => "雨が降ってる。",
            };
            text.to_string()
        });
        or(join([road, rain, track]), none)
    }

    fn damage(&self) -> ApiResult<String> {
        let r = self.api.car()?;
        let Some(c) = &r.car else {
            return Ok("車の状態は取れていない。".into());
        };
        let repair = |s: Option<f64>| s.filter(|s| *s >= 0.5);
        let must = repair(c.pit.repair_left_s);
        if self.session().flags.contains(&Flag::Repair) || must.is_some() {
            return Ok("修理が要る、この周ピットに入ろう。".into());
        }
        if let Some(heat) = c.warnings.iter().find_map(|w| warning(*w)) {
            return Ok(heat.into());
        }
        let optional = repair(c.pit.optional_repair_left_s);
        if !c.warnings.contains(&Warning::Damage) && optional.is_none() {
            return Ok("ダメージは無い、このまま行ける。".into());
        }
        // No game tells what the wings have lost; the laps do.
        let p = self.b.player.as_ref();
        let lost = p.and_then(|p| Some(p.last_lap_s? - p.best_lap_s?));
        Ok(match lost {
            Some(d) if d > HURT_S => "ダメージでペースが落ちてる、ピットで直そう。",
            Some(_) => "ダメージは軽い、このまま行こう。",
            None => "ダメージがある、1周様子を見よう。",
        }
        .into())
    }

    /// Where a tyre is, and what it shows.
    fn each_tyre(
        &self,
        of: impl Fn(&vr_core::Tyre) -> Option<f64>,
    ) -> ApiResult<Vec<(&'static str, f64)>> {
        let names = ["左前", "右前", "左後ろ", "右後ろ"];
        let r = self.api.car()?;
        let Some(c) = &r.car else {
            return Ok(Vec::new());
        };
        let all = c.tyres.all();
        let each = names.into_iter().zip(&all);
        Ok(each.filter_map(|(n, (_, t))| Some((n, of(t)?))).collect())
    }

    fn tyres(&self) -> ApiResult<String> {
        let tread = self.each_tyre(|t| t.min_tread())?;
        let worst = tread.into_iter().min_by(|a, b| a.1.total_cmp(&b.1));
        Ok(match worst {
            None => "タイヤの状態は取れていない。".into(),
            Some((n, w)) if w < GONE => format!("{n}が限界だ、替えよう。"),
            Some((n, w)) if w < WORN => format!("{n}が減ってる、労ろう。"),
            Some(_) => "タイヤは大丈夫だ、攻めていい。".into(),
        })
    }

    fn tyre_temps(&self) -> ApiResult<String> {
        let temps = self.each_tyre(|t| t.mean_temp_c())?;
        if temps.is_empty() {
            return Ok("タイヤの温度は取れていない。".into());
        }
        let mean = temps.iter().map(|t| t.1).sum::<f64>() / temps.len() as f64;
        let hot = temps.iter().max_by(|a, b| a.1.total_cmp(&b.1));
        let verdict = match hot {
            Some((n, t)) if t - mean >= HOT_C => format!("{n}だけ熱い、労ろう。"),
            _ => "偏りは無い、大丈夫だ。".into(),
        };
        Ok(format!("最後に止まった時で平均{mean:.0}度。{verdict}"))
    }

    fn forecast(&self) -> ApiResult<String> {
        let r = self.api.forecast()?;
        let (Some(p), Some(end), true) = (r.position, r.finish, self.race()) else {
            return Ok("あと2周走れば見通しを出せる。".into());
        };
        let head = match (end, p) {
            (1, 1) => "このまま優勝できる。".to_string(),
            (1, _) => "優勝できる、攻めよう。".into(),
            _ => format!("このままなら{end}番手。"),
        };
        let meets = |ahead| r.next(ahead).and_then(|c| c.meets_in_laps);
        let next = match (end.cmp(&p), meets(true), meets(false)) {
            (std::cmp::Ordering::Less, Some(l), _) => Some(format!("{}で前に追いつく。", laps(l))),
            (std::cmp::Ordering::Greater, _, Some(l)) => {
                Some(format!("{}で後ろが来る、守ろう。", laps(l)))
            }
            _ if p == 1 => None,
            // What it would take, if that is within reach.
            _ => r.next(true).and_then(|c| {
                let more = c.at_flag_s? / r.laps_left.filter(|l| *l >= 1.0)?;
                (more > 0.0 && more <= MORE_S)
                    .then(|| format!("今より毎周{}速ければ前に届く。", sec(more)))
            }),
        };
        Ok(join([Some(head), next]))
    }
}

/// `セクター2の中盤`
fn spot(starts: &[f64], s: &StretchLoss) -> String {
    if let Some(name) = &s.name {
        return name.clone();
    }
    let from = starts.get(s.sector - 1).copied().unwrap_or(0.0);
    let to = starts.get(s.sector).copied().unwrap_or(1.0);
    let at = ((s.from + s.to) / 2.0 - from) / (to - from);
    let part = if at < 1.0 / 3.0 {
        "序盤"
    } else if at < 2.0 / 3.0 {
        "中盤"
    } else {
        "終盤"
    };
    format!("セクター{}の{part}", s.sector)
}

/// What is shown, and what to do about it.
fn act_on(f: Flag) -> &'static str {
    match f {
        Flag::Green => "グリーン",
        Flag::Yellow => "イエロー、気をつけて",
        Flag::YellowWaving => "近くでイエロー、抑えて",
        Flag::Caution => "フルコースコーション、追い抜き禁止",
        Flag::Red => "赤旗、減速",
        Flag::Blue => "青旗、後ろに譲ろう",
        Flag::White => "ファイナルラップ",
        Flag::Checkered => "チェッカー",
        Flag::Black => "黒旗、ペナルティだ",
        Flag::Repair => "修理旗、この周ピットに入ろう",
        Flag::Disqualify => "失格だ",
        Flag::Debris => "落下物に注意",
        Flag::OneLapToGreen => "次の周でグリーン",
        Flag::SlowCar => "前に遅い車、注意",
        Flag::BlackWhite => "白黒旗、走り方の警告だ",
    }
}

fn wetness(w: &str) -> &str {
    match w {
        "dry" => "ドライ",
        "mostly dry" => "ほぼドライ",
        "very lightly wet" => "わずかに濡れてる",
        "lightly wet" => "少し濡れてる",
        "moderately wet" => "濡れてる",
        "very wet" => "かなり濡れてる",
        "extremely wet" => "水浸し",
        other => other,
    }
}

fn warning(w: Warning) -> Option<&'static str> {
    Some(match w {
        Warning::WaterTemp => "水温が高い、前から離れて冷やそう。",
        Warning::OilTemp => "油温が高い、少し抑えよう。",
        Warning::OilPressure => "油圧が低い、止めた方がいい。",
        Warning::FuelPressure => "燃圧が低い、ガス欠に注意。",
        Warning::Stalled => "エンジンが止まってる、かけ直そう。",
        Warning::Engine => "エンジンの警告灯、回転を抑えよう。",
        Warning::FlatTyre => "パンクだ、ピットへ。",
        Warning::Clutch => "クラッチが熱い、発進は丁寧に。",
        Warning::PitLimiter | Warning::RevLimiter | Warning::Damage => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{Corner, Speeds};

    fn corner(number: usize, loss_s: f64, brake_m: Option<f64>, min_kmh: f64) -> Corner {
        Corner {
            number,
            from: 0.0,
            apex: 0.0,
            to: 0.0,
            loss_s,
            brake_m,
            min_kmh,
            throttle_m: Some(0.0),
            exit_kmh: 150.0,
            reference_min_kmh: 90.0,
            name: None,
            brake_before_m: None,
            reference_brake_before_m: None,
            gear: None,
            reference_gear: None,
            entry_kmh: 0.0,
            reference_entry_kmh: 0.0,
            min_throttle: 0,
            reference_min_throttle: 0,
        }
    }

    fn report(corners: Vec<Corner>, usual_s: Vec<f64>, usual_laps: usize) -> CornersReport {
        CornersReport {
            lap: 5,
            time_s: 90.0,
            reference_lap: 3,
            reference_s: 89.5,
            length_m: 4000.0,
            corners,
            usual_s,
            usual_laps,
            speeds: Speeds {
                top_kmh: 250.0,
                top: 0.0,
                slow: None,
                medium: None,
                fast: None,
            },
        }
    }

    #[test]
    fn the_corner_that_loses_most_is_told_with_what_to_do() {
        let r = report(
            vec![
                corner(1, 0.10, Some(-12.0), 0.0),
                corner(2, 0.30, Some(-7.4), 0.0),
                corner(3, 0.05, None, -4.0),
            ],
            vec![],
            0,
        );
        assert_eq!(
            corner_advice(&r).unwrap(),
            "2コーナーで0.3秒失ってる。ブレーキをあと5m遅らせられる。"
        );
        let mut named = r.clone();
        named.corners[1].name = Some("スプーン".into());
        assert_eq!(
            corner_advice(&named).unwrap(),
            "スプーンで0.3秒失ってる。ブレーキをあと5m遅らせられる。"
        );
        // A habit counts more than one lap; the last corner has its name.
        let r = report(r.corners, vec![0.0, 0.04, 0.12], 3);
        assert_eq!(
            corner_advice(&r).unwrap(),
            "最終コーナーで0.1秒失ってる。ボトムスピードをあと4キロ上げられる。"
        );
    }

    #[test]
    fn small_losses_and_small_differences_are_not_advice() {
        let r = report(vec![corner(1, 0.049, Some(-20.0), 0.0)], vec![], 0);
        assert_eq!(corner_advice(&r), None);
        let mut c = corner(1, 0.05, Some(-4.9), -2.9);
        assert_eq!(
            corner_advice(&report(vec![c.clone()], vec![], 0)).unwrap(),
            "1コーナーで0.05秒失ってる。ラインを見直そう。"
        );
        c.throttle_m = Some(5.0);
        assert_eq!(
            corner_advice(&report(vec![c.clone()], vec![], 0)).unwrap(),
            "1コーナーで0.05秒失ってる。アクセルをあと5m早く開けられる。"
        );
        c.throttle_m = None;
        c.brake_m = Some(13.0);
        assert_eq!(
            corner_advice(&report(vec![c], vec![], 0)).unwrap(),
            "1コーナーで0.05秒失ってる。ブレーキが15m奥すぎる、少し手前で。"
        );
    }
}
