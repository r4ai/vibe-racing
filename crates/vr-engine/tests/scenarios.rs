//! Hand-built frames for the paths the synthetic race does not reach:
//! practice / qualifying, multi-class, timed races, fallbacks and resets.

use vr_core::*;
use vr_engine::*;

fn car(idx: usize, pos: u32, laps: i32, pct: f64) -> CarState {
    CarState {
        idx,
        number: format!("{}", idx + 10),
        driver: format!("Driver {idx}"),
        team: None,
        car_name: None,
        class_name: Some("GT3".into()),
        class_id: Some(1),
        rating: None,
        is_player: false,
        position: Some(pos),
        class_position: Some(pos),
        lap: Some(laps + 1),
        laps_completed: Some(laps),
        lap_dist_pct: Some(pct),
        on_pit_road: false,
        surface: TrackSurface::OnTrack,
        last_lap_s: Some(100.0),
        best_lap_s: Some(99.0),
        gap_to_leader_s: None,
        est_lap_s: Some(100.0),
        extra: Default::default(),
    }
}

fn frame(kind: SessionKind, t: f64, cars: Vec<CarState>, player: usize) -> Frame {
    Frame {
        game: "test".into(),
        tick: t as u64,
        timestamp_ms: (t * 1000.0) as i64,
        session: SessionState {
            session_num: 0,
            kind,
            phase: SessionPhase::Racing,
            time_s: t,
            time_remaining_s: None,
            laps_remaining: None,
            laps_total: None,
            flags: vec![Flag::Green],
            track: Some(TrackInfo {
                name: "Test Ring".into(),
                config: None,
                length_m: None,
            }),
            uid: None,
            time_total_s: None,
            time_of_day_s: None,
        },
        player_car_idx: Some(player),
        player: Some(PlayerTelemetry {
            on_track: true,
            fuel_l: Some(20.0),
            ..Default::default()
        }),
        cars,
        weather: None,
    }
}

fn state() -> RaceState {
    RaceState::new("test", SourceKind::Live, Capabilities::ALL)
}

#[test]
fn practice_gap_is_best_lap_delta() {
    let mut a = car(0, 1, 3, 0.2);
    a.best_lap_s = Some(98.5);
    let mut me = car(1, 2, 3, 0.9);
    me.best_lap_s = Some(99.25);
    let mut c = car(2, 3, 3, 0.5);
    c.best_lap_s = Some(100.0);
    let mut st = state();
    st.ingest(frame(SessionKind::Practice, 10.0, vec![a, me, c], 1));
    let b = st.brief().unwrap();
    let ahead = b.ahead.as_ref().unwrap();
    assert_eq!(ahead.mode, GapMode::BestLap);
    assert!((ahead.gap_s.unwrap() - 0.75).abs() < 1e-9);
    assert!((b.behind.as_ref().unwrap().gap_s.unwrap() - 0.75).abs() < 1e-9);
    assert!(b.to_text().contains("best-lap delta"));
    // Position changes outside races are not events.
    assert!(
        !st.event_log()
            .last(50)
            .iter()
            .any(|e| matches!(e.kind, EventKind::PositionChanged { .. }))
    );
}

#[test]
fn race_gap_falls_back_to_track_progress() {
    let a = car(0, 1, 5, 0.50);
    let me = car(1, 2, 5, 0.40);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 10.0, vec![a, me], 1));
    let g = st.brief().unwrap().ahead.unwrap();
    assert_eq!(g.mode, GapMode::TrackProgress);
    // 0.1 lap × 100 s reference lap (last lap).
    assert!((g.gap_s.unwrap() - 10.0).abs() < 1e-6, "{g:?}");
    assert_eq!(g.laps, 0);
    assert!(st.brief().unwrap().to_text().contains("(est)"));
}

#[test]
fn lapped_gap_reports_whole_laps() {
    let mut a = car(0, 1, 7, 0.6);
    a.gap_to_leader_s = Some(0.0);
    let mut me = car(1, 2, 5, 0.5);
    me.gap_to_leader_s = Some(210.0);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 10.0, vec![a, me], 1));
    let b = st.brief().unwrap();
    let g = b.ahead.as_ref().unwrap();
    assert_eq!(g.laps, 2);
    assert_eq!(g.mode, GapMode::RaceTime);
    assert!(b.to_text().contains("+2L"), "{}", b.to_text());
}

#[test]
fn leader_has_no_car_ahead() {
    let mut me = car(0, 1, 5, 0.5);
    me.gap_to_leader_s = Some(0.0);
    let mut b = car(1, 2, 5, 0.45);
    b.gap_to_leader_s = Some(5.0);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 10.0, vec![me, b], 0));
    let br = st.brief().unwrap();
    assert!(br.ahead.is_none());
    assert!((br.behind.as_ref().unwrap().gap_s.unwrap() - 5.0).abs() < 1e-9);
    assert!(br.to_text().contains("ahead: none (leading)"));
    let g = st.gaps(&GapParams::default()).unwrap();
    assert!(g.to_text().contains("ahead: none"));
}

#[test]
fn multi_class_scopes_to_player_class() {
    let mut proto = car(0, 1, 5, 0.9);
    proto.class_id = Some(2);
    proto.class_name = Some("LMP2".into());
    proto.class_position = Some(1);
    let mut gt_lead = car(1, 2, 5, 0.6);
    gt_lead.class_position = Some(1);
    let mut me = car(2, 3, 5, 0.55);
    me.class_position = Some(2);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 10.0, vec![proto, gt_lead, me], 2));

    let b = st.brief().unwrap();
    assert_eq!(
        b.ahead.as_ref().unwrap().car.idx,
        1,
        "ahead is the GT3 leader, not the prototype"
    );
    let text = b.to_text();
    assert!(text.contains("you: class GT3 P2/2 (overall P3/3"), "{text}");

    let s = st.standings(&StandingsParams::default()).unwrap();
    assert!(s.class_only);
    assert_eq!(s.rows.len(), 2);
    assert!(s.to_text().starts_with("[test live"), "{}", s.to_text());
    assert!(s.to_text().contains("class standings:"));
    let all = st
        .standings(&StandingsParams {
            class_only: Some(false),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(all.rows.len(), 3);
    assert!(all.to_text().contains("\nstandings:"));
}

#[test]
fn nearby_marks_other_class_and_lapped_cars() {
    let mut me = car(0, 1, 10, 0.50);
    me.class_position = Some(1);
    // Other class car 1 s ahead on track, a lap up.
    let mut fast = car(1, 2, 11, 0.51);
    fast.class_id = Some(2);
    fast.class_name = Some("LMP2".into());
    // Same class car 2 s behind on track, a lap down.
    let slow = car(2, 3, 9, 0.48);
    // Far away car is excluded.
    let far = car(3, 4, 10, 0.9);
    // Car in the pits is excluded.
    let mut pit = car(4, 5, 10, 0.505);
    pit.on_pit_road = true;
    let mut st = state();
    st.ingest(frame(
        SessionKind::Race,
        10.0,
        vec![me, fast, slow, far, pit],
        0,
    ));
    let g = st.gaps(&GapParams::default()).unwrap();
    assert_eq!(g.nearby.len(), 2, "{:?}", g.nearby);
    let fast = g.nearby.iter().find(|r| r.car.idx == 1).unwrap();
    assert!(!fast.same_class);
    assert_eq!(fast.lap_diff, 1);
    assert!((fast.rel_s - 1.0).abs() < 1e-6);
    let slow = g.nearby.iter().find(|r| r.car.idx == 2).unwrap();
    assert_eq!(slow.lap_diff, -1);
    assert!((slow.rel_s + 2.0).abs() < 1e-6);
    let text = g.to_text();
    assert!(text.contains("[other class LMP2]"), "{text}");
    assert!(text.contains("[lapping you +1L]"), "{text}");
    assert!(text.contains("[lapped car -1L]"), "{text}");
    // Narrow window.
    let g = st
        .gaps(&GapParams {
            car: None,
            window_s: Some(1.5),
        })
        .unwrap();
    assert_eq!(g.nearby.len(), 1);
}

#[test]
fn nearby_empty_text() {
    let me = car(0, 1, 10, 0.5);
    let far = car(1, 2, 10, 0.9);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 1.0, vec![me, far], 0));
    assert!(
        st.gaps(&GapParams::default())
            .unwrap()
            .to_text()
            .contains("nobody close")
    );
}

