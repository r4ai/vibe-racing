//! Sector times, the gap as it is now and where time is lost, from cars that
//! drive each sector at an even speed.

use vr_core::*;
use vr_engine::*;

const STARTS: [f64; 3] = [0.0, 0.3, 0.7];

/// How a car drives: seconds per sector, and how late it passes the line.
#[derive(Clone)]
struct Drive {
    sectors: [f64; 3],
    late: f64,
}

impl Drive {
    fn lap(&self) -> f64 {
        self.sectors.iter().sum()
    }

    /// Laps completed and lap fraction at `t`.
    fn at(&self, t: f64) -> (i32, f64) {
        let t = t - self.late;
        let lap = (t / self.lap() + 1e-9).floor();
        let mut rest = (t - lap * self.lap()).max(0.0);
        for (i, s) in self.sectors.iter().enumerate() {
            let to = STARTS.get(i + 1).copied().unwrap_or(1.0);
            if rest < s - 1e-9 {
                return (lap as i32, STARTS[i] + (to - STARTS[i]) * rest / s);
            }
            rest -= s;
        }
        (lap as i32 + 1, 0.0)
    }
}

fn frame(t: f64, drives: &[Drive], sectors: &[f64]) -> Frame {
    let mut order: Vec<usize> = (0..drives.len()).collect();
    let progress = |i: &usize| {
        let (l, p) = drives[*i].at(t);
        f64::from(l) + p
    };
    order.sort_by(|a, b| progress(b).total_cmp(&progress(a)));
    let cars = drives.iter().enumerate().map(|(i, d)| {
        let (laps, pct) = d.at(t);
        let pos = order.iter().position(|o| *o == i).unwrap() as u32 + 1;
        CarState {
            idx: i,
            number: format!("{}", i + 10),
            driver: format!("Driver {i}"),
            team: None,
            car_name: None,
            class_name: None,
            class_id: Some(1),
            rating: None,
            is_player: i == 0,
            position: Some(pos),
            class_position: Some(pos),
            lap: Some(laps + 1),
            laps_completed: Some(laps),
            lap_dist_pct: Some(pct),
            on_pit_road: false,
            surface: TrackSurface::OnTrack,
            last_lap_s: (laps > 0).then(|| d.lap()),
            best_lap_s: (laps > 0).then(|| d.lap()),
            gap_to_leader_s: None,
            est_lap_s: Some(100.0),
            extra: Default::default(),
        }
    });
    Frame {
        game: "test".into(),
        tick: 0,
        timestamp_ms: 0,
        session: SessionState {
            session_num: 0,
            kind: SessionKind::Race,
            phase: SessionPhase::Racing,
            time_s: t,
            time_remaining_s: None,
            laps_remaining: Some(20),
            laps_total: None,
            flags: vec![Flag::Green],
            track: Some(TrackInfo {
                name: "Ring".into(),
                config: None,
                length_m: Some(5000.0),
                sectors: sectors.to_vec(),
            }),
            uid: None,
            time_total_s: None,
            time_of_day_s: None,
        },
        player_car_idx: Some(0),
        player: Some(PlayerTelemetry {
            on_track: true,
            ..Default::default()
        }),
        cars: cars.collect(),
        weather: None,
    }
}

fn state() -> RaceState {
    RaceState::new("test", SourceKind::Live, Capabilities::ALL)
}

/// Ten frames a second from 1 s to `until`.
fn drive(drives: &[Drive], sectors: &[f64], until: f64) -> RaceState {
    let mut st = state();
    for i in 10..=(until * 10.0).round() as i64 {
        st.ingest(frame(i as f64 / 10.0, drives, sectors));
    }
    st
}

/// The player is 4 s behind a car that is faster in the second sector.
fn pair() -> Vec<Drive> {
    vec![
        Drive {
            sectors: [30.0, 40.0, 30.0],
            late: 4.0,
        },
        Drive {
            sectors: [30.5, 39.0, 30.2],
            late: 0.0,
        },
    ]
}

fn close(a: Option<f64>, b: f64) {
    assert!(a.is_some_and(|a| (a - b).abs() < 1e-6), "{a:?} != {b}");
}

