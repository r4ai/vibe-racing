use std::time::{Duration, Instant};

use vr_core::{Poll, SourceError, TelemetrySource};
use vr_replay::*;

fn short_race() -> Vec<vr_core::Frame> {
    synth_frames(
        SynthConfig {
            laps: 2,
            cars: 4,
            ..Default::default()
        },
        Some(20.0),
    )
}

fn drain(src: &mut dyn TelemetrySource) -> Vec<vr_core::Frame> {
    let mut out = Vec::new();
    loop {
        match src.poll(Duration::from_millis(50)).unwrap() {
            Poll::Frame(f) => out.push(*f),
            Poll::Idle => {}
            Poll::Disconnected => return out,
        }
    }
}

#[test]
fn record_then_replay_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("race.ndjson");
    let frames = short_race();
    {
        let mut rec = Recorder::create(&path, 0.0).unwrap();
        for f in &frames {
            rec.record(f).unwrap();
        }
        rec.flush().unwrap();
    }
    let mut src = ReplaySource::open(&path, 0.0).unwrap();
    assert_eq!(src.game(), "synthetic", "game is taken from the recording");
    assert!(!src.reconnectable());
    assert_eq!(src.capabilities(), vr_core::Capabilities::ALL);
    let back = drain(&mut src);
    assert_eq!(back, frames);
    assert!(
        matches!(src.poll(Duration::ZERO).unwrap(), Poll::Disconnected),
        "stays ended"
    );
}

#[test]
fn recorder_downsamples_but_keeps_session_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.ndjson");
    let mut frames = short_race(); // 10 Hz for 20 s => 200 frames
    let n = frames.len();
    let mut next = frames[0].clone();
    next.session.session_num = 1;
    next.session.time_s = 0.0;
    frames.push(next);
    {
        let mut rec = Recorder::create(&path, 2.0).unwrap();
        for f in &frames {
            rec.record(f).unwrap();
        }
    } // flushed on drop
    let back = drain(&mut ReplaySource::open(&path, 0.0).unwrap());
    // ~2 Hz over 20 s, plus the new session's first frame.
    assert!(
        (38..=42).contains(&(back.len() - 1)),
        "{} of {n}",
        back.len()
    );
    assert_eq!(back.last().unwrap().session.session_num, 1);
    for w in back[..back.len() - 1].windows(2) {
        assert!(w[1].session.time_s - w[0].session.time_s >= 0.5 - 1e-9);
    }
}

#[test]
fn replay_skips_blank_lines_and_rejects_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.ndjson");
    let f = serde_json::to_string(&short_race()[0]).unwrap();
    std::fs::write(&path, format!("\n{f}\n\n{{not json\n")).unwrap();
    let mut src = ReplaySource::open(&path, 0.0).unwrap();
    assert!(matches!(src.poll(Duration::ZERO).unwrap(), Poll::Frame(_)));
    assert!(matches!(
        src.poll(Duration::ZERO),
        Err(SourceError::Format(_))
    ));
}

#[test]
fn empty_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.ndjson");
    std::fs::write(&path, "").unwrap();
    let mut src = ReplaySource::open(&path, 0.0).unwrap();
    assert_eq!(src.game(), "replay");
    assert!(matches!(
        src.poll(Duration::ZERO).unwrap(),
        Poll::Disconnected
    ));
    assert!(matches!(
        ReplaySource::open(dir.path().join("nope"), 0.0),
        Err(SourceError::Io(_))
    ));
}

#[test]
fn replay_is_paced_by_session_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paced.ndjson");
    let frames = synth_frames(SynthConfig::default(), Some(1.0)); // 10 frames, 0.1..1.0 s
    let mut rec = Recorder::create(&path, 0.0).unwrap();
    for f in &frames {
        rec.record(f).unwrap();
    }
    drop(rec);
    // 4x speed: 0.9 s of session time => ~225 ms.
    let mut src = ReplaySource::open(&path, 4.0).unwrap();
    let t0 = Instant::now();
    // A tiny timeout yields Idle while the next frame is not due.
    let mut idles = 0;
    let mut got = 0;
    while got < frames.len() {
        match src.poll(Duration::from_millis(5)).unwrap() {
            Poll::Frame(_) => got += 1,
            Poll::Idle => idles += 1,
            Poll::Disconnected => panic!("ended early"),
        }
    }
    let el = t0.elapsed();
    assert!(el >= Duration::from_millis(200), "{el:?}");
    assert!(el < Duration::from_millis(1500), "{el:?}");
    assert!(idles > 0);
}