#[test]
fn fuel_instant_estimate_and_timed_race() {
    let me = car(0, 1, 2, 0.25);
    let mut f = frame(SessionKind::Race, 10.0, vec![me], 0);
    f.session.time_remaining_s = Some(450.0);
    let p = f.player.as_mut().unwrap();
    p.fuel_l = Some(50.0);
    p.fuel_use_lph = Some(72.0); // 2 L per 100 s lap
    p.fuel_capacity_l = Some(100.0);
    let mut st = state();
    st.ingest(f);
    let fuel = st.fuel().unwrap().fuel.unwrap();
    assert_eq!(fuel.basis, FuelBasis::Instant);
    assert!((fuel.per_lap_l.unwrap() - 2.0).abs() < 1e-9);
    assert!((fuel.laps_of_fuel.unwrap() - 25.0).abs() < 1e-9);
    // 0.25 + 4.5 laps of time -> finish line after 5 laps: 4.75 laps to drive.
    assert!(
        (fuel.laps_to_finish.unwrap() - 4.75).abs() < 1e-9,
        "{fuel:?}"
    );
    assert!((fuel.to_finish_l.unwrap() - (9.5 - 50.0)).abs() < 1e-9);
    assert_eq!(fuel.add_with_margin_l, Some(0.0));
    assert_eq!(fuel.last_lap_on_fuel, Some(2 + 25));
    let text = st.fuel().unwrap().to_text();
    assert!(text.contains("instant estimate"), "{text}");
    assert!(text.contains("enough, surplus 40.5L"), "{text}");
    assert!(text.contains("/100L"), "{text}");
}

#[test]
fn fuel_unknown_without_consumption() {
    let me = car(0, 1, 2, 0.25);
    let mut f = frame(SessionKind::Race, 10.0, vec![me], 0);
    f.player.as_mut().unwrap().fuel_use_lph = None;
    let mut st = state();
    st.ingest(f);
    let fuel = st.fuel().unwrap().fuel.unwrap();
    assert_eq!(fuel.basis, FuelBasis::None);
    assert!(fuel.laps_of_fuel.is_none());
    assert!(
        st.fuel()
            .unwrap()
            .to_text()
            .contains("per-lap usage unknown")
    );
}

#[test]
fn laps_to_finish_respects_phase_and_lap_count() {
    let me = car(0, 1, 2, 0.25);
    let mut f = frame(SessionKind::Race, 10.0, vec![me.clone()], 0);
    f.session.laps_remaining = Some(3);
    f.player.as_mut().unwrap().fuel_use_lph = Some(36.0);
    let mut st = state();
    st.ingest(f.clone());
    assert!((st.fuel().unwrap().fuel.unwrap().laps_to_finish.unwrap() - 2.75).abs() < 1e-9);

    f.session.phase = SessionPhase::Checkered;
    st.ingest(f.clone());
    assert_eq!(st.fuel().unwrap().fuel.unwrap().laps_to_finish, Some(0.0));
    f.session.phase = SessionPhase::CoolDown;
    st.ingest(f);
    assert_eq!(st.fuel().unwrap().fuel.unwrap().laps_to_finish, Some(0.0));
}

#[test]
fn pending_lap_waits_for_new_time_then_times_out() {
    let mut me = car(0, 1, 1, 0.99);
    me.last_lap_s = Some(100.0);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 100.0, vec![me.clone()], 0));
    // Crossing the line, the game has not published the new lap time yet.
    me.laps_completed = Some(2);
    me.lap_dist_pct = Some(0.01);
    st.ingest(frame(SessionKind::Race, 101.0, vec![me.clone()], 0));
    assert!(st.laps(&LapsParams::default()).unwrap().laps.is_empty());
    // New time arrives.
    me.last_lap_s = Some(99.5);
    st.ingest(frame(SessionKind::Race, 101.5, vec![me.clone()], 0));
    let laps = st.laps(&LapsParams::default()).unwrap().laps;
    assert_eq!(laps.len(), 1);
    assert_eq!(laps[0].lap, 2);
    assert_eq!(laps[0].time_s, Some(99.5));

    // Next crossing with an identical lap time: accepted after the timeout.
    me.laps_completed = Some(3);
    st.ingest(frame(SessionKind::Race, 201.0, vec![me.clone()], 0));
    st.ingest(frame(SessionKind::Race, 203.0, vec![me.clone()], 0));
    assert_eq!(
        st.laps(&LapsParams::default()).unwrap().laps.len(),
        1,
        "still waiting at 2 s"
    );
    st.ingest(frame(SessionKind::Race, 204.5, vec![me.clone()], 0));
    let laps = st.laps(&LapsParams::default()).unwrap().laps;
    assert_eq!(laps.len(), 2);
    assert_eq!(laps[1].time_s, Some(99.5));
}

