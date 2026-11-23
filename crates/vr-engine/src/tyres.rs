//! The class by the tyres they are on: who changed to what, and how fast they
//! go on it. What a call between dry and wet tyres is made from.

use vr_core::{CarState, Frame};

use crate::RaceState;
use crate::report::{LapRecord, TyreGroup};
use crate::state::car_ref;
use crate::strategy::median;

/// Clean laps since the last stop that make a car's pace on its tyres.
const PACE_LAPS: usize = 3;

/// The groups, fastest first; empty when everyone is on the same tyres on a
/// dry track, or the game does not tell the tyres of the others.
pub(crate) fn field_tyres(state: &RaceState, f: &Frame) -> Vec<TyreGroup> {
    let Some(me) = f.player_car() else {
        return Vec::new();
    };
    let class: Vec<&CarState> = f
        .cars
        .iter()
        .filter(|c| c.class_id == me.class_id && c.extra.tyre_compound.is_some())
        .collect();
    groups(
        &class,
        me.idx,
        |idx| state.car_laps(idx),
        f.weather.as_ref().is_some_and(wet),
    )
}

fn wet(w: &vr_core::Weather) -> bool {
    w.declared_wet == Some(true)
        || w.wetness
            .as_deref()
            .is_some_and(|s| s != "dry" && s != "mostly dry")
}

/// A car's pace on the tyres it is on: its clean laps since the last stop.
fn pace(laps: &[LapRecord]) -> Option<f64> {
    let after = laps.iter().rposition(|l| l.pit).map_or(0, |i| i + 1);
    let clean: Vec<f64> = laps[after..]
        .iter()
        .filter(|l| l.is_clean())
        .filter_map(|l| l.time_s)
        .rev()
        .take(PACE_LAPS)
        .collect();
    median(&clean)
}

