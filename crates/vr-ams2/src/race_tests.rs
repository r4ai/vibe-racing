//! A race driven through the shared memory layout and into the analysis, to
//! show that what the adapter makes up (clock, gaps, sectors, fuel flow) is
//! what the engine needs.

use vr_core::Capabilities;
use vr_engine::{FuelBasis, GapMode, RaceApi, RaceState, SectorBasis, SectorsParams, SourceKind};

use crate::convert::build_frame;
use crate::layout::testutil::Image;
use crate::tracker::{GAME_INGAME_PLAYING, Tracker};

const LENGTH: f64 = 4000.0;
const DT: f64 = 0.05;
/// Lap times of the three cars; the player drives the second.
const LAP: [f64; 3] = [80.0, 81.0, 82.0];
const START: [f64; 3] = [30.0, 15.0, 0.0];
const ME: usize = 1;

fn race_until(end_s: f64) -> RaceState {
    let mut img = Image::new();
    img.set_u32("mGameState", 0, GAME_INGAME_PLAYING);
    img.set_u32("mSessionState", 0, 5);
    img.set_u32("mRaceState", 0, 2);
    img.set_i32("mNumParticipants", 0, 3);
    img.set_i32("mViewedParticipantIndex", 0, ME as i32);
    img.set_str("mTrackLocation", 0, "Interlagos");
    img.set_f32("mTrackLength", 0, LENGTH as f32);
    img.set_i32("mNumSectors", 0, 3);
    img.set_u32("mLapsInEvent", 0, 20);
    img.set_f32("mFuelCapacity", 0, 100.0);
    img.set_u32("mCarFlags", 0, 1 << 1);
    for (i, name) in ["A", "B", "C"].into_iter().enumerate() {
        img.part_bool(i, "mIsActive", true);
        img.part_str(i, "mName", name);
        img.set_str("mCarClassNames", i, "GT3");
        img.set_f32("mLastLapTimes", i, -1.0);
    }
    let mut st = RaceState::new("ams2", SourceKind::Live, Capabilities::ALL);
    let mut t = Tracker::default();
    let mut dist = START;
    let mut laps = [0u32; 3];
    let mut crossed = [0.0f64; 3];
    let mut now = 0.0;
    while now <= end_s {
        for i in 0..3 {
            dist[i] += LENGTH / LAP[i] * DT;
            if dist[i] >= LENGTH {
                dist[i] -= LENGTH;
                laps[i] += 1;
                img.set_f32("mLastLapTimes", i, (now - crossed[i]) as f32);
                crossed[i] = now;
            }
            img.part_f32(i, "mCurrentLapDistance", 0, dist[i] as f32);
            img.part_u32(i, "mLapsCompleted", laps[i]);
            img.part_u32(i, "mCurrentLap", laps[i] + 1);
            img.part_u32(i, "mCurrentSector", (dist[i] / LENGTH * 3.0) as u32);
        }
        let progress = |i: usize| f64::from(laps[i]) + dist[i] / LENGTH;
        for i in 0..3 {
            let ahead = (0..3).filter(|j| progress(*j) > progress(i)).count();
            img.part_u32(i, "mRacePosition", ahead as u32 + 1);
        }
        let fuel = 60.0 - 2.5 * progress(ME);
        img.set_f32("mFuelLevel", 0, (fuel / 100.0) as f32);
        img.set_f32("mSpeed", 0, (LENGTH / LAP[ME]) as f32);
        let frame = build_frame(&img.snapshot(), &mut t, 0, (now * 1000.0) as i64, now);
        st.ingest_at(frame, (now * 1000.0) as i64);
        now += DT;
    }
    st
}

#[test]
fn the_engine_reads_an_ams2_race() {
    let st = race_until(400.0);
    let b = st.brief().unwrap();
    assert_eq!(b.meta.game, "ams2");
    let me = b.player.unwrap();
    assert_eq!(me.car.driver, "B");
    assert_eq!(me.car.position, Some(2));
    let last = me.last_lap_s.unwrap();
    assert!((last - LAP[ME]).abs() < 0.06, "{last}");

    // The gap at the line on the player's 4th lap.
    let ahead = b.ahead.unwrap();
    assert_eq!(ahead.car.driver, "A");
    assert_eq!(ahead.mode, GapMode::RaceTime);
    let want =
        (4.0 * LENGTH - START[ME]) / LENGTH * LAP[ME] - (4.0 * LENGTH - START[0]) / LENGTH * LAP[0];
    let gap = ahead.gap_s.unwrap();
    assert!((gap - want).abs() < 0.1, "{gap} vs {want}");
    let trend = ahead.trend_s_per_lap.unwrap();
    assert!((trend - 1.0).abs() < 0.1, "a second a lap: {trend}");
    let behind = b.behind.unwrap();
    assert_eq!(behind.car.driver, "C");

    let fuel = b.fuel.unwrap();
    assert_eq!(fuel.basis, FuelBasis::History);
    let per_lap = fuel.per_lap_l.unwrap();
    assert!((per_lap - 2.5).abs() < 0.01, "{per_lap}");

    let s = st.sectors(&SectorsParams::default()).unwrap();
    assert_eq!(s.basis, SectorBasis::GameLines);
    assert_eq!(s.starts.len(), 3);
    for (got, want) in s.starts.iter().zip([0.0, 1.0 / 3.0, 2.0 / 3.0]) {
        assert!((got - want).abs() < 0.005, "{:?}", s.starts);
    }

    let live = st.live().unwrap().values;
    assert_eq!(live.get("tc"), Some(&0.0));
    let flow = st
        .frame()
        .unwrap()
        .player
        .as_ref()
        .unwrap()
        .fuel_use_lph
        .unwrap();
    // 2.5 L a lap of 81 s.
    assert!((flow - 2.5 / 81.0 * 3600.0).abs() < 1.0, "{flow}");
}