#[test]
fn sector_times_of_all_cars_come_from_their_positions() {
    let st = drive(&pair(), &STARTS, 250.0);
    let r = st.sectors(&SectorsParams::default()).unwrap();
    assert_eq!(r.basis, SectorBasis::GameLines);
    assert_eq!(r.starts, STARTS);
    let roles: Vec<(&str, usize)> = r
        .rows
        .iter()
        .map(|r| (r.role.as_str(), r.car.idx))
        .collect();
    assert_eq!(roles, [("you", 0), ("ahead", 1)]);
    let me = &r.rows[0];
    assert_eq!(me.lap, Some(2));
    for (t, want) in me.last.iter().zip([30.0, 40.0, 30.0]) {
        close(*t, want);
    }
    for (t, want) in r.rows[1].last.iter().zip([30.5, 39.0, 30.2]) {
        close(*t, want);
    }
    for (t, want) in me.best.iter().zip([30.0, 40.0, 30.0]) {
        close(*t, want);
    }
    // 46 s into the lap: one sector done.
    close(me.current[0], 30.0);
    assert_eq!(me.current[1..], [None, None]);
    assert_eq!(r.versus.as_ref().unwrap().idx, 1);
    for (d, want) in r.delta.iter().zip([-0.5, 1.0, -0.2]) {
        close(*d, want);
    }
}

#[test]
fn losses_and_gains_are_found_along_the_lap() {
    let st = drive(&pair(), &STARTS, 250.0);
    let r = st.sectors(&SectorsParams::default()).unwrap();
    // A second over 40 % of the lap: an eighth of it per 5 %.
    assert_eq!(r.losses.len(), 3);
    for l in &r.losses {
        assert_eq!(l.sector, 2);
        close(Some(l.lost_s), 0.125);
        close(Some(l.to - l.from), 0.05);
        close(l.from_m, 5000.0 * l.from);
        close(l.to_m, 5000.0 * l.to);
    }
    assert_eq!(r.gains.len(), 3);
    assert!(r.gains.iter().all(|g| g.sector == 1), "{:?}", r.gains);
    close(Some(r.gains[0].lost_s), -0.5 / 6.0);
    let text = r.to_text();
    assert!(text.contains("derived from car positions; the game's sector lines"));
    assert!(
        text.contains("you: P2 #10 Driver 0 last 30.000 / 40.000 / 30.000"),
        "{text}"
    );
    assert!(
        text.contains(
            "you vs #11 Driver 1 (last lap, + is slower): S1 -0.50s / S2 +1.00s / S3 -0.20s"
        ),
        "{text}"
    );
    assert!(text.contains("lost most: S2 "), "{text}");
    assert!(text.contains("won most: S1 "), "{text}");
}

#[test]
fn the_best_sectors_may_come_from_different_laps() {
    let mut st = state();
    let slow_start = Drive {
        sectors: [33.0, 37.0, 30.0],
        late: 0.0,
    };
    let slow_middle = Drive {
        sectors: [30.0, 40.0, 30.0],
        late: 0.0,
    };
    for i in 10..=3500 {
        let t = i as f64 / 10.0;
        // Both take 100 s: the second lap is driven the other way.
        let d = if t < 200.0 { &slow_start } else { &slow_middle };
        st.ingest(frame(t, std::slice::from_ref(d), &STARTS));
    }
    let r = st.sectors(&SectorsParams::default()).unwrap();
    for (t, want) in r.rows[0].best.iter().zip([30.0, 37.0, 30.0]) {
        close(*t, want);
    }
    for (t, want) in r.rows[0].last.iter().zip([30.0, 40.0, 30.0]) {
        close(*t, want);
    }
    assert!(r.versus.is_none() && r.delta.is_empty() && r.losses.is_empty());
}

#[test]
fn without_sector_lines_there_are_thirds() {
    let even = vec![Drive {
        sectors: [30.0, 40.0, 30.0],
        late: 0.0,
    }];
    for lines in [&[][..], &[0.0], &[0.2, 0.6], &[f64::NAN, 2.0]] {
        let st = drive(&even, lines, 250.0);
        let r = st.sectors(&SectorsParams::default()).unwrap();
        assert_eq!(r.basis, SectorBasis::Thirds, "{lines:?}");
        assert_eq!(r.starts, [0.0, 1.0 / 3.0, 2.0 / 3.0]);
        let sum: f64 = r.rows[0].last.iter().flatten().sum();
        close(Some(sum), 100.0);
        assert!(r.to_text().contains("thirds of the lap"));
    }
}