#[test]
fn caution_laps_are_excluded_from_pace() {
    let mut me = car(0, 1, 1, 0.9);
    me.last_lap_s = None;
    let mut st = state();
    let mut t = 0.0;
    for (lap, time, yellow) in [(2, 100.0, false), (3, 130.0, true), (4, 102.0, false)] {
        t += time;
        let mut f = frame(SessionKind::Race, t - 1.0, vec![me.clone()], 0);
        if yellow {
            f.session.flags = vec![Flag::Caution];
        }
        st.ingest(f);
        me.laps_completed = Some(lap);
        me.last_lap_s = Some(time);
        st.ingest(frame(SessionKind::Race, t, vec![me.clone()], 0));
    }
    let r = st.laps(&LapsParams::default()).unwrap();
    assert_eq!(r.laps.len(), 3);
    assert!(r.laps[1].caution);
    assert!(!r.laps[1].is_clean());
    assert!((r.avg_clean_lap_s.unwrap() - 101.0).abs() < 1e-9);
    assert_eq!(r.best_lap_s, Some(100.0));
    let text = r.to_text();
    assert!(text.contains("caution"), "{text}");
}

#[test]
fn fuel_used_none_after_refuel() {
    let mut me = car(0, 1, 1, 0.9);
    let mut st = state();
    let mut f = frame(SessionKind::Race, 1.0, vec![me.clone()], 0);
    f.player.as_mut().unwrap().fuel_l = Some(10.0);
    st.ingest(f);
    me.laps_completed = Some(2);
    me.last_lap_s = Some(101.0);
    let mut f = frame(SessionKind::Race, 2.0, vec![me.clone()], 0);
    f.player.as_mut().unwrap().fuel_l = Some(8.0);
    st.ingest(f);
    me.laps_completed = Some(3);
    me.last_lap_s = Some(102.0);
    let mut f = frame(SessionKind::Race, 3.0, vec![me], 0);
    f.player.as_mut().unwrap().fuel_l = Some(40.0);
    st.ingest(f);
    let laps = st.laps(&LapsParams::default()).unwrap().laps;
    assert_eq!(laps[0].fuel_used_l, Some(2.0));
    assert_eq!(laps[1].fuel_used_l, None);
}

#[test]
fn session_change_resets_history_and_logs_events() {
    let mut me = car(0, 1, 1, 0.9);
    let mut st = state();
    st.ingest(frame(SessionKind::Practice, 1.0, vec![me.clone()], 0));
    me.laps_completed = Some(2);
    me.last_lap_s = Some(90.0);
    st.ingest(frame(SessionKind::Practice, 2.0, vec![me.clone()], 0));
    assert_eq!(st.laps(&LapsParams::default()).unwrap().laps.len(), 1);

    let mut f = frame(SessionKind::Race, 0.5, vec![car(0, 1, 0, 0.1)], 0);
    f.session.session_num = 1;
    f.session.phase = SessionPhase::GetInCar;
    st.ingest(f.clone());
    assert!(st.laps(&LapsParams::default()).unwrap().laps.is_empty());
    f.session.phase = SessionPhase::Racing;
    f.session.flags = vec![Flag::Green, Flag::Blue];
    st.ingest(f);
    let ev = st.event_log().last(20);
    assert!(ev.iter().any(|e| matches!(
        e.kind,
        EventKind::SessionChanged {
            session_num: 1,
            kind: SessionKind::Race
        }
    )));
    assert!(ev.iter().any(|e| matches!(
        e.kind,
        EventKind::PhaseChanged {
            to: SessionPhase::Racing,
            ..
        }
    )));
    assert!(
        ev.iter()
            .any(|e| matches!(e.kind, EventKind::FlagRaised { flag: Flag::Blue }))
    );
    let text = st.events(&EventsParams::default()).unwrap().to_text();
    assert!(text.contains("session: race"), "{text}");
    assert!(text.contains("session phase: racing"), "{text}");
}

#[test]
fn lap_counter_going_backwards_keeps_the_laps() {
    let mut me = car(0, 1, 1, 0.9);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 1.0, vec![me.clone()], 0));
    me.laps_completed = Some(2);
    me.last_lap_s = Some(91.0);
    st.ingest(frame(SessionKind::Race, 2.0, vec![me.clone()], 0));
    assert_eq!(st.laps(&LapsParams::default()).unwrap().laps.len(), 1);
    me.laps_completed = Some(0);
    st.ingest(frame(SessionKind::Race, 3.0, vec![me.clone()], 0));
    st.ingest(frame(SessionKind::Race, 4.0, vec![me.clone()], 0));
    assert_eq!(st.laps(&LapsParams::default()).unwrap().laps.len(), 1);
    // Counted from there on.
    me.laps_completed = Some(1);
    me.last_lap_s = Some(90.0);
    st.ingest(frame(SessionKind::Race, 5.0, vec![me], 0));
    let laps = st.laps(&LapsParams::default()).unwrap().laps;
    assert_eq!(laps.len(), 2);
    assert_eq!((laps[1].lap, laps[1].time_s), (1, Some(90.0)));
}

#[test]
fn laps_for_other_car_and_unknown_car() {
    let mut a = car(0, 1, 1, 0.9);
    let me = car(1, 2, 1, 0.8);
    let mut st = state();
    st.ingest(frame(
        SessionKind::Race,
        1.0,
        vec![a.clone(), me.clone()],
        1,
    ));
    a.laps_completed = Some(2);
    a.last_lap_s = Some(95.0);
    st.ingest(frame(SessionKind::Race, 2.0, vec![a, me], 1));
    let r = st
        .laps(&LapsParams {
            car: Some("#10".into()),
            last: Some(5),
        })
        .unwrap();
    assert_eq!(r.car.unwrap().idx, 0);
    assert_eq!(r.laps.len(), 1);
    assert!(
        r.laps[0].fuel_used_l.is_none(),
        "fuel is tracked for the player only"
    );
    assert!(matches!(
        st.laps(&LapsParams {
            car: Some("77".into()),
            last: None
        }),
        Err(ApiError::NotFound(_))
    ));
}

