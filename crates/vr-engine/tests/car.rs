//! The car: handling, in-car settings, warnings, kept sessions.

use vr_core::*;
use vr_engine::*;

const DT: f64 = 1.0 / 60.0;

struct Sim {
    st: RaceState,
    t: f64,
    frame: Frame,
}

fn car(lap: i32) -> CarState {
    CarState {
        idx: 0,
        number: "7".into(),
        driver: "Me".into(),
        team: None,
        car_name: Some("SF23".into()),
        class_name: None,
        class_id: Some(1),
        rating: None,
        is_player: true,
        position: Some(3),
        class_position: Some(3),
        lap: Some(lap),
        laps_completed: Some(lap - 1),
        lap_dist_pct: Some(0.0),
        on_pit_road: false,
        surface: TrackSurface::OnTrack,
        last_lap_s: None,
        best_lap_s: None,
        gap_to_leader_s: Some(0.0),
        est_lap_s: Some(100.0),
        extra: CarExtra::default(),
    }
}

impl Sim {
    fn new() -> Self {
        let frame = Frame {
            game: "test".into(),
            tick: 0,
            timestamp_ms: 1_795_334_860_000,
            session: SessionState {
                session_num: 0,
                kind: SessionKind::Race,
                phase: SessionPhase::Racing,
                time_s: 0.0,
                time_remaining_s: None,
                laps_remaining: Some(50),
                laps_total: Some(50),
                flags: vec![Flag::Green],
                track: Some(TrackInfo {
                    name: "Test Ring".into(),
                    config: None,
                    length_m: None,
                    sectors: Vec::new(),
                }),
                uid: Some(1),
                time_total_s: None,
                time_of_day_s: None,
            },
            player_car_idx: Some(0),
            player: Some(PlayerTelemetry {
                on_track: true,
                speed_mps: Some(50.0),
                rpm: Some(7500.0),
                gear: Some(4),
                throttle: Some(0.5),
                brake: Some(0.0),
                fuel_l: Some(40.0),
                car: CarTelemetry {
                    clutch: Some(1.0),
                    steering_rad: Some(0.2),
                    yaw_rate_rps: Some(0.4),
                    lat_accel_mps2: Some(20.0),
                    velocity_x_mps: Some(50.0),
                    velocity_y_mps: Some(0.0),
                    abs_active: Some(false),
                    ..Default::default()
                },
                ..Default::default()
            }),
            cars: vec![car(1)],
            weather: Some(Weather {
                wetness: Some("dry".into()),
                ..Default::default()
            }),
        };
        Self {
            st: RaceState::new("test", SourceKind::Live, Capabilities::ALL),
            t: 0.0,
            frame,
        }
    }

    fn p(&mut self) -> &mut PlayerTelemetry {
        self.frame.player.as_mut().unwrap()
    }

    /// Runs for `s` seconds; `f` gets the share of it that has passed.
    fn run(&mut self, s: f64, mut f: impl FnMut(&mut PlayerTelemetry, f64)) {
        let n = (s / DT).round() as usize;
        for i in 0..n {
            self.t += DT;
            f(
                self.frame.player.as_mut().unwrap(),
                (i + 1) as f64 / n as f64,
            );
            self.frame.session.time_s = self.t;
            self.frame.timestamp_ms = 1_795_334_860_000 + (self.t * 1000.0) as i64;
            self.st.ingest(self.frame.clone());
        }
    }

    fn next_lap(&mut self) {
        let c = &mut self.frame.cars[0];
        c.laps_completed = c.lap;
        c.lap = c.lap.map(|l| l + 1);
    }

    fn texts(&self) -> Vec<String> {
        self.st
            .event_log()
            .last(100)
            .into_iter()
            .map(|e| e.text)
            .collect()
    }

    fn handling(&self) -> HandlingReport {
        self.st.handling(&HandlingParams::default()).unwrap()
    }
}

fn corner(p: &mut PlayerTelemetry, _: f64) {
    p.car.steering_rad = Some(0.2);
    p.car.yaw_rate_rps = Some(0.4);
    p.car.lat_accel_mps2 = Some(20.0);
}