#[test]
fn synthetic_source_is_paced_and_finite() {
    let mut s = SynthSource::new(
        SynthConfig {
            laps: 1,
            cars: 2,
            ..Default::default()
        },
        50.0,
    );
    assert_eq!(s.game(), "synthetic");
    assert!(!s.reconnectable());
    let t0 = Instant::now();
    let mut n = 0;
    while n < 10 {
        if let Poll::Frame(_) = s.poll(Duration::from_millis(1)).unwrap() {
            n += 1;
        }
    }
    // 10 steps of 0.1 s at 50x => ~20 ms.
    assert!(t0.elapsed() >= Duration::from_millis(15));
    let mut fast = SynthSource::new(
        SynthConfig {
            laps: 1,
            cars: 2,
            ..Default::default()
        },
        0.0,
    );
    let frames = drain(&mut fast);
    assert!(frames.len() > 900, "one lap at 10 Hz");
    assert!(matches!(
        fast.poll(Duration::ZERO).unwrap(),
        Poll::Disconnected
    ));
}

#[test]
fn synthetic_race_story() {
    let frames = synth_frames(SynthConfig::default(), None);
    let player = |f: &vr_core::Frame| f.player_car().unwrap().clone();
    // Fuel drops, is refilled at the stop, never negative.
    let fuels: Vec<f64> = frames
        .iter()
        .map(|f| f.player.as_ref().unwrap().fuel_l.unwrap())
        .collect();
    assert!(fuels.iter().all(|f| *f >= 0.0));
    assert!(
        fuels.iter().any(|f| (*f - 60.0).abs() < 0.5),
        "refuelled to capacity"
    );
    // Player visits pit road on laps 9/10 only.
    let pit_laps: std::collections::BTreeSet<i32> = frames
        .iter()
        .map(player)
        .filter(|c| c.on_pit_road)
        .map(|c| c.lap.unwrap())
        .collect();
    assert_eq!(pit_laps.into_iter().collect::<Vec<_>>(), vec![9, 10]);
    // Positions are a permutation 1..=n in every frame.
    for f in frames.iter().step_by(97) {
        let mut pos: Vec<u32> = f.cars.iter().map(|c| c.position.unwrap()).collect();
        pos.sort();
        assert_eq!(pos, (1..=f.cars.len() as u32).collect::<Vec<_>>());
        let leader = f.cars.iter().find(|c| c.position == Some(1)).unwrap();
        assert!(leader.gap_to_leader_s.unwrap().abs() < 1e-9);
    }
    // Flags: yellow window, white on the last lap, chequered at the end.
    let has = |flag| frames.iter().any(|f| f.session.flags.contains(&flag));
    assert!(has(vr_core::Flag::YellowWaving) && has(vr_core::Flag::White));
    assert!(
        frames
            .last()
            .unwrap()
            .session
            .flags
            .contains(&vr_core::Flag::Checkered)
    );
    let at = |t: f64| frames.iter().find(|f| f.session.time_s >= t).unwrap();
    assert!(
        at(310.0)
            .session
            .flags
            .contains(&vr_core::Flag::YellowWaving)
    );
    assert_eq!(at(149.0).player.as_ref().unwrap().incidents, Some(0));
    assert_eq!(at(151.0).player.as_ref().unwrap().incidents, Some(2));
    // Laps remaining counts down to zero.
    assert_eq!(frames.last().unwrap().session.laps_remaining, Some(0));
    assert_eq!(frames[0].session.laps_remaining, Some(20));
}

#[test]
fn recorder_flush_and_duplicate_times() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.ndjson");
    let frames = short_race();
    let mut rec = Recorder::create(&path, 2.0).unwrap();
    rec.record(&frames[0]).unwrap();
    // Same session time again is a duplicate.
    rec.record(&frames[0]).unwrap();
    // Exactly 0.5 s later is recorded at 2 Hz.
    let mut later = frames[0].clone();
    later.session.time_s += 0.5;
    rec.record(&later).unwrap();
    rec.flush().unwrap();
    // Visible without dropping the recorder.
    let n = std::fs::read_to_string(&path).unwrap().lines().count();
    assert_eq!(n, 2);
    drop(rec);
}

#[test]
fn synthetic_metadata_and_ticks() {
    let mut s = SynthSource::new(
        SynthConfig {
            laps: 1,
            cars: 2,
            ..Default::default()
        },
        0.0,
    );
    assert_eq!(s.capabilities(), vr_core::Capabilities::ALL);
    let ticks: Vec<u64> = (0..3)
        .map(|_| match s.poll(Duration::ZERO).unwrap() {
            Poll::Frame(f) => f.tick,
            _ => panic!(),
        })
        .collect();
    assert_eq!(ticks, vec![1, 2, 3]);
}
