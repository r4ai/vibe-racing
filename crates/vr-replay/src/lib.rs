//! File-based and synthetic telemetry sources.
//!
//! * [`ReplaySource`] / [`Recorder`]: normalized [`vr_core::Frame`]s as NDJSON,
//!   so any game recorded once can be replayed without the game.
//! * [`SynthSource`]: a deterministic generated race (pit stops, a yellow,
//!   an incident) for demos and tests.

mod replay;
mod synth;

pub use replay::*;
pub use synth::*;

use std::time::{Duration, Instant};

/// Paces playback so that session time advances `speed`× faster than wall
/// clock. `speed <= 0` means "as fast as possible".
#[derive(Debug)]
pub(crate) struct Pacer {
    speed: f64,
    origin: Option<(Instant, f64)>,
}

impl Pacer {
    pub(crate) fn new(speed: f64) -> Self {
        Self {
            speed,
            origin: None,
        }
    }

    /// Wait (at most `timeout`) until session time `t` is due. Returns false
    /// if it is still not due after waiting.
    pub(crate) fn wait_until(&mut self, t: f64, timeout: Duration) -> bool {
        if self.speed <= 0.0 {
            return true;
        }
        let (start, t0) = *self.origin.get_or_insert((Instant::now(), t));
        if t < t0 {
            // Session time jumped backwards (new session): re-anchor.
            self.origin = Some((Instant::now(), t));
            return true;
        }
        let due = start + Duration::from_secs_f64((t - t0) / self.speed);
        let now = Instant::now();
        if due <= now {
            return true;
        }
        let wait = due - now;
        if wait > timeout {
            std::thread::sleep(timeout);
            false
        } else {
            std::thread::sleep(wait);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacer_schedules_relative_to_first_sample() {
        // Anchor at t=5 s; 10x speed -> t=6 s is due 100 ms later.
        let mut p = Pacer::new(10.0);
        assert!(p.wait_until(5.0, Duration::ZERO));
        let t0 = Instant::now();
        assert!(
            !p.wait_until(6.0, Duration::from_millis(20)),
            "not due within 20 ms"
        );
        assert!(p.wait_until(6.0, Duration::from_secs(1)));
        let el = t0.elapsed();
        assert!(el >= Duration::from_millis(95), "{el:?}");
        assert!(el < Duration::from_millis(400), "{el:?}");
        // Already due: no wait.
        let t1 = Instant::now();
        assert!(p.wait_until(5.5, Duration::from_secs(1)));
        assert!(t1.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn pacer_reanchors_on_time_going_backwards() {
        let mut p = Pacer::new(1.0);
        assert!(p.wait_until(100.0, Duration::ZERO));
        let t0 = Instant::now();
        assert!(
            p.wait_until(1.0, Duration::from_secs(5)),
            "new session starts immediately"
        );
        // Re-anchored at t=1: t=1.05 is due 50 ms later, not ~99 s ago.
        assert!(!p.wait_until(1.05, Duration::from_millis(10)));
        assert!(t0.elapsed() < Duration::from_millis(200));
        assert!(
            p.wait_until(1.0, Duration::ZERO),
            "same time as anchor is due"
        );
    }

    #[test]
    fn pacer_unpaced() {
        let mut p = Pacer::new(0.0);
        assert!(p.wait_until(1e9, Duration::ZERO));
    }
}
