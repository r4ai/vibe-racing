//! What the engineer says about race events by itself, without a model: an
//! event is a fact, and the words for it are always the same. In one evening
//! of racing the agent was asked about 183 events and said nothing about 126.
//!
//! What is said is what to do: a driver at speed cannot work it out from
//! numbers.

use serde_json::Value;

use crate::expr::Values;
use crate::watch::{Group, RaceEvent, WATCH};

/// What becomes of an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Told {
    /// Said as it is.
    Say(String),
    /// A topic of `vr say`.
    Topic(&'static str),
    /// Not worth a word.
    Nothing,
    /// For the agent to think about.
    Ask,
}

/// An incident of fewer points is not news to who had it.
const INCIDENT: i64 = 4;
/// Slower than this the car stands, is towed or lines up.
const DRIVING_KMH: f64 = 30.0;

fn is(v: &Values, key: &str) -> bool {
    v.get(key).is_some_and(|x| *x > 0.0)
}

fn text(e: &RaceEvent, key: &str) -> String {
    match e.rest.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

fn int(e: &RaceEvent, key: &str) -> Option<i64> {
    e.rest.get(key)?.as_i64()
}

fn number(v: &Value, key: &str) -> Option<f64> {
    v.get(key)?.as_f64()
}

/// A car this close is fought with.
const CLOSE_S: f64 = 1.5;

fn close(v: &Values, key: &str) -> bool {
    v.get(key).is_some_and(|s| *s < CLOSE_S)
}

/// Percent of the lap: the line was crossed a moment ago.
const JUST_OVER: f64 = 3.0;

fn finished(v: &Values) -> Told {
    Told::Say(match v.get("position") {
        Some(p) => format!("チェッカー。{p:.0}位だ、お疲れさま。"),
        None => "チェッカー。お疲れさま。".into(),
    })
}

/// What to make of one event, with the race as it is now.
pub fn tell(e: &RaceEvent, v: &Values) -> Told {
    if e.kind == WATCH {
        return Told::Ask;
    }
    let driving = v.get("speed").is_none_or(|s| *s >= DRIVING_KMH);
    let say = |s: &str| Told::Say(s.to_string());
    match e.kind.as_str() {
        "flag_raised" => match e.flag.as_deref().unwrap_or_default() {
            // What is to be acted on at once is said twice, as on a real radio:
            // once may be lost in the engine noise.
            "yellow" => say("イエロー、イエロー。"),
            "yellow_waving" => say("イエロー、イエロー。近くだ、抑えて。"),
            "caution" => say("コーション、コーション。追い抜き禁止。"),
            "red" => say("レッドフラッグ、レッドフラッグ。減速。"),
            "blue" => say("ブルーフラッグ、後ろに譲ろう。"),
            "white" if close(v, "gap_behind") => say("ファイナルラップ。後ろを抑えよう。"),
            "white" if close(v, "gap_ahead") => say("ファイナルラップ。前を狙おう。"),
            "white" => say("ファイナルラップ。"),
            // Out for the leader; told once the driver is over the line too.
            "checkered" if v.get("lap_pct").is_none_or(|p| *p < JUST_OVER) => finished(v),
            "checkered" => Told::Nothing,
            "black" => {
                say("ブラックフラッグ、ブラックフラッグ。ペナルティだ、ピットで消化しよう。")
            }
            "repair" => say("ミートボールフラッグ。修理だ、ボックス、ボックス。"),
            "disqualify" => say("失格だ。"),
            "debris" => say("デブリに注意。"),
            "slow_car" => say("前に遅い車、注意。"),
            "black_white" => say("白黒旗、走り方の警告だ。"),
            "green" if is(v, "racing") => say("グリーン。"),
            _ => Told::Nothing,
        },
        "flag_cleared" => say("イエロー解除。"),
        "position_changed" => {
            let (Some(from), Some(to)) = (int(e, "from"), int(e, "to")) else {
                return Told::Nothing;
            };
            // Places lost in the pits are no news.
            if is(v, "on_pit") || !driving {
                return Told::Nothing;
            }
            Told::Say(if to < from {
                format!("{to}番手に上がった。")
            } else {
                format!("{to}番手に落ちた。")
            })
        }
        "lap_completed" if is(v, "chequered") => finished(v),
        "lap_completed" | "personal_best" => Told::Topic("lap"),
        "pit_entry" => {
            let me = e.rest.get("is_player").and_then(Value::as_bool);
            if me != Some(false) || !is(v, "racing") {
                return Told::Nothing;
            }
            Told::Say(format!("{}号車がピットイン。", text(e, "number")))
        }
        // What has become of the car, not how many points it was.
        "incident" if int(e, "added").is_some_and(|n| n >= INCIDENT) => Told::Topic("damage"),
        "damage" => match text(e, "warning").as_str() {
            "damage" => Told::Topic("damage"),
            // Low while the engine starts or the car is towed.
            _ if !driving || !is(v, "on_track") => Told::Nothing,
            "water_temp" => say("水温が高い、前から離れて冷やそう。"),
            "oil_temp" => say("油温が高い、少し抑えよう。"),
            "oil_pressure" => say("油圧が低い、止めた方がいい。"),
            "fuel_pressure" => say("燃圧が低い、ガス欠に注意。"),
            _ => Told::Ask,
        },
        "fuel_low" => Told::Topic("fuel"),
        "weather_changed" => Told::Topic("weather"),
        "adjustment" => {
            let name = match text(e, "control").as_str() {
                "brake_bias" => "ブレーキバイアス".to_string(),
                "traction_control" => "トラコン".into(),
                "abs" => "ABS".into(),
                other => other.replace('_', " "),
            };
            match e.rest.get("to").and_then(Value::as_f64) {
                Some(to) => Told::Say(format!("{name} {to}。")),
                None => Told::Nothing,
            }
        }
        "strategy" => match &e.advice {
            Some(a) => advice(&a.call, &Value::Object(a.rest.clone())),
            // What the engineer looked out for itself.
            None => Told::Ask,
        },
        _ => Told::Nothing,
    }
}

/// The strategy is worked out by `vr`: only the words are missing.
fn advice(call: &str, a: &Value) -> Told {
    let car = || {
        a.pointer("/car/number")
            .and_then(Value::as_str)
            .map(|n| format!("{n}号車"))
            .unwrap_or_else(|| "車".into())
    };
    let count = |key: &str| number(a, key).unwrap_or(0.0);
    Told::Say(match call {
        "fuel_stop" => format!("燃料のため、{:.0}周目までにピット。", count("by_lap")),
        // Told ahead; the box call itself comes before pit road (`BoxCall`).
        "box_this_lap" => "この周ボックスだ。入口の手前でもう一度呼ぶ。".into(),
        "stay_out" => match number(a, "next") {
            Some(n) => {
                format!("ステイアウト、ステイアウト。この周は入らない、ピットは{n:.0}周目。")
            }
            None => "ステイアウト、ステイアウト。この周は入らない。".into(),
        },
        "window_open" => format!("{:.0}周目までならいつ入ってもいい。", count("until_lap")),
        "undercut_threat" => format!("後ろの{}に先に入られると抜かれる、先に入ろう。", car()),
        "answer" => format!(
            "後ろの{}がピット。{:.0}周以内に入ろう。",
            car(),
            count("within_laps")
        ),
        "undercut_chance" => format!("今入れば前の{}を抜ける、ピットに入ろう。", car()),
        "overcut_chance" => format!(
            "前の{}がピット。あと{:.0}周引っ張れば前に出る。",
            car(),
            count("laps")
        ),
        "cheap_stop" => "コーション中だ、今ピットに入ると得。".into(),
        "plan" => {
            let stops: Vec<String> = a
                .get("stops")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(Value::to_string)
                .collect();
            if stops.is_empty() {
                "ピット無しで完走する。".into()
            } else {
                format!("ピットは{}周目だ。", stops.join("周目と"))
            }
        }
        _ => return Told::Ask,
    })
}

/// Where the box call is made: this much of the lap before pit road begins,
/// and no later than this.
const BOX_FROM_PCT: f64 = 12.0;
const BOX_TO_PCT: f64 = 2.0;
/// Where pit road begins is not known yet: half way round, so that it is
/// never too late; the words say when to come in, not that it is now.
const BOX_LAP_PCT: f64 = 50.0;

/// The call to come in, made where the driver can act on it: on the way to
/// pit road on the lap of the stop, once.
#[derive(Debug, Default)]
pub struct BoxCall {
    told_lap: Option<f64>,
}

impl BoxCall {
    pub fn check(&mut self, v: &Values) -> Option<&'static str> {
        let stop_now = v.get("stop_in").is_some_and(|s| s.abs() < 0.5);
        if !(stop_now && is(v, "racing") && is(v, "on_track")) || is(v, "on_pit") {
            return None;
        }
        let lap = *v.get("lap")?;
        let (near, text) = match v.get("to_pit") {
            Some(d) => (
                (BOX_TO_PCT..=BOX_FROM_PCT).contains(d),
                "ボックス、ボックス。",
            ),
            None => (
                v.get("lap_pct").is_some_and(|p| *p >= BOX_LAP_PCT),
                "ボックス、ボックス。この周の終わりに入る。",
            ),
        };
        if !near || self.told_lap == Some(lap) {
            return None;
        }
        self.told_lap = Some(lap);
        Some(text)
    }
}

