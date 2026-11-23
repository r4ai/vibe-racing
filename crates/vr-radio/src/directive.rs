//! Radio settings the agent changes for the driver, who cannot reach a mouse
//! at 250 km/h: `[[radio volume=60]]` in the reply. The directives are taken
//! out of the stream before anything is shown or spoken.

use crate::expr::Expr;
use crate::rules::{EVERY_S, Rules, Scope, Watch};
use crate::watch::Group;

pub const MAX_VOLUME: u8 = 150;
/// Speech rate in percent of the engine's own.
pub const SPEED: std::ops::RangeInclusive<u16> = 50..=200;

/// A value given as such, or relative to the current one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Amount {
    Set(f64),
    Add(f64),
}

impl Amount {
    fn parse(s: &str) -> Option<Self> {
        let n = s.trim_end_matches('%').parse::<f64>().ok()?;
        if !n.is_finite() {
            return None;
        }
        Some(if s.starts_with(['+', '-']) {
            Self::Add(n)
        } else {
            Self::Set(n)
        })
    }

    pub fn of(self, current: f64) -> f64 {
        match self {
            Self::Set(n) => n,
            Self::Add(n) => current + n,
        }
    }
}

/// What the agent asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Directive {
    Volume(Amount),
    Speed(Amount),
    Muted(bool),
    /// A character, a style, or both.
    Speaker(String),
    Proactive(bool),
    Event(Group, bool),
    /// Seconds between calls that can wait.
    Interval(Amount),
    /// When speech keeps quiet; `None`: never.
    Hold(Scope, Option<Expr>),
    HoldMax(Amount),
    /// A standing order.
    Note(String),
    /// Drops standing orders and watches.
    Forget(String),
    Watch(Watch),
    /// Laps between looks at the strategy; 0: never.
    Planning(u16),
    /// The usual questions are answered without the agent.
    Quick(bool),
    /// The engineer's own notes on the strategy.
    Plan(String),
    /// Something the desk is to look into.
    Study(String),
}

impl Directive {
    /// Saying it twice does it twice.
    pub fn relative(&self) -> bool {
        matches!(
            self,
            Self::Volume(Amount::Add(_))
                | Self::Speed(Amount::Add(_))
                | Self::Interval(Amount::Add(_))
                | Self::HoldMax(Amount::Add(_))
        )
    }
}

/// What became of it: the value now in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setting {
    /// Percent.
    Volume(u8),
    /// Percent.
    Speed(u16),
    Muted(bool),
    /// Left to the window, which knows the voices.
    Speaker(String),
    Proactive(bool),
    Event(Group, bool),
    Interval(u16),
    Hold(Scope, Option<String>),
    HoldMax(u16),
    Note(String),
    /// How many went.
    Forgot(usize),
    Watch(String),
    Planning(u16),
    Quick(bool),
}

fn on_off(b: bool) -> &'static str {
    if b { "オン" } else { "オフ" }
}

impl std::fmt::Display for Setting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Volume(v) => write!(f, "音量 {v}%"),
            Self::Speed(v) => write!(f, "話速 {:.2}", f64::from(*v) / 100.0),
            Self::Muted(m) => write!(f, "読み上げ {}", on_off(!m)),
            Self::Speaker(s) => write!(f, "話者 {s}"),
            Self::Proactive(b) => write!(f, "自動連絡 {}", on_off(*b)),
            Self::Event(g, b) => write!(f, "自動連絡（{}）{}", g.label(), on_off(*b)),
            Self::Interval(s) => write!(f, "自動連絡の間隔 {s}秒以上"),
            Self::Hold(scope, when) => {
                let what = match scope {
                    Scope::All => "話しかけない条件",
                    Scope::Calls => "自動連絡を控える条件",
                    Scope::Quiet => "自動連絡をしない条件",
                };
                write!(f, "{what} {}", when.as_deref().unwrap_or("なし"))
            }
            Self::HoldMax(s) => write!(f, "話すのを待つ上限 {s}秒"),
            Self::Note(n) => write!(f, "覚えた指示: {n}"),
            Self::Forgot(n) => write!(f, "指示と見張りを {n} 件取り消し"),
            Self::Watch(w) => write!(f, "見張り: {w}"),
            Self::Planning(0) => write!(f, "裏での作戦立案 オフ"),
            Self::Planning(n) => write!(f, "裏での作戦立案 {n}周ごと"),
            Self::Quick(b) => write!(f, "定型の質問への即答 {}", on_off(*b)),
        }
    }
}

fn unreadable(s: &str) -> String {
    let short: String = s.chars().take(40).collect();
    format!("ラジオの設定を読めません: {short}")
}