#[test]
fn standings_top_and_around_without_player() {
    let cars: Vec<CarState> = (0..6).map(|i| car(i, i as u32 + 1, 3, 0.5)).collect();
    let mut f = frame(SessionKind::Race, 1.0, cars, 0);
    f.player_car_idx = None;
    let mut st = state();
    st.ingest(f);
    let top = st
        .standings(&StandingsParams {
            top: Some(2),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(top.rows.len(), 2);
    assert_eq!(top.rows[0].car.position, Some(1));
    let around = st
        .standings(&StandingsParams {
            around: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(around.rows.len(), 3, "no player: first 2n+1 rows");
    // No player: brief still answers.
    let b = st.brief().unwrap();
    assert!(b.player.is_none() && b.ahead.is_none());
    assert!(b.fuel.is_some());
}

#[test]
fn gap_to_car_behind_by_number() {
    let mut a = car(0, 1, 5, 0.6);
    a.gap_to_leader_s = Some(0.0);
    let mut me = car(1, 2, 5, 0.5);
    me.gap_to_leader_s = Some(10.0);
    let mut c = car(2, 3, 5, 0.45);
    c.gap_to_leader_s = Some(15.5);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 1.0, vec![a, me, c], 1));
    let g = st
        .gaps(&GapParams {
            car: Some("12".into()),
            window_s: None,
        })
        .unwrap();
    assert_eq!(g.target_is_ahead, Some(false));
    assert!((g.target.as_ref().unwrap().gap_s.unwrap() - 5.5).abs() < 1e-9);
    assert!(
        g.to_text().contains("target: P3 #12 Driver 2 -5.50s"),
        "{}",
        g.to_text()
    );
}

#[test]
fn incidents_and_pit_events_for_player() {
    let mut me = car(0, 1, 1, 0.5);
    let mut st = state();
    let mut f = frame(SessionKind::Race, 1.0, vec![me.clone()], 0);
    f.player.as_mut().unwrap().incidents = Some(0);
    st.ingest(f);
    me.on_pit_road = true;
    let mut f = frame(SessionKind::Race, 2.0, vec![me.clone()], 0);
    f.player.as_mut().unwrap().incidents = Some(4);
    st.ingest(f);
    me.on_pit_road = false;
    st.ingest(frame(SessionKind::Race, 3.0, vec![me], 0));
    let ev = st.event_log().last(20);
    assert!(
        ev.iter()
            .any(|e| matches!(e.kind, EventKind::Incident { added: 4, total: 4 }))
    );
    assert!(ev.iter().any(|e| e.text == "pit in: you"));
    assert!(ev.iter().any(|e| e.text == "pit out: you"));
}

#[test]
fn disconnected_oneshot_meta_text() {
    let mut st = RaceState::new("iracing", SourceKind::Oneshot, Capabilities::default());
    st.ingest(frame(SessionKind::Race, 1.0, vec![car(0, 1, 1, 0.5)], 0));
    st.set_connected(false);
    let line = render::meta_line(&st.meta());
    assert!(line.contains("DISCONNECTED"), "{line}");
    assert!(line.contains("history=unavailable"), "{line}");
    assert!(line.starts_with("[iracing oneshot"));
    let r = RaceState::new("x", SourceKind::Replay, Capabilities::default());
    assert!(render::meta_line(&r.meta()).starts_with("[x replay"));
    assert!(r.meta().age_ms.is_none());
}

#[test]
fn empty_reports_have_text() {
    let st = state();
    assert!(
        st.standings(&StandingsParams::default())
            .unwrap()
            .to_text()
            .contains("no standings")
    );
    assert!(st.fuel().unwrap().to_text().contains("fuel: unavailable"));
    assert!(
        st.laps(&LapsParams::default())
            .unwrap()
            .to_text()
            .contains("no car")
    );
    assert!(
        st.events(&EventsParams::default())
            .unwrap()
            .to_text()
            .contains("no events")
    );
    assert!(st.snapshot().unwrap().frame.is_none());
    let g = st
        .gaps(&GapParams {
            car: Some("1".into()),
            window_s: None,
        })
        .unwrap();
    assert!(g.target.is_none());
}

#[test]
fn event_log_is_bounded_and_pages() {
    let mut log = EventLog::with_capacity(3);
    for i in 0..5 {
        log.push(
            i as f64,
            0,
            EventKind::Incident { added: 1, total: i },
            format!("e{i}"),
        );
    }
    assert_eq!(log.last_seq(), 5);
    let all = log.since(0, 100);
    assert_eq!(all.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
    assert_eq!(log.since(3, 100).len(), 2);
    assert_eq!(log.since(0, 1)[0].seq, 5, "limit keeps newest");
    assert_eq!(render::event_to_text(&all[0]), "#3 t=0m02s e2");
}

#[test]
fn shared_state_delegates() {
    use std::sync::{Arc, RwLock};
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 1.0, vec![car(0, 1, 1, 0.5)], 0));
    let shared: SharedState = Arc::new(RwLock::new(st));
    assert!(shared.brief().unwrap().session.is_some());
    assert_eq!(
        shared
            .standings(&StandingsParams::default())
            .unwrap()
            .rows
            .len(),
        1
    );
    assert!(shared.gaps(&GapParams::default()).is_ok());
    assert!(shared.fuel().unwrap().fuel.is_some());
    assert!(shared.laps(&LapsParams::default()).unwrap().car.is_some());
    assert!(shared.events(&EventsParams::default()).unwrap().last_seq > 0);
    assert!(shared.snapshot().unwrap().frame.is_some());
}

#[test]
fn brief_text_shows_session_details() {
    let mut me = car(0, 1, 4, 0.5);
    me.on_pit_road = true;
    let mut f = frame(SessionKind::Race, 1.0, vec![me], 0);
    f.session.laps_total = Some(20);
    f.session.laps_remaining = Some(16);
    f.session.time_remaining_s = Some(3725.0);
    f.session.track = Some(TrackInfo {
        name: "Spa".into(),
        config: Some("GP".into()),
        length_m: None,
    });
    f.session.flags = vec![];
    f.player.as_mut().unwrap().incidents = Some(3);
    let mut st = state();
    st.ingest(f);
    let text = st.brief().unwrap().to_text();
    for want in [
        "@ Spa - GP",
        "lap 5/20",
        "16 laps left",
        "1h02m05s left",
        "flags: none",
        "3x",
        "ON PIT ROAD",
    ] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
}

// ---------------------------------------------------------------------------
// Exact-value tests added from cargo-mutants survivors.
// ---------------------------------------------------------------------------

#[test]
fn standings_intervals_and_lap_filters_are_exact() {
    let mut a = car(0, 1, 5, 0.6);
    a.gap_to_leader_s = Some(0.0);
    let mut me = car(1, 2, 5, 0.5);
    me.gap_to_leader_s = Some(2.5);
    me.last_lap_s = Some(0.0);
    me.best_lap_s = Some(-1.0);
    let mut c = car(2, 3, 5, 0.4);
    c.gap_to_leader_s = Some(6.0);
    let mut d = car(3, 4, 5, 0.3);
    d.gap_to_leader_s = None;
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 1.0, vec![d, c, me, a], 1));
    let rows = st.standings(&StandingsParams::default()).unwrap().rows;
    let pos: Vec<u32> = rows.iter().map(|r| r.car.position.unwrap()).collect();
    assert_eq!(pos, vec![1, 2, 3, 4], "sorted by position");
    let ints: Vec<Option<f64>> = rows.iter().map(|r| r.interval_s).collect();
    assert_eq!(ints, vec![None, Some(2.5), Some(3.5), None]);
    assert_eq!(rows[1].last_lap_s, None, "zero lap time is 'no time'");
    assert_eq!(rows[1].best_lap_s, None);
    assert_eq!(rows[0].last_lap_s, Some(100.0));
    assert_eq!(rows[0].best_lap_s, Some(99.0));
    assert_eq!(rows.iter().filter(|r| r.is_player).count(), 1);
    assert!(rows[1].is_player);
}

