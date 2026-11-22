//! Microphone capture (cpal) and speaker playback (rodio).
//!
//! Both devices live on their own threads: cpal streams are not `Send` on
//! every platform. The microphone stays open so that pressing the
//! push-to-talk button records from the very first syllable.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
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
}

pub trait Output: Send + Sync + 'static {
    /// Queues a WAV clip after whatever is playing.
    fn play(&self, wav: Vec<u8>);
    /// Stops playback and drops the queue.
    fn stop(&self);
    fn is_idle(&self) -> bool;
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

    fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }
}

/// A recorder for machines without a microphone: always silent.
pub struct NoMic;

impl Recorder for NoMic {
    fn start(&self) {}
    fn stop(&self) -> Recording {
        Recording::default()
    }
    fn level(&self) -> f32 {
        0.0
    }
}

enum OutCmd {
    Play(Vec<u8>),
    Stop,
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
    fn no_mic_is_silent() {
        NoMic.start();
        assert_eq!(NoMic.stop(), Recording::default());
        assert_eq!(NoMic.level(), 0.0);
    }
}
