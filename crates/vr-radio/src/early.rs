//! Transcribes while the button is still held.
//!
//! At every pause the words since the last cut are transcribed. A long part
//! is cut off for good, so that a long call is not transcribed as a whole at
//! the end. A short one is only a guess that the driver is done: it is used
//! when nothing more is said, and forgotten otherwise.

use std::future::Future;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::task::JoinHandle;

use crate::audio::{Recorder, Recording};
use crate::stt::Transcriber;
use crate::vad::Pauses;

/// A part this long is cut off at a pause. Shorter ones lack the context to
/// tell words apart.
const CUT_S: f32 = 3.0;

type Text = JoinHandle<Result<String>>;

#[derive(Default)]
pub struct Early {
    pauses: Option<Pauses>,
    samples: Vec<f32>,
    /// Parts cut off, in order, and where the next one starts.
    parts: Vec<Text>,
    cut: usize,
    /// The words from `cut` to a pause, in case they are the last ones.
    guess: Option<(usize, Text)>,
}

fn start(transcriber: &Arc<dyn Transcriber>, samples: &[f32], rate: u32) -> Text {
    let transcriber = transcriber.clone();
    let rec = Recording {
        samples: samples.to_vec(),
        rate,
    };
    tokio::task::spawn_blocking(move || transcriber.transcribe(&rec.to_wav()))
}

impl Early {
    /// Looks at what was recorded since the last time.
    pub fn update(&mut self, recorder: &dyn Recorder, transcriber: &Arc<dyn Transcriber>) {
        let new = recorder.peek(self.samples.len());
        if new.rate == 0 {
            return;
        }
        self.samples.extend_from_slice(&new.samples);
        let pauses = self.pauses.get_or_insert_with(|| Pauses::new(new.rate));
        let Some(end) = pauses.push(&new.samples) else {
            return;
        };
        let part = &self.samples[self.cut..end.max(self.cut)];
        let text = start(transcriber, part, new.rate);
        if part.len() as f32 >= CUT_S * new.rate as f32 {
            self.parts.push(text);
            self.cut = end;
            self.guess = None;
        } else {
            self.guess = Some((end, text));
        }
    }