#[test]
fn steering_more_without_turning_more_is_understeer() {
    let mut s = Sim::new();
    assert!(!s.handling().ready);
    s.run(3.0, corner);
    assert!(s.handling().ready, "grip is known after 120 samples");
    // Half a second of winding on lock while the car turns less.
    s.run(0.5, |p, x| {
        p.car.steering_rad = Some(0.2 + 0.3 * x);
        p.car.yaw_rate_rps = Some(0.4 - 0.05 * x);
    });
    s.next_lap();
    s.run(0.1, corner);
    let h = s.handling();
    let b = h.mean.unwrap();
    // On the throttle: the window and the smoothing eat the first part.
    assert!((0.15..=0.6).contains(&b.understeer.exit), "{b:?}");
    assert_eq!(b.understeer.entry, 0.0);
    assert_eq!(b.understeer.mid, 0.0);
    assert_eq!(b.oversteer.total(), 0.0);
    assert_eq!(h.laps_sampled, 1);
    assert_eq!(h.laps[0].lap, 1);
}

#[test]
fn turning_in_is_not_understeer() {
    let mut s = Sim::new();
    s.run(3.0, corner);
    // The car follows the steering.
    s.run(0.5, |p, x| {
        p.car.steering_rad = Some(0.2 + 0.3 * x);
        p.car.yaw_rate_rps = Some(0.4 + 0.3 * x);
    });
    // Slow, far from the limit: nothing to say either.
    s.run(0.5, |p, x| {
        p.car.steering_rad = Some(0.2 + 0.3 * x);
        p.car.yaw_rate_rps = Some(0.4 - 0.05 * x);
        p.car.lat_accel_mps2 = Some(7.0);
    });
    s.next_lap();
    s.run(0.1, corner);
    let total = s.handling().mean.map_or(0.0, |b| b.understeer.total());
    assert_eq!(total, 0.0);
}

#[test]
fn counter_steering_is_oversteer_by_phase() {
    for (throttle, brake) in [(0.0, 0.5), (0.0, 0.0), (0.8, 0.0)] {
        let mut s = Sim::new();
        s.run(1.0, corner);
        s.run(1.0, |p, _| {
            p.throttle = Some(throttle);
            p.brake = Some(brake);
            p.car.steering_rad = Some(-0.3);
            p.car.yaw_rate_rps = Some(0.5);
        });
        s.next_lap();
        s.run(0.1, corner);
        let o = s.handling().mean.unwrap().oversteer;
        let (hit, rest) = if brake > 0.0 {
            (o.entry, o.mid + o.exit)
        } else if throttle > 0.3 {
            (o.exit, o.entry + o.mid)
        } else {
            (o.mid, o.entry + o.exit)
        };
        assert!((0.9..=1.0).contains(&hit), "{o:?}");
        assert!(rest < 0.06, "{o:?}");
    }
}

#[test]
fn wheelspin_and_locking_need_a_known_gear() {
    let mut s = Sim::new();
    let straight = |p: &mut PlayerTelemetry, _: f64| {
        p.car.lat_accel_mps2 = Some(1.0);
        p.throttle = Some(0.9);
        p.brake = Some(0.0);
        p.rpm = Some(7500.0);
    };
    // Too early to know what the gear should show.
    s.run(0.6, |p, _| {
        p.car.lat_accel_mps2 = Some(1.0);
        p.throttle = Some(0.9);
        p.rpm = Some(8500.0);
    });
    s.next_lap();
    s.run(4.0, straight);
    s.run(0.5, |p, _| p.rpm = Some(8200.0));
    s.run(0.5, |p, _| {
        p.throttle = Some(0.0);
        p.brake = Some(0.8);
        p.rpm = Some(6800.0);
    });
    s.next_lap();
    s.run(0.1, straight);
    let h = s.handling();
    let first = h.laps.iter().find(|l| l.lap == 1);
    assert!(first.is_none_or(|l| l.behaviour.wheelspin_s == 0.0));
    let b = h.laps.last().unwrap().behaviour;
    assert!((0.4..=0.51).contains(&b.wheelspin_s), "{b:?}");
    assert!((0.48..=0.51).contains(&b.rear_lock_s), "{b:?}");
    // Right after a gear change nothing counts.
    s.run(0.4, |p, _| {
        p.gear = Some(5);
        p.throttle = Some(0.9);
        p.brake = Some(0.0);
        p.rpm = Some(9900.0);
    });
    s.next_lap();
    s.run(0.1, straight);
    let last = s.handling().laps.last().unwrap().behaviour;
    assert_eq!(last.wheelspin_s, 0.0, "{last:?}");
}

