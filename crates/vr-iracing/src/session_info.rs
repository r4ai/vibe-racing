//! iRacing "session info" YAML.
//!
//! iRacing emits YAML that is not always valid: free-text values such as
//! driver and team names are written unquoted, so a name containing `: `,
//! ` #`, or starting with `*`, `&`, `@` … breaks a strict parser. We quote
//! those values before parsing, then read only the fields we need from a
//! generic [`serde_yaml::Value`] so schema drift between sim versions does not
//! break us.

use serde_yaml::Value;
use vr_core::SessionKind;

/// Keys whose values are free text typed by users or content authors.
const FREE_TEXT_KEYS: &[&str] = &[
    "UserName",
    "TeamName",
    "AbbrevName",
    "Initials",
    "CarScreenName",
    "CarScreenNameShort",
    "CarClassShortName",
    "CarNumber",
    "ClubName",
    "DivisionName",
    "TrackName",
    "TrackDisplayName",
    "TrackDisplayShortName",
    "TrackConfigName",
    "TrackCity",
    "TrackCountry",
    "SeriesName",
    "SessionName",
    "EventType",
    "CarDesignStr",
    "HelmetDesignStr",
    "SuitDesignStr",
    "CarPath",
    "LicString",
    "FlairName",
];

/// Decode iRacing's latin-1, NUL-terminated session string.
pub fn decode(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes[..end].iter().map(|&b| b as char).collect()
}

/// Single-quote free-text values so the YAML parses.
pub fn sanitize_yaml(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 256);
    for line in src.lines() {
        out.push_str(&sanitize_line(line));
        out.push('\n');
    }
    out
}

fn sanitize_line(line: &str) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let (dash, body) = match rest.strip_prefix("- ") {
        Some(b) => ("- ", b),
        None => ("", rest),
    };
    let Some(colon) = body.find(": ") else {
        return line.replace('\t', " ");
    };
    let key = &body[..colon];
    if !FREE_TEXT_KEYS.contains(&key) {
        return line.replace('\t', " ");
    }
    let value = body[colon + 2..].trim();
    if value.is_empty() {
        return line.to_string();
    }
    // Already properly double-quoted (e.g. CarNumber: "12").
    if value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
        && !value[1..value.len() - 1].contains('"')
    {
        return line.to_string();
    }
    let escaped = value.replace('\'', "''");
    format!("{indent}{dash}{key}: '{escaped}'")
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionDef {
    pub num: i32,
    pub kind: SessionKind,
    pub laps: Option<i32>,
    pub time_s: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DriverInfo {
    pub car_idx: usize,
    pub name: String,
    pub team: Option<String>,
    pub number: String,
    pub car_name: Option<String>,
    pub class_name: Option<String>,
    pub class_id: Option<i32>,
    pub irating: Option<i32>,
    pub est_lap_s: Option<f64>,
    pub is_pace_car: bool,
    pub is_spectator: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionInfo {
    pub track_name: Option<String>,
    pub track_config: Option<String>,
    pub track_length_m: Option<f64>,
    pub sessions: Vec<SessionDef>,
    pub player_car_idx: Option<usize>,
    pub fuel_kg_per_l: Option<f64>,
    pub fuel_capacity_l: Option<f64>,
    pub drivers: Vec<DriverInfo>,
}

fn get<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, k| cur.get(*k))
}

fn as_string(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
    .filter(|s| !s.is_empty())
}

fn as_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        // "5.51 km", "3600.0000 sec", "0.75 kg/l"
        Value::String(s) => s.split_whitespace().next()?.parse().ok(),
        _ => None,
    }
}

