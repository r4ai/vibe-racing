//! The shared memory block, field by field, as `SharedMemory.h` (version 14)
//! declares it. Offsets follow the C layout rules (4-byte alignment for
//! numbers, none for `bool` and `char`), so the table below is the only place
//! that knows the order.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;
use vr_core::Channel;

pub const STRING_LEN: usize = 64;
pub const PARTICIPANTS: usize = 64;
pub const TYRES: usize = 4;
pub const COMPOUND_LEN: usize = 40;

/// The oldest layout read: Project CARS 2 with the participant arrays.
pub const MIN_VERSION: u32 = 9;
/// The newest layout known; later ones only append.
pub const KNOWN_VERSION: u32 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    U32,
    I32,
    F32,
    Bool,
    /// A NUL-terminated string of `len` bytes.
    Str(usize),
}

impl Ty {
    fn size(self) -> usize {
        match self {
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::Bool => 1,
            Self::Str(n) => n,
        }
    }

    fn align(self) -> usize {
        match self {
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::Bool | Self::Str(_) => 1,
        }
    }
}

/// One declared field.
struct Def {
    name: &'static str,
    ty: Ty,
    count: usize,
    /// The first shared memory version that has it.
    since: u32,
    unit: &'static str,
    /// Kept by the game for compatibility only; never exported.
    obsolete: bool,
}

const fn f(name: &'static str, ty: Ty, count: usize, since: u32, unit: &'static str) -> Def {
    Def {
        name,
        ty,
        count,
        since,
        unit,
        obsolete: false,
    }
}

const fn old(name: &'static str) -> Def {
    Def {
        obsolete: true,
        ..f(name, Ty::F32, TYRES, MIN_VERSION, "")
    }
}

use Ty::{Bool, F32, I32, Str, U32};
const S: Ty = Str(STRING_LEN);
const B: u32 = MIN_VERSION;
const N: usize = PARTICIPANTS;
const T: usize = TYRES;

/// `ParticipantInfo`, 64 of which follow `mNumParticipants`.
const PARTICIPANT: &[Def] = &[
    f("mIsActive", Bool, 1, B, ""),
    f("mName", S, 1, B, ""),
    f("mWorldPosition", F32, 3, B, "m"),
    f("mCurrentLapDistance", F32, 1, B, "m"),
    f("mRacePosition", U32, 1, B, ""),
    f("mLapsCompleted", U32, 1, B, ""),
    f("mCurrentLap", U32, 1, B, ""),
    f("mCurrentSector", I32, 1, B, ""),
];

/// The marker for the participant array in [`FIELDS`].
const PARTICIPANT_INFO: &str = "mParticipantInfo";