#[test]
fn abs_and_slip_angle_are_collected_and_zones_name_the_place() {
    let mut s = Sim::new();
    s.frame.cars[0].lap_dist_pct = Some(0.42);
    s.run(1.0, |p, _| p.car.abs_active = Some(true));
    s.run(0.5, |p, _| {
        p.car.abs_active = Some(false);
        p.car.velocity_y_mps = Some(50.0 * 10f64.to_radians().tan());
        p.car.steering_rad = Some(-0.3);
    });
    // Zones need at least half a lap.
    assert!(s.handling().zones.is_empty());
    for i in 0..60 {
        s.frame.cars[0].lap_dist_pct = Some(0.42 + 0.009 * f64::from(i));
        s.run(DT, |p, _| {
            corner(p, 0.0);
            p.car.velocity_y_mps = Some(0.0);
        });
    }
    let z = s.handling().zones;
    assert_eq!(z.len(), 1, "{z:?}");
    assert_eq!((z[0].from_pct, z[0].to_pct), (0.4, 0.45));
    assert_eq!(z[0].tendency, Tendency::OversteerExit);
    s.next_lap();
    s.run(0.1, corner);
    let b = s.handling().mean.unwrap();
    assert!((b.abs_s - 1.0).abs() < 0.03, "{b:?}");
    assert!((b.max_slip_deg - 10.0).abs() < 1e-6, "{b:?}");
}

#[test]
fn nothing_is_judged_in_the_pits_or_off_track() {
    let mut s = Sim::new();
    s.frame.cars[0].on_pit_road = true;
    s.run(1.0, |p, _| p.car.abs_active = Some(true));
    s.frame.cars[0].on_pit_road = false;
    s.p().on_track = false;
    s.run(1.0, |_, _| {});
    s.p().on_track = true;
    s.p().speed_mps = Some(14.9);
    s.run(1.0, |_, _| {});
    s.next_lap();
    s.run(0.1, |p, _| p.car.abs_active = Some(false));
    assert_eq!(s.handling().mean, None);
    assert!(s.handling().to_text().contains("no complete lap yet"));
}