/// Closer than this, the car is fought with at every corner.
const BATTLE_S: f64 = 1.0;

/// What the agent is to know of the moment a call comes in: wheel to wheel,
/// the driver has room for a few words only.
pub fn pressure(v: &Values) -> Option<String> {
    if !(is(v, "racing") && is(v, "on_track")) || is(v, "on_pit") {
        return None;
    }
    let near = |k: &str| v.get(k).copied().filter(|s| (0.0..BATTLE_S).contains(s));
    let mut fight = Vec::new();
    if is(v, "alongside") {
        fight.push("横に車".to_string());
    }
    if let Some(s) = near("gap_ahead") {
        fight.push(format!("前と{s:.1}秒"));
    }
    if let Some(s) = near("gap_behind") {
        fight.push(format!("後ろと{s:.1}秒"));
    }
    (!fight.is_empty()).then(|| {
        format!(
            "いま接近戦（{}）。1 文、30 字までで、すぐ動ける一言だけ。命令は 2 回繰り返していい。",
            fight.join("・")
        )
    })
}

/// Several events of one moment: places gained one by one are one change, and
/// the same is not said twice.
pub fn merge(events: &[RaceEvent]) -> Vec<RaceEvent> {
    let mut out: Vec<RaceEvent> = Vec::new();
    for e in events {
        let same = out.iter_mut().find(|o| o.kind == e.kind);
        match (e.kind.as_str(), same) {
            ("position_changed", Some(o)) => {
                let from = o.rest.get("from").cloned();
                *o = e.clone();
                if let Some(from) = from {
                    o.rest.insert("from".into(), from);
                }
            }
            ("incident", Some(o)) => {
                let added = int(o, "added").unwrap_or(0) + int(e, "added").unwrap_or(0);
                *o = e.clone();
                o.rest.insert("added".into(), added.into());
            }
            _ => out.push(e.clone()),
        }
    }
    // Back where it was.
    out.retain(|e| e.kind != "position_changed" || e.rest.get("from") != e.rest.get("to"));
    out
}

