//! What the microphone hears: a synthesized voice, cockpit noise, the click
//! of the button.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::json;
use vr_radio::http;
use vr_radio::voicevox::{Synth, Voicevox};
use vr_radio::wav;

use crate::scenario::{Piece, pieces};

/// Sample rate of the simulated microphone.
pub const RATE: u32 = 24_000;
/// Before the first word and after the last one, as a driver holds it.
pub const LEAD_S: f32 = 0.25;
pub const TAIL_S: f32 = 0.35;

/// A call as recorded by the microphone.
#[derive(Debug, Clone)]
pub struct Utterance {
    pub samples: Vec<f32>,
    /// Where the last word ends, in seconds.
    pub speech_end_s: f32,
}

impl Utterance {
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / RATE as f32
    }
}

/// How a call is turned into sound.
pub struct Spec<'a> {
    pub say: &'a str,
    pub voice: &'a str,
    pub snr_db: Option<f32>,
    pub cut_start_s: f32,
    pub cut_end_s: f32,
    pub duration_s: f32,
}

pub struct Voices {
    pub cache: PathBuf,
    pub voicevox_url: String,
    pub openrouter_key: Option<String>,
}

impl Voices {
    pub fn utterance(&self, spec: &Spec) -> Result<Utterance> {
        let mut speech = Vec::new();
        for piece in pieces(spec.say) {
            match piece {
                Piece::Words(w) => speech.extend(self.words(&w, spec.voice)?),
                Piece::Pause(s) => speech.resize(speech.len() + secs(s), 0.0),
            }
        }
        Ok(compose(speech, spec))
    }

    /// A voice saying `words`, trimmed, kept on disk: every run hears the same.
    fn words(&self, words: &str, voice: &str) -> Result<Vec<f32>> {
        let name = format!("{:016x}.pcm", fnv(&format!("{voice}|{words}")));
        let path = self.cache.join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok(from_pcm(&bytes));
        }
        let samples = trim(&self.synthesize(words, voice)?);
        std::fs::create_dir_all(&self.cache)?;
        std::fs::write(&path, to_pcm(&samples))?;
        Ok(samples)
    }

    fn synthesize(&self, words: &str, voice: &str) -> Result<Vec<f32>> {
        let (kind, name) = voice.split_once(':').context("voice is kind:name")?;
        let model = match kind {
            "vv" => {
                let wav = Voicevox::new(self.voicevox_url.clone(), 1.1)
                    .synthesize(words, name.parse()?)?;
                return read_wav(&wav);
            }
            "gemini" => "google/gemini-3.8-flash-tts",
            "eleven" => "elevenlabs/eleven-multilingual-v2",
            "aura" => "deepgram/aura-2",
            other => bail!("unknown voice kind {other}"),
        };
        let key = self
            .openrouter_key
            .as_deref()
            .context("this voice needs OPENROUTER_API_KEY")?;
        let resp = http::agent(Duration::from_secs(90))
            .post("https://openrouter.ai/api/v1/audio/speech")
            .header("Authorization", format!("Bearer {key}"))
            .send_json(json!({
                "model": model,
                "input": words,
                "voice": name,
                "response_format": "pcm",
            }))?;
        let mut resp = http::check(resp, "voice")?;
        // Documented by the response type: audio/pcm;rate=24000;channels=1.
        Ok(from_pcm(&resp.body_mut().read_to_vec()?))
    }
}

fn secs(s: f32) -> usize {
    (s * RATE as f32) as usize
}