/// Closes a quoted value.
fn closing(c: char) -> Option<char> {
    match c {
        '"' => Some('"'),
        '\'' => Some('\''),
        '“' => Some('”'),
        '「' => Some('」'),
        _ => None,
    }
}

/// `key=`, but not the `==` of a condition.
fn begins(word: &[char]) -> Option<usize> {
    let key = word
        .iter()
        .position(|c| !(c.is_ascii_lowercase() || matches!(c, '_' | '.')))?;
    (key > 0 && word[key] == '=' && word.get(key + 1) != Some(&'=')).then_some(key)
}

/// `key=value` pairs. A value is quoted, or ends where the next key begins:
/// names may have spaces.
fn pairs(args: &str) -> Vec<(String, String)> {
    let c: Vec<char> = args.chars().collect();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i].is_whitespace() {
            i += 1;
            continue;
        }
        let Some(key) = begins(&c[i..]) else {
            // Belongs to the value before it.
            let from = i;
            while i < c.len() && !c[i].is_whitespace() {
                i += 1;
            }
            let word: String = c[from..i].iter().collect();
            match out.last_mut() {
                Some((_, v)) => {
                    v.push(' ');
                    v.push_str(&word);
                }
                None => out.push((word, String::new())),
            }
            continue;
        };
        let name: String = c[i..i + key].iter().collect();
        i += key + 1;
        let close = c.get(i).copied().and_then(closing);
        let end = close.and_then(|q| c[i + 1..].iter().position(|x| *x == q));
        let value: String = match end {
            Some(n) => {
                let v = c[i + 1..i + 1 + n].iter().collect();
                i += n + 2;
                v
            }
            None => {
                let from = i;
                while i < c.len() && !c[i].is_whitespace() {
                    i += 1;
                }
                c[from..i].iter().collect()
            }
        };
        out.push((name, value));
    }
    out
}

/// `radio volume=60 speed=+0.1` → one directive or error each.
fn parse(body: &str) -> Vec<Result<Directive, String>> {
    let body = body.trim();
    let Some(args) = body
        .strip_prefix("radio")
        .filter(|a| a.is_empty() || a.starts_with(char::is_whitespace))
    else {
        return vec![Err(unreadable(body))];
    };
    let pairs = pairs(args);
    if pairs.is_empty() {
        return vec![Err(unreadable(body))];
    }
    let mut out = Vec::new();
    // `when` takes what follows it.
    let mut watch: Option<Watch> = None;
    let close = |w: &mut Option<Watch>, out: &mut Vec<_>| {
        if let Some(w) = w.take() {
            out.push(check(w));
        }
    };
    for (key, value) in &pairs {
        let bad = || Err(unreadable(&format!("{key}={value}")));
        match (key.as_str(), &mut watch) {
            ("when", _) => {
                close(&mut watch, &mut out);
                watch = Some(Watch {
                    when: value.trim().into(),
                    every: EVERY_S,
                    ..Watch::default()
                });
            }
            ("do", Some(w)) => w.act = Some(value.trim().into()).filter(|v: &String| !v.is_empty()),
            ("say", Some(w)) => {
                w.say = Some(value.trim().into()).filter(|v: &String| !v.is_empty());
            }
            ("tell", Some(w)) => match value.trim() {
                t if crate::quick::TOPICS.contains(&t) => w.tell = Some(t.into()),
                _ => out.push(bad()),
            },
            ("every", Some(w)) => match value.trim_end_matches(['s', '秒']).parse() {
                Ok(n) => w.every = n,
                Err(_) => out.push(bad()),
            },
            ("once", Some(w)) => match value.as_str() {
                "on" => w.once = true,
                "off" => w.once = false,
                _ => out.push(bad()),
            },
            _ => {
                close(&mut watch, &mut out);
                out.push(pair(key, value).unwrap_or_else(|| {
                    Err(unreadable(&if value.is_empty() && !body.contains('=') {
                        key.clone()
                    } else {
                        format!("{key}={value}")
                    }))
                }));
            }
        }
    }
    close(&mut watch, &mut out);
    out
}

fn expr(s: &str) -> Result<Expr, String> {
    s.parse()
        .map_err(|e| format!("条件を読めません: {s}（{e}）"))
}

fn check(w: Watch) -> Result<Directive, String> {
    expr(&w.when)?;
    Ok(Directive::Watch(w))
}

