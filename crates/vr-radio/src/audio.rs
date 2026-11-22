//! Microphone capture (cpal) and speaker playback (rodio).
//!
//! Both devices live on their own threads: cpal streams are not `Send` on
//! every platform. The microphone stays open so that pressing the
//! push-to-talk button records from the very first syllable.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rodio::cpal;

use crate::wav;

/// Mono samples captured between push and release.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recording {
    pub samples: Vec<f32>,
    pub rate: u32,
}

impl Recording {
    pub fn seconds(&self) -> f32 {
        if self.rate == 0 {
            return 0.0;
        }
        self.samples.len() as f32 / self.rate as f32
    }

    /// 16 kHz mono WAV for the transcriber.
    pub fn to_wav(&self) -> Vec<u8> {
        let samples = wav::resample(&self.samples, self.rate, wav::STT_RATE);
        wav::encode_pcm16(&samples, wav::STT_RATE)
    }
}

pub trait Recorder: Send + Sync + 'static {
    fn start(&self);
    fn stop(&self) -> Recording;
    /// Input level (RMS, 0.0..=1.0) of the last few milliseconds.
    fn level(&self) -> f32;
    /// What was recorded so far, from the given sample on.
    fn peek(&self, from: usize) -> Recording;
}

pub trait Output: Send + Sync + 'static {
    /// Queues a WAV clip after whatever is playing.
    fn play(&self, wav: Vec<u8>);
    /// Stops playback and drops the queue.
    fn stop(&self);
    fn is_idle(&self) -> bool;
    /// Loudness from now on, of the clip already playing too (1.0 = as is).
    fn set_volume(&self, _volume: f32) {}
}

#[derive(Default)]
struct MicShared {
    recording: AtomicBool,
    closed: AtomicBool,
    rate: AtomicU32,
    level: AtomicU32,
    buf: Mutex<Vec<f32>>,
}

/// The default input device, always open.
pub struct Mic {
    shared: Arc<MicShared>,
    pub device: String,
}

impl Mic {
    pub fn open() -> Result<Self> {
        let shared = Arc::new(MicShared::default());
        let (ready_tx, ready_rx) = mpsc::channel::<Result<String>>();
        let thread_shared = shared.clone();
        std::thread::Builder::new()
            .name("vr-radio-mic".into())
            .spawn(move || capture(thread_shared, ready_tx))?;
        let device = ready_rx
            .recv()
            .map_err(|_| anyhow!("microphone thread ended"))??;
        Ok(Self { shared, device })
    }
}

impl MicShared {
    /// Takes one block of interleaved input from the device callback.
    fn ingest(&self, interleaved: &[f32], channels: u16) {
        let mono = wav::downmix(interleaved, channels);
        self.level
            .store(wav::rms(&mono).to_bits(), Ordering::Relaxed);
        if self.recording.load(Ordering::Relaxed) {
            self.buf.lock().unwrap().extend_from_slice(&mono);
        }
    }
}

/// Opens the default input on this thread and keeps it running until the
/// `Mic` is dropped. cpal is used directly: rodio's `Microphone` stops for
/// good after a single buffer overrun.
fn capture(shared: Arc<MicShared>, ready: mpsc::Sender<Result<String>>) {
    let stream = open_input(shared.clone()).context("cannot open the default microphone");
    let (stream, info) = match stream {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(info));
    while !shared.closed.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(stream);
}

fn open_input(shared: Arc<MicShared>) -> Result<(cpal::Stream, String)> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| anyhow!("no input device"))?;
    let supported = device.default_input_config()?;
    let config = supported.config();
    let channels = config.channels;
    shared.rate.store(config.sample_rate, Ordering::Relaxed);
    let info = format!("{} Hz, {channels} ch", config.sample_rate);

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        shared: Arc<MicShared>,
    ) -> Result<cpal::Stream>
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        let channels = config.channels;
        let mut block = Vec::new();
        Ok(device.build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                block.clear();
                block.extend(data.iter().map(|s| cpal::Sample::to_sample::<f32>(*s)));
                shared.ingest(&block, channels);
            },
            |e| tracing::warn!("microphone: {e}"),
            None,
        )?)
    }

    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, config, shared)?,
        cpal::SampleFormat::I16 => build::<i16>(&device, config, shared)?,
        cpal::SampleFormat::U16 => build::<u16>(&device, config, shared)?,
        cpal::SampleFormat::I32 => build::<i32>(&device, config, shared)?,
        other => bail!("unsupported sample format {other:?}"),
    };
    stream.play()?;
    Ok((stream, info))
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
    }
}

impl Recorder for Mic {
    fn start(&self) {
        self.shared.buf.lock().unwrap().clear();
        self.shared.recording.store(true, Ordering::Relaxed);
    }