/// Places the speech between lead and tail, cuts it and adds the cockpit.
fn compose(mut speech: Vec<f32>, spec: &Spec) -> Utterance {
    normalize_level(&mut speech);
    let cut = secs(spec.cut_start_s).min(speech.len());
    speech.drain(..cut);
    speech.truncate(speech.len() - secs(spec.cut_end_s).min(speech.len()));
    // Cut means the button was not down: no margin on that side.
    let lead = if spec.cut_start_s > 0.0 { 0.0 } else { LEAD_S };
    let tail = if spec.cut_end_s > 0.0 { 0.0 } else { TAIL_S };
    let mut samples = vec![0.0; secs(lead)];
    samples.extend(&speech);
    let speech_end_s = samples.len() as f32 / RATE as f32;
    samples.resize(samples.len() + secs(tail), 0.0);
    samples.resize(samples.len().max(secs(spec.duration_s)), 0.0);
    let speech_end_s = if speech.is_empty() {
        samples.len() as f32 / RATE as f32
    } else {
        speech_end_s
    };
    // A headset is never silent; 30 dB is a quiet room.
    cockpit(&mut samples, &speech, spec.snr_db.unwrap_or(30.0));
    Utterance {
        samples,
        speech_end_s,
    }
}

/// Speech at a typical headset level.
const SPEECH_RMS: f32 = 0.08;

fn normalize_level(samples: &mut [f32]) {
    let level = active_rms(samples);
    if level > 0.0 {
        for s in samples {
            *s = (*s * SPEECH_RMS / level).clamp(-1.0, 1.0);
        }
    }
}

/// Level of the parts that carry speech, not of the pauses.
fn active_rms(samples: &[f32]) -> f32 {
    let blocks: Vec<f32> = samples.chunks(480).map(wav::rms).collect();
    let peak = blocks.iter().copied().fold(0.0, f32::max);
    let active: Vec<f32> = blocks.into_iter().filter(|b| *b > peak * 0.1).collect();
    wav::rms(&active)
}

/// Engine harmonics sweeping with the revs, wind, the click of the button.
/// Deterministic: the same call always sounds the same.
fn cockpit(samples: &mut [f32], speech: &[f32], snr_db: f32) {
    let level = if speech.is_empty() {
        SPEECH_RMS
    } else {
        active_rms(speech)
    };
    let target = level / 10f32.powf(snr_db / 20.0);
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut white = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    };
    let (mut phase, mut wind) = (0.0f32, 0.0f32);
    let noise: Vec<f32> = (0..samples.len())
        .map(|i| {
            let t = i as f32 / RATE as f32;
            // 110..250 Hz, upshifting every 1.7 s.
            let f = 110.0 + 140.0 * (t / 1.7).fract();
            phase = (phase + f / RATE as f32).fract();
            let engine: f32 = (1..=8)
                .map(|k| (phase * k as f32 * std::f32::consts::TAU).sin() / k as f32)
                .sum();
            wind = 0.95 * wind + 0.05 * white();
            engine * 0.5 + wind * 4.0
        })
        .collect();
    let scale = target / wav::rms(&noise).max(1e-9);
    for (s, n) in samples.iter_mut().zip(&noise) {
        *s = (*s + n * scale).clamp(-1.0, 1.0);
    }
    for (i, s) in samples.iter_mut().take(120).enumerate() {
        *s = (*s + 0.1 * (1.0 - i as f32 / 120.0) * white()).clamp(-1.0, 1.0);
    }
}

/// Without the silence engines put around the words.
fn trim(samples: &[f32]) -> Vec<f32> {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let loud = |s: &&f32| s.abs() > peak * 0.02;
    let start = samples.iter().position(|s| loud(&s)).unwrap_or(0);
    let end = samples.iter().rposition(|s| loud(&s)).map_or(0, |i| i + 1);
    samples[start.min(end)..end].to_vec()
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn from_pcm(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
        .collect()
}

fn to_pcm(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect()
}

/// Mono 16-bit WAV at any rate, resampled to [`RATE`].
pub fn read_wav(bytes: &[u8]) -> Result<Vec<f32>> {
    let (rate, data) = wav_data(bytes)?;
    Ok(wav::resample(&from_pcm(data), rate, RATE))
}

/// Sample rate and PCM bytes of a mono 16-bit WAV.
fn wav_data(bytes: &[u8]) -> Result<(u32, &[u8])> {
    let word = |i: usize| -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?))
    };
    let rate = word(24).context("short WAV")?;
    let mut i = 12;
    while let Some(len) = word(i + 4) {
        if &bytes[i..i + 4] == b"data" {
            let end = (i + 8 + len as usize).min(bytes.len());
            return Ok((rate, &bytes[i + 8..end]));
        }
        i += 8 + len as usize;
    }
    bail!("WAV without data")
}