/// Whether a condition only looks at the cars beside: the spotter's work.
pub fn spotters<'a>(keys: impl IntoIterator<Item = &'a str>) -> bool {
    let mut keys = keys.into_iter().peekable();
    keys.peek().is_some() && keys.all(|k| matches!(k, "alongside" | "left" | "right"))
}

/// What an event is about, for the log of the window.
pub fn about(e: &RaceEvent) -> String {
    match e.group() {
        Some(g) if e.kind != WATCH => g.label().to_string(),
        _ => Group::Strategy.label().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(json: &str) -> RaceEvent {
        RaceEvent::parse(json).unwrap_or_else(|| panic!("{json}"))
    }

    fn values(v: &[(&str, f64)]) -> Values {
        v.iter().map(|(k, x)| (k.to_string(), *x)).collect()
    }

    fn racing() -> Values {
        values(&[
            ("racing", 1.0),
            ("on_track", 1.0),
            ("speed", 180.0),
            ("gap_ahead", 0.62),
            ("gap_behind", 1.24),
            ("position", 4.0),
        ])
    }

    fn said(json: &str, v: &Values) -> String {
        match tell(&event(json), v) {
            Told::Say(s) => s,
            other => panic!("{json}: {other:?}"),
        }
    }

    fn flag(name: &str) -> String {
        format!(r#"{{"type":"flag_raised","text":"t","flag":"{name}"}}"#)
    }

    #[test]
    fn flags_are_said_as_they_are() {
        let v = racing();
        for (name, want) in [
            ("yellow", "イエロー、イエロー。"),
            ("yellow_waving", "イエロー、イエロー。近くだ、抑えて。"),
            ("caution", "コーション、コーション。追い抜き禁止。"),
            ("red", "レッドフラッグ、レッドフラッグ。減速。"),
            ("blue", "ブルーフラッグ、後ろに譲ろう。"),
            ("white", "ファイナルラップ。後ろを抑えよう。"),
            ("checkered", "チェッカー。4位だ、お疲れさま。"),
            (
                "black",
                "ブラックフラッグ、ブラックフラッグ。ペナルティだ、ピットで消化しよう。",
            ),
            (
                "repair",
                "ミートボールフラッグ。修理だ、ボックス、ボックス。",
            ),
            ("disqualify", "失格だ。"),
            ("debris", "デブリに注意。"),
            ("slow_car", "前に遅い車、注意。"),
            ("black_white", "白黒旗、走り方の警告だ。"),
            ("green", "グリーン。"),
        ] {
            assert_eq!(said(&flag(name), &v), want);
        }
        for (ahead, behind, want) in [
            (1.49, 1.5, "ファイナルラップ。前を狙おう。"),
            (1.5, 1.5, "ファイナルラップ。"),
        ] {
            let near = values(&[("gap_ahead", ahead), ("gap_behind", behind)]);
            assert_eq!(said(&flag("white"), &near), want);
        }
        // The green flag of a session that has not begun.
        assert_eq!(tell(&event(&flag("green")), &Values::new()), Told::Nothing);
        assert_eq!(tell(&event(&flag("one_lap_to_green")), &v), Told::Nothing);
        assert_eq!(
            said(&flag("checkered"), &Values::new()),
            "チェッカー。お疲れさま。"
        );
        // Out before the driver is over the line: told when the lap ends.
        let mut short = v.clone();
        short.insert("lap_pct".into(), 3.0);
        assert_eq!(tell(&event(&flag("checkered")), &short), Told::Nothing);
        short.insert("lap_pct".into(), 2.9);
        assert_eq!(
            said(&flag("checkered"), &short),
            "チェッカー。4位だ、お疲れさま。"
        );
        let lap = r#"{"type":"lap_completed","text":"t","lap":9}"#;
        assert_eq!(tell(&event(lap), &v), Told::Topic("lap"));
        short.insert("chequered".into(), 1.0);
        assert_eq!(said(lap, &short), "チェッカー。4位だ、お疲れさま。");
        let cleared = r#"{"type":"flag_cleared","text":"t","flag":"yellow"}"#;
        assert_eq!(said(cleared, &v), "イエロー解除。");
    }

    #[test]
    fn places_are_news_while_driving() {
        let moved = |from: i32, to: i32| {
            format!(r#"{{"type":"position_changed","text":"t","from":{from},"to":{to}}}"#)
        };
        assert_eq!(said(&moved(5, 4), &racing()), "4番手に上がった。");
        assert_eq!(said(&moved(5, 6), &racing()), "6番手に落ちた。");
        for (key, x) in [("on_pit", 1.0), ("speed", 29.9)] {
            let mut v = racing();
            v.insert(key.into(), x);
            assert_eq!(tell(&event(&moved(5, 6)), &v), Told::Nothing, "{key}");
        }
        let mut v = racing();
        v.insert("speed".into(), 30.0);
        assert_eq!(said(&moved(5, 6), &v), "6番手に落ちた。");
        let odd = r#"{"type":"position_changed","text":"t"}"#;
        assert_eq!(tell(&event(odd), &v), Told::Nothing);
    }

    #[test]
    fn small_matters_are_not_told() {
        let v = racing();
        let incident =
            |n: i32| format!(r#"{{"type":"incident","text":"t","added":{n},"total":9}}"#);
        assert_eq!(tell(&event(&incident(3)), &v), Told::Nothing);
        assert_eq!(tell(&event(&incident(4)), &v), Told::Topic("damage"));
        for kind in ["phase_changed", "session_changed", "pit_exit", "later"] {
            let e = event(&format!(r#"{{"type":"{kind}","text":"t"}}"#));
            assert_eq!(tell(&e, &v), Told::Nothing, "{kind}");
        }
        let pit = |me: bool| {
            format!(r#"{{"type":"pit_entry","text":"t","number":"16","is_player":{me}}}"#)
        };
        assert_eq!(said(&pit(false), &v), "16号車がピットイン。");
        assert_eq!(tell(&event(&pit(true)), &v), Told::Nothing);
        // Everybody comes in after the race.
        assert_eq!(tell(&event(&pit(false)), &Values::new()), Told::Nothing);
    }

    #[test]
    fn warnings_count_while_driving() {
        let warning = |w: &str| format!(r#"{{"type":"damage","text":"t","warning":"{w}"}}"#);
        let v = racing();
        for v in [&v, &Values::new()] {
            assert_eq!(tell(&event(&warning("damage")), v), Told::Topic("damage"));
        }
        for (w, want) in [
            ("water_temp", "水温が高い、前から離れて冷やそう。"),
            ("oil_temp", "油温が高い、少し抑えよう。"),
            ("oil_pressure", "油圧が低い、止めた方がいい。"),
            ("fuel_pressure", "燃圧が低い、ガス欠に注意。"),
        ] {
            assert_eq!(said(&warning(w), &v), want);
        }
        assert_eq!(tell(&event(&warning("other")), &v), Told::Ask);
        // Towed back to the pits.
        let mut towed = racing();
        towed.insert("speed".into(), 0.0);
        assert_eq!(
            tell(&event(&warning("oil_pressure")), &towed),
            Told::Nothing
        );
        let mut out = racing();
        out.insert("on_track".into(), 0.0);
        assert_eq!(tell(&event(&warning("oil_pressure")), &out), Told::Nothing);
    }

    #[test]
    fn fuel_weather_laps_and_settings() {
        let v = racing();
        let fuel = r#"{"type":"fuel_low","text":"t","laps":2.46}"#;
        assert_eq!(tell(&event(fuel), &v), Told::Topic("fuel"));
        for (kind, topic) in [
            ("lap_completed", "lap"),
            ("personal_best", "lap"),
            ("weather_changed", "weather"),
        ] {
            let e = event(&format!(r#"{{"type":"{kind}","text":"t","lap":3}}"#));
            assert_eq!(tell(&e, &v), Told::Topic(topic));
            assert!(crate::quick::TOPICS.contains(&topic));
        }
        let set = |c: &str| {
            format!(r#"{{"type":"adjustment","text":"t","control":"{c}","from":54.0,"to":54.5}}"#)
        };
        assert_eq!(said(&set("brake_bias"), &v), "ブレーキバイアス 54.5。");
        assert_eq!(said(&set("traction_control"), &v), "トラコン 54.5。");
        assert_eq!(said(&set("abs"), &v), "ABS 54.5。");
        assert_eq!(said(&set("weight_jacker"), &v), "weight jacker 54.5。");
        let odd = r#"{"type":"adjustment","text":"t","control":"abs"}"#;
        assert_eq!(tell(&event(odd), &v), Told::Nothing);
    }

    #[test]
    fn strategy_is_put_into_words() {
        let v = racing();
        let a = |json: &str| format!(r#"{{"type":"strategy","text":"t","advice":{json}}}"#);
        let car = r#""car":{"number":"16"},"gap_s":1.24,"gain_s":0.8"#;
        for (json, want) in [
            (
                r#"{"call":"fuel_stop","by_lap":12}"#.to_string(),
                "燃料のため、12周目までにピット。",
            ),
            (
                r#"{"call":"box_this_lap","lap":8}"#.into(),
                "この周ボックスだ。入口の手前でもう一度呼ぶ。",
            ),
            (
                r#"{"call":"stay_out","next":15}"#.into(),
                "ステイアウト、ステイアウト。この周は入らない、ピットは15周目。",
            ),
            (
                r#"{"call":"stay_out","next":null}"#.into(),
                "ステイアウト、ステイアウト。この周は入らない。",
            ),
            (
                r#"{"call":"window_open","until_lap":9}"#.into(),
                "9周目までならいつ入ってもいい。",
            ),
            (
                format!(r#"{{"call":"undercut_threat",{car}}}"#),
                "後ろの16号車に先に入られると抜かれる、先に入ろう。",
            ),
            (
                r#"{"call":"answer","car":{"number":"16"},"within_laps":2}"#.into(),
                "後ろの16号車がピット。2周以内に入ろう。",
            ),
            (
                format!(r#"{{"call":"undercut_chance",{car}}}"#),
                "今入れば前の16号車を抜ける、ピットに入ろう。",
            ),
            (
                r#"{"call":"overcut_chance","car":{"number":"16"},"laps":3}"#.into(),
                "前の16号車がピット。あと3周引っ張れば前に出る。",
            ),
            (
                r#"{"call":"cheap_stop"}"#.into(),
                "コーション中だ、今ピットに入ると得。",
            ),
            (
                r#"{"call":"plan","stops":[]}"#.into(),
                "ピット無しで完走する。",
            ),
            (
                r#"{"call":"plan","stops":[8,16]}"#.into(),
                "ピットは8周目と16周目だ。",
            ),
        ] {
            assert_eq!(said(&a(&json), &v), want);
        }
        assert_eq!(tell(&event(&a(r#"{"call":"new"}"#)), &v), Told::Ask);
        assert_eq!(tell(&RaceEvent::planned("t".into()), &v), Told::Ask);
        assert_eq!(tell(&RaceEvent::watch("t".into()), &v), Told::Ask);
    }

    #[test]
    fn the_box_call_comes_once_on_the_way_to_pit_road() {
        let at = |stop_in: f64, to_pit: Option<f64>, lap_pct: f64, lap: f64| {
            let mut v = racing();
            v.insert("stop_in".into(), stop_in);
            v.insert("lap".into(), lap);
            v.insert("lap_pct".into(), lap_pct);
            if let Some(d) = to_pit {
                v.insert("to_pit".into(), d);
            }
            v
        };
        let mut b = BoxCall::default();
        // Not the lap of the stop, or too far from pit road.
        assert_eq!(b.check(&at(1.0, Some(5.0), 90.0, 7.0)), None);
        assert_eq!(b.check(&at(0.0, Some(12.1), 80.0, 8.0)), None);
        // Too late to act on.
        assert_eq!(b.check(&at(0.0, Some(1.9), 90.0, 8.0)), None);
        assert_eq!(
            b.check(&at(0.0, Some(12.0), 80.0, 8.0)),
            Some("ボックス、ボックス。")
        );
        // Once a lap.
        assert_eq!(b.check(&at(0.0, Some(2.0), 90.0, 8.0)), None);
        assert_eq!(
            b.check(&at(0.0, Some(2.0), 90.0, 9.0)),
            Some("ボックス、ボックス。")
        );
        // Where pit road begins is not known: half way round, saying when.
        let mut b = BoxCall::default();
        assert_eq!(b.check(&at(0.0, None, 49.9, 8.0)), None);
        assert_eq!(
            b.check(&at(0.0, None, 50.0, 8.0)),
            Some("ボックス、ボックス。この周の終わりに入る。")
        );
        // In the pits already, or not racing.
        let mut b = BoxCall::default();
        let mut pitting = at(0.0, Some(5.0), 90.0, 8.0);
        pitting.insert("on_pit".into(), 1.0);
        assert_eq!(b.check(&pitting), None);
        let mut slow = at(0.0, Some(5.0), 90.0, 8.0);
        slow.insert("racing".into(), 0.0);
        assert_eq!(b.check(&slow), None);
        let mut stop = at(0.0, Some(5.0), 90.0, 8.0);
        stop.remove("stop_in");
        assert_eq!(b.check(&stop), None);
    }

    #[test]
    fn wheel_to_wheel_the_answers_are_short() {
        let mut v = racing();
        // 0.62 ahead, 1.24 behind.
        assert_eq!(
            pressure(&v).as_deref(),
            Some(
                "いま接近戦（前と0.6秒）。1 文、30 字までで、すぐ動ける一言だけ。命令は 2 回繰り返していい。"
            )
        );
        v.insert("gap_ahead".into(), 1.0);
        v.insert("gap_behind".into(), 0.99);
        v.insert("alongside".into(), 1.0);
        assert_eq!(
            pressure(&v).as_deref(),
            Some(
                "いま接近戦（横に車・後ろと1.0秒）。1 文、30 字までで、すぐ動ける一言だけ。命令は 2 回繰り返していい。"
            )
        );
        v.insert("gap_behind".into(), 1.0);
        v.insert("alongside".into(), 0.0);
        assert_eq!(pressure(&v), None);
        // Not in the pits, nor outside the race.
        let mut pits = racing();
        pits.insert("on_pit".into(), 1.0);
        assert_eq!(pressure(&pits), None);
        let mut slow = racing();
        slow.insert("racing".into(), 0.0);
        assert_eq!(pressure(&slow), None);
    }

    #[test]
    fn what_belongs_together_is_told_once() {
        let moved = |from: i32, to: i32| {
            event(&format!(
                r#"{{"type":"position_changed","text":"t","from":{from},"to":{to}}}"#
            ))
        };
        let m = merge(&[moved(5, 7), moved(7, 9), moved(9, 10)]);
        assert_eq!(m.len(), 1);
        assert_eq!(
            said(
                r#"{"type":"position_changed","text":"t","from":5,"to":10}"#,
                &racing()
            ),
            match tell(&m[0], &racing()) {
                Told::Say(s) => s,
                o => panic!("{o:?}"),
            }
        );
        // There and back again.
        assert!(merge(&[moved(5, 6), moved(6, 5)]).is_empty());
        let hit = |n: i32, t: i32| {
            event(&format!(
                r#"{{"type":"incident","text":"t","added":{n},"total":{t}}}"#
            ))
        };
        let m = merge(&[hit(2, 2), hit(2, 4)]);
        assert_eq!(m.len(), 1);
        assert_eq!(tell(&m[0], &racing()), Told::Topic("damage"));
        let y = event(&flag("yellow"));
        assert_eq!(merge(&[y.clone(), hit(1, 1)]).len(), 2);
        assert_eq!(about(&y), "旗");
        assert_eq!(about(&RaceEvent::watch("t".into())), "作戦");
    }

    #[test]
    fn conditions_of_the_spotter() {
        assert!(spotters(["alongside"]));
        assert!(spotters(["left", "right"]));
        assert!(!spotters(["alongside", "lap"]));
        assert!(!spotters([]));
    }
}
