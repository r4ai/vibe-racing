//! What changes from moment to moment, as one flat set of numbers: for rules
//! that are checked many times a second.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use vr_core::{Flag, Frame, SessionKind, SessionPhase, TrackSurface};

use crate::RaceState;
use crate::report::Meta;
use crate::state::is_multi_class;

/// The values and what they mean. Switches are 0 or 1.
pub const LIVE_KEYS: &[(&str, &str)] = &[
    ("speed", "km/h"),
    ("throttle", "0-100"),
    ("brake", "0-100"),
    ("steer", "degrees, positive to the left"),
    ("lat_g", "g, positive to the left"),
    ("long_g", "g, negative when braking"),
    ("gear", "-1 reverse, 0 neutral"),
    ("rpm", ""),
    ("abs", "ABS at work, 0/1"),
    ("tc", "traction control at work, 0/1"),
    ("lap", "lap the car is on"),
    ("lap_pct", "progress around the lap, 0-100"),
    ("lap_m", "metres from the line"),
    ("lap_time", "seconds into the lap"),
    ("last_lap", "s"),
    ("best_lap", "s"),
    (
        "delta_best",
        "s against the own best lap, negative is faster",
    ),
    ("position", "in class"),
    ("cars", "in class"),
    (
        "laps_left",
        "laps to the flag, the rest of this one included",
    ),
    ("time_left", "s"),
    ("session_time", "s"),
    ("gap_ahead", "s to the car one place ahead"),
    ("gap_behind", "s to the car one place behind"),
    ("trend_ahead", "s per lap, negative: closing in"),
    ("trend_behind", "s per lap, negative: being caught"),
    ("near_ahead", "s to the next car on track, any class or lap"),
    (
        "near_behind",
        "s to the car behind on track, any class or lap",
    ),
    (
        "lapping_behind",
        "s to a car behind on track that is laps ahead",
    ),
    ("near_ahead_m", "m along the track to the next car ahead"),
    ("near_behind_m", "m along the track to the next car behind"),
    ("alongside", "a car beside, 0/1"),
    ("left", "cars beside on the left, 0-2"),
    ("right", "cars beside on the right, 0-2"),
    ("live_position", "in class, as the cars run on track now"),
    ("gap_leader", "s to the leader of the class"),
    ("leader_lap", "s, last lap of the leader of the class"),
    ("ahead_lap", "s, last lap of the car one place ahead"),
    ("fuel", "litres"),
    ("fuel_laps", "laps of fuel"),
    ("fuel_spare", "litres left at the flag, negative: short"),
    ("stops", "pit stops made"),
    ("tyre_age", "laps"),
    ("stop_in", "laps to the planned stop, 0: end of this lap"),
    ("on_pit", "on pit road, 0/1"),
    ("in_stall", "0/1"),
    ("on_track", "in the car, 0/1"),
    ("off_track", "0/1"),
    ("incidents", "points"),
    ("water", "degrees C"),
    ("oil", "degrees C"),
    ("track_temp", "degrees C"),
    ("rain", "0-100"),
    ("p2p", "push-to-pass left"),
    ("drs", "DRS open, 0/1"),
    ("drs_available", "DRS may be opened, 0/1"),
    ("damage", "the worst damaged part, 0-100"),
    ("lap_invalid", "this lap will not count, 0/1"),
    ("yellow", "local or full course, 0/1"),
    ("caution", "full course, 0/1"),
    ("blue", "0/1"),
    ("white", "last lap, 0/1"),
    ("racing", "a race under way, 0/1"),
    ("chequered", "the chequered flag is out, 0/1"),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveReport {
    pub meta: Meta,
    /// Only what is known at the moment.
    pub values: BTreeMap<String, f64>,
}

/// Cars further away on track are not near.
const NEAR_S: f64 = 30.0;
const G: f64 = 9.806_65;

pub(crate) fn live(state: &RaceState) -> LiveReport {
    LiveReport {
        meta: state.meta(),
        values: state
            .frame()
            .map(|f| values(state, f))
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, v)| v.is_finite())
            .map(|(k, v)| (k.to_string(), (v * 1000.0).round() / 1000.0))
            .collect(),
    }
}

/// Cars on the left and on the right.
fn beside(alongside: Option<&str>) -> (f64, f64) {
    match alongside {
        Some("left") => (1.0, 0.0),
        Some("right") => (0.0, 1.0),
        Some("left and right") => (1.0, 1.0),
        Some("two on the left") => (2.0, 0.0),
        Some("two on the right") => (0.0, 2.0),
        _ => (0.0, 0.0),
    }
}

