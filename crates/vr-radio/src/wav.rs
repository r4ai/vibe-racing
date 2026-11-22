//! Minimal PCM helpers: downmix, resample and encode 16-bit mono WAV.

/// Sample rate sent to the transcriber (what Whisper-class models use).
pub const STT_RATE: u32 = 16_000;

/// Averages interleaved frames into mono.
pub fn downmix(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let channels = usize::from(channels.max(1));
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Linear-interpolation resampling. Good enough for speech recognition.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    // Equal rates and empty inputs fall through to an identical copy.
    if from == 0 || to == 0 {
        return input.to_vec();
    }
    let out_len = (input.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    let step = f64::from(from) / f64::from(to);
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * step;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            // pos < input.len(): i < out_len = len * to / from.
            let a = input[idx];
            let b = input[(idx + 1).min(input.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// Encodes mono samples (-1.0..=1.0) as a 16-bit PCM WAV file.
pub fn encode_pcm16(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Root-mean-square level of a block (0.0 for silence).
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_frames() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.25, -0.25], 1), vec![0.25, -0.25]);
        assert_eq!(downmix(&[0.3], 0), vec![0.3]);
    }

    #[test]
    fn resample_halves_and_interpolates() {
        assert_eq!(resample(&[0.0, 1.0, 2.0, 3.0], 2, 1), vec![0.0, 2.0]);
        assert_eq!(resample(&[0.0, 1.0], 1, 2), vec![0.0, 0.5, 1.0, 1.0]);
        assert_eq!(resample(&[0.1, 0.2], 48_000, 48_000), vec![0.1, 0.2]);
        assert_eq!(resample(&[], 48_000, 16_000), Vec::<f32>::new());
        // A zero rate cannot be resampled: the input is returned as is.
        assert_eq!(resample(&[0.1, 0.2], 0, 16_000), vec![0.1, 0.2]);
        assert_eq!(resample(&[0.1, 0.2], 16_000, 0), vec![0.1, 0.2]);
        assert_eq!(resample(&[0.0; 48_000], 48_000, 16_000).len(), 16_000);
    }

    #[test]
    fn wav_header_and_samples() {
        let wav = encode_pcm16(&[0.0, 1.0, -1.0, 2.0], 16_000);
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        let samples: Vec<i16> = wav[44..]
            .chunks(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(samples, vec![0, 32767, -32767, 32767]);
    }

    #[test]
    fn rms_of_constant_and_empty() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.5, -0.5]), 0.5);
    }
}
