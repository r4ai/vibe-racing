//! Facts taken from the telemetry, and whether a spoken reply states them.
//!
//! The race moves while the radio works, so a number is right when it lies
//! between the values seen from shortly before the end of the question to
//! the start of the answer, plus what rounding for speech allows. An answer
//! from older data falls outside.

use serde::{Deserialize, Serialize};
use vr_core::Flag;
use vr_engine::{Brief, StandingsReport, StrategyReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fact {
    GapAhead,
    GapBehind,
    GapLeader,
    TrendBehind,
    Position,
    LapsLeft,
    FuelLaps,
    FuelAdd,
    FuelPerLap,
    LastLap,
    BestLap,
    AvgLap,
    LastLapAhead,
    NameAhead,
    CarsBehind,
    Yellow,
    /// Least tread of the tyres, in percent.
    TyreTread,
    /// Push-to-pass uses left.
    P2p,
    BrakeBias,
    TractionControl,
    /// Mandatory repairs.
    RepairS,
    /// Best lap of the previous session.
    PastBest,
    /// Lap to stop on, of any plan about as good as the best.
    StopLap,
    /// Seconds a stop costs.
    PitLoss,
    /// Place after a stop now.
    Rejoin,
}

/// The telemetry at one moment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// Seconds since the call started.
    pub t: f64,
    pub brief: Brief,
    pub standings: StandingsReport,
    #[serde(default)]
    pub past_best_s: Option<f64>,
    #[serde(default)]
    pub strategy: Option<StrategyReport>,
}

impl Sample {
    /// The moving values on one line, for the judges.
    pub fn line(&self) -> String {
        let b = &self.brief;
        let gap = |g: &Option<vr_engine::GapInfo>, sign: char| {
            g.as_ref().map_or("--".to_string(), |g| {
                format!(
                    "#{} {} {sign}{:.2}s{}",
                    g.car.number,
                    g.car.driver,
                    g.gap_s.unwrap_or(f64::NAN),
                    if g.on_pit_road { " PIT" } else { "" }
                )
            })
        };
        let position = b.player.as_ref().and_then(|p| p.car.position);
        let (laps, need, add) = b.fuel.as_ref().map_or((None, None, None), |f| {
            (f.laps_of_fuel, f.to_finish_l, f.add_with_margin_l)
        });
        let left = b.session.as_ref().and_then(|s| s.laps_remaining);
        let flags = b.session.as_ref().map(|s| &s.flags);
        format!(
            "t={:+.0}s P{} | ahead {} | behind {} | fuel lasts {:.1} laps, short {:.1}L, add {:.1}L | {} laps left | {:?}",
            self.t,
            position.unwrap_or(0),
            gap(&b.ahead, '+'),
            gap(&b.behind, '-'),
            laps.unwrap_or(f64::NAN),
            need.unwrap_or(f64::NAN),
            add.unwrap_or(f64::NAN),
            left.unwrap_or(-1),
            flags,
        )
    }
}

/// A fact at one moment.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A number and what rounding for speech may change.
    Number(f64, f64),
    /// Any of these words.
    Words(Vec<String>),
}

/// A plan that loses less is as good as the best.
const AS_GOOD_S: f64 = 1.0;

fn control(b: &Brief, id: &str) -> Option<f64> {
    let controls = &b.car.as_ref()?.controls;
    controls.iter().find(|c| c.id == id).map(|c| c.value)
}