/// Length of a mono 16-bit WAV.
pub fn wav_seconds(bytes: &[u8]) -> f32 {
    wav_data(bytes).map_or(0.0, |(rate, d)| d.len() as f32 / 2.0 / rate as f32)
}

/// A recording of the user instead of a synthesized voice.
pub fn load_recording(path: &Path) -> Result<Vec<f32>> {
    read_wav(&std::fs::read(path).with_context(|| format!("{}", path.display()))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(s: f32) -> Vec<f32> {
        (0..secs(s)).map(|i| (i as f32 * 0.2).sin() * 0.5).collect()
    }

    fn spec(snr_db: Option<f32>) -> Spec<'static> {
        Spec {
            say: "",
            voice: "vv:1",
            snr_db,
            cut_start_s: 0.0,
            cut_end_s: 0.0,
            duration_s: 0.0,
        }
    }

    #[test]
    fn speech_sits_between_lead_and_tail() {
        let u = compose(tone(1.0), &spec(None));
        assert!((u.seconds() - (1.0 + LEAD_S + TAIL_S)).abs() < 0.001);
        assert!((u.speech_end_s - (1.0 + LEAD_S)).abs() < 0.001);
        let cut = compose(
            tone(1.0),
            &Spec {
                cut_start_s: 0.3,
                cut_end_s: 0.2,
                ..spec(None)
            },
        );
        assert!((cut.seconds() - 0.5).abs() < 0.001, "{}", cut.seconds());
        assert!((cut.speech_end_s - 0.5).abs() < 0.001);
        let button = compose(
            vec![],
            &Spec {
                duration_s: 2.0,
                ..spec(None)
            },
        );
        assert!((button.seconds() - 2.0).abs() < 0.001);
        assert_eq!(button.speech_end_s, button.seconds());
    }

    #[test]
    fn noise_has_the_requested_level() {
        for snr in [20.0f32, 6.0, 0.0] {
            let u = compose(tone(2.0), &spec(Some(snr)));
            // The tail carries noise only.
            let tail = &u.samples[secs(u.speech_end_s) + 10..];
            let measured = 20.0 * (SPEECH_RMS / wav::rms(tail)).log10();
            assert!((measured - snr).abs() < 2.5, "{snr}: {measured}");
        }
        let a = compose(tone(0.5), &spec(Some(5.0)));
        let b = compose(tone(0.5), &spec(Some(5.0)));
        assert_eq!(a.samples, b.samples);
    }

    #[test]
    fn wav_and_pcm_round_trip() {
        let s = tone(0.5);
        let bytes = wav::encode_pcm16(&s, RATE);
        let back = read_wav(&bytes).unwrap();
        assert_eq!(back.len(), s.len());
        assert!(back.iter().zip(&s).all(|(a, b)| (a - b).abs() < 0.001));
        assert!((wav_seconds(&bytes) - 0.5).abs() < 0.001);
        assert!(read_wav(b"RIFF").is_err());
        let pcm = from_pcm(&to_pcm(&s));
        assert!(pcm.iter().zip(&s).all(|(a, b)| (a - b).abs() < 0.001));
    }

    #[test]
    fn trims_silence() {
        let mut s = vec![0.0; 100];
        s.extend([0.5, -0.5, 0.4]);
        s.resize(200, 0.0);
        assert_eq!(trim(&s), vec![0.5, -0.5, 0.4]);
        assert_eq!(trim(&[0.0; 10]), Vec::<f32>::new());
    }
}
