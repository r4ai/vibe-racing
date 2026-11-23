//! The strategy against generated races, whose truth is known.

use vr_core::*;
use vr_engine::*;
use vr_replay::{SynthConfig, synth_frames};

fn race() -> SynthConfig {
    SynthConfig {
        laps: 30,
        tyre_deg_s: 0.2,
        lap_noise_s: 0.15,
        fuel_start_l: 200.0,
        fuel_capacity_l: 200.0,
        player_pit_lap: None,
        rival_pit_lap: None,
        yellow: None,
        // Others stop, so that the cost of it is known.
        stops: vec![(6, 8), (7, 9), (8, 9), (9, 10)],
        ..Default::default()
    }
}

/// The state when the player is in the middle of `lap`.
fn at(cfg: SynthConfig, lap: i32) -> RaceState {
    let player = cfg.player_idx;
    let mut st = RaceState::new("test", SourceKind::Live, Capabilities::ALL);
    for f in synth_frames(cfg, None) {
        let c = &f.cars[player];
        let there = c.lap == Some(lap) && c.lap_dist_pct.is_some_and(|p| p > 0.5);
        st.ingest(f);
        if there {
            return st;
        }
    }
    panic!("lap {lap} was not reached");
}

/// When the player finishes.
fn finish(cfg: SynthConfig) -> f64 {
    let (player, laps) = (cfg.player_idx, cfg.laps);
    synth_frames(cfg, None)
        .into_iter()
        .find(|f| f.cars[player].laps_completed == Some(laps))
        .expect("the player finishes")
        .session
        .time_s
}

#[test]
fn fade_and_pit_loss_are_measured() {
    let r = at(race(), 14).strategy().unwrap();
    assert!(r.missing.is_empty(), "{:?}", r.missing);
    let loss = r.pit_loss.seconds.unwrap();
    // 25 s in the lane; the tyres are new for a part of it.
    assert!((loss - 25.0).abs() < 1.5, "{loss}");
    assert_eq!((r.pit_loss.basis, r.pit_loss.stops), (Basis::Field, 4));
    let f = r.fade.unwrap();
    // Cars differ by 40 %: the player's own value lies between.
    let own = f.own_s_per_lap.unwrap();
    assert!((0.1..0.3).contains(&own), "{own}");
    assert!((f.field_s_per_lap.unwrap() - 0.2).abs() < 0.06, "{f:?}");
    assert_eq!((f.basis, f.fresh_gain_stops), (Basis::Both, 4));
    assert!(f.fresh_gain_s.unwrap() < -0.8, "{f:?}");
    assert_eq!((r.stops_done, r.tyre_age), (0, 14));
    assert_eq!(r.fuel_to_end, Some(true));
}

#[test]
fn the_best_stop_is_near_the_true_one() {
    // Every lap tried out; the race itself tells the truth.
    let times: Vec<(i32, f64)> = (10..=22)
        .map(|lap| {
            let cfg = SynthConfig {
                player_pit_lap: Some(lap),
                ..race()
            };
            (lap, finish(cfg))
        })
        .collect();
    let best = times.iter().map(|t| t.1).fold(f64::INFINITY, f64::min);
    let r = at(race(), 13).strategy().unwrap();
    let one = r.plans.iter().find(|p| p.stops.len() == 1).unwrap();
    let chosen = times.iter().find(|t| t.0 == one.stops[0]).unwrap().1;
    assert!(
        chosen - best < 1.5,
        "lap {} loses {:.1}s: {times:?}",
        one.stops[0],
        chosen - best
    );
}

#[test]
fn fuel_sets_a_window() {
    let cfg = SynthConfig {
        fuel_start_l: 32.0,
        fuel_capacity_l: 51.0,
        fuel_per_lap_l: 2.5,
        ..race()
    };
    let r = at(cfg, 10).strategy().unwrap();
    assert_eq!(r.fuel_to_end, Some(false));
    // A tank is 20.4 laps: not before the end of lap 10. 12.8 laps are on
    // board, of which the way to the line took a part.
    let (from, to) = r.fuel_window.unwrap();
    assert_eq!(from, 10);
    assert!((11..=12).contains(&to), "{to}");
    assert!(r.plans.iter().all(|p| !p.stops.is_empty()));
    for p in r.plans.iter().filter(|p| p.stops.len() == 1) {
        assert!((from..=to).contains(&p.stops[0]), "{p:?}");
    }
    assert!(matches!(
        r.calls.iter().find(|c| matches!(c, StrategyCall::WindowOpen { .. })),
        Some(StrategyCall::WindowOpen { until_lap }) if *until_lap == to
    ));
}

