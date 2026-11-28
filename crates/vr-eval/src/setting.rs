//! Settings of the radio a call should change: checked by value, not judged.

use anyhow::{Result, bail, ensure};
use vr_radio::directive::Setting;
use vr_radio::rules::Scope;

fn on_off(b: bool) -> String {
    if b { "on" } else { "off" }.into()
}

/// `(key, value)`, as the scenario files write them.
pub fn pair(s: &Setting) -> (String, String) {
    match s {
        Setting::Volume(v) => ("volume".into(), v.to_string()),
        Setting::Speed(v) => ("speed".into(), format!("{:.2}", f64::from(*v) / 100.0)),
        Setting::Muted(m) => ("mute".into(), on_off(*m)),
        Setting::Speaker(s) => ("speaker".into(), s.clone()),
        Setting::Proactive(p) => ("proactive".into(), on_off(*p)),
        Setting::Event(g, on) => (format!("event.{}", g.key()), on_off(*on)),
        Setting::Interval(s) => ("interval".into(), s.to_string()),
        Setting::Hold(scope, when) => {
            let key = match scope {
                Scope::All => "hold",
                Scope::Calls => "hold_calls",
                Scope::Quiet => "quiet",
            };
            (key.into(), when.clone().unwrap_or("off".into()))
        }
        Setting::HoldMax(s) => ("hold_max".into(), s.to_string()),
        Setting::Note(n) => ("note".into(), n.clone()),
        Setting::Forgot(n) => ("forget".into(), n.to_string()),
        Setting::Watch(w) => ("when".into(), w.clone()),
        Setting::Planning(0) => ("planning".into(), "off".into()),
        Setting::Planning(n) => ("planning".into(), n.to_string()),
    }
}

/// `volume<100`, `speed>1.2`, `event.lap=on`, `hold~brake|throttle` (contains
/// one of them), `hold_calls?` (may change or not).
#[derive(Debug, Clone, PartialEq)]
struct Expected {
    key: String,
    op: char,
    value: String,
}

fn parse(s: &str) -> Result<Expected> {
    if let Some(key) = s.strip_suffix('?') {
        ensure!(!key.is_empty(), "`{s}` is incomplete");
        return Ok(Expected {
            key: key.into(),
            op: '?',
            value: String::new(),
        });
    }
    let Some(i) = s.find(['=', '<', '>', '~']) else {
        bail!("`{s}` is not key=value, key<value, key>value or key~text");
    };
    let e = Expected {
        key: s[..i].into(),
        op: s.as_bytes()[i] as char,
        value: s[i + 1..].into(),
    };
    ensure!(
        !e.key.is_empty() && !e.value.is_empty(),
        "`{s}` is incomplete"
    );
    ensure!(
        matches!(e.op, '=' | '~') || e.value.parse::<f64>().is_ok(),
        "`{s}` compares no number"
    );
    Ok(e)
}

pub fn check_syntax(expected: &[String]) -> Result<()> {
    expected.iter().try_for_each(|e| parse(e).map(|_| ()))
}