const FIELDS: &[Def] = &[
    f("mVersion", U32, 1, B, ""),
    f("mBuildVersionNumber", U32, 1, B, ""),
    f("mGameState", U32, 1, B, ""),
    f("mSessionState", U32, 1, B, ""),
    f("mRaceState", U32, 1, B, ""),
    f("mViewedParticipantIndex", I32, 1, B, ""),
    f("mNumParticipants", I32, 1, B, ""),
    f(PARTICIPANT_INFO, U32, 0, B, ""),
    f("mUnfilteredThrottle", F32, 1, B, ""),
    f("mUnfilteredBrake", F32, 1, B, ""),
    f("mUnfilteredSteering", F32, 1, B, ""),
    f("mUnfilteredClutch", F32, 1, B, ""),
    f("mCarName", S, 1, B, ""),
    f("mCarClassName", S, 1, B, ""),
    f("mLapsInEvent", U32, 1, B, ""),
    f("mTrackLocation", S, 1, B, ""),
    f("mTrackVariation", S, 1, B, ""),
    f("mTrackLength", F32, 1, B, "m"),
    f("mNumSectors", I32, 1, B, ""),
    f("mLapInvalidated", Bool, 1, B, ""),
    f("mBestLapTime", F32, 1, B, "s"),
    f("mLastLapTime", F32, 1, B, "s"),
    f("mCurrentTime", F32, 1, B, "s"),
    f("mSplitTimeAhead", F32, 1, B, "s"),
    f("mSplitTimeBehind", F32, 1, B, "s"),
    f("mSplitTime", F32, 1, B, "s"),
    f("mEventTimeRemaining", F32, 1, B, "s"),
    f("mPersonalFastestLapTime", F32, 1, B, "s"),
    f("mWorldFastestLapTime", F32, 1, B, "s"),
    f("mCurrentSector1Time", F32, 1, B, "s"),
    f("mCurrentSector2Time", F32, 1, B, "s"),
    f("mCurrentSector3Time", F32, 1, B, "s"),
    f("mFastestSector1Time", F32, 1, B, "s"),
    f("mFastestSector2Time", F32, 1, B, "s"),
    f("mFastestSector3Time", F32, 1, B, "s"),
    f("mPersonalFastestSector1Time", F32, 1, B, "s"),
    f("mPersonalFastestSector2Time", F32, 1, B, "s"),
    f("mPersonalFastestSector3Time", F32, 1, B, "s"),
    f("mWorldFastestSector1Time", F32, 1, B, "s"),
    f("mWorldFastestSector2Time", F32, 1, B, "s"),
    f("mWorldFastestSector3Time", F32, 1, B, "s"),
    f("mHighestFlagColour", U32, 1, B, ""),
    f("mHighestFlagReason", U32, 1, B, ""),
    f("mPitMode", U32, 1, B, ""),
    f("mPitSchedule", U32, 1, B, ""),
    f("mCarFlags", U32, 1, B, ""),
    f("mOilTempCelsius", F32, 1, B, "C"),
    f("mOilPressureKPa", F32, 1, B, "kPa"),
    f("mWaterTempCelsius", F32, 1, B, "C"),
    f("mWaterPressureKPa", F32, 1, B, "kPa"),
    f("mFuelPressureKPa", F32, 1, B, "kPa"),
    f("mFuelLevel", F32, 1, B, ""),
    f("mFuelCapacity", F32, 1, B, "l"),
    f("mSpeed", F32, 1, B, "m/s"),
    f("mRpm", F32, 1, B, "rpm"),
    f("mMaxRPM", F32, 1, B, "rpm"),
    f("mBrake", F32, 1, B, ""),
    f("mThrottle", F32, 1, B, ""),
    f("mClutch", F32, 1, B, ""),
    f("mSteering", F32, 1, B, ""),
    f("mGear", I32, 1, B, ""),
    f("mNumGears", I32, 1, B, ""),
    f("mOdometerKM", F32, 1, B, "km"),
    f("mAntiLockActive", Bool, 1, B, ""),
    f("mLastOpponentCollisionIndex", I32, 1, B, ""),
    f("mLastOpponentCollisionMagnitude", F32, 1, B, ""),
    f("mBoostActive", Bool, 1, B, ""),
    f("mBoostAmount", F32, 1, B, "%"),
    f("mOrientation", F32, 3, B, "rad"),
    f("mLocalVelocity", F32, 3, B, "m/s"),
    f("mWorldVelocity", F32, 3, B, "m/s"),
    f("mAngularVelocity", F32, 3, B, "rad/s"),
    f("mLocalAcceleration", F32, 3, B, "m/s2"),
    f("mWorldAcceleration", F32, 3, B, "m/s2"),
    f("mExtentsCentre", F32, 3, B, "m"),
    f("mTyreFlags", U32, T, B, ""),
    f("mTerrain", U32, T, B, ""),
    f("mTyreY", F32, T, B, "m"),
    f("mTyreRPS", F32, T, B, "rad/s"),
    old("mTyreSlipSpeed"),
    f("mTyreTemp", F32, T, B, "C"),
    old("mTyreGrip"),
    f("mTyreHeightAboveGround", F32, T, B, "m"),
    old("mTyreLateralStiffness"),
    f("mTyreWear", F32, T, B, ""),
    f("mBrakeDamage", F32, T, B, ""),
    f("mSuspensionDamage", F32, T, B, ""),
    f("mBrakeTempCelsius", F32, T, B, "C"),
    f("mTyreTreadTemp", F32, T, B, "K"),
    f("mTyreLayerTemp", F32, T, B, "K"),
    f("mTyreCarcassTemp", F32, T, B, "K"),
    f("mTyreRimTemp", F32, T, B, "K"),
    f("mTyreInternalAirTemp", F32, T, B, "K"),
    f("mCrashState", U32, 1, B, ""),
    f("mAeroDamage", F32, 1, B, ""),
    f("mEngineDamage", F32, 1, B, ""),
    f("mAmbientTemperature", F32, 1, B, "C"),
    f("mTrackTemperature", F32, 1, B, "C"),
    f("mRainDensity", F32, 1, B, ""),
    f("mWindSpeed", F32, 1, B, "m/s"),
    f("mWindDirectionX", F32, 1, B, ""),
    f("mWindDirectionY", F32, 1, B, ""),
    f("mCloudBrightness", F32, 1, B, ""),
    f("mSequenceNumber", U32, 1, B, ""),
    f("mWheelLocalPositionY", F32, T, B, "m"),
    f("mSuspensionTravel", F32, T, B, "m"),
    f("mSuspensionVelocity", F32, T, B, "m/s"),
    f("mAirPressure", F32, T, B, ""),
    f("mEngineSpeed", F32, 1, B, "rad/s"),
    f("mEngineTorque", F32, 1, B, "N*m"),
    f("mWings", F32, 2, B, ""),
    f("mHandBrake", F32, 1, B, ""),
    f("mCurrentSector1Times", F32, N, B, "s"),
    f("mCurrentSector2Times", F32, N, B, "s"),
    f("mCurrentSector3Times", F32, N, B, "s"),
    f("mFastestSector1Times", F32, N, B, "s"),
    f("mFastestSector2Times", F32, N, B, "s"),
    f("mFastestSector3Times", F32, N, B, "s"),
    f("mFastestLapTimes", F32, N, B, "s"),
    f("mLastLapTimes", F32, N, B, "s"),
    f("mLapsInvalidated", Bool, N, B, ""),
    f("mRaceStates", U32, N, B, ""),
    f("mPitModes", U32, N, B, ""),
    f("mOrientations", F32, N * 3, B, "rad"),
    f("mSpeeds", F32, N, B, "m/s"),
    f("mCarNames", S, N, B, ""),
    f("mCarClassNames", S, N, B, ""),
    f("mEnforcedPitStopLap", I32, 1, B, ""),
    f("mTranslatedTrackLocation", S, 1, B, ""),
    f("mTranslatedTrackVariation", S, 1, B, ""),
    f("mBrakeBias", F32, 1, B, ""),
    f("mTurboBoostPressure", F32, 1, B, ""),
    f("mTyreCompound", Str(COMPOUND_LEN), T, B, ""),
    f("mPitSchedules", U32, N, B, ""),
    f("mHighestFlagColours", U32, N, B, ""),
    f("mHighestFlagReasons", U32, N, B, ""),
    f("mNationalities", U32, N, B, ""),
    f("mSnowDensity", F32, 1, B, ""),
    // AMS2 additions.
    f("mSessionDuration", F32, 1, 10, "min"),
    f("mSessionAdditionalLaps", I32, 1, 10, ""),
    f("mTyreTempLeft", F32, T, 10, "C"),
    f("mTyreTempCenter", F32, T, 10, "C"),
    f("mTyreTempRight", F32, T, 10, "C"),
    f("mDrsState", U32, 1, 10, ""),
    f("mRideHeight", F32, T, 10, ""),
    f("mJoyPad0", U32, 1, 10, ""),
    f("mDPad", U32, 1, 10, ""),
    f("mAntiLockSetting", I32, 1, 11, ""),
    f("mTractionControlSetting", I32, 1, 11, ""),
    f("mErsDeploymentMode", I32, 1, 12, ""),
    f("mErsAutoModeEnabled", Bool, 1, 12, ""),
    f("mClutchTemp", F32, 1, 12, "K"),
    f("mClutchWear", F32, 1, 12, ""),
    f("mClutchOverheated", Bool, 1, 12, ""),
    f("mClutchSlipping", Bool, 1, 12, ""),
    f("mYellowFlagState", I32, 1, 13, ""),
    f("mSessionIsPrivate", Bool, 1, 14, ""),
    f("mLaunchStage", I32, 1, 14, ""),
];

