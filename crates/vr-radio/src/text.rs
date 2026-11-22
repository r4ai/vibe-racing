//! Splits the agent's streamed reply into sentences that can be spoken as
//! soon as they are complete, and strips what a voice must not read aloud.

/// Characters that always end a sentence.
const HARD_STOPS: [char; 7] = ['。', '！', '？', '!', '?', '\n', '…'];
/// Characters where a long sentence may be cut early to start speaking sooner.
const SOFT_STOPS: [char; 3] = ['、', '，', ','];

/// Accumulates streamed text and yields speakable sentences.
#[derive(Debug, Clone)]
pub struct SentenceSplitter {
    buf: String,
    /// Cut at a soft stop once the pending sentence has this many characters.
    soft_limit: usize,
}

impl Default for SentenceSplitter {
    fn default() -> Self {
        Self::new(40)
    }
}

impl SentenceSplitter {
    pub fn new(soft_limit: usize) -> Self {
        Self {
            buf: String::new(),
            soft_limit,
        }
    }

    /// Adds a chunk and returns the sentences completed by it.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(end) = self.find_cut() {
            if end == 0 {
                break; // never loop on an empty cut
            }
            let sentence: String = self.buf.drain(..end).collect();
            if let Some(s) = speakable(&sentence) {
                out.push(s);
            }
        }
        out
    }

    /// Returns whatever is left once the reply is complete.
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        speakable(&rest)
    }

    /// Discards pending text (barge-in).
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Byte index just past the first cut point, if any.
    fn find_cut(&self) -> Option<usize> {
        let mut chars = self.buf.char_indices().peekable();
        let mut count = 0;
        while let Some((i, c)) = chars.next() {
            count += 1;
            let end = i + c.len_utf8();
            if HARD_STOPS.contains(&c) {
                // Keep runs like "！？" or "……" together.
                let mut end = end;
                while let Some(&(j, n)) = chars.peek() {
                    if HARD_STOPS.contains(&n) && n != '\n' {
                        end = j + n.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                return Some(end);
            }
            if c == '.' {
                // "0.6" or "vr.exe" must not split; only ". " does.
                match chars.peek() {
                    Some(&(_, n)) if n.is_whitespace() => return Some(end),
                    Some(_) => {}
                    None => return None, // wait for the next chunk
                }
            }
            if SOFT_STOPS.contains(&c) && count >= self.soft_limit {
                return Some(end);
            }
        }
        None
    }
}

/// Removes Markdown and decoration; `None` if nothing is left to say.
pub fn speakable(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        let line = line.trim_start();
        let line = line
            .trim_start_matches(['#', '>'])
            .trim_start_matches("- ")
            .trim_start_matches("* ");
        for c in line.chars() {
            match c {
                '*' | '`' | '_' | '|' | '#' | '~' => {}
                c if c.is_whitespace() => {
                    if !out.ends_with(' ') && !out.is_empty() {
                        out.push(' ');
                    }
                }
                c => out.push(c),
            }
        }
        if !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
    }
    let out = out.trim().to_string();
    let has_content = out
        .chars()
        .any(|c| c.is_alphanumeric() || ('\u{3040}'..='\u{9fff}').contains(&c));
    has_content.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split_all(chunks: &[&str]) -> Vec<String> {
        let mut s = SentenceSplitter::default();
        let mut out: Vec<String> = chunks.iter().flat_map(|c| s.push(c)).collect();
        out.extend(s.finish());
        out
    }

    #[test]
    fn splits_japanese_sentences_across_chunks() {
        assert_eq!(
            split_all(&[
                "前の Ito まで 0.",
                "6、毎周 0.4 詰めてる",
                "。2 周で射程内。"
            ]),
            vec!["前の Ito まで 0.6、毎周 0.4 詰めてる。", "2 周で射程内。"]
        );
    }

    #[test]
    fn emits_only_completed_sentences_while_streaming() {
        let mut s = SentenceSplitter::default();
        assert!(s.push("燃料はあと").is_empty());
        assert_eq!(s.push("5 周。給油"), vec!["燃料はあと5 周。"]);
        assert_eq!(s.finish().as_deref(), Some("給油"));
        assert_eq!(s.finish(), None);
    }

    #[test]
    fn keeps_punctuation_runs_together() {
        assert_eq!(
            split_all(&["ボックス！？了解。"]),
            vec!["ボックス！？", "了解。"]
        );
    }

    #[test]
    fn ascii_period_splits_only_before_whitespace() {
        assert_eq!(
            split_all(&["Gap is 0.6. Push now."]),
            vec!["Gap is 0.6.", "Push now."]
        );
        // A trailing '.' waits: the next chunk may be a digit.
        let mut s = SentenceSplitter::default();
        assert!(s.push("lap 3.").is_empty());
        assert!(s.push("5 done").is_empty());
        assert_eq!(s.finish().as_deref(), Some("lap 3.5 done"));
    }

    #[test]
    fn soft_stop_cuts_only_at_the_limit() {
        let mut s = SentenceSplitter::new(5);
        // "あいう、" is 4 chars: below the limit, no cut.
        assert!(s.push("あいう、").is_empty());
        // "あいう、えお、" reaches 7 chars at the second comma (>= 5).
        assert_eq!(s.push("えお、か"), vec!["あいう、えお、"]);
        let mut s = SentenceSplitter::new(4);
        assert_eq!(s.push("あいう、え"), vec!["あいう、"]);
    }

    #[test]
    fn newline_ends_a_sentence() {
        assert_eq!(split_all(&["一行目\n二行目"]), vec!["一行目", "二行目"]);
    }

    #[test]
    fn strips_markdown() {
        assert_eq!(
            speakable("## **前**との差は `0.6` 秒").as_deref(),
            Some("前との差は 0.6 秒")
        );
        assert_eq!(speakable("- 燃料 OK").as_deref(), Some("燃料 OK"));
        assert_eq!(speakable("| a | b |").as_deref(), Some("a b"));
    }

    #[test]
    fn lines_join_with_single_spaces() {
        assert_eq!(speakable("前\n後").as_deref(), Some("前 後"));
        assert_eq!(speakable("a \nb").as_deref(), Some("a b"));
        assert_eq!(speakable("\n\nb").as_deref(), Some("b"));
    }

    #[test]
    fn drops_text_without_content() {
        assert_eq!(speakable("。"), None);
        assert_eq!(speakable("  **  \n"), None);
        assert_eq!(speakable("---"), None);
        assert_eq!(speakable("1").as_deref(), Some("1"));
        assert_eq!(speakable("あ").as_deref(), Some("あ"));
    }

    #[test]
    fn clear_discards_pending_text() {
        let mut s = SentenceSplitter::default();
        s.push("途中まで");
        s.clear();
        assert_eq!(s.finish(), None);
    }
}