#[test]
fn a_stop_is_placed_among_the_others() {
    let st = at(race(), 12);
    let r = st.strategy().unwrap();
    let j = r.rejoin_now.unwrap();
    let me = st.standings(&StandingsParams::default()).unwrap();
    let mine = me.rows.iter().find(|r| r.is_player).unwrap();
    let after = mine.gap_to_leader_s.unwrap() + r.pit_loss.seconds.unwrap();
    let place = me
        .rows
        .iter()
        .filter(|r| !r.is_player && r.gap_to_leader_s.unwrap() <= after)
        .count() as u32
        + 1;
    assert_eq!(j.position, place);
    assert!(j.position > mine.car.position.unwrap());
    assert!(j.ahead_s.unwrap() >= 0.0);
}

#[test]
fn rivals_show_what_their_stops_would_do() {
    let r = at(race(), 14).strategy().unwrap();
    let behind = r.rivals.iter().find(|c| c.net_gap_s < 0.0).unwrap();
    let ahead = r.rivals.iter().rev().find(|c| c.net_gap_s > 0.0).unwrap();
    // On tyres 14 laps old, a lap on new ones gains a lot more than a tenth.
    assert!(ahead.undercut_gain_s[0] > 1.0, "{ahead:?}");
    assert!(ahead.undercut_gain_s.windows(2).all(|w| w[1] > w[0]));
    assert_eq!(ahead.undercut_gain_s.len(), 4);
    assert!(behind.threat_gain_s[0] > 1.0, "{behind:?}");
    assert!(behind.undercut_gain_s.is_empty());
    assert!(
        r.rivals
            .windows(2)
            .all(|w| w[0].net_gap_s >= w[1].net_gap_s)
    );
}

#[test]
fn strategy_calls_are_made_once_as_they_come_up() {
    // The car behind stops at the end of lap 14.
    let cfg = SynthConfig {
        stops: vec![(6, 8), (7, 9), (8, 9), (1, 14)],
        ..race()
    };
    let st = at(cfg, 16);
    let log = st.event_log().last(200);
    let advice: Vec<&StrategyCall> = log
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Strategy { advice } => Some(advice),
            _ => None,
        })
        .collect();
    let plans = advice
        .iter()
        .filter(|c| matches!(c, StrategyCall::Plan { .. }))
        .count();
    assert!((1..=4).contains(&plans), "{advice:?}");
    let answers: Vec<_> = advice
        .iter()
        .filter_map(|c| match c {
            StrategyCall::Answer { car, within_laps } => Some((car.number.clone(), *within_laps)),
            _ => None,
        })
        .collect();
    // Stopped 3.1 s behind: on new tyres it takes the place in two laps.
    assert_eq!(answers, [("9".to_string(), 2)], "{advice:?}");
    assert!(
        advice
            .iter()
            .any(|c| matches!(c, StrategyCall::UndercutThreat { car, .. } if car.number == "9")),
        "{advice:?}"
    );
    let boxed = advice
        .iter()
        .filter(|c| matches!(c, StrategyCall::BoxThisLap { .. }))
        .count();
    assert_eq!(boxed, 1, "{advice:?}");
}

#[test]
fn without_stops_the_cost_comes_from_earlier_races() {
    let cfg = SynthConfig {
        stops: vec![],
        ..race()
    };
    let mut st = at(cfg, 8);
    let r = st.strategy().unwrap();
    assert!(r.plans.is_empty());
    assert!(r.missing.iter().any(|m| m.contains("nobody stopped")));
    assert_eq!(r.pit_loss.basis, Basis::Unknown);
    st.set_prior_pit_loss(Some(24.0));
    let r = st.strategy().unwrap();
    assert_eq!(
        (r.pit_loss.seconds, r.pit_loss.basis),
        (Some(24.0), Basis::Past)
    );
    assert!(!r.plans.is_empty());

    // What a kept race says about it.
    let mut done = RaceState::new("test", SourceKind::Live, Capabilities::ALL);
    synth_frames(race(), None)
        .into_iter()
        .for_each(|f| done.ingest(f));
    let record = SessionRecord::of(&done).unwrap();
    assert!((pit_loss_of(&record).unwrap() - 25.0).abs() < 1.5);
}

