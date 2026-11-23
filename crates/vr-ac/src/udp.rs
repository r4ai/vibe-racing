//! Assetto Corsa's UDP remote telemetry: the player's car every physics step
//! (`RTCarInfo`) and every car's finished laps (`RTLap`), from any machine
//! that can reach the game's PC on port 9996.
//!
//! Two sockets: one subscribed to the updates, one to the laps, as the game
//! keeps one subscription per client.

use std::collections::BTreeMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use serde_json::Value;
use vr_core::{
    Capabilities, CarExtra, CarState, CarTelemetry, Extras, Frame, PlayerTelemetry, Poll,
    SessionKind, SessionPhase, SessionState, SourceError, TelemetrySource, TrackInfo, TrackSurface,
    Warning, Wheel, Wheels,
};

use crate::convert::{GAME, PLAYER_IDX};
use crate::layout::{HANDSHAKE_RESPONSE, Layout, RT_CAR_INFO, RT_LAP, View};
use crate::session::{Sample, Tracker};
use crate::source::now_ms;

pub const PORT: u16 = 9996;
const G: f64 = 9.806_65;
/// Ask again when the game does not answer within this.
const RETRY: Duration = Duration::from_secs(2);
/// Without a packet for this long, the game is gone.
const SILENT: Duration = Duration::from_secs(5);

// operationId
const HANDSHAKE: i32 = 0;
const SUBSCRIBE_UPDATE: i32 = 1;
const SUBSCRIBE_SPOT: i32 = 2;
const DISMISS: i32 = 3;

/// What a client sends: `identifier`, `version`, `operationId`.
pub fn handshaker(operation: i32) -> [u8; 12] {
    let mut b = [0u8; 12];
    b[0..4].copy_from_slice(&1i32.to_le_bytes());
    b[4..8].copy_from_slice(&1i32.to_le_bytes());
    b[8..12].copy_from_slice(&operation.to_le_bytes());
    b
}

/// `127.0.0.1`, `gamepc:9996`...; the port defaults to the game's.
pub fn resolve(host: &str) -> Result<SocketAddr, SourceError> {
    let with_port = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{PORT}")
    };
    with_port
        .to_socket_addrs()
        .map_err(|e| SourceError::NotRunning(format!("{host}: {e}")))?
        .next()
        .ok_or_else(|| SourceError::NotRunning(format!("{host}: no address")))
}

/// One subscription.
struct Channel {
    sock: UdpSocket,
    operation: i32,
    answered: bool,
    asked: Option<Instant>,
}

impl Channel {
    fn open(server: SocketAddr, operation: i32) -> Result<Self, SourceError> {
        let any: SocketAddr = if server.is_ipv4() {
            "0.0.0.0:0".parse().unwrap_or(server)
        } else {
            "[::]:0".parse().unwrap_or(server)
        };
        let sock = UdpSocket::bind(any)?;
        sock.connect(server)?;
        sock.set_nonblocking(true)?;
        Ok(Self {
            sock,
            operation,
            answered: false,
            asked: None,
        })
    }

    fn ask(&mut self) {
        if !self.answered && self.asked.is_none_or(|t| t.elapsed() >= RETRY) {
            self.asked = Some(Instant::now());
            // Refused while the game is not up yet; asked again later.
            let _ = self.sock.send(&handshaker(HANDSHAKE));
        }
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        let n = self.sock.recv(buf).ok()?;
        if n == HANDSHAKE_RESPONSE.size && !self.answered {
            self.answered = true;
            let _ = self.sock.send(&handshaker(self.operation));
        }
        Some(n)
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        if self.answered {
            let _ = self.sock.send(&handshaker(DISMISS));
        }
    }
}

/// A car's last finished lap.
#[derive(Debug, Clone, PartialEq)]
struct Lap {
    lap: i32,
    driver: Option<String>,
    car: Option<String>,
    time_ms: i32,
}

/// Assetto Corsa over UDP.
pub struct UdpSource {
    server: SocketAddr,
    update: Channel,
    spot: Channel,
    handshake: Option<Vec<u8>>,
    car: Option<Vec<u8>>,
    laps: BTreeMap<i32, Lap>,
    tracker: Tracker,
    tick: u64,
    heard: Instant,
}

impl UdpSource {
    pub fn open(server: SocketAddr) -> Result<Self, SourceError> {
        Ok(Self {
            server,
            update: Channel::open(server, SUBSCRIBE_UPDATE)?,
            spot: Channel::open(server, SUBSCRIBE_SPOT)?,
            handshake: None,
            car: None,
            laps: BTreeMap::new(),
            tracker: Tracker::default(),
            tick: 0,
            heard: Instant::now(),
        })
    }