fn groups(
    cars: &[&CarState],
    me: usize,
    laps: impl Fn(usize) -> Vec<LapRecord>,
    wet: bool,
) -> Vec<TyreGroup> {
    let mut out: Vec<TyreGroup> = Vec::new();
    for c in cars {
        let Some(compound) = c.extra.tyre_compound.clone() else {
            continue;
        };
        let p = pace(&laps(c.idx));
        let i = match out.iter().position(|g| g.compound == compound) {
            Some(i) => i,
            None => {
                out.push(TyreGroup {
                    compound,
                    cars: Vec::new(),
                    you: false,
                    pace_s: None,
                    fastest: None,
                    fastest_s: None,
                });
                out.len() - 1
            }
        };
        let g = &mut out[i];
        g.cars.push(car_ref(c));
        g.you |= c.idx == me;
        if let Some(p) = p
            && g.fastest_s.is_none_or(|f| p < f)
        {
            g.fastest_s = Some(p);
            g.fastest = Some(car_ref(c));
        }
    }
    if out.len() < 2 && !wet {
        return Vec::new();
    }
    for g in &mut out {
        let paces: Vec<f64> = g.cars.iter().filter_map(|c| pace(&laps(c.idx))).collect();
        g.pace_s = median(&paces);
    }
    out.sort_by(|a, b| match (a.pace_s, b.pace_s) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.cars.len().cmp(&a.cars.len()),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lap(lap: i32, t: f64, pit: bool) -> LapRecord {
        LapRecord {
            lap,
            time_s: Some(t),
            fuel_used_l: None,
            pit,
            caution: false,
            position: None,
            session_time_s: 0.0,
        }
    }

    fn car(idx: usize, compound: &str) -> CarState {
        let mut c = CarState {
            idx,
            number: format!("{}", idx + 1),
            driver: String::new(),
            team: None,
            car_name: None,
            class_name: None,
            class_id: None,
            rating: None,
            is_player: idx == 0,
            position: None,
            class_position: None,
            lap: None,
            laps_completed: None,
            lap_dist_pct: None,
            on_pit_road: false,
            surface: vr_core::TrackSurface::OnTrack,
            last_lap_s: None,
            best_lap_s: None,
            gap_to_leader_s: None,
            est_lap_s: None,
            extra: Default::default(),
        };
        c.extra.tyre_compound = Some(compound.into());
        c
    }

    #[test]
    fn the_pace_on_the_tyres_is_from_the_laps_since_the_stop() {
        // Wet laps, the stop, the out-lap, then laps on the new tyres.
        let laps = [
            lap(1, 130.0, false),
            lap(2, 131.0, false),
            lap(3, 150.0, true),
            lap(4, 124.0, false),
            lap(5, 122.0, false),
        ];
        assert_eq!(pace(&laps), Some(123.0));
        // Just changed: nothing on them yet.
        assert_eq!(pace(&laps[..3]), None);
        // Never stopped: the last three clean ones.
        assert_eq!(pace(&laps[..2]), Some(130.5));
    }

    #[test]
    fn the_field_by_tyres_fastest_first() {
        let cars = [
            car(0, "Wet"),
            car(1, "Wet"),
            car(2, "Dry"),
            car(3, "Dry"),
            car(4, "Wet"),
        ];
        let refs: Vec<&CarState> = cars.iter().collect();
        let laps = |idx: usize| match idx {
            0 => vec![lap(5, 130.0, false)],
            1 => vec![lap(5, 128.0, false)],
            2 => vec![lap(4, 140.0, true), lap(5, 121.0, false)],
            3 => vec![lap(5, 140.0, true)],
            _ => vec![lap(5, 132.0, false)],
        };
        let g = groups(&refs, 0, laps, true);
        assert_eq!(g.len(), 2);
        assert_eq!(
            (g[0].compound.as_str(), g[0].cars.len(), g[0].you),
            ("Dry", 2, false)
        );
        // Only car 3 has a clean lap on the dry tyres.
        assert_eq!((g[0].pace_s, g[0].fastest_s), (Some(121.0), Some(121.0)));
        assert_eq!(g[0].fastest.as_ref().map(|c| c.idx), Some(2));
        assert_eq!(
            (g[1].compound.as_str(), g[1].you, g[1].pace_s),
            ("Wet", true, Some(130.0))
        );
        assert_eq!(g[1].fastest.as_ref().map(|c| c.idx), Some(1));
        // Everyone on the same tyres: only worth telling when it is wet.
        let same: Vec<&CarState> = cars.iter().filter(|c| c.idx != 2 && c.idx != 3).collect();
        assert!(groups(&same, 0, laps, false).is_empty());
        assert_eq!(groups(&same, 0, laps, true).len(), 1);
    }
}

#[cfg(test)]
mod call {
    use crate::report::{CarRef, TyreGroup};
    use crate::strategy::tyre_call;

    fn group(compound: &str, you: bool, pace: Option<f64>) -> TyreGroup {
        TyreGroup {
            compound: compound.into(),
            cars: vec![CarRef {
                idx: 0,
                number: "1".into(),
                driver: String::new(),
                class_name: None,
                position: None,
                class_position: None,
            }],
            you,
            pace_s: pace,
            fastest: None,
            fastest_s: None,
        }
    }

    #[test]
    fn a_change_pays_when_the_gain_outruns_the_stop() {
        let g = [
            group("Dry", false, Some(120.0)),
            group("Wet", true, Some(123.2)),
        ];
        // 3.2 s a lap pays a 24.5 s stop back in 7.7 laps.
        let c = tyre_call(&g, Some(24.5), Some(11)).unwrap();
        assert_eq!(c.compound, "Dry");
        assert!((c.gain_s_per_lap - 3.2).abs() < 1e-9);
        assert!((c.payback_laps.unwrap() - 24.5 / 3.2).abs() < 1e-9);
        assert_eq!(c.worth, Some(true));
        assert_eq!(
            tyre_call(&g, Some(24.5), Some(7)).unwrap().worth,
            Some(false)
        );
        // Exactly paid back on the last lap is no gain.
        assert_eq!(
            tyre_call(&g, Some(32.0), Some(10)).unwrap().worth,
            Some(false)
        );
        // Without the cost of a stop: the gain alone.
        let c = tyre_call(&g, None, Some(11)).unwrap();
        assert_eq!((c.payback_laps, c.worth), (None, None));
        // Less than half a second, or yours the fastest: nothing to call.
        let close = [
            group("Dry", false, Some(122.8)),
            group("Wet", true, Some(123.2)),
        ];
        assert!(tyre_call(&close, Some(24.5), Some(11)).is_none());
        let at = [
            group("Dry", false, Some(122.7)),
            group("Wet", true, Some(123.2)),
        ];
        assert!(tyre_call(&at, Some(24.5), Some(11)).is_some());
        let mine = [
            group("Dry", true, Some(120.0)),
            group("Wet", false, Some(123.2)),
        ];
        assert!(tyre_call(&mine, Some(24.5), Some(11)).is_none());
        assert!(
            tyre_call(
                &[group("Wet", true, None), group("Dry", false, Some(1.0))],
                None,
                None
            )
            .is_none()
        );
    }
}