#[test]
fn a_stop_for_fuel_is_planned_before_anybody_stopped() {
    let cfg = SynthConfig {
        stops: vec![],
        fuel_start_l: 32.0,
        fuel_capacity_l: 51.0,
        fuel_per_lap_l: 2.5,
        ..race()
    };
    let r = at(cfg, 7).strategy().unwrap();
    assert_eq!(
        (r.pit_loss.seconds, r.pit_loss.basis),
        (None, Basis::Unknown)
    );
    assert!(r.missing[0].contains("assume 30s"), "{:?}", r.missing);
    let (from, to) = r.fuel_window.unwrap();
    assert!((from..=to).contains(&r.plans[0].stops[0]), "{r:?}");
    let text = r.to_text();
    assert!(text.contains("\nmissing: the cost of a stop"), "{text}");
}

#[test]
fn practice_has_no_strategy() {
    let mut st = RaceState::new("test", SourceKind::Live, Capabilities::ALL);
    assert_eq!(st.strategy().unwrap().missing, ["no data yet"]);
    let mut f = synth_frames(race(), Some(5.0)).pop().unwrap();
    f.session.kind = SessionKind::Practice;
    st.ingest(f);
    assert_eq!(st.strategy().unwrap().missing, ["not a race"]);
}

fn brief_lap(st: &RaceState) -> f64 {
    let lap = st.brief().unwrap().ahead.unwrap().their_last_lap_s.unwrap();
    (lap * 1000.0).round() / 1000.0
}

#[test]
fn live_values_are_flat_numbers() {
    let mut cfg = race();
    cfg.yellow = Some((600.0, 700.0));
    let st = at(cfg.clone(), 4);
    let r = st.live().unwrap();
    let known: Vec<&str> = LIVE_KEYS.iter().map(|k| k.0).collect();
    for k in r.values.keys() {
        assert!(known.contains(&k.as_str()), "{k} is not described");
    }
    let v = |k: &str| *r.values.get(k).unwrap_or_else(|| panic!("{k}"));
    assert_eq!((v("lap"), v("racing"), v("yellow")), (4.0, 1.0, 0.0));
    assert!((50.0..52.0).contains(&v("lap_pct")), "{}", v("lap_pct"));
    assert_eq!((v("stops"), v("tyre_age"), v("on_pit")), (0.0, 4.0, 0.0));
    assert_eq!((v("cars"), v("alongside")), (cfg.cars as f64, 0.0));
    let brief = st.brief().unwrap();
    // As it is now, not as it was at the line.
    let gap = brief.ahead.unwrap().live_s.unwrap();
    assert!((v("gap_ahead") - gap).abs() < 0.001);
    assert_eq!((v("left"), v("right")), (0.0, 0.0));
    assert_eq!(v("live_position"), v("position"));
    assert_eq!(v("ahead_lap"), brief_lap(&st));
    assert!(v("near_behind") > 0.0);
    assert!(v("speed") > 50.0);
    // The lap after the planned stop there is none.
    assert!(!r.values.contains_key("stop_in"));

    let cfg = SynthConfig {
        player_pit_lap: Some(5),
        yellow: Some((600.0, 700.0)),
        ..race()
    };
    let st = at(cfg, 7);
    let v = st.live().unwrap().values;
    assert_eq!((v["stops"], v["tyre_age"]), (1.0, 2.0));
    assert_eq!((v["yellow"], v["caution"]), (1.0, 0.0));
    let text = st.live().unwrap().to_text();
    assert!(
        text.contains("\nstops 1\n") || text.contains("\nstops 1 | "),
        "{text}"
    );
    assert!(text.contains("\ndelta_best -- | "), "{text}");

    let st = RaceState::new("test", SourceKind::Live, Capabilities::ALL);
    assert!(st.live().unwrap().values.is_empty());
}

#[test]
fn the_planned_stop_counts_down() {
    let st = at(race(), 14);
    let plan = st.strategy().unwrap().plans[0].stops[0];
    let v = st.live().unwrap().values["stop_in"];
    assert!((v - f64::from(plan - 14)).abs() <= 2.0, "{v} {plan}");
}