    pub fn server(&self) -> SocketAddr {
        self.server
    }

    fn take_lap(&mut self, b: &[u8]) {
        let v = View::new(&RT_LAP, b);
        let Some(id) = v.i32("carIdentifierNumber") else {
            return;
        };
        self.laps.insert(
            id,
            Lap {
                lap: v.i32("lap").unwrap_or(0),
                driver: v.text("driverName"),
                car: v.text("carName"),
                time_ms: v.i32("time").unwrap_or(0),
            },
        );
    }

    fn frame(&mut self, b: &[u8]) -> Frame {
        let now = now_ms();
        let h = self
            .handshake
            .as_deref()
            .map(|h| View::new(&HANDSHAKE_RESPONSE, h));
        let text = |n: &str| h.and_then(|h| h.text(n));
        self.tick += 1;
        build_udp_frame(
            &View::new(&RT_CAR_INFO, b),
            h.as_ref(),
            &self.laps,
            &mut self.tracker,
            self.tick,
            now,
            (text("trackName"), text("trackConfig")),
        )
    }
}

fn lap_s(ms: Option<i32>) -> Option<f64> {
    ms.filter(|ms| (1..36_000_000).contains(ms))
        .map(|ms| f64::from(ms) / 1000.0)
}

fn build_udp_frame(
    c: &View<'_>,
    h: Option<&View<'_>>,
    laps: &BTreeMap<i32, Lap>,
    tracker: &mut Tracker,
    tick: u64,
    now_ms: i64,
    (track, config): (Option<String>, Option<String>),
) -> Frame {
    let lap_count = c.i32("lapCount").filter(|l| *l >= 0);
    let event = format!(
        "{}@{}/{}",
        h.and_then(|h| h.text("carName")).unwrap_or_default(),
        track.clone().unwrap_or_default(),
        config.clone().unwrap_or_default()
    );
    let clock = tracker.advance(&Sample {
        now_ms,
        kind: -1,
        event,
        completed_laps: lap_count.unwrap_or(0),
        distance_m: 0.0,
        time_left_s: None,
    });
    let in_pit = c.flag("isInPit") == Some(true);
    let wheel = |i: usize| Wheel {
        speed_mps: c
            .f64_at("wheelAngularSpeed", i)
            .zip(c.f64_at("tyreRadius", i).filter(|r| *r > 0.0))
            .map(|(w, r)| w.abs() * r),
        load_n: c.f64_at("load", i),
        camber_rad: c.f64_at("camberRAD", i),
        ..Default::default()
    };
    let mut cars = vec![CarState {
        idx: PLAYER_IDX,
        number: PLAYER_IDX.to_string(),
        driver: h
            .and_then(|h| h.text("driverName"))
            .unwrap_or_else(|| "Player".into()),
        team: None,
        car_name: h.and_then(|h| h.text("carName")),
        class_name: None,
        class_id: None,
        rating: None,
        is_player: true,
        position: None,
        class_position: None,
        lap: lap_count.map(|l| l + 1),
        laps_completed: lap_count,
        lap_dist_pct: c
            .f64("carPositionNormalized")
            .filter(|p| (0.0..=1.0).contains(p)),
        on_pit_road: in_pit,
        surface: TrackSurface::OnTrack,
        last_lap_s: lap_s(c.i32("lastLap")),
        best_lap_s: lap_s(c.i32("bestLap")),
        gap_to_leader_s: None,
        est_lap_s: None,
        extra: CarExtra {
            gear: c.i32("gear").map(|g| g - 1),
            rpm: c.f64("engineRPM"),
            ..Default::default()
        },
    }];
    for (&id, l) in laps {
        let Ok(idx) = usize::try_from(id) else {
            continue;
        };
        if idx == PLAYER_IDX {
            continue;
        }
        cars.push(CarState {
            idx,
            number: idx.to_string(),
            driver: l.driver.clone().unwrap_or_else(|| format!("Car {idx}")),
            team: None,
            car_name: l.car.clone(),
            class_name: None,
            class_id: None,
            rating: None,
            is_player: false,
            position: None,
            class_position: None,
            lap: Some(l.lap + 1),
            laps_completed: Some(l.lap),
            lap_dist_pct: None,
            on_pit_road: false,
            surface: TrackSurface::OnTrack,
            last_lap_s: lap_s(Some(l.time_ms)),
            best_lap_s: None,
            gap_to_leader_s: None,
            est_lap_s: None,
            extra: CarExtra::default(),
        });
    }
    let mut warnings = Vec::new();
    if c.flag("isEngineLimiterOn") == Some(true) {
        warnings.push(Warning::RevLimiter);
    }
    let player = PlayerTelemetry {
        on_track: true,
        speed_mps: c.f64("speed_Ms"),
        rpm: c.f64("engineRPM"),
        gear: c.i32("gear").map(|g| g - 1),
        throttle: c.f64("gas"),
        brake: c.f64("brake"),
        current_lap_s: c
            .i32("lapTime")
            .filter(|t| *t >= 0)
            .map(|t| f64::from(t) / 1000.0),
        car: CarTelemetry {
            clutch: c.f64("clutch").map(|v| (1.0 - v).clamp(0.0, 1.0)),
            lat_accel_mps2: c.f64("accG_horizontal").map(|a| -a * G),
            long_accel_mps2: c.f64("accG_frontal").map(|a| a * G),
            abs_active: c.flag("isAbsInAction"),
            warnings,
            wheels: Some(Wheels {
                lf: wheel(0),
                rf: wheel(1),
                lr: wheel(2),
                rr: wheel(3),
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    Frame {
        game: GAME.into(),
        tick,
        timestamp_ms: now_ms,
        session: SessionState {
            session_num: clock.session_num,
            // Not told over UDP.
            kind: SessionKind::Other,
            phase: SessionPhase::Racing,
            time_s: clock.time_s,
            time_remaining_s: None,
            laps_remaining: None,
            laps_total: None,
            flags: Vec::new(),
            track: track.map(|name| TrackInfo {
                name,
                config,
                length_m: None,
                sectors: Vec::new(),
            }),
            uid: Some(clock.uid),
            time_total_s: None,
            time_of_day_s: None,
        },
        player_car_idx: Some(PLAYER_IDX),
        player: Some(player),
        cars,
        weather: None,
    }
}

fn object(layout: &Layout, b: &[u8]) -> Value {
    Value::Object(View::new(layout, b).object())
}

impl TelemetrySource for UdpSource {
    fn game(&self) -> &str {
        GAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            multi_car: !self.laps.is_empty(),
            fuel: false,
            ..Capabilities::default()
        }
    }

    fn extras(&mut self) -> Option<Extras> {
        let car = self.car.as_deref()?;
        let mut channels = View::new(&RT_CAR_INFO, car).channels("udp.car");
        let mut info = serde_json::Map::new();
        if let Some(h) = &self.handshake {
            channels.extend(View::new(&HANDSHAKE_RESPONSE, h).channels("udp.handshake"));
            info.insert("handshake".into(), object(&HANDSHAKE_RESPONSE, h));
        }
        let laps: Vec<Value> = self
            .laps
            .iter()
            .map(|(id, l)| {
                serde_json::json!({
                    "carIdentifierNumber": id,
                    "lap": l.lap,
                    "driverName": l.driver,
                    "carName": l.car,
                    "time": l.time_ms,
                })
            })
            .collect();
        info.insert("laps".into(), Value::Array(laps));
        info.insert("server".into(), Value::String(self.server.to_string()));
        channels.sort_by(|a, b| a.name.cmp(&b.name));
        Some(Extras {
            channels,
            info: Some(Value::Object(info)),
            setup: None,
        })
    }

    fn poll(&mut self, timeout: Duration) -> Result<Poll, SourceError> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 2048];
        loop {
            self.update.ask();
            self.spot.ask();
            while let Some(n) = self.spot.recv(&mut buf) {
                self.heard = Instant::now();
                if n == HANDSHAKE_RESPONSE.size {
                    self.handshake.get_or_insert_with(|| buf[..n].to_vec());
                } else if n == RT_LAP.size {
                    self.take_lap(&buf[..n]);
                }
            }
            while let Some(n) = self.update.recv(&mut buf) {
                self.heard = Instant::now();
                if n == HANDSHAKE_RESPONSE.size {
                    self.handshake = Some(buf[..n].to_vec());
                } else if n == RT_LAP.size {
                    self.take_lap(&buf[..n]);
                } else if n == RT_CAR_INFO.size {
                    let b = buf[..n].to_vec();
                    let frame = self.frame(&b);
                    self.car = Some(b);
                    return Ok(Poll::Frame(Box::new(frame)));
                }
            }
            if self.update.answered && self.heard.elapsed() > SILENT {
                return Ok(Poll::Disconnected);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(Poll::Idle);
            }
            std::thread::sleep(Duration::from_millis(2).min(deadline - now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Writer;

    fn response() -> Vec<u8> {
        let mut w = Writer::new(&HANDSHAKE_RESPONSE);
        w.text("carName", "ks_mazda_mx5_cup")
            .text("driverName", "Ada Lovelace")
            .i32("identifier", 4242)
            .i32("version", 1)
            .text("trackName", "magione")
            .text("trackConfig", "");
        w.bytes
    }

    fn car_info(lap_count: i32) -> Vec<u8> {
        let mut w = Writer::new(&RT_CAR_INFO);
        w.text("identifier", "a")
            .i32("size", 328)
            .f32("speed_Kmh", 144.0)
            .f32("speed_Ms", 40.0)
            .i32("isAbsInAction", 1)
            .i32("isEngineLimiterOn", 1)
            .i32("isInPit", 0)
            .f32("accG_horizontal", 1.0)
            .f32("accG_frontal", -0.5)
            .i32("lapTime", 12_345)
            .i32("lastLap", 70_100)
            .i32("bestLap", 0)
            .i32("lapCount", lap_count)
            .f32("gas", 1.0)
            .f32("clutch", 0.0)
            .f32("engineRPM", 6500.0)
            .i32("gear", 3)
            .f32s("wheelAngularSpeed", &[100.0; 4])
            .f32s("tyreRadius", &[0.3; 4])
            .f32s("load", &[2500.0; 4])
            .f32("carPositionNormalized", 0.42);
        w.bytes
    }

    fn rt_lap(id: i32, lap: i32, ms: i32) -> Vec<u8> {
        let mut w = Writer::new(&RT_LAP);
        w.i32("carIdentifierNumber", id)
            .i32("lap", lap)
            .text("driverName", "Grace Hopper")
            .text("carName", "ks_mazda_mx5_cup")
            .i32("time", ms);
        w.bytes
    }

    fn op(b: &[u8]) -> i32 {
        i32::from_le_bytes(b[8..12].try_into().unwrap())
    }

    #[test]
    fn handshakes_subscribes_and_reads_the_car_and_the_laps() {
        let game = UdpSocket::bind("127.0.0.1:0").unwrap();
        game.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let addr = game.local_addr().unwrap();
        let mut src = UdpSource::open(addr).unwrap();
        assert_eq!(src.server(), addr);
        assert_eq!(src.game(), "ac");
        assert!(src.extras().is_none());
        assert!(matches!(src.poll(Duration::ZERO).unwrap(), Poll::Idle));

        // Two clients say hello; each is answered and subscribes.
        let mut buf = [0u8; 64];
        let mut clients = Vec::new();
        for _ in 0..2 {
            let (n, from) = game.recv_from(&mut buf).unwrap();
            assert_eq!((n, op(&buf)), (12, HANDSHAKE));
            assert_eq!(&buf[..8], &[1, 0, 0, 0, 1, 0, 0, 0]);
            game.send_to(&response(), from).unwrap();
            clients.push(from);
        }
        assert!(matches!(
            src.poll(Duration::from_millis(50)).unwrap(),
            Poll::Idle
        ));
        let mut subs = BTreeMap::new();
        for _ in 0..2 {
            let (_, from) = game.recv_from(&mut buf).unwrap();
            subs.insert(op(&buf), from);
        }
        assert_eq!(
            subs.keys().copied().collect::<Vec<_>>(),
            [SUBSCRIBE_UPDATE, SUBSCRIBE_SPOT]
        );

        game.send_to(&rt_lap(3, 5, 71_500), subs[&SUBSCRIBE_SPOT])
            .unwrap();
        game.send_to(&rt_lap(0, 4, 70_100), subs[&SUBSCRIBE_SPOT])
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        game.send_to(&car_info(4), subs[&SUBSCRIBE_UPDATE]).unwrap();
        let f = loop {
            if let Poll::Frame(f) = src.poll(Duration::from_millis(200)).unwrap() {
                break f;
            }
        };
        assert_eq!(f.tick, 1);
        let track = f.session.track.as_ref().unwrap();
        assert_eq!(
            (track.name.as_str(), track.config.as_deref()),
            ("magione", None)
        );
        assert_eq!(f.session.kind, SessionKind::Other);
        let me = f.player_car().unwrap();
        assert_eq!(me.driver, "Ada Lovelace");
        assert_eq!(me.car_name.as_deref(), Some("ks_mazda_mx5_cup"));
        assert_eq!((me.lap, me.laps_completed), (Some(5), Some(4)));
        assert_eq!((me.last_lap_s, me.best_lap_s), (Some(70.1), None));
        assert!((me.lap_dist_pct.unwrap() - 0.42).abs() < 1e-6);
        let other = f.car(3).unwrap();
        assert_eq!(
            (other.driver.as_str(), other.laps_completed),
            ("Grace Hopper", Some(5))
        );
        assert_eq!(other.last_lap_s, Some(71.5));
        assert_eq!(f.cars.len(), 2, "the player's own lap is not another car");
        let p = f.player.as_ref().unwrap();
        assert_eq!(
            (p.speed_mps, p.gear, p.rpm),
            (Some(40.0), Some(2), Some(6500.0))
        );
        assert_eq!(p.current_lap_s, Some(12.345));
        let car = &p.car;
        assert_eq!((car.abs_active, car.clutch), (Some(true), Some(1.0)));
        assert_eq!(car.warnings, [Warning::RevLimiter]);
        assert!((car.lat_accel_mps2.unwrap() + G).abs() < 1e-9);
        assert!((car.long_accel_mps2.unwrap() + 0.5 * G).abs() < 1e-6);
        let w = car.wheels.as_ref().unwrap();
        assert!((w.lf.speed_mps.unwrap() - 30.0).abs() < 1e-4);
        assert_eq!(w.rr.load_n, Some(2500.0));
        assert!(src.capabilities().multi_car);

        let x = src.extras().unwrap();
        assert!(
            x.channels
                .iter()
                .any(|c| c.name == "udp.car.slipAngle_ContactPatch")
        );
        assert!(
            x.channels
                .iter()
                .any(|c| c.name == "udp.handshake.identifier")
        );
        let info = x.info.unwrap();
        assert_eq!(info["handshake"]["identifier"], 4242);
        assert_eq!(info["laps"][1]["carIdentifierNumber"], 3);
        assert_eq!(info["server"], addr.to_string());

        // Leaving tells the game.
        drop(src);
        let mut ops = Vec::new();
        for _ in 0..2 {
            game.recv_from(&mut buf).unwrap();
            ops.push(op(&buf));
        }
        assert_eq!(ops, [DISMISS, DISMISS]);
        let _ = clients;
    }

    #[test]
    fn asks_again_and_notices_silence() {
        assert_eq!(resolve("127.0.0.1").unwrap().port(), PORT);
        assert_eq!(resolve("127.0.0.1:1234").unwrap().port(), 1234);
        assert!(resolve("no such host.invalid").is_err());
        assert_eq!(op(&handshaker(SUBSCRIBE_SPOT)), 2);

        let game = UdpSocket::bind("127.0.0.1:0").unwrap();
        game.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut src = UdpSource::open(game.local_addr().unwrap()).unwrap();
        let _ = src.poll(Duration::ZERO);
        let mut buf = [0u8; 64];
        let (_, a) = game.recv_from(&mut buf).unwrap();
        let (_, b) = game.recv_from(&mut buf).unwrap();
        // Not answered: asked again after a while.
        src.update.asked = Some(Instant::now() - RETRY);
        let _ = src.poll(Duration::ZERO);
        let (_, again) = game.recv_from(&mut buf).unwrap();
        assert_eq!(op(&buf), HANDSHAKE);
        assert!(again == a || again == b);
        // Answered, then silent.
        game.send_to(&response(), src.update.sock.local_addr().unwrap())
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(matches!(src.poll(Duration::ZERO).unwrap(), Poll::Idle));
        src.heard = Instant::now() - SILENT - Duration::from_millis(1);
        assert!(matches!(
            src.poll(Duration::ZERO).unwrap(),
            Poll::Disconnected
        ));
        // A lap count going down is the session again.
        let mut t = Tracker::default();
        let h = response();
        let hv = View::new(&HANDSHAKE_RESPONSE, &h);
        let laps = BTreeMap::new();
        let b = car_info(3);
        let f = build_udp_frame(
            &View::new(&RT_CAR_INFO, &b),
            Some(&hv),
            &laps,
            &mut t,
            1,
            0,
            (None, None),
        );
        assert_eq!(f.session.uid, Some(0));
        let b = car_info(0);
        let f = build_udp_frame(
            &View::new(&RT_CAR_INFO, &b),
            Some(&hv),
            &laps,
            &mut t,
            2,
            10,
            (None, None),
        );
        assert_eq!(f.session.uid, Some(1));
        let f = build_udp_frame(
            &View::new(&RT_CAR_INFO, &b),
            None,
            &laps,
            &mut t,
            3,
            20,
            (None, None),
        );
        assert_eq!(f.player_car().unwrap().driver, "Player");
        assert_eq!(f.session.session_num, 1, "another car: another session");
    }
}