    fn stop(&self) -> Recording {
        self.shared.recording.store(false, Ordering::Relaxed);
        Recording {
            samples: std::mem::take(&mut *self.shared.buf.lock().unwrap()),
            rate: self.shared.rate.load(Ordering::Relaxed),
        }
    }

    fn peek(&self, from: usize) -> Recording {
        let buf = self.shared.buf.lock().unwrap();
        Recording {
            samples: buf.get(from..).unwrap_or_default().to_vec(),
            rate: self.shared.rate.load(Ordering::Relaxed),
        }
    }

    fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }
}

/// How often a device that could not be opened is tried again.
pub const RETRY: Duration = Duration::from_secs(5);

/// A device that is opened as soon as it can be.
struct Late<T> {
    slot: Arc<OnceLock<T>>,
}

impl<T: Send + Sync + 'static> Late<T> {
    /// Tries at once, then every `retry` on a thread. `state` is told when
    /// it fails for the first time and when it works.
    fn open(
        name: &str,
        retry: Duration,
        open: impl Fn() -> Result<T> + Send + 'static,
        state: impl Fn(Result<()>) + Send + 'static,
    ) -> Self {
        let slot = Arc::new(OnceLock::new());
        match open() {
            Ok(d) => {
                let _ = slot.set(d);
                state(Ok(()));
            }
            Err(e) => {
                state(Err(e));
                let weak = Arc::downgrade(&slot);
                let _ = std::thread::Builder::new()
                    .name(name.into())
                    .spawn(move || {
                        loop {
                            std::thread::sleep(retry);
                            let Some(slot) = weak.upgrade() else { return };
                            if let Ok(d) = open() {
                                let _ = slot.set(d);
                                state(Ok(()));
                                return;
                            }
                        }
                    });
            }
        }
        Self { slot }
    }

    fn get(&self) -> Option<&T> {
        self.slot.get()
    }
}

/// The default microphone, as soon as there is one.
pub struct LateMic(Late<Mic>);

impl LateMic {
    pub fn open(state: impl Fn(Result<()>) + Send + 'static) -> Self {
        Self(Late::open(
            "vr-radio-mic-retry",
            RETRY,
            || {
                let mic = Mic::open()?;
                tracing::info!("microphone: {}", mic.device);
                Ok(mic)
            },
            state,
        ))
    }
}

impl Recorder for LateMic {
    fn start(&self) {
        if let Some(m) = self.0.get() {
            m.start();
        }
    }
    fn stop(&self) -> Recording {
        self.0.get().map(Mic::stop).unwrap_or_default()
    }
    fn level(&self) -> f32 {
        self.0.get().map_or(0.0, Mic::level)
    }
    fn peek(&self, from: usize) -> Recording {
        self.0.get().map(|m| m.peek(from)).unwrap_or_default()
    }
}

/// The default output device, as soon as there is one.
pub struct LateSpeakers {
    late: Late<Speakers>,
    /// Set before the device was there (bits of an `f32`).
    volume: Arc<AtomicU32>,
}

impl LateSpeakers {
    pub fn open(state: impl Fn(Result<()>) + Send + 'static) -> Self {
        let volume = Arc::new(AtomicU32::new(1f32.to_bits()));
        let set = volume.clone();
        let late = Late::open(
            "vr-radio-speakers-retry",
            RETRY,
            move || {
                let s = Speakers::open()?;
                s.set_volume(f32::from_bits(set.load(Ordering::Relaxed)));
                Ok(s)
            },
            state,
        );
        Self { late, volume }
    }
}

impl Output for LateSpeakers {
    fn play(&self, wav: Vec<u8>) {
        if let Some(s) = self.late.get() {
            s.play(wav);
        }
    }
    fn stop(&self) {
        if let Some(s) = self.late.get() {
            s.stop();
        }
    }
    fn is_idle(&self) -> bool {
        self.late.get().is_none_or(Speakers::is_idle)
    }
    fn set_volume(&self, volume: f32) {
        self.volume.store(volume.to_bits(), Ordering::Relaxed);
        if let Some(s) = self.late.get() {
            s.set_volume(volume);
        }
    }
}

/// A recorder for machines without a microphone: always silent.
pub struct NoMic;

impl Recorder for NoMic {
    fn start(&self) {}
    fn stop(&self) -> Recording {
        Recording::default()
    }
    fn peek(&self, _: usize) -> Recording {
        Recording::default()
    }

    fn level(&self) -> f32 {
        0.0
    }
}

enum OutCmd {
    Play(Vec<u8>),
    Stop,
    Volume(f32),
}

/// The default output device.
pub struct Speakers {
    tx: Mutex<mpsc::Sender<OutCmd>>,
    idle: Arc<AtomicBool>,
}

