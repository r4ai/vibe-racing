//! What the wheels did, from a generated race with known wheels.

use vr_core::*;
use vr_engine::*;
use vr_replay::{SynthConfig, synth_frames};

/// A wheel on a good day.
fn wheel(speed: f64) -> Wheel {
    Wheel {
        speed_mps: Some(speed),
        pressure_kpa: Some(140.0),
        surface_c: [Some(70.0), Some(72.0), Some(75.0)],
        ride_height_m: Some(0.010),
        shock_mps: Some(0.02),
        brake_bar: Some(10.0),
        ..Default::default()
    }
}

/// `change` sees every frame, with the progress around the lap.
fn drive(change: impl Fn(&mut Frame, f64)) -> RaceState {
    let cfg = SynthConfig {
        laps: 6,
        cars: 3,
        yellow: None,
        player_pit_lap: None,
        rival_pit_lap: None,
        dt: 0.1,
        ..Default::default()
    };
    let mut st = RaceState::new("test", SourceKind::Replay, Capabilities::ALL);
    for mut f in synth_frames(cfg, None) {
        let pct = f.player_car().and_then(|c| c.lap_dist_pct).unwrap_or(0.0);
        let p = f.player.as_mut().unwrap();
        let v = p.speed_mps.unwrap();
        (p.throttle, p.brake) = (Some(0.0), Some(0.0));
        p.car.lat_accel_mps2 = Some(0.0);
        (p.car.velocity_x_mps, p.car.velocity_y_mps) = (Some(v), Some(0.0));
        p.car.wheels = Some(Wheels {
            lf: wheel(v),
            rf: wheel(v),
            lr: wheel(v),
            rr: wheel(v),
        });
        change(&mut f, pct);
        st.ingest(f);
    }
    st
}

fn wheels(f: &mut Frame) -> &mut Wheels {
    f.player.as_mut().unwrap().car.wheels.as_mut().unwrap()
}

/// 0.1 s steps in a tenth of a 90 s lap.
const TENTH_S: f64 = 9.0;

#[test]
fn locking_and_spinning_are_counted_per_wheel_and_lap() {
    let st = drive(|f, pct| {
        let p = f.player.as_mut().unwrap();
        let v = p.speed_mps.unwrap();
        if (0.2..0.3).contains(&pct) {
            p.brake = Some(0.8);
            wheels(f).rf.speed_mps = Some(v * 0.5);
            // Just not locking.
            wheels(f).lf.speed_mps = Some(v * 0.881);
        } else if (0.5..0.6).contains(&pct) {
            p.throttle = Some(0.9);
            wheels(f).lr.speed_mps = Some(v * 1.11);
            wheels(f).rr.speed_mps = Some(v * 1.099);
        } else if pct > 0.9 {
            // Coasting: no matter.
            wheels(f).rr.speed_mps = Some(v * 0.5);
        }
    });
    let c = st.chassis().unwrap();
    assert!(c.laps >= 4, "{}", c.laps);
    assert!((c.lock_s.rf - TENTH_S).abs() < 0.3, "{:?}", c.lock_s);
    assert_eq!((c.lock_s.lf, c.lock_s.lr, c.lock_s.rr), (0.0, 0.0, 0.0));
    assert!((c.deepest_lock.rf - 0.5).abs() < 1e-9);
    assert!((c.deepest_lock.lf - 0.119).abs() < 1e-9);
    assert!((c.spin_s.lr - TENTH_S).abs() < 0.3, "{:?}", c.spin_s);
    assert_eq!((c.spin_s.rr, c.spin_s.lf), (0.0, 0.0));
    assert!((c.front_brake.unwrap() - 0.5).abs() < 1e-9);
}

#[test]
fn slides_and_excursions_do_not_count() {
    let st = drive(|f, pct| {
        if !(0.2..0.25).contains(&pct) {
            return;
        }
        wheels(f).lf.ride_height_m = Some(0.0);
        wheels(f).rf.ride_height_m = Some(0.001);
        let p = f.player.as_mut().unwrap();
        let v = p.speed_mps.unwrap();
        p.brake = Some(1.0);
        wheels(f).lf.speed_mps = Some(0.0);
        if f.player_car().unwrap().lap.unwrap() % 2 == 0 {
            f.player.as_mut().unwrap().car.velocity_y_mps = Some(v * 0.2);
        } else {
            let i = f.player_car_idx.unwrap();
            let c = f.cars.iter_mut().find(|c| c.idx == i).unwrap();
            c.surface = TrackSurface::OffTrack;
        }
    });
    let c = st.chassis().unwrap();
    assert_eq!((c.lock_s.lf, c.touch_front_s), (0.0, 0.0));
    assert!(c.laps >= 4);
}