#[test]
fn no_sectors_before_a_lap_from_line_to_line() {
    let r = state().sectors(&SectorsParams::default()).unwrap();
    assert_eq!(r.basis, SectorBasis::None);
    assert!(r.to_text().contains("sectors: unavailable"));
    // Out on track, but not over the line yet.
    let st = drive(&pair()[..1], &STARTS, 50.0);
    // The first lap began at 4 s.
    let r = st.sectors(&SectorsParams::default()).unwrap();
    assert_eq!(r.rows[0].last, [None, None, None]);
    close(r.rows[0].current[0], 30.0);
    let st = drive(&pair()[..1], &STARTS, 3.0);
    assert_eq!(
        st.sectors(&SectorsParams::default()).unwrap().basis,
        SectorBasis::None
    );
}

#[test]
fn a_jump_breaks_the_lap_but_not_the_best() {
    let drives = [Drive {
        sectors: [30.0, 40.0, 30.0],
        late: 0.0,
    }];
    let mut st = drive(&drives, &STARTS, 250.0);
    let mut f = frame(250.1, &drives, &STARTS);
    f.cars[0].lap_dist_pct = Some(0.95);
    st.ingest(f);
    let mut f = frame(250.2, &drives, &STARTS);
    f.cars[0].surface = TrackSurface::NotInWorld;
    st.ingest(f);
    let r = st.sectors(&SectorsParams::default()).unwrap();
    assert_eq!(r.rows[0].current, [None, None, None]);
    close(r.rows[0].best[1], 40.0);
    // Neither does a hole in the data.
    let mut st = drive(&drives, &STARTS, 250.0);
    st.ingest(frame(251.1, &drives, &STARTS));
    let r = st.sectors(&SectorsParams::default()).unwrap();
    assert_eq!(r.rows[0].current, [None, None, None]);
    let mut st = drive(&drives, &STARTS, 250.0);
    st.ingest(frame(251.0, &drives, &STARTS));
    let r = st.sectors(&SectorsParams::default()).unwrap();
    close(r.rows[0].current[0], 30.0);
}

#[test]
fn a_lap_through_the_pits_sets_no_best() {
    let drives = [Drive {
        sectors: [30.0, 40.0, 30.0],
        late: 0.0,
    }];
    let mut st = state();
    for i in 10..=2500 {
        let t = i as f64 / 10.0;
        let mut f = frame(t, &drives, &STARTS);
        f.cars[0].on_pit_road = (150.0..160.0).contains(&t);
        st.ingest(f);
    }
    let r = st.sectors(&SectorsParams::default()).unwrap();
    assert_eq!(r.rows[0].best, [None, None, None]);
    close(r.rows[0].last[1], 40.0);
}

#[test]
fn the_gap_is_that_of_this_moment() {
    let st = drive(&pair(), &STARTS, 250.4);
    let g = st.brief().unwrap().ahead.unwrap();
    // In the middle of the player's third lap: 4 s late, 0.3 s slower per lap
    // twice, then half a second won and some lost again.
    let at = |d: &Drive, pct: f64| -> f64 {
        // When the third lap reaches `pct` of the second sector.
        d.late + 2.0 * d.lap() + d.sectors[0] + d.sectors[1] * (pct - 0.3) / 0.4
    };
    let p = pair();
    // The player is at 46.4 %, and has passed 46 % last.
    let gate = 0.46;
    close(g.live_s, at(&p[0], gate) - at(&p[1], gate));
    close(g.live_change_s, 0.3);
    assert_eq!(g.live_ahead, Some(true));
    let text = st.brief().unwrap().to_text();
    assert!(
        text.contains(" | now 4.50s (+0.30s since a lap ago)"),
        "{text}"
    );
}