#[test]
fn standings_player_detection_uses_flag_or_index() {
    // Adapter marks the player car but the frame has no player index.
    let mut me = car(1, 2, 5, 0.5);
    me.is_player = true;
    let mut f = frame(SessionKind::Race, 1.0, vec![car(0, 1, 5, 0.6), me], 1);
    f.player_car_idx = None;
    let mut st = state();
    st.ingest(f);
    let rows = st.standings(&StandingsParams::default()).unwrap().rows;
    assert_eq!(
        rows.iter().map(|r| r.is_player).collect::<Vec<_>>(),
        vec![false, true]
    );
}

#[test]
fn standings_around_without_player_keeps_2n_plus_1() {
    let cars: Vec<CarState> = (0..9).map(|i| car(i, i as u32 + 1, 3, 0.5)).collect();
    let mut f = frame(SessionKind::Race, 1.0, cars, 0);
    f.player_car_idx = None;
    let mut st = state();
    st.ingest(f);
    let r = st
        .standings(&StandingsParams {
            around: Some(2),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(r.rows.len(), 5);
    let r = st
        .standings(&StandingsParams {
            around: Some(3),
            top: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(r.rows.len(), 7, "around wins over top");
}

#[test]
fn standings_around_player_clamps_at_edges() {
    let cars: Vec<CarState> = (0..6).map(|i| car(i, i as u32 + 1, 3, 0.5)).collect();
    for (player, want) in [
        (0usize, vec![1, 2, 3]),
        (5, vec![4, 5, 6]),
        (2, vec![1, 2, 3, 4, 5]),
    ] {
        let mut st = state();
        st.ingest(frame(SessionKind::Race, 1.0, cars.clone(), player));
        let r = st
            .standings(&StandingsParams {
                around: Some(2),
                ..Default::default()
            })
            .unwrap();
        let pos: Vec<u32> = r.rows.iter().map(|r| r.car.position.unwrap()).collect();
        assert_eq!(pos, want, "player idx {player}");
    }
}

#[test]
fn target_direction_without_positions_uses_progress() {
    let mut st = state();
    let mk = |idx, laps, pct| {
        let mut c = car(idx, 1, laps, pct);
        c.position = None;
        c.class_position = None;
        c
    };
    // Same lap: further round is ahead.
    st.ingest(frame(
        SessionKind::Practice,
        1.0,
        vec![mk(0, 3, 0.2), mk(1, 3, 0.7), mk(2, 2, 0.9)],
        0,
    ));
    let ahead = |st: &RaceState, n: &str| {
        st.gaps(&GapParams {
            car: Some(n.into()),
            window_s: None,
        })
        .unwrap()
        .target_is_ahead
    };
    assert_eq!(ahead(&st, "11"), Some(true), "3.7 laps vs 3.2");
    // A lap less but further round the lap is still behind.
    assert_eq!(ahead(&st, "12"), Some(false), "2.9 laps vs 3.2");
    // Comparing with yourself is never 'ahead'.
    assert_eq!(ahead(&st, "10"), Some(false));
    // Missing lap data counts as zero progress.
    let mut none = mk(3, 0, 0.0);
    none.laps_completed = None;
    none.lap_dist_pct = None;
    st.ingest(frame(
        SessionKind::Practice,
        2.0,
        vec![mk(0, 0, 0.0), none],
        0,
    ));
    assert_eq!(ahead(&st, "13"), Some(false));
}

#[test]
fn target_direction_with_positions() {
    let mut st = state();
    st.ingest(frame(
        SessionKind::Race,
        1.0,
        vec![car(0, 1, 3, 0.1), car(1, 2, 3, 0.9), car(2, 3, 3, 0.95)],
        1,
    ));
    let ahead = |n: &str| {
        st.gaps(&GapParams {
            car: Some(n.into()),
            window_s: None,
        })
        .unwrap()
        .target_is_ahead
    };
    // Position wins over track progress.
    assert_eq!(ahead("10"), Some(true));
    assert_eq!(ahead("12"), Some(false));
    assert_eq!(ahead("11"), Some(false), "self");
}

#[test]
fn set_source_and_frame_counter() {
    let mut st = state();
    assert_eq!(st.frames_ingested(), 0);
    st.set_source(
        "acc",
        Capabilities {
            fuel: true,
            ..Default::default()
        },
    );
    let m = st.meta();
    assert_eq!(m.game, "acc");
    assert!(m.capabilities.fuel && !m.capabilities.gaps);
    for i in 0..3 {
        st.ingest(frame(
            SessionKind::Race,
            i as f64,
            vec![car(0, 1, 1, 0.5)],
            0,
        ));
    }
    assert_eq!(st.frames_ingested(), 3);
}

#[test]
fn meta_age_is_time_since_ingest() {
    let mut st = state();
    st.ingest_at(
        frame(SessionKind::Race, 1.0, vec![car(0, 1, 1, 0.5)], 0),
        now_ms() - 1_000,
    );
    let age = st.meta().age_ms.unwrap();
    assert!((1_000..5_000).contains(&age), "{age}");
    // Clock skew never yields a negative age.
    st.ingest_at(
        frame(SessionKind::Race, 2.0, vec![car(0, 1, 1, 0.5)], 0),
        now_ms() + 60_000,
    );
    assert_eq!(st.meta().age_ms, Some(0));
}

#[test]
fn new_session_starts_clean() {
    let mut me = car(0, 2, 1, 0.9);
    me.class_position = Some(2);
    let mut st = state();
    let mut f = frame(SessionKind::Race, 1.0, vec![me.clone()], 0);
    f.player.as_mut().unwrap().incidents = Some(4);
    st.ingest(f);
    me.laps_completed = Some(2);
    me.last_lap_s = Some(90.0);
    st.ingest(frame(SessionKind::Race, 2.0, vec![me.clone()], 0));
    let before = st.event_log().last_seq();

    // Next session: same flags, different position, incident counter restarted.
    let mut me2 = car(0, 5, 0, 0.1);
    me2.class_position = Some(5);
    let mut f = frame(SessionKind::Race, 0.5, vec![me2.clone()], 0);
    f.session.session_num = 1;
    f.player.as_mut().unwrap().incidents = Some(1);
    st.ingest(f.clone());
    let new: Vec<EventKind> = st
        .event_log()
        .since(before, 100)
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(
        new.contains(&EventKind::FlagRaised { flag: Flag::Green }),
        "flags re-announced: {new:?}"
    );
    assert!(
        !new.iter()
            .any(|k| matches!(k, EventKind::PositionChanged { .. })),
        "{new:?}"
    );
    assert!(
        !new.iter().any(|k| matches!(k, EventKind::Incident { .. })),
        "{new:?}"
    );

    // First lap of the new session is not a personal best, even if slower than before.
    me2.laps_completed = Some(1);
    me2.last_lap_s = Some(95.0);
    f.cars = vec![me2.clone()];
    f.session.time_s = 1.0;
    st.ingest(f.clone());
    me2.laps_completed = Some(2);
    me2.last_lap_s = Some(94.0);
    f.cars = vec![me2];
    f.session.time_s = 2.0;
    st.ingest(f);
    let pbs: Vec<f64> = st
        .event_log()
        .since(before, 100)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::PersonalBest { time_s, .. } => Some(time_s),
            _ => None,
        })
        .collect();
    assert_eq!(pbs, vec![94.0]);
}

#[test]
fn pending_lap_timeout_boundary() {
    let mut me = car(0, 1, 1, 0.9);
    me.last_lap_s = Some(100.0);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 10.0, vec![me.clone()], 0));
    me.laps_completed = Some(2);
    st.ingest(frame(SessionKind::Race, 20.0, vec![me.clone()], 0));
    st.ingest(frame(SessionKind::Race, 23.0, vec![me.clone()], 0));
    assert!(
        st.laps(&LapsParams::default()).unwrap().laps.is_empty(),
        "exactly 3 s: still waiting"
    );
    st.ingest(frame(SessionKind::Race, 23.01, vec![me], 0));
    assert_eq!(st.laps(&LapsParams::default()).unwrap().laps.len(), 1);
}