fn pair(key: &str, value: &str) -> Option<Result<Directive, String>> {
    let switch = match value {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    };
    let text = || Some(value.trim().to_string()).filter(|t| !t.is_empty());
    let hold = |scope| {
        let off = matches!(value, "off" | "none" | "なし");
        match if off { Ok(None) } else { expr(value).map(Some) } {
            _ if value.trim().is_empty() => None,
            Ok(e) => Some(Ok(Directive::Hold(scope, e))),
            Err(e) => Some(Err(e)),
        }
    };
    Some(Ok(match key {
        "volume" => Directive::Volume(Amount::parse(value)?),
        "speed" => Directive::Speed(Amount::parse(value)?),
        "mute" => Directive::Muted(switch?),
        "speaker" => Directive::Speaker(text()?),
        "proactive" => Directive::Proactive(switch?),
        "interval" => Directive::Interval(Amount::parse(value.trim_end_matches('秒'))?),
        "hold" => return hold(Scope::All),
        "hold_calls" => return hold(Scope::Calls),
        "quiet" => return hold(Scope::Quiet),
        "hold_max" => Directive::HoldMax(Amount::parse(value.trim_end_matches('秒'))?),
        "note" => Directive::Note(text()?),
        "forget" => Directive::Forget(text()?),
        "planning" if switch == Some(false) => Directive::Planning(0),
        "planning" if switch == Some(true) => Directive::Planning(Rules::default().planning),
        "planning" => Directive::Planning(value.trim_end_matches('周').parse().ok()?),
        "quick" => Directive::Quick(switch?),
        "plan" => Directive::Plan(text()?),
        "study" => Directive::Study(text()?),
        k => Directive::Event(Group::parse(k.strip_prefix("event.")?)?, switch?),
    }))
}

/// Longer than any directive: what never closes was not one.
const MAX_LEN: usize = 1500;

type Found = Vec<Result<Directive, String>>;

/// Takes the directives out of streamed text.
#[derive(Debug, Default)]
pub struct Stripper {
    /// Held back: may be (the start of) a directive.
    held: String,
}

impl Stripper {
    /// Adds a chunk; returns what is left of the text and the directives.
    pub fn push(&mut self, chunk: &str) -> (String, Found) {
        self.held.push_str(chunk);
        let mut text = String::new();
        let mut found = Vec::new();
        loop {
            let Some(start) = self.held.find("[[") else {
                // A single bracket at the end may be half of one.
                let keep = usize::from(self.held.ends_with('['));
                let rest = self.held.split_off(self.held.len() - keep);
                text.push_str(&std::mem::replace(&mut self.held, rest));
                break;
            };
            text.push_str(&self.held[..start]);
            self.held.drain(..start);
            let end = self.held.find("]]");
            let line = self.held.find('\n');
            match (end, line) {
                (Some(e), l) if l.is_none_or(|l| e < l) => {
                    found.extend(parse(&self.held[2..e]));
                    self.held.drain(..e + 2);
                }
                (_, Some(l)) => {
                    found.push(Err(unreadable(&self.held[2..l])));
                    self.held.drain(..l);
                }
                _ if self.held.len() > MAX_LEN => {
                    found.push(Err(unreadable(&self.held[2..])));
                    self.held.clear();
                }
                _ => break,
            }
        }
        (text, found)
    }