#[test]
fn the_running_order_is_ahead_of_the_official_one() {
    let st = {
        let mut st = drive(&pair(), &STARTS, 250.0);
        // The places still say that the car behind is ahead.
        let mut f = frame(250.1, &pair(), &STARTS);
        f.cars[0].class_position = Some(1);
        f.cars[0].position = Some(1);
        f.cars[1].class_position = Some(2);
        f.cars[1].position = Some(2);
        st.ingest(f);
        st
    };
    let b = st.brief().unwrap();
    assert_eq!(b.player.as_ref().unwrap().live_position, Some(2));
    assert!(b.ahead.is_none());
    let g = b.running_ahead.as_ref().unwrap();
    assert_eq!((g.car.idx, g.live_ahead), (1, Some(true)));
    assert!(b.running_behind.is_none());
    let behind = b.behind.as_ref().unwrap();
    assert_eq!((behind.car.idx, behind.live_ahead), (1, Some(true)));
    let text = b.to_text();
    assert!(text.contains("running P2 on track now"), "{text}");
    assert!(text.contains("AHEAD of you on track"), "{text}");
    assert!(text.contains("running ahead now: P2 #11"), "{text}");
    assert_eq!(st.live().unwrap().values["live_position"], 2.0);
    // The same as the places: nothing more to tell.
    let b = drive(&pair(), &STARTS, 250.0).brief().unwrap();
    assert!(b.running_ahead.is_none() && b.running_behind.is_none());
    assert_eq!(b.player.unwrap().live_position, Some(2));
}

#[test]
fn the_leader_is_told_when_it_is_not_the_car_ahead() {
    let mut drives = pair();
    drives.push(Drive {
        sectors: [29.0, 39.0, 29.0],
        late: 0.0,
    });
    let st = drive(&drives, &STARTS, 250.0);
    let b = st.brief().unwrap();
    assert_eq!(b.ahead.as_ref().unwrap().car.idx, 1);
    let l = b.leader.as_ref().unwrap();
    assert_eq!(l.car.idx, 2);
    assert!(b.to_text().contains("leader: P1 #12"));
    let v = st.live().unwrap().values;
    assert_eq!(v["leader_lap"], 97.0);
    assert_eq!(v["ahead_lap"], 99.7);
    let r = st
        .sectors(&SectorsParams {
            car: Some("#12".into()),
        })
        .unwrap();
    let roles: Vec<&str> = r.rows.iter().map(|r| r.role.as_str()).collect();
    assert_eq!(roles, ["you", "asked", "ahead"]);
    assert_eq!(r.versus.unwrap().idx, 2);
    for (d, want) in r.delta.iter().zip([1.0, 1.0, 1.0]) {
        close(*d, want);
    }
    assert!(
        st.sectors(&SectorsParams {
            car: Some("99".into())
        })
        .is_err()
    );
    // Second: the car ahead is the leader.
    let b = drive(&pair(), &STARTS, 250.0).brief().unwrap();
    assert!(b.leader.is_none());
}

#[test]
fn cars_beside_are_counted_by_side() {
    let drives = pair();
    for (told, want) in [
        (None, (0.0, 0.0)),
        (Some("left"), (1.0, 0.0)),
        (Some("right"), (0.0, 1.0)),
        (Some("left and right"), (1.0, 1.0)),
        (Some("two on the left"), (2.0, 0.0)),
        (Some("two on the right"), (0.0, 2.0)),
    ] {
        let mut st = state();
        let mut f = frame(1.0, &drives, &STARTS);
        f.player.as_mut().unwrap().car.alongside = told.map(String::from);
        st.ingest(f);
        let v = st.live().unwrap().values;
        assert_eq!((v["left"], v["right"]), want, "{told:?}");
        assert_eq!(v["alongside"], f64::from(u8::from(told.is_some())));
    }
}

