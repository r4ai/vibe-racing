//! Radio settings the agent changes for the driver, who cannot reach a mouse
//! at 250 km/h: `[[radio volume=60]]` in the reply. The directives are taken
//! out of the stream before anything is shown or spoken.

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
        }
    }
}

fn unreadable(s: &str) -> String {
    let short: String = s.chars().take(40).collect();
    format!("ラジオの設定を読めません: {short}")
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
    // A word without `=` belongs to the value before it (names with spaces).
    let mut pairs: Vec<String> = Vec::new();
    for word in args.split_whitespace() {
        match pairs.last_mut() {
            Some(last) if !word.contains('=') => {
                last.push(' ');
                last.push_str(word);
            }
            _ => pairs.push(word.to_string()),
        }
    }
    if pairs.is_empty() {
        return vec![Err(unreadable(body))];
    }
    pairs
        .iter()
        .map(|p| pair(p).ok_or_else(|| unreadable(p)))
        .collect()
}

fn pair(p: &str) -> Option<Directive> {
    let (key, value) = p.split_once('=')?;
    let switch = match value {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    };
    Some(match key {
        "volume" => Directive::Volume(Amount::parse(value)?),
        "speed" => Directive::Speed(Amount::parse(value)?),
        "mute" => Directive::Muted(switch?),
        "speaker" if !value.is_empty() => Directive::Speaker(value.to_string()),
        "proactive" => Directive::Proactive(switch?),
        k => Directive::Event(Group::parse(k.strip_prefix("event.")?)?, switch?),
    })
}

/// Longer than any directive: what never closes was not one.
const MAX_LEN: usize = 200;

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
            ]
        );
    }
}