    /// The reply is complete: what was held back is text, or cut off.
    pub fn finish(&mut self) -> (String, Found) {
        let held = std::mem::take(&mut self.held);
        match held.strip_prefix("[[") {
            Some(body) => (String::new(), vec![Err(unreadable(body))]),
            None => (held, Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(s: &str) -> (String, Found) {
        let mut st = Stripper::default();
        let (mut text, mut found) = st.push(s);
        let (rest, more) = st.finish();
        text.push_str(&rest);
        found.extend(more);
        (text, found)
    }

    fn bad(s: &str) -> Result<Directive, String> {
        Err(format!("ラジオの設定を読めません: {s}"))
    }

    #[test]
    fn every_setting_is_read() {
        let d = |s: &str| one(&format!("[[radio {s}]]")).1;
        let volume = |a| [Ok(Directive::Volume(a))];
        assert_eq!(d("volume=60"), volume(Amount::Set(60.0)));
        assert_eq!(d("volume=60%"), volume(Amount::Set(60.0)));
        assert_eq!(d("volume=+20"), volume(Amount::Add(20.0)));
        assert_eq!(d("volume=-20"), volume(Amount::Add(-20.0)));
        assert_eq!(d("speed=1.3"), [Ok(Directive::Speed(Amount::Set(1.3)))]);
        assert_eq!(d("speed=-0.1"), [Ok(Directive::Speed(Amount::Add(-0.1)))]);
        assert_eq!(d("mute=on"), [Ok(Directive::Muted(true))]);
        assert_eq!(d("mute=off"), [Ok(Directive::Muted(false))]);
        assert_eq!(
            d("speaker=四国めたん ノーマル"),
            [Ok(Directive::Speaker("四国めたん ノーマル".into()))]
        );
        assert_eq!(d("proactive=off"), [Ok(Directive::Proactive(false))]);
        assert_eq!(
            d("event.lap_completed=on"),
            [Ok(Directive::Event(Group::Lap, true))]
        );
        assert_eq!(
            d("event.weather=off"),
            [Ok(Directive::Event(Group::Weather, false))]
        );
        assert_eq!(
            d("volume=60 mute=off"),
            [
                Ok(Directive::Volume(Amount::Set(60.0))),
                Ok(Directive::Muted(false))
            ]
        );
    }

    #[test]
    fn rules_are_read() {
        let d = |s: &str| one(&format!("[[radio {s}]]")).1;
        let e = |s: &str| -> Expr { s.parse().unwrap() };
        assert_eq!(
            d("interval=60 hold_max=5秒"),
            [
                Ok(Directive::Interval(Amount::Set(60.0))),
                Ok(Directive::HoldMax(Amount::Set(5.0)))
            ]
        );
        assert_eq!(
            d("interval=+20"),
            [Ok(Directive::Interval(Amount::Add(20.0)))]
        );
        assert_eq!(
            d(r#"hold="brake > 5 or abs(steer) >= 45" hold_calls=off"#),
            [
                Ok(Directive::Hold(
                    Scope::All,
                    Some(e("brake > 5 or abs(steer) >= 45"))
                )),
                Ok(Directive::Hold(Scope::Calls, None))
            ]
        );
        // Quotes forgotten.
        assert_eq!(
            d("hold_calls=brake > 5 and lap == 3"),
            [Ok(Directive::Hold(
                Scope::Calls,
                Some(e("brake > 5 and lap == 3"))
            ))]
        );
        assert_eq!(
            d("hold=「throttle < 90」"),
            [Ok(Directive::Hold(Scope::All, Some(e("throttle < 90"))))]
        );
        assert_eq!(
            d(r#"note="順位は毎回 言う" forget=n1"#),
            [
                Ok(Directive::Note("順位は毎回 言う".into())),
                Ok(Directive::Forget("n1".into()))
            ]
        );
        assert_eq!(
            d("planning=off planning=on planning=5周"),
            [
                Ok(Directive::Planning(0)),
                Ok(Directive::Planning(5)),
                Ok(Directive::Planning(5))
            ]
        );
        assert_eq!(
            d(r#"plan="15周目に入る。 [#9] を警戒""#),
            [Ok(Directive::Plan("15周目に入る。 [#9] を警戒".into()))]
        );
    }

    #[test]
    fn watches_are_read() {
        let d = |s: &str| one(&format!("[[radio {s}]]")).1;
        let watch = |w: Watch| Ok(Directive::Watch(w));
        assert_eq!(
            d(r#"when="gap_behind < 1" do="差と名前を言う" every=60 volume=60"#),
            [
                watch(Watch {
                    when: "gap_behind < 1".into(),
                    act: Some("差と名前を言う".into()),
                    every: 60,
                    ..Watch::default()
                }),
                Ok(Directive::Volume(Amount::Set(60.0)))
            ]
        );
        assert_eq!(
            d(r#"when="fuel_laps < 2" say="燃料{fuel_laps}周" once=on when="lap % 5 == 0""#),
            [
                watch(Watch {
                    when: "fuel_laps < 2".into(),
                    say: Some("燃料{fuel_laps}周".into()),
                    every: EVERY_S,
                    once: true,
                    ..Watch::default()
                }),
                watch(Watch {
                    when: "lap % 5 == 0".into(),
                    every: EVERY_S,
                    ..Watch::default()
                })
            ]
        );
        assert_eq!(
            d(r#"when="gap <" do="x""#),
            [Err(
                "条件を読めません: gap <（式が途中で終わっている）".into()
            )]
        );
        assert_eq!(d("do=x"), [bad("do=x")]);
        let found = d(r#"when="lap > 1" every=often once=yes"#);
        assert_eq!(found[..2], [bad("every=often"), bad("once=yes")]);
        assert!(matches!(found[2], Ok(Directive::Watch(_))));
        assert_eq!(
            d("hold=brake >"),
            [Err(
                "条件を読めません: brake >（式が途中で終わっている）".into()
            )]
        );
        assert_eq!(d("hold="), [bad("hold=")]);
        assert_eq!(d("note="), [bad("note=")]);
        assert_eq!(d("planning=often"), [bad("planning=often")]);
    }

    #[test]
    fn what_cannot_be_read_is_reported_and_taken_out() {
        for (s, e) in [
            ("[[radio volume=loud]]", "volume=loud"),
            ("[[radio volume=inf]]", "volume=inf"),
            ("[[radio mute=yes]]", "mute=yes"),
            ("[[radio speaker=]]", "speaker="),
            ("[[radio color=red]]", "color=red"),
            ("[[radio event.nothing=on]]", "event.nothing=on"),
            ("[[radio]]", "radio"),
            ("[[radios volume=1]]", "radios volume=1"),
            ("[[note]]", "note"),
        ] {
            assert_eq!(one(s), (String::new(), vec![bad(e)]), "{s}");
        }
        assert_eq!(
            one("了解[[radio vol"),
            ("了解".into(), vec![bad("radio vol")])
        );
        assert_eq!(
            one("了解[[radio vol\n次。"),
            ("了解\n次。".into(), vec![bad("radio vol")])
        );
        let long = |n| format!("[[{}", "x".repeat(n));
        let mut st = Stripper::default();
        assert_eq!(st.push(&long(MAX_LEN - 2)), (String::new(), vec![]));
        let mut st = Stripper::default();
        assert_eq!(
            st.push(&long(MAX_LEN - 1)),
            (String::new(), vec![bad(&"x".repeat(40))])
        );
        assert_eq!(st.push("後。").0, "後。");
    }

    /// However the reply is cut into chunks, the result is the same.
    #[test]
    fn chunks_may_cut_a_directive_anywhere() {
        let reply = "音量[1]を下げた。[[radio volume=-20]]ほかは？[";
        let want = "音量[1]を下げた。ほかは？[";
        let chars: Vec<char> = reply.chars().collect();
        for size in 1..chars.len() {
            let mut st = Stripper::default();
            let mut text = String::new();
            let mut found = Vec::new();
            for c in chars.chunks(size) {
                let (t, f) = st.push(&c.iter().collect::<String>());
                assert!(!t.contains("[["), "{size}");
                text.push_str(&t);
                found.extend(f);
            }
            let (t, f) = st.finish();
            text.push_str(&t);
            found.extend(f);
            assert_eq!(text, want, "{size}");
            assert_eq!(found, [Ok(Directive::Volume(Amount::Add(-20.0)))], "{size}");
        }
    }

    #[test]
    fn plain_text_is_not_delayed() {
        let mut st = Stripper::default();
        assert_eq!(st.push("前は 0.6。").0, "前は 0.6。");
        assert_eq!(st.push("a [b] c").0, "a [b] c");
        assert_eq!(st.push("x[").0, "x");
        assert_eq!(st.push("1]").0, "[1]");
        assert_eq!(st.finish(), (String::new(), vec![]));
    }

    #[test]
    fn amounts_are_absolute_or_relative() {
        assert_eq!(Amount::Set(60.0).of(100.0), 60.0);
        assert_eq!(Amount::Add(-20.0).of(100.0), 80.0);
    }

    #[test]
    fn settings_are_named() {
        let names: Vec<String> = [
            Setting::Volume(60),
            Setting::Speed(130),
            Setting::Muted(true),
            Setting::Speaker("ずんだもん".into()),
            Setting::Proactive(false),
            Setting::Event(Group::Fuel, true),
            Setting::Interval(60),
            Setting::Hold(Scope::All, Some("brake > 5".into())),
            Setting::Hold(Scope::Calls, None),
            Setting::HoldMax(5),
            Setting::Note("短く".into()),
            Setting::Forgot(2),
            Setting::Watch("lap > 1 → 知らせる".into()),
            Setting::Planning(0),
            Setting::Planning(3),
        ]
        .iter()
        .map(Setting::to_string)
        .collect();
        assert_eq!(
            names,
            [
                "音量 60%",
                "話速 1.30",
                "読み上げ オフ",
                "話者 ずんだもん",
                "自動連絡 オフ",
                "自動連絡（燃料）オン",
                "自動連絡の間隔 60秒以上",
                "話しかけない条件 brake > 5",
                "自動連絡を控える条件 なし",
                "話すのを待つ上限 5秒",
                "覚えた指示: 短く",
                "指示と見張りを 2 件取り消し",
                "見張り: lap > 1 → 知らせる",
                "裏での作戦立案 オフ",
                "裏での作戦立案 3周ごと",
            ]
        );
    }
}
