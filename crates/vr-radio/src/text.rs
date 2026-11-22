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
    /// Nothing was cut yet: speaking starts with the first clause.
    first: bool,
}

/// Length from which the first clause is spoken on its own; shorter ones
/// too when they hold a number (`6位、`), the answer a driver waits for.
const FIRST_CLAUSE: usize = 6;

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
            first: true,
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
                self.first = false;
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
        let mut digit = false;
        while let Some((i, c)) = chars.next() {
            count += 1;
            digit |= c.is_ascii_digit() || ('０'..='９').contains(&c);
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
            let limit = if self.first {
                FIRST_CLAUSE.min(self.soft_limit)
            } else {
                self.soft_limit
            };
            if SOFT_STOPS.contains(&c) && (count >= limit || (self.first && digit)) {
                return Some(end);
            }
        }
        None
    }
}

/// Whether `text` must not be read aloud to a driver who speaks `language`:
/// an English sentence in a Japanese radio is an error of the agent (an
/// expired login...), not an answer. Names and units alone are fine.
pub fn foreign(text: &str, language: Option<&str>) -> bool {
    let japanese = |c: char| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}');
    language == Some("ja")
        && !text.chars().any(japanese)
        && text.chars().filter(char::is_ascii_alphabetic).count() >= 16
}

/// Japanese without the blanks that written text puts around numbers and
/// Latin words. VOICEVOX pauses 0.4 s at every one, and reads `1 分` as two
/// words. Those between Latin words stay.
pub fn joined(text: &str) -> String {
    let wide = |c: Option<char>| c.is_some_and(|c| c >= '\u{2e80}');
    let mut out = String::with_capacity(text.len());
    let mut blank = false;
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\u{3000}') {
            blank = true;
            continue;
        }
        if blank && !wide(out.chars().next_back()) && !wide(Some(c)) {
            out.push(' ');
        }
        blank = false;
        out.push(c);
    }
    out
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
    let out = joined(out.trim());
    let has_content = out
        .chars()
        .any(|c| c.is_alphanumeric() || ('\u{3040}'..='\u{9fff}').contains(&c));
    has_content.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanks_go_where_japanese_begins_or_ends() {
        assert_eq!(
            joined("残り 13 周、Ito まで 0.6 秒"),
            "残り13周、Itoまで0.6秒"
        );
        assert_eq!(joined("1 分 30 秒 6"), "1分30秒6");
        assert_eq!(joined("うしろ　は"), "うしろは");
        assert_eq!(joined("A. Senna と Demo Raceway"), "A. SennaとDemo Raceway");
        assert_eq!(joined("a  b"), "a b");
        assert_eq!(joined("前は 0.6\n後ろは 0.8"), "前は0.6\n後ろは0.8");
        assert_eq!(joined(" 前 "), "前");
        // The first character that counts as Japanese.
        assert_eq!(joined("a \u{2e80}"), "a\u{2e80}");
        assert_eq!(joined("a \u{2e7f}"), "a \u{2e7f}");
    }

    #[test]
    fn the_first_clause_is_spoken_early_and_only_the_first() {
        let mut s = SentenceSplitter::default();
        assert_eq!(s.push("前の Ito まで 0.6 秒、"), ["前のItoまで0.6秒、"]);
        assert!(s.push("毎周 0.4 秒ずつ詰めてる、").is_empty());
        assert_eq!(s.push("2 周で射程内。").len(), 1);
        // Five characters and the comma are enough, fewer are not (了解、).
        assert_eq!(SentenceSplitter::default().push("一二三四五、").len(), 1);
        assert!(SentenceSplitter::default().push("一二三四、").is_empty());
        // A short first clause with a number is spoken at once, a later one
        // waits as before.
        let mut s = SentenceSplitter::default();
        assert_eq!(s.push("6位、後ろ 0.8、"), ["6位、"]);
        assert!(s.push("迫ってる").is_empty());
        assert_eq!(
            SentenceSplitter::default().push("0.5秒差、毎周"),
            ["0.5秒差、"]
        );
        assert!(SentenceSplitter::default().push("了解、").is_empty());
        assert_eq!(SentenceSplitter::default().push("６位、"), ["６位、"]);
        assert!(SentenceSplitter::default().push("六位、").is_empty());
    }

    #[test]
    fn agent_errors_in_another_language_are_not_for_the_driver() {
        let error = "Failed to authenticate: OAuth session expired";
        assert!(foreign(error, Some("ja")));
        assert!(!foreign(error, Some("en")));
        assert!(!foreign(error, None));
        assert!(!foreign("前は Kowalski、P4 だ。", Some("ja")));
        assert!(!foreign("OK, P4.", Some("ja")));
        assert!(!foreign("abcdefghijklmno", Some("ja")));
        assert!(foreign("abcdefghijklmnop", Some("ja")));
    }

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
            vec!["前のItoまで0.6、", "毎周0.4詰めてる。", "2周で射程内。"]
        );
    }

    #[test]
    fn emits_only_completed_sentences_while_streaming() {
        let mut s = SentenceSplitter::default();
        assert!(s.push("燃料はあと").is_empty());
        assert_eq!(s.push("5 周。給油"), vec!["燃料はあと5周。"]);
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
            Some("前との差は0.6秒")
        );
        assert_eq!(speakable("- 燃料 OK").as_deref(), Some("燃料OK"));
        assert_eq!(speakable("| a | b |").as_deref(), Some("a b"));
    }

    #[test]
    fn lines_join_with_single_spaces() {
        assert_eq!(speakable("前\n後").as_deref(), Some("前後"));
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