/// Where a field is.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub ty: Ty,
    pub count: usize,
    pub offset: usize,
    pub since: u32,
    pub unit: &'static str,
    obsolete: bool,
    /// Inside `ParticipantInfo`: the offset is from the start of one entry.
    participant: bool,
}

pub struct Layout {
    fields: Vec<Field>,
    by_name: HashMap<&'static str, usize>,
    participant_by_name: HashMap<&'static str, usize>,
    pub participant_offset: usize,
    pub participant_size: usize,
    /// Of the whole block.
    #[cfg_attr(not(test), allow(dead_code))]
    pub size: usize,
}

fn align_up(n: usize, a: usize) -> usize {
    n.div_ceil(a) * a
}

/// Lay out `defs` from `start`; returns the fields and the end offset.
fn place(defs: &[Def], start: usize, participant: bool) -> (Vec<Field>, usize) {
    let mut at = start;
    let fields = defs
        .iter()
        .map(|d| {
            at = align_up(at, d.ty.align());
            let field = Field {
                name: d.name,
                ty: d.ty,
                count: d.count,
                offset: at,
                since: d.since,
                unit: d.unit,
                obsolete: d.obsolete,
                participant,
            };
            at += d.ty.size() * d.count;
            field
        })
        .collect();
    (fields, at)
}

