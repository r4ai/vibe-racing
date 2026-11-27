use vr_core::Capabilities;
use vr_engine::*;
use vr_replay::{SynthConfig, synth_frames};

fn run(cfg: SynthConfig, until_s: Option<f64>) -> RaceState {
    let mut st = RaceState::new("synthetic", SourceKind::Synthetic, Capabilities::ALL);
    for f in synth_frames(cfg, until_s) {
        let ts = f.timestamp_ms;
        st.ingest_at(f, ts);
    }
    st
}

#[test]
fn records_player_laps_with_fuel_and_pit_flags() {
    let st = run(SynthConfig::default(), None);
    let laps = st
        .laps(&LapsParams {
            car: None,
            last: Some(100),
        })
        .unwrap();
    assert_eq!(
        laps.laps.len(),
        20,
        "{:#?}",
        laps.laps.iter().map(|l| l.lap).collect::<Vec<_>>()
    );
    let l9 = laps.laps.iter().find(|l| l.lap == 9).unwrap();
    let l10 = laps.laps.iter().find(|l| l.lap == 10).unwrap();
    assert!(l9.pit && l10.pit, "in-lap and out-lap are pit laps");
    // Refuelled during lap 9 -> fuel use unknown for that lap.
    assert!(l9.fuel_used_l.is_none());
    let clean: Vec<_> = laps
        .laps
        .iter()
        .filter(|l| l.is_clean() && l.lap > 1)
        .collect();
    for l in &clean {
        let used = l.fuel_used_l.unwrap();
        assert!((used - 2.5).abs() < 0.1, "lap {} used {used}", l.lap);
        let t = l.time_s.unwrap();
        assert!((89.0..92.0).contains(&t), "lap {} time {t}", l.lap);
    }
}

#[test]
fn fuel_estimate_before_pit() {
    // Stop at ~6.5 laps: 30 L start, 2.5 L/lap.
    let st = run(SynthConfig::default(), Some(6.5 * 90.6));
    let fuel = st.fuel().unwrap().fuel.unwrap();
    assert_eq!(fuel.basis, FuelBasis::History);
    let pl = fuel.per_lap_l.unwrap();
    assert!((pl - 2.5).abs() < 0.1, "per lap {pl}");
    let laps_left = fuel.laps_of_fuel.unwrap();
    assert!((4.5..6.0).contains(&laps_left), "laps of fuel {laps_left}");
    // ~13.5 laps to go needs far more than in the tank.
    assert!(fuel.to_finish_l.unwrap() > 20.0);
    assert!(fuel.add_with_margin_l.unwrap() > fuel.to_finish_l.unwrap());
}

#[test]
fn gaps_and_trend_to_car_ahead() {
    let st = run(SynthConfig::default(), Some(4.5 * 90.6));
    let b = st.brief().unwrap();
    let me = b.player.as_ref().unwrap();
    let ahead = b.ahead.as_ref().expect("car ahead");
    assert_eq!(ahead.mode, GapMode::RaceTime);
    assert!(ahead.gap_s.unwrap() > 0.0);
    assert_eq!(ahead.car.position.unwrap() + 1, me.car.position.unwrap());
    // Player is faster than the car directly ahead -> gap shrinking.
    let trend = ahead.trend_s_per_lap.expect("trend");
    assert!(trend < 0.0, "trend {trend}");
    assert!(b.behind.is_some());
    let text = b.to_text();
    assert!(text.contains("ahead:"), "{text}");
    assert!(text.contains("fuel:"), "{text}");
}

#[test]
fn emits_flag_incident_pit_and_lap_events() {
    let st = run(SynthConfig::default(), None);
    let ev = st
        .events(&EventsParams {
            since: None,
            limit: Some(10_000),
        })
        .unwrap();
    let has = |pred: &dyn Fn(&EventKind) -> bool| ev.events.iter().any(|e| pred(&e.kind));
    assert!(has(&|k| matches!(
        k,
        EventKind::FlagRaised {
            flag: vr_core::Flag::YellowWaving
        }
    )));
    assert!(has(&|k| matches!(
        k,
        EventKind::FlagCleared {
            flag: vr_core::Flag::YellowWaving
        }
    )));
    assert!(has(&|k| matches!(
        k,
        EventKind::Incident { added: 2, total: 2 }
    )));
    assert!(has(&|k| matches!(
        k,
        EventKind::PitEntry {
            is_player: true,
            ..
        }
    )));
    assert!(has(&|k| matches!(
        k,
        EventKind::PitExit {
            is_player: true,
            ..
        }
    )));
    assert!(
        has(&|k| matches!(
            k,
            EventKind::PitEntry {
                is_player: false,
                ..
            }
        )),
        "rival pit"
    );
    assert!(has(&|k| matches!(
        k,
        EventKind::LapCompleted { lap: 20, .. }
    )));
    assert!(has(&|k| matches!(
        k,
        EventKind::FlagRaised {
            flag: vr_core::Flag::Checkered
        }
    )));

    // Paging with `since`.
    let mid = ev.events[ev.events.len() / 2].seq;
    let later = st
        .events(&EventsParams {
            since: Some(mid),
            limit: None,
        })
        .unwrap();
    assert!(later.events.iter().all(|e| e.seq > mid));
}