fn lap_events(times: &[f64]) -> Vec<EventKind> {
    let mut me = car(0, 1, 0, 0.9);
    me.last_lap_s = None;
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 0.0, vec![me.clone()], 0));
    for (i, t) in times.iter().enumerate() {
        me.laps_completed = Some(i as i32 + 1);
        me.last_lap_s = Some(*t);
        st.ingest(frame(
            SessionKind::Race,
            (i + 1) as f64 * 100.0,
            vec![me.clone()],
            0,
        ));
    }
    st.event_log()
        .last(100)
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

#[test]
fn personal_best_threshold() {
    let pbs = |times: &[f64]| -> Vec<f64> {
        lap_events(times)
            .into_iter()
            .filter_map(|k| match k {
                EventKind::PersonalBest { time_s, .. } => Some(time_s),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        pbs(&[90.0]),
        Vec::<f64>::new(),
        "first lap is never announced"
    );
    assert_eq!(pbs(&[90.0, 89.9]), vec![89.9]);
    assert_eq!(
        pbs(&[90.0, 90.1, 89.9995]),
        Vec::<f64>::new(),
        "within 0.5 ms is not a new best"
    );
    assert_eq!(pbs(&[90.0, 89.999]), vec![89.999]);
    assert_eq!(pbs(&[90.0, 89.0, 89.5, 88.0]), vec![89.0, 88.0]);
    let laps: Vec<(i32, Option<f64>)> = lap_events(&[91.0, 92.0])
        .into_iter()
        .filter_map(|k| match k {
            EventKind::LapCompleted { lap, time_s, .. } => Some((lap, time_s)),
            _ => None,
        })
        .collect();
    assert_eq!(laps, vec![(1, Some(91.0)), (2, Some(92.0))]);
}

#[test]
fn gap_trend_is_exact() {
    // Car ahead: player gap to leader 10, 10.5, 11, 11.5 at successive lines;
    // the car ahead stays at 5 -> gap grows 0.5 s/lap.
    let mut st = state();
    let mut a = car(0, 1, 0, 0.95);
    a.gap_to_leader_s = Some(0.0);
    let mut b = car(1, 2, 0, 0.9);
    b.gap_to_leader_s = Some(5.0);
    let mut me = car(2, 3, 0, 0.8);
    for (lap, pg) in [(1, 10.0), (2, 10.5), (3, 11.0), (4, 11.5)] {
        me.gap_to_leader_s = Some(pg);
        st.ingest(frame(
            SessionKind::Race,
            lap as f64 * 100.0 - 1.0,
            vec![a.clone(), b.clone(), me.clone()],
            2,
        ));
        me.laps_completed = Some(lap);
        st.ingest(frame(
            SessionKind::Race,
            lap as f64 * 100.0,
            vec![a.clone(), b.clone(), me.clone()],
            2,
        ));
    }
    let g = st.brief().unwrap().ahead.unwrap();
    assert_eq!(g.car.idx, 1);
    assert!((g.gap_s.unwrap() - 6.5).abs() < 1e-9);
    assert!((g.trend_s_per_lap.unwrap() - 0.5).abs() < 1e-9, "{g:?}");
    // Not sampled outside races.
    let mut st = state();
    me.laps_completed = Some(0);
    st.ingest(frame(
        SessionKind::Practice,
        1.0,
        vec![a.clone(), b.clone(), me.clone()],
        2,
    ));
    me.laps_completed = Some(1);
    st.ingest(frame(SessionKind::Practice, 2.0, vec![a, b, me], 2));
    assert!(st.brief().unwrap().ahead.unwrap().trend_s_per_lap.is_none());
}

#[test]
fn position_change_events_are_exact() {
    let mut me = car(1, 3, 1, 0.5);
    let mut st = state();
    st.ingest(frame(
        SessionKind::Race,
        1.0,
        vec![car(0, 1, 1, 0.6), me.clone()],
        1,
    ));
    me.position = Some(2);
    st.ingest(frame(
        SessionKind::Race,
        2.0,
        vec![car(0, 1, 1, 0.6), me.clone()],
        1,
    ));
    me.position = Some(2);
    st.ingest(frame(
        SessionKind::Race,
        3.0,
        vec![car(0, 1, 1, 0.6), me.clone()],
        1,
    ));
    me.position = None;
    st.ingest(frame(
        SessionKind::Race,
        4.0,
        vec![car(0, 1, 1, 0.6), me.clone()],
        1,
    ));
    me.position = Some(4);
    st.ingest(frame(
        SessionKind::Race,
        5.0,
        vec![car(0, 1, 1, 0.6), me],
        1,
    ));
    let texts: Vec<String> = st
        .event_log()
        .last(50)
        .into_iter()
        .filter(|e| matches!(e.kind, EventKind::PositionChanged { .. }))
        .map(|e| e.text)
        .collect();
    assert_eq!(
        texts,
        vec!["gained position: P3 -> P2", "lost position: P2 -> P4"]
    );
}

#[test]
fn incident_delta_is_exact() {
    let mut st = state();
    for (t, inc) in [(1.0, 2), (2.0, 2), (3.0, 5), (4.0, 4)] {
        let mut f = frame(SessionKind::Race, t, vec![car(0, 1, 1, 0.5)], 0);
        f.player.as_mut().unwrap().incidents = Some(inc);
        st.ingest(f);
    }
    let inc: Vec<(i32, i32, String)> = st
        .event_log()
        .last(50)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Incident { added, total } => Some((added, total, e.text)),
            _ => None,
        })
        .collect();
    assert_eq!(inc, vec![(3, 5, "incident +3x (total 5x)".to_string())]);
}