impl Fact {
    pub fn value(self, s: &Sample) -> Option<Value> {
        let b = &s.brief;
        let gap = |g: &Option<vr_engine::GapInfo>| g.as_ref()?.gap_s;
        let lap = |v: Option<f64>| v.map(|v| Value::Number(v, 0.11));
        Some(match self {
            // Gaps are spoken in 0.1 s; fast changing.
            Fact::GapAhead => Value::Number(gap(&b.ahead)?, 0.11),
            Fact::GapBehind => Value::Number(gap(&b.behind)?, 0.11),
            Fact::GapLeader => Value::Number(
                s.standings
                    .rows
                    .iter()
                    .find(|r| r.is_player)?
                    .gap_to_leader_s?,
                0.11,
            ),
            Fact::TrendBehind => Value::Number(b.behind.as_ref()?.trend_s_per_lap?.abs(), 0.11),
            Fact::Position => Value::Number(f64::from(b.player.as_ref()?.car.position?), 0.0),
            // "11 laps left" counts the current lap; the one after it is 10.
            Fact::LapsLeft => {
                Value::Number(f64::from(b.session.as_ref()?.laps_remaining?) - 0.5, 0.5)
            }
            Fact::FuelLaps => Value::Number(b.fuel.as_ref()?.laps_of_fuel?, 0.5),
            // Between the bare need and the need with margin.
            Fact::FuelAdd => {
                let f = b.fuel.as_ref()?;
                let (need, margin) = (f.to_finish_l?, f.add_with_margin_l?);
                Value::Number((need + margin) / 2.0, (margin - need) / 2.0 + 0.5)
            }
            Fact::FuelPerLap => Value::Number(b.fuel.as_ref()?.per_lap_l?, 0.11),
            Fact::LastLap => lap(b.player.as_ref()?.last_lap_s)?,
            Fact::BestLap => lap(b.player.as_ref()?.best_lap_s)?,
            Fact::AvgLap => lap(b.player.as_ref()?.avg_lap_s)?,
            Fact::LastLapAhead => lap(b.ahead.as_ref()?.their_last_lap_s)?,
            Fact::NameAhead => {
                let car = &b.ahead.as_ref()?.car;
                let name = car.driver.split_whitespace().last()?.to_string();
                Value::Words(vec![kana(&name).into(), name, format!("{}番", car.number)])
            }
            Fact::CarsBehind => {
                let p = b.player.as_ref()?;
                let position = p.car.position? as usize;
                Value::Number((p.cars_total - position) as f64, 0.0)
            }
            Fact::TyreTread => {
                let t = &b.car.as_ref()?.tyres;
                let all = [&t.lf, &t.rf, &t.lr, &t.rr];
                let least = all
                    .iter()
                    .flat_map(|t| t.tread.iter().flatten())
                    .copied()
                    .reduce(f64::min)?;
                Value::Number(least * 100.0, 1.0)
            }
            Fact::P2p => Value::Number(f64::from(b.car.as_ref()?.p2p_count?), 0.0),
            Fact::BrakeBias => Value::Number(control(b, "brake_bias")?, 0.05),
            Fact::TractionControl => Value::Number(control(b, "traction_control")?, 0.0),
            Fact::RepairS => Value::Number(b.car.as_ref()?.pit.repair_left_s?, 1.0),
            Fact::PastBest => lap(s.past_best_s)?,
            Fact::StopLap => {
                let plans = &s.strategy.as_ref()?.plans;
                let best = plans.first()?.stops.len();
                let stops: Vec<f64> = plans
                    .iter()
                    .filter(|p| p.behind_best_s < AS_GOOD_S && p.stops.len() == best)
                    .filter_map(|p| p.stops.first())
                    .map(|l| f64::from(*l))
                    .collect();
                let lo = stops.iter().copied().reduce(f64::min)?;
                let hi = stops.iter().copied().reduce(f64::max)?;
                Value::Number((lo + hi) / 2.0, (hi - lo) / 2.0)
            }
            Fact::PitLoss => Value::Number(s.strategy.as_ref()?.pit_loss.seconds?, 0.6),
            Fact::Rejoin => {
                let r = s.strategy.as_ref()?.rejoin_now.as_ref()?;
                Value::Number(f64::from(r.position), 0.0)
            }
            Fact::Yellow => {
                let on = b
                    .session
                    .as_ref()?
                    .flags
                    .iter()
                    .any(|f| matches!(f, Flag::Yellow | Flag::YellowWaving | Flag::Caution));
                let words: &[&str] = if on {
                    &["イエロー", "黄旗"]
                } else {
                    &["グリーン", "出ていない", "出てない", "なし", "無し"]
                };
                Value::Words(words.iter().map(|w| w.to_string()).collect())
            }
        })
    }