#[test]
fn advice_follows_what_the_car_can_adjust() {
    let mut s = Sim::new();
    let control = |id: &str, value| Control {
        id: id.into(),
        value,
    };
    let spin = |s: &mut Sim| {
        s.run(1.5, |p, _| {
            p.throttle = Some(0.8);
            p.car.steering_rad = Some(-0.3);
            p.car.yaw_rate_rps = Some(0.5);
        });
        s.next_lap();
    };
    s.p().car.controls = vec![
        control("brake_bias", 57.25),
        control("traction_control", 2.0),
    ];
    s.st.set_extras(Extras {
        setup: Some(Setup {
            items: vec![
                SetupItem {
                    path: "Chassis/Rear/ArbPosition".into(),
                    value: "P2".into(),
                },
                SetupItem {
                    path: "Chassis/Front/ArbPosition".into(),
                    value: "P1".into(),
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    });
    spin(&mut s);
    s.run(0.1, corner);
    assert!(s.handling().advice.is_empty(), "one lap is not enough");
    spin(&mut s);
    s.run(0.1, corner);
    let h = s.handling();
    let tc = &h.advice[0];
    assert_eq!(tc.place, Place::InCar);
    assert_eq!(tc.target, "traction_control");
    assert_eq!(tc.current.as_deref(), Some("2"));
    assert_eq!(tc.change, "one step more intervention");
    assert_eq!(tc.reason, "wheelspin 0.0s and exit oversteer 1.5s per lap");
    let bar = &h.advice[1];
    assert_eq!(bar.place, Place::Garage);
    assert_eq!(bar.target, "Chassis/Rear/ArbPosition");
    assert_eq!(bar.current.as_deref(), Some("P2"));
    assert_eq!(h.advice.len(), 2, "{:?}", h.advice);
    let text = h.to_text();
    assert!(
        text.contains("[in the car] traction_control (now 2): one step more intervention"),
        "{text}"
    );
    assert!(
        text.contains("brake_bias 57.2 | traction_control 2"),
        "{text}"
    );
}

#[test]
fn tyre_temperatures_suggest_pressure_and_camber() {
    let mut s = Sim::new();
    let tyre = |t: [f64; 3]| Tyre {
        cold_kpa: Some(138.0),
        temp_c: t.map(Some),
        ..Default::default()
    };
    // Outer, middle, inner.
    s.p().car.tyres = Tyres {
        lf: tyre([80.0, 92.0, 92.0]),
        rf: tyre([80.0, 85.9, 80.0]),
        lr: tyre([80.0, 87.5, 95.0]),
        rr: tyre([90.0, 79.0, 80.0]),
    };
    s.run(DT, |_, _| {});
    let a = s.handling().advice;
    let got: Vec<(&str, &str)> = a
        .iter()
        .map(|a| (a.target.as_str(), a.change.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("LF pressure", "3-5 kPa lower"),
            ("LR camber", "less negative camber"),
            ("RR pressure", "3-5 kPa higher"),
        ]
    );
    assert_eq!(a[0].current.as_deref(), Some("138 kPa cold"));
    assert_eq!(a[0].reason, "hotter in the middle than at the edges by 6C");
}

#[test]
fn settings_count_once_they_rest() {
    let mut s = Sim::new();
    let set = |s: &mut Sim, v| {
        s.p().car.controls = vec![Control {
            id: "brake_bias".into(),
            value: v,
        }];
    };
    set(&mut s, 56.0);
    s.run(1.0, |_, _| {});
    // Turned through, and back: no change.
    set(&mut s, 56.5);
    s.run(1.0, |_, _| {});
    set(&mut s, 56.0);
    s.run(2.0, |_, _| {});
    assert!(s.st.adjustments().is_empty());
    set(&mut s, 56.5);
    s.run(1.0, |_, _| {});
    set(&mut s, 57.0);
    s.run(1.4, |_, _| {});
    assert!(s.st.adjustments().is_empty(), "not resting for 1.5 s yet");
    s.run(0.2, |_, _| {});
    let a = s.st.adjustments();
    assert_eq!(a.len(), 1);
    assert_eq!(
        (a[0].control.as_str(), a[0].from, a[0].to, a[0].lap),
        ("brake_bias", 56.0, 57.0, Some(1))
    );
    assert!(s.texts().contains(&"brake_bias 56 -> 57".to_string()));
    s.run(5.0, |_, _| {});
    assert_eq!(s.st.adjustments().len(), 1);
    let text = s.st.car().unwrap().to_text();
    assert!(text.contains("brake_bias 56 -> 57"), "{text}");
}

#[test]
fn damage_is_told_once_and_not_for_a_stalled_engine() {
    let mut s = Sim::new();
    s.p().car.warnings = vec![Warning::OilPressure, Warning::Stalled];
    s.run(0.1, |_, _| {});
    assert!(!s.texts().iter().any(|t| t.starts_with("car warning")));
    s.p().car.warnings = vec![Warning::Damage, Warning::Stalled, Warning::RevLimiter];
    s.run(0.1, |_, _| {});
    s.run(0.1, |_, _| {});
    let hits = |s: &Sim, w: &str| s.texts().iter().filter(|t| *t == w).count();
    assert_eq!(hits(&s, "car warning: repairs needed"), 1);
    s.p().car.warnings = vec![Warning::OilPressure];
    s.run(0.1, |_, _| {});
    assert_eq!(hits(&s, "car warning: oil pressure"), 1);
    let brief = s.st.brief().unwrap().to_text();
    assert!(brief.contains("car: WARNING oil pressure"), "{brief}");
}

#[test]
fn weather_changes_are_events() {
    let mut s = Sim::new();
    s.run(0.1, |_, _| {});
    s.frame.weather = None;
    s.run(0.1, |_, _| {});
    s.frame.weather = Some(Weather {
        wetness: Some("lightly wet".into()),
        ..Default::default()
    });
    s.run(0.1, |_, _| {});
    let t = s.texts();
    assert_eq!(
        t.iter()
            .filter(|t| t.starts_with("track now"))
            .collect::<Vec<_>>(),
        ["track now lightly wet (was dry)"]
    );
}

/// Drives laps of 2 s that use `per_lap` litres.
fn laps(s: &mut Sim, n: usize, per_lap: f64) {
    for _ in 0..n {
        s.run(2.0, |p, _| {
            p.fuel_l = p.fuel_l.map(|f| f - per_lap * DT / 2.0);
        });
        s.next_lap();
        let t = s.t;
        s.frame.cars[0].last_lap_s = Some(90.0 + t / 1000.0);
    }
}

#[test]
fn low_fuel_warns_at_five_three_and_one_lap() {
    let mut s = Sim::new();
    laps(&mut s, 1, 2.0);
    let warned = |s: &Sim| {
        s.st.event_log()
            .last(500)
            .into_iter()
            .filter_map(|e| match e.kind {
                EventKind::FuelLow { laps } => Some(laps),
                _ => None,
            })
            .collect::<Vec<f64>>()
    };
    laps(&mut s, 12, 2.0);
    assert!(warned(&s).is_empty(), "{:?}", warned(&s));
    laps(&mut s, 7, 2.0);
    let w = warned(&s);
    assert_eq!(w.len(), 3, "{w:?}");
    for (got, limit) in w.iter().zip([5.5, 3.5, 1.5]) {
        assert!(*got < limit && *got > limit - 1.1, "{w:?}");
    }
    // Refuelled: warned again when it runs low again.
    s.p().fuel_l = Some(40.0);
    laps(&mut s, 2, 2.0);
    assert_eq!(warned(&s).len(), 3);
    s.p().fuel_l = Some(6.0);
    laps(&mut s, 1, 2.0);
    assert_eq!(warned(&s).len(), 4);
}

#[test]
fn enough_fuel_to_the_end_never_warns() {
    let mut s = Sim::new();
    s.frame.session.laps_remaining = Some(2);
    s.p().fuel_l = Some(12.0);
    laps(&mut s, 3, 2.0);
    assert!(!s.texts().iter().any(|t| t.starts_with("fuel for")));
}

#[test]
fn instant_fuel_is_not_taken_at_standstill_or_in_the_pits() {
    let mut s = Sim::new();
    s.p().fuel_use_lph = Some(2.5);
    s.p().speed_mps = Some(14.9);
    s.run(DT, |_, _| {});
    assert_eq!(s.st.fuel().unwrap().fuel.unwrap().basis, FuelBasis::None);
    s.p().speed_mps = Some(15.0);
    s.run(DT, |_, _| {});
    assert_eq!(s.st.fuel().unwrap().fuel.unwrap().basis, FuelBasis::Instant);
    s.frame.cars[0].on_pit_road = true;
    s.run(DT, |_, _| {});
    let f = s.st.fuel().unwrap().fuel.unwrap();
    assert_eq!(
        (f.basis, f.per_lap_l, f.laps_of_fuel),
        (FuelBasis::None, None, None)
    );
}

#[test]
fn a_restart_with_the_same_number_starts_over_and_is_kept() {
    let mut s = Sim::new();
    laps(&mut s, 4, 2.0);
    s.run(4.0, |_, _| {});
    assert!(s.st.take_finished().is_empty());
    s.frame.session.uid = Some(2);
    s.frame.cars[0] = car(1);
    s.t = 0.0;
    s.run(0.1, |_, _| {});
    let done = s.st.take_finished();
    assert_eq!(done.len(), 1);
    let r = &done[0];
    assert_eq!(r.id, "20261122-080740-race-test-ring");
    assert_eq!(r.uid, Some(1));
    assert_eq!(r.laps_completed, 4);
    assert_eq!(r.car.as_deref(), Some("SF23"));
    assert_eq!(r.standings.len(), 1);
    assert!(r.standings[0].is_player);
    assert!(s.st.take_finished().is_empty(), "taken once");
    assert!(s.st.laps(&LapsParams::default()).unwrap().laps.is_empty());
    // The new session has its own events.
    let new = SessionRecord::of(&s.st);
    assert_eq!(new, None, "nothing worth keeping yet");
}

#[test]
fn time_going_back_is_a_restart_too() {
    let mut s = Sim::new();
    s.run(70.0, |_, _| {});
    s.t = 65.1;
    s.run(0.1, |_, _| {});
    assert!(s.st.take_finished().is_empty(), "4.8 s back is a rewind");
    s.t = 10.0;
    s.run(0.1, |_, _| {});
    assert_eq!(s.st.take_finished().len(), 1);
}

#[test]
fn records_of_one_session_are_joined() {
    let mut a = Sim::new();
    laps(&mut a, 3, 2.0);
    a.run(4.0, |_, _| {});
    let mut old = SessionRecord::of(&a.st).unwrap();
    old.note = Some("wet setup".into());
    // The collector restarts and sees the rest only.
    let mut b = Sim::new();
    b.t = a.t;
    b.frame = a.frame.clone();
    b.run(0.1, |_, _| {});
    laps(&mut b, 2, 2.0);
    b.run(4.0, |_, _| {});
    let mut new = SessionRecord::of(&b.st).unwrap();
    assert!(new.same_session(&old));
    new.merge_older(&old);
    assert_eq!(new.id, old.id);
    assert_eq!(new.note.as_deref(), Some("wet setup"));
    let n: Vec<i32> = new.laps.iter().map(|l| l.lap).collect();
    assert_eq!(n, [1, 2, 3, 4, 5]);
    assert_eq!(new.laps_completed, 5);
    let times: Vec<i64> = new.events.iter().map(|e| e.timestamp_ms).collect();
    assert!(times.is_sorted(), "{times:?}");

    let mut other = old.clone();
    other.started_ms += 30_000;
    assert!(!new.same_session(&other));
    other.started_ms -= 1;
    assert!(new.same_session(&other));
    other.uid = Some(9);
    assert!(!new.same_session(&other));
}

#[test]
fn channels_info_and_setup_come_from_the_extras() {
    let mut s = Sim::new();
    s.run(DT, |_, _| {});
    assert_eq!(s.st.channels(&ChannelsParams::default()).unwrap().total, 0);
    assert_eq!(s.st.info(&InfoParams::default()).unwrap().value, None);
    let channel = |name: &str, unit: &str, description: &str| Channel {
        name: name.into(),
        unit: unit.into(),
        description: description.into(),
        value: 1.5.into(),
    };
    s.st.set_extras(Extras {
        channels: vec![
            channel("LFtempCL", "C", "LF tire left carcass temperature"),
            channel("WindVel", "m/s", "Wind velocity"),
        ],
        info: Some(serde_json::json!({ "A": { "B": [10, 20] } })),
        setup: Some(Setup {
            name: Some("suzuka.sto".into()),
            modified: Some(true),
            passed_tech: Some(false),
            items: vec![SetupItem {
                path: "Chassis/Front/ArbPosition".into(),
                value: "P2".into(),
            }],
            unavailable: Vec::new(),
        }),
    });
    let get = |f: &str| {
        s.st.channels(&ChannelsParams {
            filter: Some(f.into()),
        })
        .unwrap()
    };
    assert_eq!(get("").channels.len(), 2);
    let hit = get("TIRE temp");
    assert_eq!((hit.total, hit.channels.len()), (2, 1));
    assert!(
        hit.to_text()
            .contains("LFtempCL = 1.5 C  # LF tire left carcass temperature")
    );
    assert!(get("tire wind").channels.is_empty());

    let info = |p: &str| {
        s.st.info(&InfoParams {
            path: Some(p.into()),
        })
    };
    assert_eq!(info("A/B/1").unwrap().value, Some(20.into()));
    assert_eq!(
        info("/A/B/").unwrap().value,
        Some(serde_json::json!([10, 20]))
    );
    assert!(matches!(info("A/X"), Err(ApiError::NotFound(_))));
    assert!(matches!(info("A/B/2"), Err(ApiError::NotFound(_))));

    let text = s.st.setup().unwrap().to_text();
    assert!(
        text.contains("SF23 | setup suzuka.sto (modified since loaded) | FAILS TECH"),
        "{text}"
    );
    assert!(
        text.contains("Chassis/Front:\n  ArbPosition: P2\n"),
        "{text}"
    );
}

#[test]
fn the_brief_tells_tyres_push_to_pass_and_settings() {
    let mut s = Sim::new();
    let c = &mut s.p().car;
    c.p2p_count = Some(138);
    c.p2p_active = Some(true);
    c.tyre_compound = Some("Hard".into());
    c.tyres.lf.tread = [Some(0.926), Some(0.908), Some(0.909)];
    c.controls = vec![Control {
        id: "brake_bias".into(),
        value: 57.2519,
    }];
    c.pit.tow_s = Some(121.8);
    c.alongside = Some("left".into());
    s.run(DT, |_, _| {});
    let text = s.st.brief().unwrap().to_text();
    let want = "car: BEING TOWED 122s | Hard LF 91% (as of the last stop) | \
                P2P/OTS 138 left ACTIVE | car alongside: left | brake_bias 57.3\n";
    assert!(text.contains(want), "{text}");
}

#[test]
fn a_moment_without_laps_does_not_lose_them() {
    let mut s = Sim::new();
    laps(&mut s, 3, 2.0);
    s.run(4.0, |_, _| {});
    let numbers = |s: &Sim| {
        let l = s.st.laps(&LapsParams::default()).unwrap().laps;
        l.iter().map(|l| l.lap).collect::<Vec<_>>()
    };
    assert_eq!(numbers(&s), [1, 2, 3]);
    let before = s.frame.cars[0].clone();
    // Being towed: shown as no laps for a moment.
    s.frame.cars[0].lap = Some(0);
    s.frame.cars[0].laps_completed = Some(0);
    s.run(0.9, |_, _| {});
    s.frame.cars[0] = before;
    s.run(4.0, |_, _| {});
    assert_eq!(numbers(&s), [1, 2, 3], "neither lost nor added");
    let h: Vec<i32> = s.handling().laps.iter().map(|l| l.lap).collect();
    assert_eq!(h, [1, 2, 3]);

    // Really one lap less: the laps stay, and the next one counts.
    s.frame.cars[0].laps_completed = Some(2);
    s.run(1.2, |_, _| {});
    assert_eq!(numbers(&s), [1, 2, 3]);
    s.frame.cars[0].laps_completed = Some(3);
    s.frame.cars[0].last_lap_s = Some(101.5);
    s.run(0.1, |_, _| {});
    let l = s.st.laps(&LapsParams::default()).unwrap().laps;
    assert_eq!(l.last().unwrap().time_s, Some(101.5));
    assert_eq!(l.len(), 4);
}

/// The generated race shows the balance it was made with.
#[test]
fn generated_balance_is_seen() {
    use vr_replay::{Balance, SynthConfig, synth_frames};
    let mean = |balance| {
        let mut st = RaceState::new("test", SourceKind::Live, Capabilities::ALL);
        let cfg = SynthConfig {
            player_pit_lap: None,
            balance: Some(balance),
            ..Default::default()
        };
        for f in synth_frames(cfg, Some(620.0)) {
            st.ingest(f);
        }
        st.handling(&HandlingParams::default()).unwrap()
    };
    let none = mean(Balance::default()).mean.unwrap();
    assert!(
        none.understeer.total() + none.oversteer.total() < 0.2,
        "{none:?}"
    );
    assert!(none.wheelspin_s + none.rear_lock_s < 0.2, "{none:?}");

    let all = Balance {
        understeer_mid_s: 3.0,
        oversteer_exit_s: 1.0,
        rear_lock_s: 0.8,
        wheelspin_s: 1.2,
    };
    let r = mean(all);
    let m = r.mean.unwrap();
    let near = |a: f64, b: f64| (a - b).abs() <= 0.6;
    assert!(near(m.understeer.mid, 3.0), "{m:?}");
    assert!(
        near(m.oversteer.exit, 1.0) && m.oversteer.exit > 0.5,
        "{m:?}"
    );
    assert!(near(m.rear_lock_s, 0.8) && m.rear_lock_s > 0.4, "{m:?}");
    assert!(near(m.wheelspin_s, 1.2) && m.wheelspin_s > 0.8, "{m:?}");
    let bias = r.advice.iter().find(|a| a.target == "brake_bias").unwrap();
    assert!(bias.change.starts_with("forward"), "{bias:?}");
}

/// Round a circle with a radius of 100 m, 4 pi seconds a lap.
fn circle(s: &mut Sim, seconds: f64, mut each: impl FnMut(&mut Frame, f64)) {
    let lap_s = 4.0 * std::f64::consts::PI;
    let n = (seconds / DT).round() as usize;
    for _ in 0..n {
        s.t += DT;
        let laps = s.t / lap_s;
        let c = &mut s.frame.cars[0];
        c.lap = Some(laps as i32 + 1);
        c.lap_dist_pct = Some(laps.fract());
        let p = s.p();
        p.speed_mps = Some(50.0);
        p.gear = Some(3);
        p.throttle = Some(0.5);
        p.car.yaw_rate_rps = Some(0.5);
        p.car.velocity_x_mps = Some(50.0);
        p.car.velocity_y_mps = Some(0.0);
        p.car.steering_rad = Some(0.1);
        p.car.lat_accel_mps2 = Some(25.0);
        s.frame.session.time_s = s.t;
        s.frame.timestamp_ms += 16;
        let t = s.t;
        each(&mut s.frame, t);
        s.st.ingest(s.frame.clone());
    }
}

#[test]
fn a_lap_is_recorded_place_by_place() {
    let mut s = Sim::new();
    // Begins within a lap: that one is not whole.
    circle(&mut s, 30.0, |_, _| {});
    let laps = s.st.take_traces();
    assert_eq!(laps.len(), 1);
    assert!(s.st.take_traces().is_empty());
    let l = &laps[0];
    assert_eq!((l.lap, l.pit), (2, false));
    let lap_s = 4.0 * std::f64::consts::PI;
    assert!((l.time_s - lap_s).abs() < 0.001, "{}", l.time_s);
    assert!((l.length_m - 628.3).abs() < 0.5, "{}", l.length_m);
    for n in [
        l.t_ms.len(),
        l.speed.len(),
        l.x.len(),
        l.y.len(),
        l.rpm.len(),
    ] {
        assert_eq!(n, TRACE_POINTS);
    }
    assert_eq!(
        (l.t_ms[0], l.speed[500], l.throttle[500], l.gear[500]),
        (0, 1800, 50, 3)
    );
    assert_eq!((l.steer[500], l.lat[500], l.brake[500]), (57, 255, 0));
    assert!((f64::from(l.t_ms[500]) - lap_s * 500.0).abs() <= 1.0);
    // The circle, in decimetres, starting to the right.
    assert_eq!((l.x[0], l.y[0]), (0, 0));
    let width = l.x.iter().max().unwrap() - l.x.iter().min().unwrap();
    let height = l.y.iter().max().unwrap();
    assert!((width - 2000).abs() <= 3, "{width}");
    assert!((height - 2000).abs() <= 3, "{height}");
    assert!(l.x[10] > 0 && l.x[999] < 0 && l.x[999] > -10);
}

#[test]
fn laps_with_a_jump_or_a_stop_at_the_pits() {
    let mut s = Sim::new();
    // Towed within lap 3, through the pit lane in lap 4.
    circle(&mut s, 70.0, |f, t| {
        let lap = f.cars[0].lap.unwrap();
        if lap == 3 && (30.0..30.5).contains(&t) {
            f.cars[0].lap_dist_pct = Some(0.1);
        }
        f.cars[0].on_pit_road = lap == 4 && (45.0..46.0).contains(&t);
    });
    let laps = s.st.take_traces();
    let got: Vec<_> = laps.iter().map(|l| (l.lap, l.pit)).collect();
    assert_eq!(got, [(2, false), (4, true), (5, false)]);
}
