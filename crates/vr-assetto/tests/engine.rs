//! Assetto Corsa's pages through the analysis: laps, fuel, sectors and the
//! wheels come out as they do for any game.

use vr_assetto::layout::{CREWCHIEF, GRAPHICS, PHYSICS, STATIC, VEHICLE, Writer};
use vr_assetto::{Snapshot, Tracker, build_frame};
use vr_core::Capabilities;
use vr_engine::{LapsParams, RaceApi, RaceState, SectorBasis, SourceKind};

const LAP_S: f64 = 90.0;
const HZ: f64 = 10.0;

/// The player and one rival, three laps and a bit at Monza.
fn race() -> RaceState {
    let mut st = RaceState::new("assetto", SourceKind::Live, Capabilities::ALL);
    let mut tracker = Tracker::default();
    let mut s = Writer::new(&STATIC);
    s.text("carModel", "ks_ferrari_488_gt3")
        .text("track", "monza")
        .text("playerName", "Ada")
        .i32("sectorCount", 3)
        .f32("maxFuel", 120.0)
        .f32s("tyreRadius", &[0.34; 4])
        .f32("trackSPlineLength", 5758.0);
    let frames = (3.4 * LAP_S * HZ) as usize;
    for i in 0..frames {
        let t = i as f64 / HZ;
        let laps = (t / LAP_S) as i32;
        let pct = (t % LAP_S) / LAP_S;
        let mut p = Writer::new(&PHYSICS);
        p.i32("packetId", i as i32)
            .f32("gas", 1.0)
            .f32("fuel", (60.0 - 2.5 * t / LAP_S) as f32)
            .f32("speedKmh", 180.0)
            .f32s("wheelAngularSpeed", &[(50.0 / 0.34) as f32; 4])
            .f32s("wheelsPressure", &[27.0; 4])
            .f32s("tyreTempM", &[85.0; 4])
            .f32s("localVelocity", &[0.0, 0.0, 50.0])
            .f32s("rideHeight", &[0.05, 0.07])
            .f32s("suspensionTravel", &[0.02; 4])
            .i32("gear", 5)
            .i32("rpms", 7000);
        let mut g = Writer::new(&GRAPHICS);
        g.i32("packetId", i as i32)
            .i32("status", 2)
            .i32("session", 2)
            .i32("completedLaps", laps)
            .i32("position", 2)
            .i32("iCurrentTime", ((t % LAP_S) * 1000.0) as i32 + 1)
            .i32(
                "iLastTime",
                if laps > 0 { (LAP_S * 1000.0) as i32 } else { 0 },
            )
            .i32(
                "iBestTime",
                if laps > 0 { (LAP_S * 1000.0) as i32 } else { 0 },
            )
            .i32("currentSectorIndex", (pct * 3.0) as i32)
            .i32("numberOfLaps", 20)
            .f32("distanceTraveled", (t * 50.0) as f32)
            .f32("normalizedCarPosition", pct as f32);
        let mut head = Writer::new(&CREWCHIEF);
        head.i32("numVehicles", 2);
        let mut cc = head.bytes;
        for (id, ahead) in [(0, 0.0), (5, 0.02)] {
            let pos = pct + ahead;
            let mut v = Writer::new(&VEHICLE);
            v.i32("carId", id)
                .text("driverName", if id == 0 { "Ada" } else { "Grace" })
                .i32("lapCount", laps + i32::from(pos >= 1.0))
                .f32("spLineLength", (pos % 1.0) as f32)
                .i32(
                    "carRealTimeLeaderboardPosition",
                    if id == 0 { 1 } else { 0 },
                )
                .i32("lastLapTimeMS", if laps > 0 { 89_000 } else { 0 })
                .i32("isConnected", 1);
            cc.extend(v.bytes);
        }
        let snap = Snapshot {
            physics: p.bytes.clone(),
            graphics: g.bytes.clone(),
            statics: s.bytes.clone(),
            crewchief: Some(cc),
        };
        let now = 1_000_000 + (t * 1000.0) as i64;
        st.ingest_at(build_frame(&snap, &mut tracker, i as u64, now), now);
    }
    st
}

#[test]
fn laps_fuel_sectors_and_rivals() {
    let st = race();
    let laps = st.laps(&LapsParams::default()).unwrap();
    let times: Vec<f64> = laps
        .laps
        .iter()
        .filter_map(|l| l.time_s)
        .map(|t| (t * 10.0).round() / 10.0)
        .collect();
    assert_eq!(times, [90.0, 90.0, 90.0], "{laps:?}");
    let fuel = st.fuel().unwrap().fuel.unwrap();
    let per_lap = fuel.per_lap_l.unwrap();
    assert!((per_lap - 2.5).abs() < 0.05, "{per_lap}");
    assert_eq!(fuel.capacity_l, Some(120.0));

    let sectors = st.sectors(&Default::default()).unwrap();
    assert_eq!(sectors.basis, SectorBasis::GameLines, "{sectors:?}");
    let starts: Vec<f64> = sectors.starts.iter().map(|s| (s * 100.0).round()).collect();
    assert_eq!(starts, [0.0, 33.0, 67.0]);

    let s = st.standings(&Default::default()).unwrap();
    assert_eq!(s.rows.len(), 2, "{s:?}");
    let brief = st.brief().unwrap().to_text();
    assert!(brief.starts_with("[assetto live"), "{brief}");
    assert!(brief.contains("monza"), "{brief}");
    assert!(brief.contains("Grace"), "{brief}");

    // The wheels are told live, so the four-wheel analysis runs too.
    let c = st.chassis().expect("wheel data");
    assert_eq!(c.laps, 3);
    assert_eq!(c.lock_s.lf, 0.0, "wheels at the car's speed");
    assert_eq!(c.spin_s.rr, 0.0);
    let kpa = c.pressure_kpa.lf.unwrap();
    assert!((kpa - 27.0 * 6.894_757).abs() < 1e-6, "{kpa}");
    let ride = &c.ride[0];
    assert_eq!(ride.from_kmh, 180.0);
    assert_eq!((ride.front_mm.round(), ride.rear_mm.round()), (50.0, 70.0));
}