    /// The whole call as text, `rec` being its recording.
    pub fn finish(
        self,
        rec: Recording,
        transcriber: Arc<dyn Transcriber>,
    ) -> impl Future<Output = Result<String>> + Send + 'static {
        let Self {
            pauses,
            parts,
            cut,
            guess,
            ..
        } = self;
        let words_after = |sample: usize| {
            pauses
                .as_ref()
                .is_none_or(|p| sample == 0 || p.words_after(sample))
        };
        // What the ticks have not seen yet is too short to hold words, as
        // the pause itself was seen.
        let last = match guess {
            Some((end, text)) if !words_after(end) => Some(text),
            // Nothing was said after the last cut.
            _ if !parts.is_empty() && !words_after(cut) => None,
            _ => {
                let rest = rec.samples.get(cut..).unwrap_or_default();
                Some(start(&transcriber, rest, rec.rate))
            }
        };
        async move {
            let mut texts = Vec::new();
            for part in parts.into_iter().chain(last) {
                match part.await {
                    Ok(Ok(text)) => texts.push(text),
                    // One part failed: the whole call once more.
                    Ok(Err(e)) if texts.is_empty() && cut == 0 => return Err(e),
                    _ => {
                        let whole = start(&transcriber, &rec.samples, rec.rate);
                        return whole.await.map_err(|e| anyhow!(e))?;
                    }
                }
            }
            texts.retain(|t| !t.trim().is_empty());
            Ok(texts.join(" "))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::wav;

    const RATE: u32 = 16_000;

    struct Tape(Mutex<Vec<f32>>);
    impl Recorder for Tape {
        fn start(&self) {}
        fn stop(&self) -> Recording {
            self.peek(0)
        }
        fn level(&self) -> f32 {
            0.0
        }
        fn peek(&self, from: usize) -> Recording {
            Recording {
                samples: self.0.lock().unwrap()[from..].to_vec(),
                rate: RATE,
            }
        }
    }

    /// Answers with the length of what it was given, in tenths of a second.
    #[derive(Default)]
    struct Lengths {
        calls: Mutex<Vec<usize>>,
        fail_at: Option<usize>,
    }
    impl Transcriber for Lengths {
        fn transcribe(&self, wav: &[u8]) -> Result<String> {
            let tenths = (wav.len() - 44) / 2 * 10 / wav::STT_RATE as usize;
            let mut calls = self.calls.lock().unwrap();
            calls.push(tenths);
            if self.fail_at == Some(calls.len()) {
                return Err(anyhow!("down"));
            }
            Ok(format!("<{tenths}>"))
        }
    }

    fn sound(pieces: &[(f32, f32)]) -> Vec<f32> {
        pieces
            .iter()
            .flat_map(|(s, level)| {
                let n = (s * RATE as f32) as usize;
                (0..n).map(move |i| (i as f32 * 0.7).sin() * level)
            })
            .collect()
    }

    const WORDS: f32 = 0.2;
    const QUIET: f32 = 0.005;

    /// Records the pieces 50 ms at a time, then lets go.
    async fn call(pieces: &[(f32, f32)], stt: Arc<Lengths>) -> (Result<String>, Vec<usize>) {
        let all = sound(pieces);
        let tape = Tape(Mutex::new(Vec::new()));
        let transcriber: Arc<dyn Transcriber> = stt.clone();
        let mut early = Early::default();
        for chunk in all.chunks(800) {
            tape.0.lock().unwrap().extend_from_slice(chunk);
            early.update(&tape, &transcriber);
            // The guesses run in the order they were made.
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let text = early.finish(tape.stop(), transcriber).await;
        let calls = stt.calls.lock().unwrap().clone();
        (text, calls)
    }

    #[tokio::test]
    async fn a_short_call_is_transcribed_once_before_the_button_is_let_go() {
        let (text, calls) = call(&[(0.3, QUIET), (1.0, WORDS), (0.6, QUIET)], Arc::default()).await;
        // Up to the pause, not to the end.
        assert_eq!((text.unwrap(), calls), ("<15>".to_string(), vec![15]));
    }

    #[tokio::test]
    async fn a_guess_is_dropped_when_the_driver_goes_on() {
        let pieces = [(0.3, QUIET), (1.0, WORDS), (0.5, QUIET), (0.7, WORDS)];
        let (text, calls) = call(&pieces, Arc::default()).await;
        assert_eq!((text.unwrap(), calls), ("<25>".to_string(), vec![15, 25]));
    }

    #[tokio::test]
    async fn a_long_call_is_cut_at_its_pause() {
        let pieces = [
            (0.3, QUIET),
            (3.0, WORDS),
            (0.5, QUIET),
            (1.0, WORDS),
            (0.6, QUIET),
        ];
        let (text, calls) = call(&pieces, Arc::default()).await;
        assert_eq!(calls, vec![35, 15]);
        assert_eq!(text.unwrap(), "<35> <15>");
    }

    #[tokio::test]
    async fn silence_after_the_last_cut_is_not_transcribed() {
        let pieces = [(0.3, QUIET), (3.5, WORDS), (1.0, QUIET)];
        let (text, calls) = call(&pieces, Arc::default()).await;
        assert_eq!((text.unwrap(), calls), ("<40>".to_string(), vec![40]));
    }

    #[tokio::test]
    async fn without_a_pause_the_whole_call_is_transcribed() {
        let (text, calls) = call(&[(1.0, WORDS)], Arc::default()).await;
        assert_eq!((text.unwrap(), calls), ("<10>".to_string(), vec![10]));
        let nothing = Early::default().finish(
            Recording {
                samples: vec![0.0; 8000],
                rate: RATE,
            },
            Arc::new(Lengths::default()),
        );
        assert_eq!(nothing.await.unwrap(), "<5>");
    }

    #[tokio::test]
    async fn a_failed_part_falls_back_to_the_whole_call() {
        let stt = Arc::new(Lengths {
            fail_at: Some(1),
            ..Lengths::default()
        });
        let pieces = [
            (0.3, QUIET),
            (3.0, WORDS),
            (0.5, QUIET),
            (1.0, WORDS),
            (0.6, QUIET),
        ];
        let (text, calls) = call(&pieces, stt).await;
        assert_eq!(text.unwrap(), "<54>");
        assert_eq!(calls, vec![35, 15, 54]);
        // A single call that fails is not repeated.
        let stt = Arc::new(Lengths {
            fail_at: Some(1),
            ..Lengths::default()
        });
        let (text, calls) = call(&[(1.0, WORDS)], stt).await;
        assert_eq!(text.unwrap_err().to_string(), "down");
        assert_eq!(calls, vec![10]);
    }
}