/// Whether the values in the end are the expected ones, and nothing else was
/// changed.
pub fn met(expected: &[String], changed: &[(String, String)]) -> bool {
    let Ok(expected) = expected
        .iter()
        .map(|e| parse(e))
        .collect::<Result<Vec<_>>>()
    else {
        return false;
    };
    let nothing_else = changed
        .iter()
        .all(|(k, _)| expected.iter().any(|e| e.key == *k));
    nothing_else
        && expected.iter().all(|e| {
            let mut values = changed.iter().filter(|(k, _)| *k == e.key).map(|(_, v)| v);
            let contains = |v: &String| e.value.split('|').any(|w| v.contains(w));
            match e.op {
                '?' => return true,
                // Watches and orders add up.
                '~' => return values.any(contains),
                _ => {}
            }
            let Some(got) = values.next_back() else {
                return false;
            };
            match (got.parse::<f64>(), e.value.parse::<f64>()) {
                (Ok(g), Ok(v)) => match e.op {
                    '<' => g < v,
                    '>' => g > v,
                    _ => (g - v).abs() < 1e-9,
                },
                _ => e.op == '=' && *got == e.value,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vr_radio::watch::Group;

    fn met_by(expected: &[&str], changed: &[(&str, &str)]) -> bool {
        let e: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        let c: Vec<_> = changed
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        met(&e, &c)
    }

    #[test]
    fn values_are_compared() {
        assert!(met_by(&["volume<100"], &[("volume", "99")]));
        assert!(!met_by(&["volume<100"], &[("volume", "100")]));
        assert!(met_by(&["speed>1.2"], &[("speed", "1.30")]));
        assert!(!met_by(&["speed>1.2"], &[("speed", "1.20")]));
        assert!(met_by(&["volume=50"], &[("volume", "50")]));
        assert!(met_by(&["event.lap=on"], &[("event.lap", "on")]));
        assert!(!met_by(&["event.lap=on"], &[("event.lap", "off")]));
        assert!(met_by(&["hold~brake"], &[("hold", "brake > 5")]));
        assert!(!met_by(&["hold~brake"], &[("hold", "throttle < 9")]));
        assert!(met_by(&["interval~3"], &[("interval", "30")]));
        assert!(met_by(
            &["when~yellow"],
            &[("when", "yellow → x"), ("when", "caution → y")]
        ));
        assert!(met_by(&["note~短|ひとこと"], &[("note", "ひとことで")]));
        assert!(met_by(&["hold=off", "quiet?"], &[("hold", "off")]));
        assert!(met_by(
            &["hold=off", "quiet?"],
            &[("quiet", "off"), ("hold", "off")]
        ));
        // The last value counts.
        assert!(met_by(
            &["volume=50"],
            &[("volume", "80"), ("volume", "50")]
        ));
    }

    #[test]
    fn nothing_else_may_change() {
        assert!(met_by(&[], &[]));
        assert!(!met_by(&[], &[("volume", "100")]));
        assert!(!met_by(&["volume<100"], &[]));
        assert!(!met_by(
            &["volume<100"],
            &[("volume", "80"), ("mute", "on")]
        ));
    }

    #[test]
    fn settings_have_the_names_of_the_directives() {
        let p = |s| pair(&s);
        assert_eq!(p(Setting::Volume(80)), ("volume".into(), "80".into()));
        assert_eq!(p(Setting::Speed(130)), ("speed".into(), "1.30".into()));
        assert_eq!(p(Setting::Muted(true)), ("mute".into(), "on".into()));
        assert_eq!(
            p(Setting::Event(Group::Position, false)),
            ("event.position".into(), "off".into())
        );
        assert_eq!(
            p(Setting::Proactive(false)),
            ("proactive".into(), "off".into())
        );
    }

    #[test]
    fn rules_have_the_names_of_the_directives() {
        let p = |s| {
            let (k, v) = pair(&s);
            format!("{k}={v}")
        };
        let brake = || Some("brake > 5".to_string());
        for (s, want) in [
            (Setting::Interval(60), "interval=60"),
            (Setting::Hold(Scope::All, brake()), "hold=brake > 5"),
            (Setting::Hold(Scope::Calls, None), "hold_calls=off"),
            (Setting::Hold(Scope::Quiet, brake()), "quiet=brake > 5"),
            (Setting::HoldMax(5), "hold_max=5"),
            (Setting::Note("短く".into()), "note=短く"),
            (Setting::Forgot(2), "forget=2"),
            (Setting::Watch("lap > 1 → x".into()), "when=lap > 1 → x"),
            (Setting::Planning(0), "planning=off"),
            (Setting::Planning(5), "planning=5"),
        ] {
            assert_eq!(p(s), want);
        }
    }

    #[test]
    fn mistakes_are_refused() {
        let check = |s: &str| check_syntax(&[s.to_string()]).is_ok();
        assert!(check("volume<100") && check("mute=on"));
        for bad in ["volume", "=1", "volume=", "mute<on"] {
            assert!(!check(bad), "{bad}");
        }
    }
}