fn as_i64(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn session_kind(s: &str) -> SessionKind {
    let s = s.to_ascii_lowercase();
    if s.contains("race") {
        SessionKind::Race
    } else if s.contains("qual") {
        SessionKind::Qualify
    } else if s.contains("warm") {
        SessionKind::Warmup
    } else if s.contains("practice") || s.contains("testing") {
        SessionKind::Practice
    } else {
        SessionKind::Other
    }
}

impl SessionInfo {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let doc: Value = serde_yaml::from_str(&sanitize_yaml(raw)).map_err(|e| e.to_string())?;
        let mut si = SessionInfo {
            track_name: as_string(get(&doc, &["WeekendInfo", "TrackDisplayName"]))
                .or_else(|| as_string(get(&doc, &["WeekendInfo", "TrackName"]))),
            track_config: as_string(get(&doc, &["WeekendInfo", "TrackConfigName"])),
            track_length_m: {
                let raw = as_string(get(&doc, &["WeekendInfo", "TrackLength"]));
                let n = as_f64(get(&doc, &["WeekendInfo", "TrackLength"]));
                n.map(|n| {
                    if raw.as_deref().is_some_and(|r| r.contains("mi")) {
                        n * 1609.344
                    } else {
                        n * 1000.0
                    }
                })
            },
            player_car_idx: as_i64(get(&doc, &["DriverInfo", "DriverCarIdx"])).map(|i| i as usize),
            fuel_kg_per_l: as_f64(get(&doc, &["DriverInfo", "DriverCarFuelKgPerLtr"])),
            fuel_capacity_l: {
                let max = as_f64(get(&doc, &["DriverInfo", "DriverCarFuelMaxLtr"]));
                let pct = as_f64(get(&doc, &["DriverInfo", "DriverCarMaxFuelPct"])).unwrap_or(1.0);
                max.map(|m| m * if pct > 0.0 { pct } else { 1.0 })
            },
            ..Default::default()
        };

        if let Some(Value::Sequence(sessions)) = get(&doc, &["SessionInfo", "Sessions"]) {
            for s in sessions {
                let Some(num) = as_i64(s.get("SessionNum")) else {
                    continue;
                };
                let laps = as_string(s.get("SessionLaps")).and_then(|l| l.parse::<i32>().ok());
                let time_s = as_string(s.get("SessionTime"))
                    .filter(|t| !t.contains("unlimited"))
                    .and_then(|t| t.split_whitespace().next()?.parse().ok());
                si.sessions.push(SessionDef {
                    num: num as i32,
                    kind: session_kind(&as_string(s.get("SessionType")).unwrap_or_default()),
                    laps,
                    time_s,
                });
            }
        }

        if let Some(Value::Sequence(drivers)) = get(&doc, &["DriverInfo", "Drivers"]) {
            for d in drivers {
                let Some(car_idx) = as_i64(d.get("CarIdx")) else {
                    continue;
                };
                si.drivers.push(DriverInfo {
                    car_idx: car_idx as usize,
                    name: as_string(d.get("UserName")).unwrap_or_else(|| format!("car {car_idx}")),
                    team: as_string(d.get("TeamName")),
                    number: as_string(d.get("CarNumber"))
                        .map(|n| n.trim_matches('"').to_string())
                        .unwrap_or_else(|| car_idx.to_string()),
                    car_name: as_string(d.get("CarScreenNameShort"))
                        .or_else(|| as_string(d.get("CarScreenName"))),
                    class_name: as_string(d.get("CarClassShortName")),
                    class_id: as_i64(d.get("CarClassID")).map(|v| v as i32),
                    irating: as_i64(d.get("IRating")).map(|v| v as i32),
                    est_lap_s: as_f64(d.get("CarClassEstLapTime")),
                    is_pace_car: as_i64(d.get("CarIsPaceCar")).unwrap_or(0) != 0,
                    is_spectator: as_i64(d.get("IsSpectator")).unwrap_or(0) != 0,
                });
            }
        }
        Ok(si)
    }

    pub fn session(&self, num: i32) -> Option<&SessionDef> {
        self.sessions.iter().find(|s| s.num == num)
    }
}

#[cfg(test)]
pub(crate) const SAMPLE: &str = r#"---
WeekendInfo:
 TrackName: spa 2024 up
 TrackDisplayName: Circuit de Spa-Francorchamps
 TrackConfigName: Grand Prix Pits
 TrackLength: 6.93 km
SessionInfo:
 Sessions:
 - SessionNum: 0
   SessionLaps: unlimited
   SessionTime: 1800.0000 sec
   SessionType: Practice
 - SessionNum: 2
   SessionLaps: 20
   SessionTime: unlimited
   SessionType: Race