impl Layout {
    fn build() -> Self {
        let (participant, end) = place(PARTICIPANT, 0, true);
        let participant_size = align_up(end, 4);
        let mut fields = Vec::new();
        let mut at = 0;
        let mut participant_offset = 0;
        for d in FIELDS {
            if d.name == PARTICIPANT_INFO {
                at = align_up(at, 4);
                participant_offset = at;
                at += participant_size * PARTICIPANTS;
                continue;
            }
            let (mut one, end) = place(std::slice::from_ref(d), at, false);
            fields.append(&mut one);
            at = end;
        }
        let size = align_up(at, 4);
        let by_name = fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name, i))
            .collect();
        let participant_by_name = participant
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name, fields.len() + i))
            .collect();
        fields.extend(participant);
        Self {
            fields,
            by_name,
            participant_by_name,
            participant_offset,
            participant_size,
            size,
        }
    }

    pub fn get() -> &'static Self {
        static L: OnceLock<Layout> = OnceLock::new();
        L.get_or_init(Self::build)
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.by_name.get(name).map(|i| &self.fields[*i])
    }

    pub fn participant_field(&self, name: &str) -> Option<&Field> {
        self.participant_by_name.get(name).map(|i| &self.fields[*i])
    }

    /// The end of the block a version writes.
    pub fn size_for(&self, version: u32) -> usize {
        self.fields
            .iter()
            .filter(|f| !f.participant && f.since <= version)
            .map(|f| f.offset + f.ty.size() * f.count)
            .max()
            .map(|end| align_up(end, 4))
            .unwrap_or(0)
    }
}

/// One copy of the block, read against the version it says it is.
#[derive(Clone)]
pub struct Snapshot<'a> {
    bytes: &'a [u8],
    pub version: u32,
}

fn finite(x: f32) -> Option<f64> {
    x.is_finite().then_some(f64::from(x))
}

/// `0.1f32` as `0.1`, not `0.10000000149011612`.
fn short(x: f32) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    x.to_string()
        .parse::<f64>()
        .ok()
        .and_then(serde_json::Number::from_f64)
        .map_or(Value::Null, Value::Number)
}

