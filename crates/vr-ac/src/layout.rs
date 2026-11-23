//! The byte layouts of what Assetto Corsa writes: the three shared-memory
//! pages, the CrewChief page of all cars, and the UDP packets.
//!
//! Each layout is the list of fields in the order of the C struct. Offsets
//! follow `#pragma pack(4)`: numbers sit on 4 bytes, UTF-16 text on 2, bytes
//! anywhere.

use std::sync::LazyLock;

use serde_json::{Value, json};
use vr_core::Channel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    I32,
    F32,
    /// One byte, 0 or 1.
    Bool,
    /// UTF-16 text of this many code units, NUL-terminated if shorter.
    Wide(usize),
    /// UTF-8 text of this many bytes, NUL-terminated if shorter.
    Narrow(usize),
}

impl Kind {
    fn size(self) -> usize {
        match self {
            Self::I32 | Self::F32 => 4,
            Self::Bool => 1,
            Self::Wide(n) => 2 * n,
            Self::Narrow(n) => n,
        }
    }

    fn align(self) -> usize {
        match self {
            Self::I32 | Self::F32 => 4,
            Self::Wide(_) => 2,
            Self::Bool | Self::Narrow(_) => 1,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    /// Array length; 1 for a single value.
    pub count: usize,
    pub offset: usize,
}

#[derive(Debug)]
pub struct Layout {
    pub fields: Vec<Field>,
    /// Of the whole struct, padded to 4 bytes.
    pub size: usize,
}

impl Layout {
    fn new(spec: &[(&'static str, Kind, usize)]) -> Self {
        let mut at: usize = 0;
        let fields = spec
            .iter()
            .map(|&(name, kind, count)| {
                at = at.next_multiple_of(kind.align());
                let f = Field {
                    name,
                    kind,
                    count,
                    offset: at,
                };
                at += kind.size() * count;
                f
            })
            .collect();
        Self {
            fields,
            size: at.next_multiple_of(4),
        }
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
}

use Kind::{Bool, F32, I32, Narrow, Wide};

/// `SPageFilePhysics`: the player's car, every physics step.
pub static PHYSICS: LazyLock<Layout> = LazyLock::new(|| {
    Layout::new(&[
        ("packetId", I32, 1),
        ("gas", F32, 1),
        ("brake", F32, 1),
        ("fuel", F32, 1),
        ("gear", I32, 1),
        ("rpms", I32, 1),
        ("steerAngle", F32, 1),
        ("speedKmh", F32, 1),
        ("velocity", F32, 3),
        ("accG", F32, 3),
        ("wheelSlip", F32, 4),
        ("wheelLoad", F32, 4),
        ("wheelsPressure", F32, 4),
        ("wheelAngularSpeed", F32, 4),
        ("tyreWear", F32, 4),
        ("tyreDirtyLevel", F32, 4),
        ("tyreCoreTemperature", F32, 4),
        ("camberRAD", F32, 4),
        ("suspensionTravel", F32, 4),
        ("drs", F32, 1),
        ("tc", F32, 1),
        ("heading", F32, 1),
        ("pitch", F32, 1),
        ("roll", F32, 1),
        ("cgHeight", F32, 1),
        ("carDamage", F32, 5),
        ("numberOfTyresOut", I32, 1),
        ("pitLimiterOn", I32, 1),
        ("abs", F32, 1),
        ("kersCharge", F32, 1),
        ("kersInput", F32, 1),
        ("autoShifterOn", I32, 1),
        ("rideHeight", F32, 2),
        ("turboBoost", F32, 1),
        ("ballast", F32, 1),
        ("airDensity", F32, 1),
        ("airTemp", F32, 1),
        ("roadTemp", F32, 1),
        ("localAngularVel", F32, 3),
        ("finalFF", F32, 1),
        ("performanceMeter", F32, 1),
        ("engineBrake", I32, 1),
        ("ersRecoveryLevel", I32, 1),
        ("ersPowerLevel", I32, 1),
        ("ersHeatCharging", I32, 1),
        ("ersIsCharging", I32, 1),
        ("kersCurrentKJ", F32, 1),
        ("drsAvailable", I32, 1),
        ("drsEnabled", I32, 1),
        ("brakeTemp", F32, 4),
        ("clutch", F32, 1),
        ("tyreTempI", F32, 4),
        ("tyreTempM", F32, 4),
        ("tyreTempO", F32, 4),
        ("isAIControlled", I32, 1),
        ("tyreContactPoint", F32, 12),
        ("tyreContactNormal", F32, 12),
        ("tyreContactHeading", F32, 12),
        ("brakeBias", F32, 1),
        ("localVelocity", F32, 3),
    ])
});

/// `SPageFileGraphic`: laps, position, flags of the player's car.
pub static GRAPHICS: LazyLock<Layout> = LazyLock::new(|| {
    Layout::new(&[
        ("packetId", I32, 1),
        ("status", I32, 1),
        ("session", I32, 1),
        ("currentTime", Wide(15), 1),
        ("lastTime", Wide(15), 1),
        ("bestTime", Wide(15), 1),
        ("split", Wide(15), 1),
        ("completedLaps", I32, 1),
        ("position", I32, 1),
        ("iCurrentTime", I32, 1),
        ("iLastTime", I32, 1),
        ("iBestTime", I32, 1),
        ("sessionTimeLeft", F32, 1),
        ("distanceTraveled", F32, 1),
        ("isInPit", I32, 1),
        ("currentSectorIndex", I32, 1),
        ("lastSectorTime", I32, 1),
        ("numberOfLaps", I32, 1),
        ("tyreCompound", Wide(33), 1),
        ("replayTimeMultiplier", F32, 1),
        ("normalizedCarPosition", F32, 1),
        ("carCoordinates", F32, 3),
        ("penaltyTime", F32, 1),
        ("flag", I32, 1),
        ("idealLineOn", I32, 1),
        ("isInPitLane", I32, 1),
        ("surfaceGrip", F32, 1),
        ("mandatoryPitDone", I32, 1),
        ("windSpeed", F32, 1),
        ("windDirection", F32, 1),
    ])
});

/// `SPageFileStatic`: the car, the track and the aids, set when the session
/// loads.
pub static STATIC: LazyLock<Layout> = LazyLock::new(|| {
    Layout::new(&[
        ("smVersion", Wide(15), 1),
        ("acVersion", Wide(15), 1),
        ("numberOfSessions", I32, 1),
        ("numCars", I32, 1),
        ("carModel", Wide(33), 1),
        ("track", Wide(33), 1),
        ("playerName", Wide(33), 1),
        ("playerSurname", Wide(33), 1),
        ("playerNick", Wide(33), 1),
        ("sectorCount", I32, 1),
        ("maxTorque", F32, 1),
        ("maxPower", F32, 1),
        ("maxRpm", I32, 1),
        ("maxFuel", F32, 1),
        ("suspensionMaxTravel", F32, 4),
        ("tyreRadius", F32, 4),
        ("maxTurboBoost", F32, 1),
        ("deprecated_1", F32, 1),
        ("deprecated_2", F32, 1),
        ("penaltiesEnabled", I32, 1),
        ("aidFuelRate", F32, 1),
        ("aidTireRate", F32, 1),
        ("aidMechanicalDamage", F32, 1),
        ("aidAllowTyreBlankets", I32, 1),
        ("aidStability", F32, 1),
        ("aidAutoClutch", I32, 1),
        ("aidAutoBlip", I32, 1),
        ("hasDRS", I32, 1),
        ("hasERS", I32, 1),
        ("hasKERS", I32, 1),
        ("kersMaxJ", F32, 1),
        ("engineBrakeSettingsCount", I32, 1),
        ("ersPowerControllerCount", I32, 1),
        ("trackSPlineLength", F32, 1),
        ("trackConfiguration", Wide(33), 1),
        ("ersMaxJ", F32, 1),
        ("isTimedRace", I32, 1),
        ("hasExtraLap", I32, 1),
        ("carSkin", Wide(33), 1),
        ("reversedGridPositions", I32, 1),
        ("PitWindowStart", I32, 1),
        ("PitWindowEnd", I32, 1),
    ])
});

/// The head of `SPageFileCrewChief`; the cars follow it.
pub static CREWCHIEF: LazyLock<Layout> = LazyLock::new(|| {
    Layout::new(&[
        ("numVehicles", I32, 1),
        ("focusVehicle", I32, 1),
        ("serverName", Narrow(512), 1),
    ])
});

/// `acsVehicleInfo`: one car of the CrewChief page.
pub static VEHICLE: LazyLock<Layout> = LazyLock::new(|| {
    Layout::new(&[
        ("carId", I32, 1),
        ("driverName", Narrow(64), 1),
        ("carModel", Narrow(64), 1),
        ("speedMS", F32, 1),
        ("bestLapMS", I32, 1),
        ("lapCount", I32, 1),
        ("currentLapInvalid", I32, 1),
        ("currentLapTimeMS", I32, 1),
        ("lastLapTimeMS", I32, 1),
        ("worldPosition", F32, 3),
        ("isCarInPitline", I32, 1),
        ("isCarInPit", I32, 1),
        ("carLeaderboardPosition", I32, 1),
        ("carRealTimeLeaderboardPosition", I32, 1),
        ("spLineLength", F32, 1),
        ("isConnected", I32, 1),
        ("suspensionDamage", F32, 4),
        ("engineLifeLeft", F32, 1),
        ("tyreInflation", F32, 4),
    ])
});

/// The UDP server's answer to a handshake.
pub static HANDSHAKE_RESPONSE: LazyLock<Layout> = LazyLock::new(|| {
    Layout::new(&[
        ("carName", Wide(50), 1),
        ("driverName", Wide(50), 1),
        ("identifier", I32, 1),
        ("version", I32, 1),
        ("trackName", Wide(50), 1),
        ("trackConfig", Wide(50), 1),
    ])
});

/// `RTCarInfo`: the player's car, every physics step, over UDP.
pub static RT_CAR_INFO: LazyLock<Layout> = LazyLock::new(|| {
    Layout::new(&[
        ("identifier", Narrow(1), 1),
        ("size", I32, 1),
        ("speed_Kmh", F32, 1),
        ("speed_Mph", F32, 1),
        ("speed_Ms", F32, 1),
        ("isAbsEnabled", Bool, 1),
        ("isAbsInAction", Bool, 1),
        ("isTcInAction", Bool, 1),
        ("isTcEnabled", Bool, 1),
        ("isInPit", Bool, 1),
        ("isEngineLimiterOn", Bool, 1),
        ("accG_vertical", F32, 1),
        ("accG_horizontal", F32, 1),
        ("accG_frontal", F32, 1),
        ("lapTime", I32, 1),
        ("lastLap", I32, 1),
        ("bestLap", I32, 1),
        ("lapCount", I32, 1),
        ("gas", F32, 1),
        ("brake", F32, 1),
        ("clutch", F32, 1),
        ("engineRPM", F32, 1),
        ("steer", F32, 1),
        ("gear", I32, 1),
        ("cgHeight", F32, 1),
        ("wheelAngularSpeed", F32, 4),
        ("slipAngle", F32, 4),
        ("slipAngle_ContactPatch", F32, 4),
        ("slipRatio", F32, 4),
        ("tyreSlip", F32, 4),
        ("ndSlip", F32, 4),
        ("load", F32, 4),
        ("Dy", F32, 4),
        ("Mz", F32, 4),
        ("tyreDirtyLevel", F32, 4),
        ("camberRAD", F32, 4),
        ("tyreRadius", F32, 4),
        ("tyreLoadedRadius", F32, 4),
        ("suspensionHeight", F32, 4),
        ("carPositionNormalized", F32, 1),
        ("carSlope", F32, 1),
        ("carCoordinates", F32, 3),
    ])
});

/// `RTLap`: a car finished a lap, over UDP.
pub static RT_LAP: LazyLock<Layout> = LazyLock::new(|| {
    Layout::new(&[
        ("carIdentifierNumber", I32, 1),
        ("lap", I32, 1),
        ("driverName", Wide(50), 1),
        ("carName", Wide(50), 1),
        ("time", I32, 1),
    ])
});

/// One struct's bytes read through its layout. Fields beyond the bytes (an
/// older game version, a short page) read as missing.
#[derive(Clone, Copy)]
pub struct View<'a> {
    pub layout: &'a Layout,
    pub bytes: &'a [u8],
}

impl<'a> View<'a> {
    pub fn new(layout: &'a Layout, bytes: &'a [u8]) -> Self {
        Self { layout, bytes }
    }

    fn raw(&self, name: &str, i: usize) -> Option<(Kind, &'a [u8])> {
        let f = self.layout.field(name)?;
        if i >= f.count {
            return None;
        }
        let at = f.offset + i * f.kind.size();
        Some((f.kind, self.bytes.get(at..at + f.kind.size())?))
    }

    pub fn i32_at(&self, name: &str, i: usize) -> Option<i32> {
        match self.raw(name, i)? {
            (I32, b) => Some(i32::from_le_bytes(b.try_into().ok()?)),
            (Bool, b) => Some(i32::from(b[0])),
            _ => None,
        }
    }

    pub fn i32(&self, name: &str) -> Option<i32> {
        self.i32_at(name, 0)
    }

    pub fn f64_at(&self, name: &str, i: usize) -> Option<f64> {
        match self.raw(name, i)? {
            (F32, b) => {
                let v = f64::from(f32::from_le_bytes(b.try_into().ok()?));
                v.is_finite().then_some(v)
            }
            _ => self.i32_at(name, i).map(f64::from),
        }
    }

    pub fn f64(&self, name: &str) -> Option<f64> {
        self.f64_at(name, 0)
    }

    pub fn flag(&self, name: &str) -> Option<bool> {
        self.i32(name).map(|v| v != 0)
    }

    pub fn arr<const N: usize>(&self, name: &str) -> [Option<f64>; N] {
        std::array::from_fn(|i| self.f64_at(name, i))
    }

    /// Text without the trailing NUL padding; `None` when empty.
    pub fn text(&self, name: &str) -> Option<String> {
        let f = self.layout.field(name)?;
        let b = self.bytes.get(f.offset..f.offset + f.kind.size())?;
        let s = match f.kind {
            Wide(_) => {
                let units: Vec<u16> = b
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .take_while(|u| *u != 0)
                    .collect();
                String::from_utf16_lossy(&units)
            }
            Narrow(_) => {
                let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
                String::from_utf8_lossy(&b[..end]).into_owned()
            }
            _ => return None,
        };
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
    }

    /// One field as JSON: a number, a text, or an array of numbers.
    pub fn value(&self, f: &Field) -> Option<Value> {
        match f.kind {
            Wide(_) | Narrow(_) => Some(json!(self.text(f.name).unwrap_or_default())),
            _ if f.count == 1 => self.number(f.name, 0),
            _ => {
                let v: Option<Vec<Value>> = (0..f.count).map(|i| self.number(f.name, i)).collect();
                v.map(Value::Array)
            }
        }
    }

    fn number(&self, name: &str, i: usize) -> Option<Value> {
        match self.raw(name, i)?.0 {
            I32 | Bool => self.i32_at(name, i).map(Value::from),
            // Seven digits are what an f32 holds.
            _ => Some(match self.f64_at(name, i) {
                Some(v) => json!(round_sig(v)),
                None => Value::Null,
            }),
        }
    }

    /// Every field under `prefix.` and its own name.
    pub fn channels(&self, prefix: &str) -> Vec<Channel> {
        self.layout
            .fields
            .iter()
            .filter_map(|f| {
                Some(Channel {
                    name: format!("{prefix}.{}", f.name),
                    unit: String::new(),
                    description: String::new(),
                    value: self.value(f)?,
                })
            })
            .collect()
    }

    /// Every field as one JSON object.
    pub fn object(&self) -> serde_json::Map<String, Value> {
        self.layout
            .fields
            .iter()
            .filter_map(|f| Some((f.name.to_string(), self.value(f)?)))
            .collect()
    }
}

fn round_sig(v: f64) -> f64 {
    if v == 0.0 {
        return 0.0;
    }
    let digits = 7 - (v.abs().log10().floor() as i32) - 1;
    let k = 10f64.powi(digits.clamp(0, 15));
    (v * k).round() / k
}

/// Writes values into a struct's bytes: for tests and for faking a game.
pub struct Writer<'a> {
    pub layout: &'a Layout,
    pub bytes: Vec<u8>,
}

impl<'a> Writer<'a> {
    pub fn new(layout: &'a Layout) -> Self {
        Self {
            layout,
            bytes: vec![0; layout.size],
        }
    }

    fn at(&mut self, name: &str, i: usize) -> (Kind, &mut [u8]) {
        let f = *self
            .layout
            .field(name)
            .unwrap_or_else(|| panic!("no field {name}"));
        assert!(i < f.count, "{name}[{i}] out of range");
        let at = f.offset + i * f.kind.size();
        (f.kind, &mut self.bytes[at..at + f.kind.size()])
    }

    pub fn i32_at(&mut self, name: &str, i: usize, v: i32) -> &mut Self {
        match self.at(name, i) {
            (I32, b) => b.copy_from_slice(&v.to_le_bytes()),
            (Bool, b) => b[0] = u8::from(v != 0),
            (k, _) => panic!("{name} is {k:?}"),
        }
        self
    }

    pub fn i32(&mut self, name: &str, v: i32) -> &mut Self {
        self.i32_at(name, 0, v)
    }

    pub fn f32_at(&mut self, name: &str, i: usize, v: f32) -> &mut Self {
        match self.at(name, i) {
            (F32, b) => b.copy_from_slice(&v.to_le_bytes()),
            (k, _) => panic!("{name} is {k:?}"),
        }
        self
    }

    pub fn f32(&mut self, name: &str, v: f32) -> &mut Self {
        self.f32_at(name, 0, v)
    }

    pub fn f32s(&mut self, name: &str, v: &[f32]) -> &mut Self {
        for (i, x) in v.iter().enumerate() {
            self.f32_at(name, i, *x);
        }
        self
    }

    pub fn text(&mut self, name: &str, s: &str) -> &mut Self {
        match self.at(name, 0) {
            (Wide(n), b) => {
                b.fill(0);
                for (i, u) in s.encode_utf16().take(n - 1).enumerate() {
                    b[2 * i..2 * i + 2].copy_from_slice(&u.to_le_bytes());
                }
            }
            (Narrow(n), b) => {
                b.fill(0);
                let s = s.as_bytes();
                let len = s.len().min(n - 1);
                b[..len].copy_from_slice(&s[..len]);
            }
            (k, _) => panic!("{name} is {k:?}"),
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offset(l: &Layout, name: &str) -> usize {
        l.field(name).unwrap().offset
    }

    #[test]
    fn offsets_and_sizes_match_the_c_structs() {
        // Text of an odd length is followed by padding before the next number.
        assert_eq!(offset(&STATIC, "numberOfSessions"), 60);
        assert_eq!(offset(&STATIC, "carModel"), 68);
        assert_eq!(offset(&STATIC, "playerNick"), 332);
        assert_eq!(offset(&STATIC, "sectorCount"), 400);
        assert_eq!(offset(&STATIC, "PitWindowEnd"), STATIC.size - 4);
        assert_eq!(offset(&GRAPHICS, "completedLaps"), 132);
        assert_eq!(offset(&GRAPHICS, "tyreCompound"), 176);
        assert_eq!(offset(&GRAPHICS, "replayTimeMultiplier"), 244);
        assert_eq!(GRAPHICS.size, 296);
        assert_eq!(offset(&PHYSICS, "velocity"), 32);
        assert_eq!(offset(&PHYSICS, "carDamage"), 224);
        assert_eq!(offset(&PHYSICS, "brakeTemp"), 348);
        assert_eq!(PHYSICS.size, 580);
        assert_eq!(CREWCHIEF.size, 520);
        assert_eq!(VEHICLE.size, 228);
        // The UDP packets, whose sizes tell them apart.
        assert_eq!(HANDSHAKE_RESPONSE.size, 408);
        assert_eq!(RT_CAR_INFO.size, 328);
        assert_eq!(offset(&RT_CAR_INFO, "size"), 4);
        assert_eq!(offset(&RT_CAR_INFO, "isEngineLimiterOn"), 25);
        assert_eq!(offset(&RT_CAR_INFO, "accG_vertical"), 28);
        assert_eq!(offset(&RT_CAR_INFO, "carCoordinates"), 316);
        assert_eq!(RT_LAP.size, 212);
    }

    #[test]
    fn reads_back_what_was_written() {
        let mut w = Writer::new(&STATIC);
        w.text("carModel", "ks_mazda_mx5_cup")
            .text("track", "  ")
            .i32("sectorCount", 3)
            .f32s("tyreRadius", &[0.3, 0.3, 0.31, 0.31]);
        let v = View::new(&STATIC, &w.bytes);
        assert_eq!(v.text("carModel").as_deref(), Some("ks_mazda_mx5_cup"));
        assert_eq!(v.text("track"), None, "blank");
        assert_eq!(v.text("sectorCount"), None, "not text");
        assert_eq!(v.i32("sectorCount"), Some(3));
        assert_eq!(v.f64("sectorCount"), Some(3.0));
        assert_eq!(v.i32("tyreRadius"), None, "not an integer");
        let r = v
            .arr::<4>("tyreRadius")
            .map(|r| r.map(|r| (r * 100.0).round()));
        assert_eq!(r, [Some(30.0), Some(30.0), Some(31.0), Some(31.0)]);
        assert_eq!(v.f64_at("tyreRadius", 4), None, "past the array");
        assert_eq!(v.f64("noSuchField"), None);
        // A short page: what lies beyond is missing, not zero.
        let short = View::new(&STATIC, &w.bytes[..402]);
        assert_eq!(short.i32("sectorCount"), None);
        assert_eq!(short.text("carModel").as_deref(), Some("ks_mazda_mx5_cup"));

        let mut w = Writer::new(&VEHICLE);
        w.text("driverName", "Jörg Ünal").i32("isConnected", 1);
        let v = View::new(&VEHICLE, &w.bytes);
        assert_eq!(v.text("driverName").as_deref(), Some("Jörg Ünal"));
        assert_eq!(v.flag("isConnected"), Some(true));
        assert_eq!(v.flag("isCarInPit"), Some(false));

        let mut w = Writer::new(&RT_CAR_INFO);
        w.i32("isAbsInAction", 7).f32("speed_Kmh", f32::NAN);
        let v = View::new(&RT_CAR_INFO, &w.bytes);
        assert_eq!(v.i32("isAbsInAction"), Some(1));
        assert_eq!(v.f64("speed_Kmh"), None, "not a number");
    }

    #[test]
    fn every_field_becomes_a_channel() {
        let mut w = Writer::new(&GRAPHICS);
        w.i32("completedLaps", 4)
            .text("tyreCompound", "Semislicks (SM)")
            .f32s("carCoordinates", &[1.5, -2.25, 1234.567]);
        let v = View::new(&GRAPHICS, &w.bytes);
        let ch = v.channels("graphics");
        assert_eq!(ch.len(), GRAPHICS.fields.len());
        let get = |n: &str| ch.iter().find(|c| c.name == n).unwrap().value.clone();
        assert_eq!(get("graphics.completedLaps"), json!(4));
        assert_eq!(get("graphics.tyreCompound"), json!("Semislicks (SM)"));
        assert_eq!(get("graphics.currentTime"), json!(""));
        assert_eq!(
            get("graphics.carCoordinates"),
            json!([1.5, -2.25, 1234.567])
        );
        let o = v.object();
        assert_eq!(o["completedLaps"], json!(4));
        assert_eq!(o.len(), GRAPHICS.fields.len());

        let mut w = Writer::new(&PHYSICS);
        w.f32("gas", f32::INFINITY);
        let v = View::new(&PHYSICS, &w.bytes);
        assert_eq!(v.object()["gas"], Value::Null);
        assert_eq!(round_sig(0.123_456_78), 0.123_456_8);
        assert_eq!(round_sig(98_765.432_1), 98_765.43);
        assert_eq!(round_sig(-0.0), 0.0);
    }
}