DriverInfo:
 DriverCarIdx: 1
 DriverCarFuelKgPerLtr: 0.750
 DriverCarFuelMaxLtr: 120.000
 DriverCarMaxFuelPct: 0.500
 Drivers:
 - CarIdx: 0
   UserName: Pace Car
   CarNumber: "0"
   CarIsPaceCar: 1
   IsSpectator: 0
 - CarIdx: 1
   UserName: Taro: The "Fast" #1
   TeamName: *Team & Co
   CarNumber: "23"
   CarScreenNameShort: BMW M4 GT3
   CarClassShortName: GT3
   CarClassID: 4083
   CarClassEstLapTime: 137.5000
   IRating: 2345
   CarIsPaceCar: 0
   IsSpectator: 0
 - CarIdx: 2
   UserName: O'Neil
   CarNumber: 7
   CarClassShortName: GT3
   CarClassID: 4083
   IRating: 1800
   CarIsPaceCar: 0
   IsSpectator: 0
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_hostile_names() {
        let s = sanitize_line("   UserName: Taro: The \"Fast\" #1");
        assert_eq!(s, "   UserName: 'Taro: The \"Fast\" #1'");
        assert_eq!(
            sanitize_line(" - UserName: O'Neil"),
            " - UserName: 'O''Neil'"
        );
        assert_eq!(
            sanitize_line("   CarNumber: \"23\""),
            "   CarNumber: \"23\""
        );
        assert_eq!(sanitize_line("   CarIdx: 1"), "   CarIdx: 1");
    }

    #[test]
    fn parses_sample() {
        let si = SessionInfo::parse(SAMPLE).unwrap();
        assert_eq!(
            si.track_name.as_deref(),
            Some("Circuit de Spa-Francorchamps")
        );
        assert!((si.track_length_m.unwrap() - 6930.0).abs() < 1e-6);
        assert_eq!(si.player_car_idx, Some(1));
        assert_eq!(si.fuel_capacity_l, Some(60.0));
        assert_eq!(si.sessions.len(), 2);
        let race = si.session(2).unwrap();
        assert_eq!(race.kind, SessionKind::Race);
        assert_eq!(race.laps, Some(20));
        assert_eq!(race.time_s, None);
        assert_eq!(si.session(0).unwrap().time_s, Some(1800.0));
        assert_eq!(si.drivers.len(), 3);
        let me = &si.drivers[1];
        assert_eq!(me.name, "Taro: The \"Fast\" #1");
        assert_eq!(me.team.as_deref(), Some("*Team & Co"));
        assert_eq!(me.number, "23");
        assert_eq!(me.est_lap_s, Some(137.5));
        assert_eq!(si.drivers[2].name, "O'Neil");
        assert_eq!(si.drivers[2].number, "7");
        assert!(si.drivers[0].is_pace_car);
    }

    #[test]
    fn sanitize_edge_cases() {
        // Embedded quotes mean it is not a properly quoted scalar.
        assert_eq!(
            sanitize_line(r#"   UserName: "A" and "B""#),
            r#"   UserName: '"A" and "B"'"#
        );
        assert_eq!(sanitize_line(r#"   UserName: "x"#), r#"   UserName: '"x'"#);
        assert_eq!(sanitize_line("   UserName: "), "   UserName: ");
        assert_eq!(
            sanitize_line("\tCarIdx: 1"),
            " CarIdx: 1",
            "tabs are not valid YAML indentation"
        );
        assert_eq!(sanitize_line("no colon here"), "no colon here");
        let doc = sanitize_yaml("A:\n - UserName: x: y\n");
        assert_eq!(doc, "A:\n - UserName: 'x: y'\n");
    }

    #[test]
    fn scalar_helpers() {
        let v: Value =
            serde_yaml::from_str("b: true\ni: '12'\nf: 1.5\nn: 7\ns: ' x '\ne: ''\nl: [1]")
                .unwrap();
        assert_eq!(as_string(v.get("b")).as_deref(), Some("true"));
        assert_eq!(as_string(v.get("n")).as_deref(), Some("7"));
        assert_eq!(as_string(v.get("s")).as_deref(), Some("x"));
        assert_eq!(as_string(v.get("e")), None);
        assert_eq!(as_string(v.get("l")), None);
        assert_eq!(as_i64(v.get("i")), Some(12));
        assert_eq!(as_i64(v.get("f")), Some(1));
        assert_eq!(as_i64(v.get("l")), None);
        assert_eq!(as_f64(v.get("i")), Some(12.0));
        assert_eq!(as_f64(v.get("l")), None);
    }

    #[test]
    fn session_kinds() {
        for (s, k) in [
            ("Race", SessionKind::Race),
            ("Heat Race", SessionKind::Race),
            ("Open Qualify", SessionKind::Qualify),
            ("Lone Qualify", SessionKind::Qualify),
            ("Warmup", SessionKind::Warmup),
            ("Practice", SessionKind::Practice),
            ("Offline Testing", SessionKind::Practice),
            ("Something", SessionKind::Other),
        ] {
            assert_eq!(session_kind(s), k, "{s}");
        }
    }

    #[test]
    fn track_and_fuel_details() {
        let si = SessionInfo::parse(SAMPLE).unwrap();
        assert_eq!(si.track_config.as_deref(), Some("Grand Prix Pits"));
        assert_eq!(si.fuel_kg_per_l, Some(0.75));
        let miles = SAMPLE.replace("TrackLength: 6.93 km", "TrackLength: 2.50 mi");
        let si = SessionInfo::parse(&miles).unwrap();
        assert!((si.track_length_m.unwrap() - 4023.36).abs() < 1e-6);
        // A zero max-fuel percentage means "not restricted".
        let full = SAMPLE.replace("DriverCarMaxFuelPct: 0.500", "DriverCarMaxFuelPct: 0.000");
        assert_eq!(
            SessionInfo::parse(&full).unwrap().fuel_capacity_l,
            Some(120.0)
        );
        let none = SAMPLE.replace(" DriverCarMaxFuelPct: 0.500\n", "");
        assert_eq!(
            SessionInfo::parse(&none).unwrap().fuel_capacity_l,
            Some(120.0)
        );
        let named = SAMPLE.replace(" TrackDisplayName: Circuit de Spa-Francorchamps\n", "");
        assert_eq!(
            SessionInfo::parse(&named).unwrap().track_name.as_deref(),
            Some("spa 2024 up")
        );
        assert!(SessionInfo::parse("a: [").is_err());
    }

    #[test]
    fn decodes_latin1_until_nul() {
        assert_eq!(decode(b"Jos\xe9\0garbage"), "José");
    }
}