#[test]
fn the_distance_to_the_cars_around_is_told_in_metres() {
    let length = frame(1.0, &pair(), &STARTS).session.track.unwrap().length_m;
    let length = length.unwrap();
    let near = |at: &[f64], change: &dyn Fn(&mut Frame)| {
        let mut drives = pair();
        drives.resize(at.len(), drives[0].clone());
        let mut f = frame(1.0, &drives, &STARTS);
        for (c, pct) in f.cars.iter_mut().zip(at) {
            c.lap_dist_pct = Some(*pct);
        }
        change(&mut f);
        let mut st = state();
        st.ingest(f);
        let v = st.live().unwrap().values;
        let share = |k: &str| v.get(k).map(|m| (m / length * 1000.0).round() / 1000.0);
        (share("near_ahead_m"), share("near_behind_m"))
    };
    let same = |_: &mut Frame| {};
    // The nearest of each side, also over the line.
    assert_eq!(
        near(&[0.5, 0.53, 0.51, 0.48, 0.4], &same),
        (Some(0.01), Some(0.02))
    );
    assert_eq!(near(&[0.99, 0.01, 0.97], &same), (Some(0.02), Some(0.02)));
    assert_eq!(near(&[0.01, 0.99], &same), (None, Some(0.02)));
    // Exactly beside counts as ahead.
    assert_eq!(near(&[0.5, 0.5], &same), (Some(0.0), None));
    // Cars in the pits or nowhere are not around.
    assert_eq!(
        near(&[0.5, 0.51, 0.49], &|f| {
            f.cars[1].on_pit_road = true;
            f.cars[2].surface = TrackSurface::NotInWorld;
        }),
        (None, None)
    );
    // From the pit lane those in it are.
    assert_eq!(
        near(&[0.5, 0.51], &|f| {
            f.cars[0].on_pit_road = true;
            f.cars[1].on_pit_road = true;
        }),
        (Some(0.01), None)
    );
    let blind = |f: &mut Frame| f.session.track.as_mut().unwrap().length_m = None;
    assert_eq!(near(&[0.5, 0.51], &blind), (None, None));
}

#[test]
fn the_race_ends_as_the_pace_says() {
    let car = |lap: f64, late: f64| Drive {
        sectors: [lap * 0.3, lap * 0.4, lap * 0.3],
        late,
    };
    // Second of three: the leader is slower, the car behind much quicker.
    let drives = [car(90.0, 5.0), car(90.5, 0.0), car(88.0, 25.0)];
    let mut st = state();
    let mut t = 0.0;
    while t <= 460.0 {
        let mut f = frame(t, &drives, &STARTS);
        f.session.laps_total = Some(15);
        f.session.laps_remaining = Some(10);
        f.player.as_mut().unwrap().fuel_l = Some(50.0);
        st.ingest(f);
        t += 0.5;
    }
    let r = st.forecast().unwrap();
    assert_eq!(r.position, Some(2));
    let left = r.laps_left.unwrap();
    let pace = r.pace_s.unwrap();
    assert!((pace - 90.0).abs() < 0.01, "{r:?}");
    assert_eq!(r.cars.len(), 2);
    let (leader, chaser) = (&r.cars[0], &r.cars[1]);
    assert!(leader.gap_s > 0.0 && chaser.gap_s < 0.0, "{r:?}");
    assert!((leader.slower_s.unwrap() - 0.5).abs() < 0.01);
    assert!((chaser.slower_s.unwrap() + 2.0).abs() < 0.01);
    let flag = leader.gap_s - leader.slower_s.unwrap() * left;
    assert!((leader.at_flag_s.unwrap() - flag).abs() < 1e-9);
    let meets = chaser.gap_s / chaser.slower_s.unwrap();
    assert!((chaser.meets_in_laps.unwrap() - meets).abs() < 1e-9);
    assert!(meets > 0.0 && meets < left);
    assert_eq!(r.next(true), Some(leader));
    assert_eq!(r.next(false), Some(chaser));
    // The leader is caught, the chaser comes past.
    assert!(leader.at_flag_s.unwrap() < 0.0 && chaser.at_flag_s.unwrap() > 0.0);
    assert_eq!(r.finish, Some(2));
    let text = r.to_text();
    assert!(
        text.contains("now P2, ") && text.contains("finish P2 | your pace 1:30."),
        "{text}"
    );
    assert!(text.contains("meets in "), "{text}");
    // Nothing to tell before there are laps, or outside a race.
    let mut st = state();
    st.ingest(frame(1.0, &drives, &STARTS));
    let r = st.forecast().unwrap();
    assert_eq!((r.pace_s, r.finish), (None, None));
    let mut f = frame(1.0, &drives, &STARTS);
    f.session.kind = SessionKind::Practice;
    st.ingest(f);
    let r = st.forecast().unwrap();
    assert_eq!((r.position, r.cars.len()), (None, 0));
    assert!(r.to_text().contains("forecast: unavailable"));
}