    /// A count of something else is not this fact: `4位` is not `残り4周`.
    fn unit(self) -> Unit {
        match self {
            Fact::Position | Fact::Rejoin => Unit::Position,
            Fact::LapsLeft | Fact::FuelLaps | Fact::StopLap => Unit::Laps,
            Fact::FuelAdd | Fact::FuelPerLap => Unit::Litres,
            Fact::CarsBehind => Unit::Cars,
            _ => Unit::Seconds,
        }
    }

    /// Whether `reply` states the fact as it was at some moment of `samples`.
    pub fn stated(self, reply: &str, samples: &[Sample]) -> bool {
        let values: Vec<Value> = samples.iter().filter_map(|s| self.value(s)).collect();
        let reply = normalize(reply);
        let mut range: Option<(f64, f64)> = None;
        for v in &values {
            match v {
                Value::Words(words) => {
                    if words.iter().any(|w| reply.contains(&normalize(w))) {
                        return true;
                    }
                }
                Value::Number(n, tolerance) => {
                    let (lo, hi) = (n - tolerance, n + tolerance);
                    range = Some(range.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
                }
            }
        }
        // Floats: 0.6 spoken for 0.71 must not depend on the last bit.
        range.is_some_and(|(lo, hi)| {
            spoken(&reply)
                .iter()
                .any(|n| n.unit == self.unit() && n.value >= lo - 1e-6 && n.value <= hi + 1e-6)
        })
    }
}

/// Driver names as a Japanese speaker may write them.
fn kana(name: &str) -> &'static str {
    match name {
        "Senna" => "セナ",
        "Hart" => "ハート",
        "Ito" => "イトウ",
        "Kowalski" => "コワルスキ",
        "Rossi" => "ロッシ",
        "Mueller" => "ミュラー",
        "Dubois" => "デュボワ",
        "Tanaka" => "タナカ",
        "Novak" => "ノバク",
        "Smith" => "スミス",
        "Lee" => "リー",
        _ => "\u{0}",
    }
}

/// Full-width digits and signs to ASCII, lower case.
pub fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '０'..='９' | 'Ａ'..='Ｚ' | 'ａ'..='ｚ' => {
                char::from_u32(c as u32 - 0xfee0).unwrap_or(c)
            }
            '．' => '.',
            '：' => ':',
            '＋' => '+',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// Numbers said in a reply. Lap times (`1分30秒3`, `1:30.3`, `90秒3`) are
/// seconds; `6秒3` is 6.3.
pub fn numbers(text: &str) -> Vec<f64> {
    spoken(text).into_iter().map(|n| n.value).collect()
}

/// A number and what it counts.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Spoken {
    value: f64,
    unit: Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    /// Seconds, or nothing said.
    Seconds,
    Position,
    Laps,
    Cars,
    Litres,
    /// A car number or another count.
    Other,
}

fn unit(before: Option<char>, after: &[char]) -> Unit {
    let after: String = after.iter().filter(|c| **c != ' ').take(3).collect();
    let starts = |words: &[&str]| words.iter().any(|w| after.starts_with(w));
    if before == Some('p') || starts(&["位", "番手"]) {
        Unit::Position
    } else if before == Some('#') || starts(&["番", "号車"]) {
        Unit::Other
    } else if starts(&["周", "ラップ", "lap"]) {
        Unit::Laps
    } else if starts(&["台"]) {
        Unit::Cars
    } else if starts(&["リ", "l"]) {
        Unit::Litres
    } else {
        Unit::Seconds
    }
}