impl Speakers {
    pub fn open() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<OutCmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let idle = Arc::new(AtomicBool::new(true));
        let thread_idle = idle.clone();
        std::thread::Builder::new()
            .name("vr-radio-speakers".into())
            .spawn(move || {
                let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx
                            .send(Err(anyhow!("cannot open the default output device: {e}")));
                        return;
                    }
                };
                let mut sink = sink;
                sink.log_on_drop(false);
                let player = rodio::Player::connect_new(sink.mixer());
                let _ = ready_tx.send(Ok(()));
                loop {
                    match rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(OutCmd::Play(bytes)) => {
                            match rodio::Decoder::new_wav(Cursor::new(bytes)) {
                                Ok(clip) => player.append(clip),
                                Err(e) => tracing::warn!("cannot decode speech clip: {e}"),
                            }
                        }
                        Ok(OutCmd::Stop) => {
                            player.clear();
                            player.play(); // `clear` also pauses
                        }
                        Ok(OutCmd::Volume(v)) => player.set_volume(v),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    thread_idle.store(player.empty(), Ordering::Relaxed);
                }
            })?;
        ready_rx
            .recv()
            .map_err(|_| anyhow!("speaker thread ended"))??;
        Ok(Self {
            tx: Mutex::new(tx),
            idle,
        })
    }

    fn send(&self, cmd: OutCmd) {
        let _ = self.tx.lock().unwrap().send(cmd);
    }
}

impl Output for Speakers {
    fn play(&self, wav: Vec<u8>) {
        self.idle.store(false, Ordering::Relaxed);
        self.send(OutCmd::Play(wav));
    }

    fn stop(&self) {
        self.send(OutCmd::Stop);
    }

    fn is_idle(&self) -> bool {
        self.idle.load(Ordering::Relaxed)
    }

    fn set_volume(&self, volume: f32) {
        self.send(OutCmd::Volume(volume));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_length_and_wav() {
        let r = Recording {
            samples: vec![0.0; 24_000],
            rate: 48_000,
        };
        assert_eq!(r.seconds(), 0.5);
        // 0.5 s at 16 kHz, 16-bit: 8000 samples * 2 bytes + header.
        assert_eq!(r.to_wav().len(), 44 + 16_000);
        assert_eq!(Recording::default().seconds(), 0.0);
    }

    #[test]
    fn ingest_tracks_level_and_records_only_while_pressed() {
        let shared = MicShared::default();
        shared.ingest(&[0.5, 0.5, -0.5, -0.5], 2);
        assert_eq!(f32::from_bits(shared.level.load(Ordering::Relaxed)), 0.5);
        assert!(shared.buf.lock().unwrap().is_empty());
        shared.recording.store(true, Ordering::Relaxed);
        shared.ingest(&[0.2, 0.4], 2);
        shared.ingest(&[0.0, 0.0], 2);
        assert_eq!(*shared.buf.lock().unwrap(), vec![0.3, 0.0]);
        assert_eq!(f32::from_bits(shared.level.load(Ordering::Relaxed)), 0.0);
    }

    /// Needs a microphone: `cargo test -p vr-radio -- --ignored live_mic`.
    #[test]
    #[ignore]
    fn live_mic_records_for_as_long_as_pressed() {
        let mic = Mic::open().unwrap();
        // Startup overruns must not stop the capture.
        std::thread::sleep(Duration::from_millis(500));
        mic.start();
        std::thread::sleep(Duration::from_secs(1));
        let rec = mic.stop();
        assert!((0.8..1.2).contains(&rec.seconds()), "{} s", rec.seconds());
    }

    #[test]
    fn a_device_is_opened_as_soon_as_it_can_be() {
        let tries = AtomicU32::new(0);
        let (tx, rx) = mpsc::channel();
        let late = Late::open(
            "test",
            Duration::from_millis(5),
            move || match tries.fetch_add(1, Ordering::SeqCst) {
                2 => Ok(7),
                n => Err(anyhow!("try {n}")),
            },
            move |r: Result<()>| tx.send(r.map_err(|e| e.to_string())).unwrap(),
        );
        assert_eq!(late.get(), None);
        let wait = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Said once that it fails, not at every try.
        assert_eq!(wait(), Err("try 0".to_string()));
        assert_eq!(wait(), Ok(()));
        assert_eq!(late.get(), Some(&7));
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[test]
    fn a_device_that_works_at_once_is_not_tried_again() {
        let (tx, rx) = mpsc::channel();
        let late = Late::open(
            "test",
            Duration::ZERO,
            || Ok(1),
            move |r| tx.send(r.is_ok()).unwrap(),
        );
        assert_eq!(late.get(), Some(&1));
        assert_eq!(rx.iter().collect::<Vec<_>>(), [true]);
    }

    #[test]
    fn no_mic_is_silent() {
        NoMic.start();
        assert_eq!(NoMic.stop(), Recording::default());
        assert_eq!(NoMic.level(), 0.0);
    }
}
