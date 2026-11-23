//! What Assetto Corsa does not keep but the analysis needs: the session's
//! number and clock, where the sectors begin, how fast the suspension moves.

/// Steps longer than this are pauses, not driving.
const MAX_STEP_S: f64 = 1.0;
/// Driven distance falling by more than this is a restart.
const RESTART_M: f64 = 50.0;
/// The countdown of a timed session rising by more than this is a restart.
const RESTART_LEFT_S: f64 = 5.0;

/// Where in the session a sample is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    pub session_num: i32,
    pub uid: i64,
    pub time_s: f64,
}

/// What one sample tells about the session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sample {
    pub now_ms: i64,
    /// `graphics.session`.
    pub kind: i32,
    /// Car, track and layout: a new one is a new session.
    pub event: String,
    pub completed_laps: i32,
    pub distance_m: f64,
    /// Of a timed session.
    pub time_left_s: Option<f64>,
}

#[derive(Debug, Default)]
pub struct Tracker {
    clock: Option<Clock>,
    last: Option<Sample>,
    sectors: Sectors,
    travel: Option<([Option<f64>; 4], f64)>,
}

impl Tracker {
    pub fn advance(&mut self, s: &Sample) -> Clock {
        let mut c = self.clock.unwrap_or(Clock {
            session_num: 0,
            uid: 0,
            time_s: 0.0,
        });
        if let Some(last) = &self.last {
            let new_session = last.kind != s.kind || last.event != s.event;
            let restarted = s.completed_laps < last.completed_laps
                || s.distance_m < last.distance_m - RESTART_M
                || s.time_left_s
                    .zip(last.time_left_s)
                    .is_some_and(|(now, before)| now > before + RESTART_LEFT_S);
            if new_session {
                c.session_num += 1;
                c.time_s = 0.0;
                self.travel = None;
            } else if restarted {
                c.uid += 1;
                c.time_s = 0.0;
                self.travel = None;
            } else {
                let step = (s.now_ms - last.now_ms) as f64 / 1000.0;
                c.time_s += step.clamp(0.0, MAX_STEP_S);
            }
            if last.event != s.event {
                self.sectors = Sectors::default();
            }
        }
        self.clock = Some(c);
        self.last = Some(s.clone());
        c
    }

    /// Where the sectors begin, once each has been entered from the one
    /// before it.
    pub fn sectors(&mut self, count: i32, index: i32, at: f64) -> Vec<f64> {
        self.sectors.see(count, index, at)
    }

    /// How fast each wheel's suspension moves, positive in bump, from the
    /// travel now and before.
    pub fn shock(&mut self, travel: [Option<f64>; 4], time_s: f64) -> [Option<f64>; 4] {
        let before = self.travel.replace((travel, time_s));
        let Some((prev, t0)) = before else {
            return [None; 4];
        };
        let dt = time_s - t0;
        if !(1e-4..=0.25).contains(&dt) {
            return [None; 4];
        }
        std::array::from_fn(|i| Some((travel[i]? - prev[i]?) / dt))
    }
}

#[derive(Debug, Default)]
struct Sectors {
    count: i32,
    starts: Vec<Option<f64>>,
    index: Option<i32>,
}