fn spoken(text: &str) -> Vec<Spoken> {
    let text = kanji_digits(text);
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let Some((n, mut end)) = number_at(&chars, i) else {
            i += 1;
            continue;
        };
        let mut value = n;
        let minutes = match chars.get(end) {
            Some('分') => true,
            Some(':') => number_at(&chars, end + 1).is_some(),
            _ => false,
        };
        let space = |mut j: usize| {
            while chars.get(j) == Some(&' ') {
                j += 1;
            }
            j
        };
        if minutes && let Some((s, e)) = number_at(&chars, space(end + 1)) {
            value = n * 60.0 + s;
            end = e;
        }
        // Tenths after the seconds: `30秒3`.
        if chars.get(end) == Some(&'秒')
            && let Some((tenths, e)) = number_at(&chars, space(end + 1))
            && (0.0..10.0).contains(&tenths)
            && tenths.fract() == 0.0
            && value.fract() == 0.0
            && !matches!(chars.get(e), Some('周' | '位' | '台' | '番' | 'l' | 'リ'))
        {
            value += tenths / 10.0;
            end = e;
        }
        let before = i.checked_sub(1).map(|b| chars[b]);
        out.push(Spoken {
            value,
            unit: unit(before, &chars[end..]),
        });
        i = end;
    }
    out
}

/// A decimal number starting at `i`, and the index after it.
fn number_at(chars: &[char], i: usize) -> Option<(f64, usize)> {
    if !chars.get(i)?.is_ascii_digit() {
        return None;
    }
    let mut j = i;
    let mut s = String::new();
    let mut dot = false;
    while let Some(c) = chars.get(j) {
        let next_is_digit = chars.get(j + 1).is_some_and(char::is_ascii_digit);
        match c {
            '0'..='9' => s.push(*c),
            '.' if !dot && next_is_digit => {
                dot = true;
                s.push('.');
            }
            _ => break,
        }
        j += 1;
    }
    Some((s.parse().ok()?, j))
}