fn text(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

impl<'a> Snapshot<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        let version = bytes
            .get(..4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0);
        Self { bytes, version }
    }

    fn known(&self, name: &str) -> Option<&'static Field> {
        Layout::get()
            .field(name)
            .filter(|f| f.since <= self.version)
    }

    fn word(&self, at: usize) -> Option<[u8; 4]> {
        self.bytes.get(at..at + 4)?.try_into().ok()
    }

    fn at(&self, f: &Field, i: usize, base: usize) -> Option<usize> {
        (i < f.count).then(|| base + f.offset + i * f.ty.size())
    }

    fn u32_in(&self, f: &Field, i: usize, base: usize) -> Option<u32> {
        let at = self.at(f, i, base)?;
        match f.ty {
            Ty::U32 | Ty::I32 => self.word(at).map(u32::from_le_bytes),
            Ty::Bool => self.bytes.get(at).map(|b| u32::from(*b)),
            Ty::F32 | Ty::Str(_) => None,
        }
    }

    fn f64_in(&self, f: &Field, i: usize, base: usize) -> Option<f64> {
        match f.ty {
            Ty::F32 => finite(f32::from_le_bytes(self.word(self.at(f, i, base)?)?)),
            Ty::I32 => self.u32_in(f, i, base).map(|v| f64::from(v as i32)),
            Ty::U32 | Ty::Bool => self.u32_in(f, i, base).map(f64::from),
            Ty::Str(_) => None,
        }
    }

    fn str_in(&self, f: &Field, i: usize, base: usize) -> Option<String> {
        let Ty::Str(n) = f.ty else { return None };
        let at = self.at(f, i, base)?;
        self.bytes.get(at..at + n).map(text)
    }

    pub fn u32_at(&self, name: &str, i: usize) -> Option<u32> {
        self.u32_in(self.known(name)?, i, 0)
    }

    pub fn u32(&self, name: &str) -> Option<u32> {
        self.u32_at(name, 0)
    }

    pub fn i32_at(&self, name: &str, i: usize) -> Option<i32> {
        self.u32_at(name, i).map(|v| v as i32)
    }

    pub fn i32(&self, name: &str) -> Option<i32> {
        self.i32_at(name, 0)
    }

    pub fn f64_at(&self, name: &str, i: usize) -> Option<f64> {
        self.f64_in(self.known(name)?, i, 0)
    }

    pub fn f64(&self, name: &str) -> Option<f64> {
        self.f64_at(name, 0)
    }

    pub fn bool_at(&self, name: &str, i: usize) -> Option<bool> {
        self.u32_at(name, i).map(|v| v != 0)
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        self.bool_at(name, 0)
    }

    pub fn str_at(&self, name: &str, i: usize) -> Option<String> {
        self.str_in(self.known(name)?, i, 0)
    }

    pub fn str(&self, name: &str) -> Option<String> {
        self.str_at(name, 0)
    }

    /// The same values, four of them, as `[LF, RF, LR, RR]`.
    pub fn tyres(&self, name: &str) -> [Option<f64>; 4] {
        std::array::from_fn(|i| self.f64_at(name, i))
    }

    fn entry(&self, car: usize) -> Option<usize> {
        let l = Layout::get();
        (car < PARTICIPANTS).then(|| l.participant_offset + car * l.participant_size)
    }

    /// `ParticipantInfo[car].name[i]`.
    pub fn part_f64(&self, car: usize, name: &str, i: usize) -> Option<f64> {
        let f = Layout::get().participant_field(name)?;
        self.f64_in(f, i, self.entry(car)?)
    }

    pub fn part_u32(&self, car: usize, name: &str) -> Option<u32> {
        let f = Layout::get().participant_field(name)?;
        self.u32_in(f, 0, self.entry(car)?)
    }

    pub fn part_str(&self, car: usize, name: &str) -> Option<String> {
        let f = Layout::get().participant_field(name)?;
        self.str_in(f, 0, self.entry(car)?)
    }

    fn values(&self, f: &Field, base: usize) -> Vec<Value> {
        (0..f.count)
            .map(|i| match f.ty {
                Ty::Str(_) => self.str_in(f, i, base).map_or(Value::Null, Value::String),
                Ty::Bool => self
                    .u32_in(f, i, base)
                    .map_or(Value::Null, |v| Value::Bool(v != 0)),
                Ty::U32 => self.u32_in(f, i, base).map_or(Value::Null, Value::from),
                Ty::I32 => self
                    .u32_in(f, i, base)
                    .map_or(Value::Null, |v| Value::from(v as i32)),
                Ty::F32 => self
                    .word(base + f.offset + i * 4)
                    .map_or(Value::Null, |w| short(f32::from_le_bytes(w))),
            })
            .collect()
    }

    /// Every field the version has, under the game's names, sorted.
    pub fn channels(&self) -> Vec<Channel> {
        let l = Layout::get();
        let mut out: Vec<Channel> = l
            .fields
            .iter()
            .filter(|f| !f.obsolete && f.since <= self.version)
            .map(|f| {
                let value = if f.participant {
                    let per_car = (0..PARTICIPANTS)
                        .map(|car| {
                            let mut v = self.values(f, self.entry(car).unwrap_or(0));
                            if v.len() == 1 {
                                v.remove(0)
                            } else {
                                Value::Array(v)
                            }
                        })
                        .collect();
                    Value::Array(per_car)
                } else {
                    let mut v = self.values(f, 0);
                    if v.len() == 1 {
                        v.remove(0)
                    } else {
                        Value::Array(v)
                    }
                };
                Channel {
                    name: if f.participant {
                        format!("{PARTICIPANT_INFO}.{}", f.name)
                    } else {
                        f.name.to_string()
                    },
                    unit: f.unit.to_string(),
                    description: String::new(),
                    value,
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

/// Writes a block for tests.
#[cfg(test)]
pub mod testutil {
    use super::*;

    pub struct Image {
        pub bytes: Vec<u8>,
    }

    impl Image {
        pub fn new() -> Self {
            let mut img = Self {
                bytes: vec![0; Layout::get().size],
            };
            img.set_u32("mVersion", 0, KNOWN_VERSION);
            img
        }

        fn at(name: &str, i: usize) -> usize {
            let f = Layout::get()
                .field(name)
                .unwrap_or_else(|| panic!("{name}"));
            assert!(i < f.count, "{name}[{i}]");
            f.offset + i * f.ty.size()
        }

        fn part_at(car: usize, name: &str, i: usize) -> usize {
            let l = Layout::get();
            let f = l.participant_field(name).unwrap();
            assert!(i < f.count);
            l.participant_offset + car * l.participant_size + f.offset + i * f.ty.size()
        }

        pub fn set_u32(&mut self, name: &str, i: usize, v: u32) {
            let at = Self::at(name, i);
            self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }

        pub fn set_i32(&mut self, name: &str, i: usize, v: i32) {
            self.set_u32(name, i, v as u32);
        }

        pub fn set_f32(&mut self, name: &str, i: usize, v: f32) {
            let at = Self::at(name, i);
            self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }

        pub fn set_bool(&mut self, name: &str, i: usize, v: bool) {
            let at = Self::at(name, i);
            self.bytes[at] = u8::from(v);
        }

        pub fn set_str(&mut self, name: &str, i: usize, v: &str) {
            let f = Layout::get().field(name).unwrap();
            let Ty::Str(n) = f.ty else { panic!("{name}") };
            let at = Self::at(name, i);
            self.bytes[at..at + n].fill(0);
            self.bytes[at..at + v.len()].copy_from_slice(v.as_bytes());
        }

        pub fn set_tyres(&mut self, name: &str, v: [f32; 4]) {
            for (i, x) in v.into_iter().enumerate() {
                self.set_f32(name, i, x);
            }
        }

        pub fn part_u32(&mut self, car: usize, name: &str, v: u32) {
            let at = Self::part_at(car, name, 0);
            self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }

        pub fn part_f32(&mut self, car: usize, name: &str, i: usize, v: f32) {
            let at = Self::part_at(car, name, i);
            self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }

        pub fn part_bool(&mut self, car: usize, name: &str, v: bool) {
            let at = Self::part_at(car, name, 0);
            self.bytes[at] = u8::from(v);
        }

        pub fn part_str(&mut self, car: usize, name: &str, v: &str) {
            let at = Self::part_at(car, name, 0);
            self.bytes[at..at + STRING_LEN].fill(0);
            self.bytes[at..at + v.len()].copy_from_slice(v.as_bytes());
        }

        pub fn snapshot(&self) -> Snapshot<'_> {
            Snapshot::new(&self.bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::Image;
    use super::*;

    /// Offsets of the C struct as compiled (from the AMS2 SDK header).
    #[test]
    fn offsets_match_the_c_header() {
        let l = Layout::get();
        let at = |n: &str| l.field(n).unwrap().offset;
        let known = [
            ("mVersion", 0),
            ("mNumParticipants", 24),
            ("mUnfilteredThrottle", 6428),
            ("mCarName", 6444),
            ("mLapsInEvent", 6572),
            ("mTrackLength", 6704),
            ("mLapInvalidated", 6712),
            ("mBestLapTime", 6716),
            ("mEventTimeRemaining", 6740),
            ("mHighestFlagColour", 6800),
            ("mFuelLevel", 6840),
            ("mAntiLockActive", 6888),
            ("mLastOpponentCollisionIndex", 6892),
            ("mBoostActive", 6900),
            ("mBoostAmount", 6904),
            ("mTyreFlags", 6992),
            ("mTyreTemp", 7072),
            ("mTyreHeightAboveGround", 7104),
            ("mTyreWear", 7136),
            ("mTyreInternalAirTemp", 7264),
            ("mCrashState", 7280),
            ("mCloudBrightness", 7316),
            ("mSequenceNumber", 7320),
            ("mAirPressure", 7372),
            ("mHandBrake", 7404),
            ("mCurrentSector1Times", 7408),
            ("mLapsInvalidated", 9456),
            ("mRaceStates", 9520),
            ("mOrientations", 10032),
            ("mSpeeds", 10800),
            ("mCarNames", 11056),
            ("mCarClassNames", 15152),
            ("mEnforcedPitStopLap", 19248),
            ("mBrakeBias", 19380),
            ("mTyreCompound", 19388),
            ("mPitSchedules", 19548),
            ("mNationalities", 20316),
            ("mSnowDensity", 20572),
            ("mSessionDuration", 20576),
            ("mTyreTempLeft", 20584),
            ("mDrsState", 20632),
            ("mRideHeight", 20636),
            ("mAntiLockSetting", 20660),
            ("mErsDeploymentMode", 20668),
            ("mErsAutoModeEnabled", 20672),
            ("mClutchTemp", 20676),
            ("mClutchOverheated", 20684),
            ("mClutchSlipping", 20685),
            ("mYellowFlagState", 20688),
            ("mSessionIsPrivate", 20692),
            ("mLaunchStage", 20696),
        ];
        for (name, want) in known {
            assert_eq!(at(name), want, "{name}");
        }
        assert_eq!(l.participant_offset, 28);
        assert_eq!(l.participant_size, 100);
        let p = |n: &str| l.participant_field(n).unwrap().offset;
        assert_eq!(
            [
                p("mIsActive"),
                p("mName"),
                p("mWorldPosition"),
                p("mCurrentLapDistance"),
                p("mRacePosition"),
                p("mLapsCompleted"),
                p("mCurrentLap"),
                p("mCurrentSector"),
            ],
            [0, 1, 68, 80, 84, 88, 92, 96]
        );
        assert_eq!(l.size, 20700);
        // CrewChief measured the version 13 struct at 20692 bytes.
        assert_eq!(l.size_for(13), 20692);
        assert_eq!(l.size_for(14), 20700);
        assert_eq!(l.size_for(9), 20576);
    }

    #[test]
    fn reads_values_and_hides_newer_fields() {
        let mut img = Image::new();
        img.set_f32("mSpeed", 0, 41.5);
        img.set_i32("mGear", 0, -1);
        img.set_bool("mAntiLockActive", 0, true);
        img.set_str("mCarName", 0, "Formula Ultimate");
        img.set_str("mTyreCompound", 2, "Soft");
        img.set_f32("mTyreWear", 3, 0.25);
        img.set_i32("mLaunchStage", 0, 2);
        img.set_f32("mFuelLevel", 0, f32::NAN);
        img.part_str(3, "mName", "Ayrton");
        img.part_f32(3, "mWorldPosition", 2, -12.5);
        img.part_u32(3, "mRacePosition", 4);
        img.part_bool(3, "mIsActive", true);

        let s = img.snapshot();
        assert_eq!(s.version, 14);
        assert_eq!(s.f64("mSpeed"), Some(41.5));
        assert_eq!(s.i32("mGear"), Some(-1));
        assert_eq!(s.f64("mGear"), Some(-1.0));
        assert_eq!(s.bool("mAntiLockActive"), Some(true));
        assert_eq!(s.str("mCarName").as_deref(), Some("Formula Ultimate"));
        assert_eq!(s.str_at("mTyreCompound", 2).as_deref(), Some("Soft"));
        assert_eq!(s.str_at("mTyreCompound", 4), None, "out of range");
        assert_eq!(
            s.tyres("mTyreWear"),
            [Some(0.0), Some(0.0), Some(0.0), Some(0.25)]
        );
        assert_eq!(s.f64("mFuelLevel"), None, "NaN");
        assert_eq!(s.f64("mCarName"), None, "not a number");
        assert_eq!(s.str("mSpeed"), None, "not a string");
        assert_eq!(s.u32("mSpeed"), None);
        assert_eq!(s.f64("mNoSuchThing"), None);
        assert_eq!(s.part_str(3, "mName").as_deref(), Some("Ayrton"));
        assert_eq!(s.part_f64(3, "mWorldPosition", 2), Some(-12.5));
        assert_eq!(s.part_f64(3, "mWorldPosition", 3), None);
        assert_eq!(s.part_u32(3, "mRacePosition"), Some(4));
        assert_eq!(s.part_u32(3, "mIsActive"), Some(1));
        assert_eq!(s.part_u32(64, "mRacePosition"), None);
        assert_eq!(s.i32("mLaunchStage"), Some(2));

        img.set_u32("mVersion", 0, 13);
        let s = img.snapshot();
        assert_eq!(s.i32("mLaunchStage"), None, "not in version 13");
        assert_eq!(s.i32("mYellowFlagState"), Some(0));
        let names: Vec<String> = s.channels().into_iter().map(|c| c.name).collect();
        assert!(!names.contains(&"mLaunchStage".to_string()));
        assert!(names.contains(&"mYellowFlagState".to_string()));

        assert_eq!(Snapshot::new(&[1, 0]).version, 0, "too short");
    }

    #[test]
    fn channels_cover_every_field_but_the_obsolete() {
        let mut img = Image::new();
        img.set_f32("mBrakeBias", 0, 0.1);
        img.set_tyres("mTyreTemp", [80.0, 81.0, 82.5, 83.0]);
        img.part_str(0, "mName", "Me");
        img.part_f32(0, "mWorldPosition", 0, 1.5);
        img.part_bool(1, "mIsActive", true);
        let ch = img.snapshot().channels();
        let get = |n: &str| ch.iter().find(|c| c.name == n).map(|c| c.value.clone());
        assert_eq!(get("mBrakeBias"), Some(serde_json::json!(0.1)));
        assert_eq!(
            get("mTyreTemp"),
            Some(serde_json::json!([80.0, 81.0, 82.5, 83.0]))
        );
        assert_eq!(get("mTyreGrip"), None, "obsolete");
        assert_eq!(get("mTyreSlipSpeed"), None);
        assert_eq!(get("mParticipantInfo.mName").unwrap()[0], "Me");
        assert_eq!(get("mParticipantInfo.mWorldPosition").unwrap()[0][0], 1.5);
        assert_eq!(get("mParticipantInfo.mIsActive").unwrap()[1], true);
        assert_eq!(
            get("mParticipantInfo.mIsActive")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(get("mCarNames").unwrap().as_array().unwrap().len(), 64);
        assert_eq!(get("mGear"), Some(serde_json::json!(0)));
        assert_eq!(get("mVersion"), Some(serde_json::json!(14)));
        let unit = ch.iter().find(|c| c.name == "mSpeed").unwrap().unit.clone();
        assert_eq!(unit, "m/s");
        assert!(ch.is_sorted_by_key(|c| c.name.clone()));
        // 3 obsolete fields are left out; ParticipantInfo adds 8.
        assert_eq!(ch.len(), FIELDS.len() - 1 - 3 + PARTICIPANT.len());
        assert_eq!(short(f32::INFINITY), Value::Null);
    }
}