#[test]
fn standings_around_player() {
    let st = run(SynthConfig::default(), Some(400.0));
    let s = st
        .standings(&StandingsParams {
            around: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(s.rows.len(), 3);
    assert!(s.rows[1].is_player);
    assert!(s.rows[1].interval_s.unwrap() > 0.0);
}

#[test]
fn gap_to_specific_car() {
    let st = run(SynthConfig::default(), Some(400.0));
    let snap = st.snapshot().unwrap().frame.unwrap();
    let leader = snap.cars.iter().find(|c| c.position == Some(1)).unwrap();
    let g = st
        .gaps(&GapParams {
            car: Some(format!("#{}", leader.number)),
            window_s: None,
        })
        .unwrap();
    assert_eq!(g.target_is_ahead, Some(true));
    assert!(g.target.unwrap().gap_s.unwrap() > 0.0);
    assert!(
        st.gaps(&GapParams {
            car: Some("999".into()),
            window_s: None
        })
        .is_err()
    );
}

#[test]
fn empty_state_is_answerable() {
    let st = RaceState::new("iracing", SourceKind::Oneshot, Capabilities::ALL);
    let b = st.brief().unwrap();
    assert!(b.session.is_none());
    assert!(!b.meta.history);
    assert!(b.to_text().contains("no telemetry"));
}

#[test]
fn reports_roundtrip_through_json() {
    let st = run(SynthConfig::default(), Some(400.0));
    let b = st.brief().unwrap();
    let back: Brief = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
    assert_eq!(back, b);
    let s = st.sectors(&SectorsParams::default()).unwrap();
    let back: SectorsReport = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
    let s = speak(&st, SpeechTopic::Ahead).unwrap();
    let back: Speech = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
    // Reports of older versions miss the new fields.
    let mut old = serde_json::to_value(&b).unwrap();
    for key in ["live_s", "live_change_s", "live_ahead"] {
        old["ahead"].as_object_mut().unwrap().remove(key);
    }
    old["player"]
        .as_object_mut()
        .unwrap()
        .remove("live_position");
    old.as_object_mut().unwrap().remove("leader");
    let old: Brief = serde_json::from_value(old).unwrap();
    assert_eq!(old.ahead.unwrap().live_s, None);
}

/// A stop is no change of pace: the trend of the gaps leaves it out.
#[test]
fn gap_trends_leave_out_the_stops() {
    let cfg = || SynthConfig {
        yellow: None,
        incident_at: None,
        rival_pit_lap: None,
        ..SynthConfig::default()
    };
    // The player stops on lap 9 (about 12 to 14 minutes in) and the leader
    // never: on the lap after, the gap to the leader has grown by the stop.
    let leader = |until: f64| run(cfg(), Some(until)).brief().unwrap().leader.unwrap();
    let after_stop = leader(1000.0);
    assert_eq!(after_stop.trend_s_per_lap, None, "{after_stop:?}");
    // A clean lap later the pace counts again: the leader is faster by tenths,
    // not by the 25 s of the stop.
    let later = leader(1200.0).trend_s_per_lap.unwrap();
    assert!((0.0..1.5).contains(&later), "{later}");
    // Before the stop there is a trend as always.
    assert!(leader(700.0).trend_s_per_lap.is_some());
}

#[test]
fn pit_road_begins_where_the_cars_drive_into_it() {
    let live = |st: &RaceState| st.live().unwrap().values;
    // Nobody has stopped yet.
    let early = live(&run(SynthConfig::default(), Some(60.0)));
    assert_eq!(early.get("pit_entry"), None);
    assert_eq!(early.get("to_pit"), None);
    // The synthetic pit road begins at 97 % of the lap.
    let st = run(SynthConfig::default(), None);
    let v = live(&st);
    let entry = v["pit_entry"];
    assert!((97.0..=97.2).contains(&entry), "{entry}");
    let to_pit = v["to_pit"];
    let lap_pct = v["lap_pct"];
    let want = (entry - lap_pct).rem_euclid(100.0);
    assert!((to_pit - want).abs() < 0.01, "{to_pit} {want}");
}