/// Which other cars' pit stops are reported.
fn pit_reported(cars: Vec<CarState>, player: usize, pitter: usize) -> bool {
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 1.0, cars.clone(), player));
    let mut cars = cars;
    cars.iter_mut()
        .find(|c| c.idx == pitter)
        .unwrap()
        .on_pit_road = true;
    st.ingest(frame(SessionKind::Race, 2.0, cars, player));
    st.event_log()
        .last(50)
        .iter()
        .any(|e| matches!(e.kind, EventKind::PitEntry { car_idx, .. } if car_idx == pitter))
}

#[test]
fn pit_events_only_for_neighbours() {
    let field: Vec<CarState> = (0..6).map(|i| car(i, i as u32 + 1, 3, 0.5)).collect();
    // Player P3 (idx 2): P1..P5 are within two places, P6 is not.
    for (pitter, want) in [(0, true), (1, true), (3, true), (4, true), (5, false)] {
        assert_eq!(
            pit_reported(field.clone(), 2, pitter),
            want,
            "pitter idx {pitter}"
        );
    }
    // Multi-class: an adjacent car of another class is not a neighbour; class
    // positions decide.
    let mut mc = field.clone();
    mc[3].class_id = Some(2);
    mc[3].class_position = Some(1);
    for (i, cp) in [(0, 1), (1, 2), (2, 3), (4, 4), (5, 5)] {
        mc[i].class_position = Some(cp);
    }
    assert!(!pit_reported(mc.clone(), 2, 3), "other class");
    assert!(
        pit_reported(mc.clone(), 2, 5),
        "class P5 is two places behind class P3"
    );
    assert!(pit_reported(mc, 2, 0));
    // Player without a position: nobody is a neighbour.
    let mut nopos = field;
    nopos[2].position = None;
    assert!(!pit_reported(nopos, 2, 1));
}

#[test]
fn track_name_with_empty_config() {
    let mut f = frame(SessionKind::Race, 1.0, vec![car(0, 1, 1, 0.5)], 0);
    f.session.track = Some(TrackInfo {
        name: "Oval".into(),
        config: Some(String::new()),
        length_m: None,
    });
    let mut st = state();
    st.ingest(f);
    assert_eq!(
        st.brief().unwrap().session.unwrap().track.as_deref(),
        Some("Oval")
    );
}

#[test]
fn negative_time_behind_leader_falls_back() {
    let mut a = car(0, 1, 5, 0.5);
    a.gap_to_leader_s = Some(-1.0);
    let mut me = car(1, 2, 5, 0.4);
    me.gap_to_leader_s = Some(3.0);
    let mut st = state();
    st.ingest(frame(
        SessionKind::Race,
        1.0,
        vec![a.clone(), me.clone()],
        1,
    ));
    assert_eq!(
        st.brief().unwrap().ahead.unwrap().mode,
        GapMode::TrackProgress
    );
    a.gap_to_leader_s = Some(0.0);
    me.gap_to_leader_s = Some(-0.5);
    st.ingest(frame(
        SessionKind::Race,
        2.0,
        vec![a.clone(), me.clone()],
        1,
    ));
    assert_eq!(
        st.brief().unwrap().ahead.unwrap().mode,
        GapMode::TrackProgress
    );
    me.gap_to_leader_s = Some(0.0);
    st.ingest(frame(SessionKind::Race, 3.0, vec![a, me], 1));
    let g = st.brief().unwrap().ahead.unwrap();
    assert_eq!(g.mode, GapMode::RaceTime, "zero is a valid time");
    assert_eq!(g.gap_s, Some(0.0));
}

// ---------------------------------------------------------------------------
// Second cargo-mutants pass.
// ---------------------------------------------------------------------------

#[test]
fn gap_trend_uses_last_four_samples() {
    // Gap to the car ahead (magnitude) at successive lines: 0,0,0,1,3,6.
    // Last 4 samples (laps 3..6) -> (6 - 0) / 3 = 2.0 s/lap.
    let mut st = state();
    let mut a = car(0, 1, 0, 0.95);
    a.gap_to_leader_s = Some(0.0);
    let mut me = car(1, 2, 0, 0.8);
    for (lap, g) in [(1, 0.0), (2, 0.0), (3, 0.0), (4, 1.0), (5, 3.0), (6, 6.0)] {
        me.gap_to_leader_s = Some(g);
        st.ingest(frame(
            SessionKind::Race,
            lap as f64 * 100.0 - 1.0,
            vec![a.clone(), me.clone()],
            1,
        ));
        me.laps_completed = Some(lap);
        st.ingest(frame(
            SessionKind::Race,
            lap as f64 * 100.0,
            vec![a.clone(), me.clone()],
            1,
        ));
    }
    let t = st.brief().unwrap().ahead.unwrap().trend_s_per_lap.unwrap();
    assert!((t - 2.0).abs() < 1e-9, "{t}");
}

#[test]
fn last_lap_delta_is_their_minus_ours() {
    let mut a = car(0, 1, 5, 0.6);
    a.last_lap_s = Some(101.5);
    let mut me = car(1, 2, 5, 0.5);
    me.last_lap_s = Some(100.0);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 1.0, vec![a, me], 1));
    let g = st.brief().unwrap().ahead.unwrap();
    assert_eq!(g.last_lap_delta_s, Some(1.5));
    assert_eq!(g.their_last_lap_s, Some(101.5));
}

#[test]
fn nearby_geometry_is_exact() {
    // Player at 0.3, car at 0.35 on the same lap: 5 s ahead (100 s lap).
    let mut me = car(0, 1, 4, 0.3);
    me.last_lap_s = Some(100.0);
    let st_for = |me: &CarState, others: Vec<CarState>, window: f64| {
        let mut cars = vec![me.clone()];
        cars.extend(others);
        let mut st = state();
        st.ingest(frame(SessionKind::Race, 1.0, cars, 0));
        st.gaps(&GapParams {
            car: None,
            window_s: Some(window),
        })
        .unwrap()
        .nearby
    };
    let n = st_for(&me, vec![car(1, 2, 4, 0.35)], 10.0);
    assert_eq!(n.len(), 1);
    assert!((n[0].rel_s - 5.0).abs() < 1e-9, "{}", n[0].rel_s);
    assert_eq!(n[0].lap_diff, 0);
    // 0.4 lap ahead on the same lap is not a lap difference.
    let n = st_for(&me, vec![car(1, 2, 4, 0.7)], 50.0);
    assert!((n[0].rel_s - 40.0).abs() < 1e-9);
    assert_eq!(n[0].lap_diff, 0);
    // Exactly on the window edge is included (0.25 lap = 25 s).
    me.lap_dist_pct = Some(0.5);
    let n = st_for(&me, vec![car(1, 2, 4, 0.75)], 25.0);
    assert_eq!(n.len(), 1);
    // Behind across the line: player 0.05, car 0.95 of the previous lap.
    me.lap_dist_pct = Some(0.05);
    me.laps_completed = Some(4);
    let n = st_for(&me, vec![car(1, 2, 3, 0.95)], 20.0);
    assert!((n[0].rel_s + 10.0).abs() < 1e-9, "{}", n[0].rel_s);
    assert_eq!(
        n[0].lap_diff, 0,
        "same race lap, just behind across the line"
    );
}