impl Sectors {
    fn see(&mut self, count: i32, index: i32, at: f64) -> Vec<f64> {
        if !(2..=100).contains(&count) {
            return Vec::new();
        }
        if count != self.count {
            *self = Self {
                count,
                starts: vec![None; count as usize],
                index: None,
            };
            self.starts[0] = Some(0.0);
        }
        if let Some(before) = self.index
            && index == before + 1
            && (1..count).contains(&index)
            && (0.0..1.0).contains(&at)
        {
            self.starts[index as usize] = Some(at);
        }
        self.index = Some(index);
        let starts: Option<Vec<f64>> = self.starts.iter().copied().collect();
        starts
            .filter(|s| s.windows(2).all(|w| w[0] < w[1]))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(now_ms: i64, laps: i32, dist: f64) -> Sample {
        Sample {
            now_ms,
            kind: 2,
            event: "car@track".into(),
            completed_laps: laps,
            distance_m: dist,
            time_left_s: None,
        }
    }

    #[test]
    fn the_clock_runs_while_driving_and_restarts_with_the_session() {
        let mut t = Tracker::default();
        let c = t.advance(&at(1_000, 0, 0.0));
        assert_eq!(
            c,
            Clock {
                session_num: 0,
                uid: 0,
                time_s: 0.0
            }
        );
        assert_eq!(t.advance(&at(1_250, 0, 10.0)).time_s, 0.25);
        // A pause of a minute counts as one second at most.
        assert_eq!(t.advance(&at(61_250, 0, 20.0)).time_s, 1.25);
        // A clock going back counts as nothing.
        assert_eq!(t.advance(&at(61_000, 1, 30.0)).time_s, 1.25);
        // Fewer laps: the same session again.
        let c = t.advance(&at(61_100, 0, 30.0));
        assert_eq!(
            c,
            Clock {
                session_num: 0,
                uid: 1,
                time_s: 0.0
            }
        );
        // Less distance: again.
        t.advance(&at(61_200, 0, 500.0));
        let c = t.advance(&at(61_300, 0, 449.0));
        assert_eq!((c.uid, c.time_s), (2, 0.0));
        let c = t.advance(&at(61_400, 0, 399.0));
        assert_eq!(
            (c.uid, c.time_s),
            (2, 0.1),
            "exactly 50 m back is not a restart"
        );
        // A timed session's countdown going up.
        let timed = |now_ms, left| Sample {
            time_left_s: Some(left),
            ..at(now_ms, 0, 400.0)
        };
        t.advance(&timed(61_500, 100.0));
        assert_eq!(t.advance(&timed(61_600, 105.0)).uid, 2);
        assert_eq!(t.advance(&timed(61_700, 110.1)).uid, 3);
        // Qualifying after the race: a new session.
        let c = t.advance(&Sample {
            kind: 1,
            ..at(62_000, 0, 400.0)
        });
        assert_eq!(
            c,
            Clock {
                session_num: 1,
                uid: 3,
                time_s: 0.0
            }
        );
        // Another car or track: another session too.
        let c = t.advance(&Sample {
            kind: 1,
            event: "other@track".into(),
            ..at(62_100, 0, 400.0)
        });
        assert_eq!((c.session_num, c.time_s), (2, 0.0));
    }

    #[test]
    fn sectors_are_learned_from_their_entries() {
        let mut t = Tracker::default();
        assert_eq!(t.sectors(1, 0, 0.5), Vec::<f64>::new(), "one sector");
        assert_eq!(t.sectors(0, 0, 0.5), Vec::<f64>::new(), "unknown");
        assert_eq!(
            t.sectors(3, 1, 0.4),
            Vec::<f64>::new(),
            "joined in sector 1"
        );
        assert_eq!(
            t.sectors(3, 2, 0.7),
            Vec::<f64>::new(),
            "sector 1 still unknown"
        );
        assert_eq!(t.sectors(3, 0, 0.01), Vec::<f64>::new());
        // Sector 2 was seen begin before.
        assert_eq!(t.sectors(3, 1, 0.33), vec![0.0, 0.33, 0.7]);
        assert_eq!(t.sectors(3, 1, 0.5), vec![0.0, 0.33, 0.7], "still in it");
        assert_eq!(
            t.sectors(3, 2, 0.71),
            vec![0.0, 0.33, 0.71],
            "the latest counts"
        );
        // Jumping back (a reset to the pits) changes nothing.
        assert_eq!(t.sectors(3, 0, 0.9), vec![0.0, 0.33, 0.71]);
        // A boundary out of order is not used.
        assert_eq!(t.sectors(3, 1, 0.8), Vec::<f64>::new());
        // A new track starts over.
        t.advance(&at(0, 0, 0.0));
        t.advance(&Sample {
            event: "new".into(),
            ..at(1, 0, 0.0)
        });
        assert_eq!(t.sectors(3, 2, 0.6), Vec::<f64>::new());
        assert_eq!(t.sectors(101, 0, 0.6), Vec::<f64>::new(), "implausible");
    }

    #[test]
    fn damper_speeds_come_from_the_travel() {
        let mut t = Tracker::default();
        let travel = |v: f64| [Some(v), Some(v), None, Some(0.0)];
        assert_eq!(t.shock(travel(0.010), 1.0), [None; 4]);
        let s = t.shock(travel(0.012), 1.02);
        assert!((s[0].unwrap() - 0.1).abs() < 1e-9, "{s:?}");
        assert_eq!(s[2], None);
        assert_eq!(s[3], Some(0.0));
        assert_eq!(t.shock(travel(0.012), 1.5), [None; 4], "a gap");
        assert_eq!(t.shock(travel(0.012), 1.5), [None; 4], "no time passed");
    }
}
