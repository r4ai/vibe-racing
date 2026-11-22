//! Finds the pauses in a recording while it is being made, so that the words
//! before them can be transcribed without waiting for the button.
//!
//! The level is compared with the quietest part of the last seconds: engine
//! and wind are steady, words are not. When the words do not stand out, no
//! pause is found and the recording is transcribed whole, as without this.

/// Length of the blocks whose level is measured.
const BLOCK_S: f32 = 0.03;
/// Words are this much louder than the background.
const MARGIN: f32 = 2.5;
/// Silence that ends a phrase.
const PAUSE_S: f32 = 0.25;
/// Sound that counts as words rather than a click.
const MIN_WORDS_S: f32 = 0.25;
/// The background is the quietest level heard in this time.
const BACKGROUND_S: f32 = 4.0;
/// A digital silence is no background: the noise of any microphone is louder.
const MIN_BACKGROUND: f32 = 0.002;

#[derive(Debug, Clone)]
pub struct Pauses {
    rate: u32,
    /// Samples not yet making up a block.
    rest: Vec<f32>,
    /// Samples in complete blocks.
    seen: usize,
    levels: Vec<f32>,
    /// Where the current phrase began, and its loud blocks.
    loud_blocks: usize,
    /// Recent level of the words.
    words_level: f32,
    quiet_blocks: usize,
    last_word: Option<usize>,
}

impl Pauses {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            rest: Vec::new(),
            seen: 0,
            levels: Vec::new(),
            loud_blocks: 0,
            words_level: 0.0,
            quiet_blocks: 0,
            last_word: None,
        }
    }

    fn block(&self) -> usize {
        ((self.rate as f32 * BLOCK_S) as usize).max(1)
    }

    fn background(&self) -> f32 {
        let n = (BACKGROUND_S / BLOCK_S) as usize;
        let recent = &self.levels[self.levels.len().saturating_sub(n)..];
        let mut sorted = recent.to_vec();
        sorted.sort_by(f32::total_cmp);
        // A tenth of the time nobody speaks, whatever the sentence.
        sorted
            .get(sorted.len() / 10)
            .copied()
            .unwrap_or(0.0)
            .max(MIN_BACKGROUND)
    }

    /// Takes the next samples; returns where a pause just began to count,
    /// as a number of samples from the start.
    pub fn push(&mut self, samples: &[f32]) -> Option<usize> {
        self.rest.extend_from_slice(samples);
        let block = self.block();
        let mut pause = None;
        let whole = self.rest.len() / block * block;
        let blocks: Vec<f32> = self.rest.drain(..whole).collect();
        for b in blocks.chunks(block) {
            self.levels.push(crate::wav::rms(b));
            self.seen += block;
            let level = *self.levels.last().expect("just pushed");
            // Without a gap for seconds the words become the background: they
            // are still as loud as the words before them.
            let like_words = self.words_level > 0.0 && level > self.words_level * 0.5;
            if level > self.background() * MARGIN || like_words {
                self.words_level = if self.loud_blocks == 0 {
                    level
                } else {
                    self.words_level * 0.9 + level * 0.1
                };
                self.loud_blocks += 1;
                self.quiet_blocks = 0;
                self.last_word = Some(self.seen);
                continue;
            }
            self.quiet_blocks += 1;
            let needed = (PAUSE_S / BLOCK_S).ceil() as usize;
            let words = (MIN_WORDS_S / BLOCK_S).ceil() as usize;
            if self.quiet_blocks == needed && self.loud_blocks >= words {
                self.loud_blocks = 0;
                self.words_level = 0.0;
                pause = Some(self.seen);
            }
        }
        pause
    }

    /// Whether words were heard after the given sample.
    pub fn words_after(&self, sample: usize) -> bool {
        self.last_word.is_some_and(|w| w > sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// `(seconds, level)` pieces of a steady tone.
    fn sound(pieces: &[(f32, f32)]) -> Vec<f32> {
        let mut out = Vec::new();
        for (s, level) in pieces {
            let n = (s * RATE as f32) as usize;
            out.extend((0..n).map(|i| (i as f32 * 0.7).sin() * level));
        }
        out
    }

    /// Feeds 50 ms at a time, like the engine; pauses in seconds.
    fn pauses(samples: &[f32]) -> (Vec<f32>, Pauses) {
        let mut p = Pauses::new(RATE);
        let found = samples
            .chunks(800)
            .filter_map(|c| p.push(c))
            .map(|i| i as f32 / RATE as f32)
            .collect();
        (found, p)
    }

    #[test]
    fn a_pause_is_found_shortly_after_the_words() {
        let (found, p) = pauses(&sound(&[(0.3, 0.005), (1.0, 0.2), (0.6, 0.005)]));
        assert_eq!(found.len(), 1);
        assert!((1.5..1.7).contains(&found[0]), "{found:?}");
        assert!(p.words_after(RATE as usize));
        assert!(!p.words_after((1.4 * RATE as f32) as usize));
    }

    #[test]
    fn every_phrase_ends_once() {
        let (found, _) = pauses(&sound(&[
            (0.3, 0.005),
            (0.8, 0.2),
            (0.5, 0.005),
            (0.8, 0.2),
            (0.8, 0.005),
        ]));
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn short_gaps_and_clicks_are_no_pauses() {
        // 0.15 s between words; a click of 0.06 s.
        let (found, _) = pauses(&sound(&[
            (0.3, 0.005),
            (0.6, 0.2),
            (0.15, 0.005),
            (0.6, 0.2),
        ]));
        assert_eq!(found, Vec::<f32>::new());
        let (found, _) = pauses(&sound(&[(0.5, 0.005), (0.06, 0.2), (1.0, 0.005)]));
        assert_eq!(found, Vec::<f32>::new());
    }

    #[test]
    fn steady_noise_is_the_background() {
        // Loud engine, words on top of it.
        let (found, _) = pauses(&sound(&[(0.6, 0.05), (1.0, 0.3), (0.6, 0.05)]));
        assert_eq!(found.len(), 1, "{found:?}");
        // Noise alone, and words that do not stand out.
        assert!(pauses(&sound(&[(3.0, 0.05)])).0.is_empty());
        assert!(
            pauses(&sound(&[(0.6, 0.05), (1.0, 0.08), (0.6, 0.05)]))
                .0
                .is_empty()
        );
        // Words without a gap for seconds are not cut.
        let (found, _) = pauses(&sound(&[(0.2, 0.005), (8.0, 0.2), (0.6, 0.005)]));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0] > 8.3, "{found:?}");
        // Silence.
        let (found, p) = pauses(&sound(&[(2.0, 0.0)]));
        assert!(found.is_empty() && !p.words_after(0));
    }
}