#[test]
fn fuel_average_excludes_pit_and_caution_laps() {
    let mut me = car(0, 1, 1, 0.9);
    let mut st = state();
    let mut fuel = 50.0;
    let mut t = 0.0;
    let mut f = frame(SessionKind::Race, t, vec![me.clone()], 0);
    f.player.as_mut().unwrap().fuel_l = Some(fuel);
    st.ingest(f);
    // (used, pit, caution)
    for (lap, (used, pit, caution)) in [
        (2.0, false, false),
        (9.0, true, false),
        (7.0, false, true),
        (3.0, false, false),
    ]
    .into_iter()
    .enumerate()
    {
        t += 50.0;
        me.on_pit_road = pit;
        let mut f = frame(SessionKind::Race, t, vec![me.clone()], 0);
        if caution {
            f.session.flags = vec![Flag::Yellow];
        }
        f.player.as_mut().unwrap().fuel_l = Some(fuel);
        st.ingest(f);
        t += 50.0;
        fuel -= used;
        me.on_pit_road = false;
        me.laps_completed = Some(lap as i32 + 2);
        me.last_lap_s = Some(100.0 + lap as f64);
        let mut f = frame(SessionKind::Race, t, vec![me.clone()], 0);
        f.player.as_mut().unwrap().fuel_l = Some(fuel);
        st.ingest(f);
    }
    let fi = st.fuel().unwrap().fuel.unwrap();
    assert_eq!(fi.laps_sampled, 2);
    assert!((fi.per_lap_l.unwrap() - 2.5).abs() < 1e-9, "{fi:?}");
}

#[test]
fn margin_is_half_a_lap_on_top_of_shortfall() {
    let me = car(0, 1, 2, 0.25);
    let mut f = frame(SessionKind::Race, 10.0, vec![me], 0);
    f.session.laps_remaining = Some(5);
    let p = f.player.as_mut().unwrap();
    p.fuel_l = Some(5.0);
    p.fuel_use_lph = Some(72.0); // 2 L / 100 s lap
    let mut st = state();
    st.ingest(f);
    let fi = st.fuel().unwrap().fuel.unwrap();
    assert!((fi.laps_to_finish.unwrap() - 4.75).abs() < 1e-9);
    assert!((fi.to_finish_l.unwrap() - 4.5).abs() < 1e-9);
    assert!((fi.add_with_margin_l.unwrap() - 5.5).abs() < 1e-9);
    assert_eq!(
        fi.last_lap_on_fuel,
        Some(4),
        "0.25 + 2.5 laps of fuel -> 2 more full laps"
    );
}

#[test]
fn timed_race_remaining_time_bounds() {
    let laps_for = |remain: f64| {
        let me = car(0, 1, 2, 0.0);
        let mut f = frame(SessionKind::Race, 10.0, vec![me], 0);
        f.session.time_remaining_s = Some(remain);
        f.player.as_mut().unwrap().fuel_use_lph = Some(36.0);
        let mut st = state();
        st.ingest(f);
        st.fuel().unwrap().fuel.unwrap().laps_to_finish
    };
    let week = 7.0 * 24.0 * 3600.0;
    assert_eq!(laps_for(week), None, "a week means unlimited");
    assert_eq!(laps_for(week - 100.0), Some(6047.0));
    assert_eq!(laps_for(-1.0), None);
    assert_eq!(laps_for(0.0), Some(0.0));
    assert_eq!(laps_for(250.0), Some(3.0));
}

#[test]
fn zero_best_lap_is_no_best() {
    let mut me = car(0, 1, 1, 0.5);
    me.best_lap_s = Some(0.0);
    let mut st = state();
    st.ingest(frame(SessionKind::Race, 1.0, vec![me], 0));
    assert_eq!(
        st.standings(&StandingsParams::default()).unwrap().rows[0].best_lap_s,
        None
    );
}

/// Frames of a race in the rain: precipitation `rain(t)` every 5 s.
fn rainy(st: &mut RaceState, until: f64, rain: impl Fn(f64) -> f64) {
    let mut t = 0.0;
    while t <= until {
        let mut f = frame(SessionKind::Race, t, vec![car(0, 1, 1, 0.5)], 0);
        f.weather = Some(vr_core::Weather {
            precipitation: Some(rain(t)),
            wetness: Some("lightly wet".into()),
            ..Default::default()
        });
        st.ingest_at(f, 0);
        t += 5.0;
    }
}

#[test]
fn rain_trend_needs_two_minutes_and_a_clear_change() {
    use vr_engine::RainDirection::{Easing, Rising, Steady};
    let trend = |until: f64, per_min: f64| {
        let mut st = state();
        rainy(&mut st, until, |t| 0.3 + per_min * t / 60.0);
        st.rain_trend()
            .map(|t| (t.direction, (t.per_min * 1000.0).round() / 1000.0))
    };
    // Not two minutes of it yet (0.9 × 120 s is the least).
    assert_eq!(trend(100.0, 0.05), None);
    assert_eq!(trend(110.0, 0.05), Some((Rising, 0.05)));
    assert_eq!(trend(600.0, 0.05), Some((Rising, 0.05)));
    assert_eq!(trend(600.0, -0.05), Some((Easing, -0.05)));
    // Under 1 %/min is steady; just over it is not.
    assert_eq!(trend(600.0, 0.0099).map(|t| t.0), Some(Steady));
    assert_eq!(trend(600.0, -0.0099).map(|t| t.0), Some(Steady));
    assert_eq!(trend(600.0, 0.0101).map(|t| t.0), Some(Rising));
    assert_eq!(trend(600.0, -0.0101).map(|t| t.0), Some(Easing));
    // Without rain data there is no trend, and the brief says so.
    let st = state();
    assert_eq!(st.rain_trend(), None);
    let mut st = state();
    rainy(&mut st, 300.0, |t| 0.1 + 0.03 * t / 60.0);
    let text = st.brief().unwrap().to_text();
    assert!(
        text.contains("weather: lightly wet (rain 25%, rising 3%/min)"),
        "{text}"
    );
}