#[test]
fn a_single_bad_lap_is_not_a_tendency() {
    let st = drive(|f, pct| {
        if f.player_car().unwrap().lap == Some(3) && (0.2..0.3).contains(&pct) {
            wheels(f).lr.ride_height_m = Some(0.0);
            wheels(f).rr.ride_height_m = Some(0.0);
        }
    });
    assert_eq!(st.chassis().unwrap().touch_rear_s, 0.0);
    let st = drive(|f, pct| {
        if (0.2..0.3).contains(&pct) {
            wheels(f).lr.ride_height_m = Some(0.0);
            wheels(f).rr.ride_height_m = Some(0.0035);
        }
    });
    let c = st.chassis().unwrap();
    assert!((c.touch_rear_s - TENTH_S).abs() < 0.3);
    assert_eq!(c.touch_front_s, 0.0);
}

#[test]
fn tyres_are_read_under_load_and_dampers_by_their_speed() {
    let st = drive(|f, pct| {
        let p = f.player.as_mut().unwrap();
        // To the left: the right side carries the car.
        p.car.lat_accel_mps2 = Some(if pct < 0.5 { 20.0 } else { 0.0 });
        if pct < 0.5 {
            wheels(f).rf.surface_c = [Some(90.0), Some(85.0), Some(80.0)];
        }
        let w = wheels(f);
        w.lf.shock_mps = Some(match (pct * 4.0) as i32 {
            0 => 0.06,
            1 => -0.06,
            2 => 0.01,
            _ => -0.01,
        });
        w.rr.shock_mps = None;
        w.rr.ride_height_m = Some(0.030);
    });
    let c = st.chassis().unwrap();
    let t = c.loaded_c.rf.unwrap();
    assert_eq!(t.map(f64::round), [90.0, 85.0, 80.0]);
    assert_eq!(c.loaded_c.lf, None);
    assert_eq!(c.pressure_kpa.lr, Some(140.0));
    let d = c.dampers.lf.unwrap();
    for share in [d.bump_low, d.bump_high, d.rebound_low, d.rebound_high] {
        assert!((share - 0.25).abs() < 0.03, "{d:?}");
    }
    assert_eq!(c.dampers.rr, None);
    // Synthetic cars drive 180 km/h: the class from 180 on.
    assert!(!c.ride.is_empty());
    assert!((c.ride.last().unwrap().front_mm - 10.0).abs() < 1e-6);
    assert!((c.ride.last().unwrap().rear_mm - 20.0).abs() < 1e-6);
    assert_eq!(c.ride.last().unwrap().to_kmh, None);
}

#[test]
fn without_wheels_there_is_nothing() {
    let st = drive(|f, _| f.player.as_mut().unwrap().car.wheels = None);
    assert_eq!(st.chassis(), None);
}

#[test]
fn a_run_is_compared_with_another_setup() {
    let mut st = drive(|_, _| {});
    let setup = |bar: &str, hot: &str| Setup {
        name: Some("race".into()),
        items: vec![
            SetupItem {
                path: "Chassis/Front/ArbPosition".into(),
                value: bar.into(),
            },
            SetupItem {
                path: "Tires/LeftFront/LastHotPressure".into(),
                value: hot.into(),
            },
        ],
        ..Setup::default()
    };
    st.set_extras(Extras {
        setup: Some(setup("P2", "150 kPa")),
        ..Extras::default()
    });
    let a = SessionRecord::of(&st).unwrap();
    assert!(a.chassis.is_some());
    let mut b = a.clone();
    b.id = "other".into();
    b.setup = Some(setup("P3", "140 kPa"));
    for l in &mut b.laps {
        l.time_s = l.time_s.map(|t| t + 0.5);
    }
    let mut elsewhere = b.clone();
    elsewhere.id = "elsewhere".into();
    elsewhere.track = Some("Monza".into());
    let mut same = b.clone();
    same.id = "same".into();
    same.setup = Some(setup("P2", "140 kPa"));
    let r = analyse(&a, &[a.clone(), same, elsewhere, b]);
    assert_eq!(r.comparisons.len(), 1);
    let c = &r.comparisons[0];
    assert_eq!(c.session, "other");
    assert_eq!(
        c.changed,
        [(
            "Chassis/Front/ArbPosition".to_string(),
            "P3".to_string(),
            "P2".to_string()
        )]
    );
    assert!((c.best_s.unwrap() + 0.5).abs() < 1e-9);
    let text = r.to_text();
    assert!(text.contains("against other: best -0.50s"), "{text}");
    assert!(text.contains("ArbPosition: P3 -> P2"), "{text}");
    assert!(text.contains("findings: none"), "{text}");
}

#[test]
fn a_telemetry_file_adds_to_what_was_seen_live() {
    let st = drive(|_, _| {});
    let mut file = SessionRecord::of(&st).unwrap();
    let mut live = file.clone();
    live.chassis = None;
    file.standings.truncate(1);
    file.player = None;
    for l in &mut file.laps {
        l.position = None;
    }
    let wanted = (live.standings.clone(), live.player.clone());
    let chassis = file.chassis.clone();
    file.merge_older(&live);
    assert_eq!((file.standings.clone(), file.player.clone()), wanted);
    assert!(file.laps.iter().all(|l| l.position.is_some()));
    // And the other way round.
    live.merge_older(&file);
    assert_eq!(live.chassis, chassis);
}