fn values(state: &RaceState, f: &Frame) -> BTreeMap<&'static str, f64> {
    let mut out = BTreeMap::new();
    let mut put = |k: &'static str, v: Option<f64>| {
        if let Some(v) = v {
            out.insert(k, v);
        }
    };
    let on = |b: bool| Some(f64::from(u8::from(b)));
    let flag = |wanted: &[Flag]| f.session.flags.iter().any(|x| wanted.contains(x));
    let race = f.session.kind == SessionKind::Race;

    put("session_time", Some(f.session.time_s));
    put(
        "time_left",
        f.session.time_remaining_s.filter(|t| *t >= 0.0),
    );
    put(
        "yellow",
        on(flag(&[Flag::Yellow, Flag::YellowWaving, Flag::Caution])),
    );
    put("caution", on(flag(&[Flag::Caution])));
    put("blue", on(flag(&[Flag::Blue])));
    put("white", on(flag(&[Flag::White])));
    put(
        "racing",
        on(race && f.session.phase == SessionPhase::Racing),
    );
    put(
        "chequered",
        on(f.session.phase == SessionPhase::Checkered || flag(&[Flag::Checkered])),
    );
    if let Some(w) = &f.weather {
        put("track_temp", w.track_temp_c);
        put("rain", w.precipitation.map(|p| p * 100.0));
    }
    if let Some(p) = &f.player {
        let c = &p.car;
        put("speed", p.speed_mps.map(|v| v * 3.6));
        put("throttle", p.throttle.map(|v| v * 100.0));
        put("brake", p.brake.map(|v| v * 100.0));
        put("steer", c.steering_rad.map(f64::to_degrees));
        put("lat_g", c.lat_accel_mps2.map(|a| a / G));
        put("long_g", c.long_accel_mps2.map(|a| a / G));
        put("gear", p.gear.map(f64::from));
        put("rpm", p.rpm);
        put("abs", c.abs_active.and_then(on));
        put("tc", c.tc_active.and_then(on));
        put("drs", c.drs_active.and_then(on));
        put("drs_available", c.drs_available.and_then(on));
        put(
            "damage",
            c.damage.as_ref().and_then(|d| d.worst()).map(|d| d * 100.0),
        );
        put("lap_time", p.current_lap_s.filter(|t| *t >= 0.0));
        put("delta_best", p.delta_to_best_s);
        put("fuel", p.fuel_l);
        put("incidents", p.incidents.map(f64::from));
        put("water", p.water_temp_c);
        put("oil", p.oil_temp_c);
        put("p2p", c.p2p_count.map(f64::from));
        put("in_stall", c.pit.in_stall.and_then(on));
        put("on_track", on(p.on_track));
        let (ahead, behind) = state.near_m(f);
        put("near_ahead_m", ahead);
        put("near_behind_m", behind);
        put("alongside", on(c.alongside.is_some()));
        let (left, right) = beside(c.alongside.as_deref());
        put("left", Some(left));
        put("right", Some(right));
    }
    if let Some(i) = state.fuel_info(f) {
        put("fuel_laps", i.laps_of_fuel);
        put("fuel_spare", i.to_finish_l.map(|l| -l));
        put("laps_left", i.laps_to_finish);
    }
    if let Some(car) = f.player_car() {
        let multi = is_multi_class(f);
        let place = if multi {
            car.class_position
        } else {
            car.position
        };
        let lap = car.lap;
        put("lap", lap.map(f64::from));
        put("lap_pct", car.lap_dist_pct.map(|p| p * 100.0));
        let length = f.session.track.as_ref().and_then(|t| t.length_m);
        put("lap_m", length.zip(car.lap_dist_pct).map(|(l, p)| l * p));
        put("last_lap", car.last_lap_s.filter(|t| *t > 0.0));
        put("best_lap", car.best_lap_s.filter(|t| *t > 0.0));
        put("position", place.map(f64::from));
        let same = f.cars.iter().filter(|c| c.class_id == car.class_id);
        put("cars", Some(same.count() as f64));
        put("on_pit", on(car.on_pit_road));
        put("off_track", on(car.surface == TrackSurface::OffTrack));
        put("lap_invalid", car.extra.lap_invalidated.and_then(on));
        for (offset, gap, trend) in [
            (-1, "gap_ahead", "trend_ahead"),
            (1, "gap_behind", "trend_behind"),
        ] {
            if let Some(g) = state.adjacent(f, offset) {
                put(
                    gap,
                    g.live_s
                        .filter(|_| g.live_ahead == Some(offset < 0))
                        .or(g.gap_s),
                );
                put(trend, g.trend_s_per_lap);
                if offset < 0 {
                    put("ahead_lap", g.their_last_lap_s);
                }
            }
        }
        put("live_position", state.live_position(f).map(f64::from));
        let first = state.leader(f).or_else(|| {
            state
                .adjacent(f, -1)
                .filter(|g| g.car.class_position.or(g.car.position) == Some(1))
        });
        if let Some(g) = first {
            put("gap_leader", g.gap_s.filter(|_| race));
            put("leader_lap", g.their_last_lap_s);
        }
        let near = state.nearby(f, Some(NEAR_S));
        let least = |v: &mut dyn Iterator<Item = f64>| v.min_by(f64::total_cmp);
        put(
            "near_ahead",
            least(&mut near.iter().map(|c| c.rel_s).filter(|s| *s > 0.0)),
        );
        put(
            "near_behind",
            least(&mut near.iter().map(|c| -c.rel_s).filter(|s| *s > 0.0)),
        );
        put(
            "lapping_behind",
            least(
                &mut near
                    .iter()
                    .filter(|c| c.lap_diff > 0)
                    .map(|c| -c.rel_s)
                    .filter(|s| race && *s > 0.0),
            ),
        );
        let laps = state.car_laps(car.idx);
        if let Some(lap) = lap {
            let (stops, age) = crate::strategy::stint(&laps, lap);
            put("stops", Some(stops as f64));
            put("tyre_age", Some(f64::from(age)));
            put(
                "stop_in",
                state
                    .next_stop()
                    .filter(|s| *s >= lap)
                    .map(|s| f64::from(s - lap)),
            );
        }
    }
    out
}