/// `三周` → `3周`, `十一` → `11`, `二十` → `20`; only before a counter, so
/// that words such as 一番 or 一気 stay words.
fn kanji_digits(text: &str) -> String {
    const COUNTERS: &str = "周位台秒分リ点";
    let digit = |c: char| "〇一二三四五六七八九".chars().position(|d| d == c);
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let (mut tens, mut ones, mut any) = (None, None, false);
        while let Some(&c) = chars.get(i) {
            if c == '十' {
                tens = Some(ones.take().unwrap_or(1));
            } else if let Some(d) = digit(c) {
                ones = Some(d);
            } else {
                break;
            }
            any = true;
            i += 1;
        }
        if !any {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if chars.get(i).is_some_and(|c| COUNTERS.contains(*c)) && i - start <= 3 {
            out.push_str(&(tens.unwrap_or(0) * 10 + ones.unwrap_or(0)).to_string());
        } else {
            out.extend(&chars[start..i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::WorldSpec;
    use crate::world::sample_at;

    fn at(t: f64) -> Sample {
        sample_at(&WorldSpec::default(), t)
    }

    #[test]
    fn spoken_numbers() {
        assert_eq!(numbers("前まで 0.6、毎周 0.4 詰めてる"), vec![0.6, 0.4]);
        assert_eq!(numbers("ベストは1分30秒3"), vec![90.3]);
        assert_eq!(
            numbers("1 分 30 秒 3 だ".replace(' ', "").as_str()),
            vec![90.3]
        );
        assert_eq!(numbers("1:30.3 と 1:31.0"), vec![90.3, 91.0]);
        assert_eq!(numbers("6秒3"), vec![6.3]);
        assert_eq!(numbers("90.3秒"), vec![90.3]);
        assert_eq!(numbers("後ろは2秒。3周で追いつく"), vec![2.0, 3.0]);
        assert_eq!(numbers("残り2秒3周"), vec![2.0, 3.0]);
        assert_eq!(
            numbers("P4、三周、十一周、二十周"),
            vec![4.0, 3.0, 11.0, 20.0]
        );
        assert_eq!(numbers("一番いい、一気に"), Vec::<f64>::new());
        assert_eq!(numbers(&normalize("０．６秒")), vec![0.6]);
        assert_eq!(numbers("4."), vec![4.0]);
    }

    /// t=620: P4, +0.56 ahead (Ito), -7.53 behind, 5.1 laps of fuel,
    /// 20.7 L short (22.0 with margin), best 1:30.0, 14 laps left.
    #[test]
    fn facts_of_a_known_moment() {
        let s = [at(620.0)];
        let yes = |f: Fact, reply: &str| assert!(f.stated(reply, &s), "{f:?} in {reply}");
        let no = |f: Fact, reply: &str| assert!(!f.stated(reply, &s), "{f:?} in {reply}");
        yes(Fact::GapAhead, "前の Ito まで 0.6。");
        no(Fact::GapAhead, "前の Ito まで 0.8。");
        no(Fact::GapAhead, "前は近い");
        yes(Fact::GapBehind, "後ろは7.5秒");
        no(Fact::GapBehind, "後ろは0.6秒");
        yes(Fact::Position, "4位");
        yes(Fact::Position, "いまP4だ");
        no(Fact::Position, "残り4周");
        no(Fact::LapsLeft, "14位");
        no(Fact::Position, "3位");
        yes(Fact::LapsLeft, "残り14周");
        yes(Fact::LapsLeft, "残り13周");
        no(Fact::LapsLeft, "残り12周");
        no(Fact::LapsLeft, "残り15周");
        yes(Fact::FuelLaps, "あと5周");
        yes(Fact::FuelLaps, "あと5.5周");
        no(Fact::FuelLaps, "あと6周");
        yes(Fact::FuelAdd, "22リッター");
        yes(Fact::FuelAdd, "20.7リッター足りない");
        no(Fact::FuelAdd, "25リッター");
        yes(Fact::FuelPerLap, "1周2.5リットル");
        yes(Fact::BestLap, "1分30秒0");
        no(Fact::BestLap, "1分31秒0");
        yes(Fact::LastLapAhead, "Ito は1分30秒8");
        yes(Fact::NameAhead, "前はItoだ");
        yes(Fact::NameAhead, "前は16番");
        no(Fact::NameAhead, "前はHart");
        yes(Fact::CarsBehind, "後ろに8台");
        yes(Fact::Yellow, "グリーンだ");
        no(Fact::Yellow, "イエロー");
        yes(Fact::TrendBehind, "毎周1.2離してる");
        yes(Fact::AvgLap, "平均1分30秒5");
        yes(Fact::LastLap, "1分30秒3");
    }

    #[test]
    fn gap_to_the_leader() {
        let s = at(620.0);
        let Some(Value::Number(v, _)) = Fact::GapLeader.value(&s) else {
            panic!()
        };
        assert!(Fact::GapLeader.stated(&format!("トップまで{v:.1}秒"), std::slice::from_ref(&s)));
    }

    /// The truth is the span of values while the radio worked.
    #[test]
    fn moving_truth_accepts_only_fresh_values() {
        let span: Vec<Sample> = [300.0, 302.0, 304.0].into_iter().map(at).collect();
        let gaps: Vec<f64> = span
            .iter()
            .map(|s| s.brief.ahead.as_ref().unwrap().gap_s.unwrap())
            .collect();
        let hi = gaps.iter().copied().fold(f64::MIN, f64::max);
        assert!(Fact::GapAhead.stated(&format!("{hi:.1}秒"), &span));
        assert!(!Fact::GapAhead.stated(&format!("{:.1}秒", hi + 0.3), &span));
        assert!(!Fact::GapAhead.stated("1秒", &[]));
    }

    #[test]
    fn yellow_is_seen() {
        assert!(Fact::Yellow.stated("イエロー出てる", &[at(310.0)]));
        assert!(!Fact::Yellow.stated("グリーン", &[at(310.0)]));
    }
}
