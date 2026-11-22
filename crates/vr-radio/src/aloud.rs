//! Numbers as they are said on the radio: `0.1秒` is コンマ1秒, `1.2秒` is
//! 1秒2, `1:55.611` is 1分55秒ロクイチイチ, `P4` is 4番手.
//!
//! Only for the voice: what is shown stays as it was written.

const DIGITS: [&str; 10] = [
    "ゼロ",
    "イチ",
    "ニー",
    "サン",
    "ヨン",
    "ゴー",
    "ロク",
    "ナナ",
    "ハチ",
    "キュー",
];

/// Digits one by one.
fn spelt(digits: &[char]) -> String {
    digits
        .iter()
        .filter_map(|c| c.to_digit(10))
        .map(|d| DIGITS[d as usize])
        .collect()
}

/// Behind the decimal point: one digit as a number, several one by one.
/// Zeros at the end say nothing.
fn fraction(digits: &[char]) -> String {
    let end = digits.iter().rposition(|c| *c != '0').map_or(0, |i| i + 1);
    match &digits[..end] {
        [] => String::new(),
        [d] => d.to_string(),
        all => spelt(all),
    }
}

/// `12.345` from `i`: the whole part, the decimals, and the index behind.
fn number(c: &[char], i: usize) -> (String, Vec<char>, usize) {
    let mut j = i;
    while c.get(j).is_some_and(char::is_ascii_digit) {
        j += 1;
    }
    let whole: String = c[i..j].iter().collect();
    let mut decimals = Vec::new();
    if c.get(j) == Some(&'.') && c.get(j + 1).is_some_and(char::is_ascii_digit) {
        j += 1;
        while c.get(j).is_some_and(char::is_ascii_digit) {
            decimals.push(c[j]);
            j += 1;
        }
    }
    (whole, decimals, j)
}

fn starts(c: &[char], i: usize, word: &str) -> bool {
    word.chars()
        .enumerate()
        .all(|(k, w)| c.get(i + k) == Some(&w))
}

/// Minutes and seconds: `1:55.611` or `1分55秒611`. The text and the index
/// behind.
fn lap_time(c: &[char], i: usize) -> Option<(String, usize)> {
    let digits = |from: usize| {
        let mut to = from;
        while c.get(to).is_some_and(char::is_ascii_digit) {
            to += 1;
        }
        (to > from).then_some(to)
    };
    let m = digits(i)?;
    let colon = c.get(m) == Some(&':');
    if !colon && c.get(m) != Some(&'分') {
        return None;
    }
    let s = digits(m + 1)?;
    if s - m - 1 > 2 || (colon && s - m - 1 != 2) {
        return None;
    }
    let seconds: String = c[m + 1..s].iter().collect();
    // Behind the seconds: `.611` or `秒611`, or `.611秒`.
    let point = match c.get(s) {
        Some('.') => true,
        Some('秒') if !colon => false,
        _ if colon => {
            return Some((format!("{}分{}秒", text(&c[i..m]), trim(&seconds)), s));
        }
        _ => return None,
    };
    let end = digits(s + 1);
    let decimals = end.map_or(&c[0..0], |e| &c[s + 1..e]);
    if point && decimals.is_empty() {
        return None;
    }
    let mut next = end.unwrap_or(s + 1);
    if point && c.get(next) == Some(&'秒') {
        next += 1;
    }
    Some((
        format!(
            "{}分{}秒{}",
            text(&c[i..m]),
            trim(&seconds),
            fraction(decimals)
        ),
        next,
    ))
}

fn text(c: &[char]) -> String {
    c.iter().collect()
}

/// `05` is 5.
fn trim(digits: &str) -> &str {
    let t = digits.trim_start_matches('0');
    if t.is_empty() { "0" } else { t }
}

/// Not seconds: said as written.
const UNITS: [&str; 12] = [
    "L", "l", "リ", "%", "％", "パ", "度", "℃", "°", "k", "m", "M",
];

pub fn aloud(input: &str) -> String {
    let c: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 16);
    let mut i = 0;
    while i < c.len() {
        let before = i.checked_sub(1).map(|k| c[k]);
        let digit = |k: usize| c.get(k).is_some_and(char::is_ascii_digit);
        let word = before.is_some_and(|b| b.is_ascii_alphanumeric());
        match c[i] {
            'P' if digit(i + 1) && !word => {
                let (n, d, j) = number(&c, i + 1);
                if d.is_empty() && !c.get(j).is_some_and(char::is_ascii_alphanumeric) {
                    out.push_str(&n);
                    out.push_str("番手");
                    i = j;
                } else {
                    out.push('P');
                    i += 1;
                }
                continue;
            }
            '#' | '＃' if digit(i + 1) => {
                let (n, _, j) = number(&c, i + 1);
                out.push_str(&n);
                if !starts(&c, j, "号車") && !starts(&c, j, "番") {
                    out.push_str("号車");
                }
                i = j;
                continue;
            }
            '+' | '＋' if digit(i + 1) => {
                out.push_str("プラス");
                i += 1;
                continue;
            }
            '-' | '−' | '－' if digit(i + 1) => {
                // `5-6周` is a range.
                out.push_str(if before.is_some_and(|b| b.is_ascii_digit()) {
                    "から"
                } else if word {
                    "-"
                } else {
                    "マイナス"
                });
                i += 1;
                continue;
            }
            '→' => {
                out.push_str("から");
                i += 1;
                continue;
            }
            '~' | '〜' | '～' if before.is_some_and(|b| b.is_ascii_digit()) && digit(i + 1) => {
                out.push_str("から");
                i += 1;
                continue;
            }
            d if d.is_ascii_digit() && !word => {
                if let Some((t, j)) = lap_time(&c, i) {
                    out.push_str(&t);
                    i = j;
                    continue;
                }
                let (whole, decimals, j) = number(&c, i);
                let seconds = c.get(j) == Some(&'秒');
                let other = UNITS.iter().any(|u| starts(&c, j, u));
                let tail = fraction(&decimals);
                if decimals.is_empty() || (other && !seconds) {
                    out.push_str(&whole);
                    if !decimals.is_empty() {
                        out.push('.');
                        out.extend(&decimals);
                    }
                    i = j;
                } else if whole.chars().all(|z| z == '0') {
                    if tail.is_empty() {
                        out.push('0');
                    } else {
                        out.push_str("コンマ");
                        out.push_str(&tail);
                    }
                    i = j;
                } else if seconds {
                    out.push_str(&whole);
                    out.push('秒');
                    out.push_str(&tail);
                    i = j + 1;
                } else {
                    out.push_str(&whole);
                    if !tail.is_empty() {
                        out.push('.');
                        out.extend(&decimals);
                    }
                    i = j;
                }
                // The units VOICEVOX spells.
                let letter = |k: usize| c.get(k).is_some_and(char::is_ascii_alphabetic);
                if starts(&c, i, "km/h") {
                    out.push_str("キロ");
                    i += 4;
                } else if matches!(c.get(i), Some('L')) && !letter(i + 1) {
                    out.push_str("リットル");
                    i += 1;
                } else if c.get(i) == Some(&'℃') {
                    out.push('度');
                    i += 1;
                } else if starts(&c, i, "°C") {
                    out.push('度');
                    i += 2;
                }
                continue;
            }
            other => out.push(other),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn says(written: &str, said: &str) {
        assert_eq!(aloud(written), said);
    }

    #[test]
    fn tenths_are_said_with_a_comma() {
        says("前まで0.1秒", "前までコンマ1秒");
        says("毎周0.4詰めてる", "毎周コンマ4詰めてる");
        says("0.08秒速い", "コンマゼロハチ秒速い");
        says("0.12", "コンマイチニー");
        says("0.50秒", "コンマ5秒");
        says("差は0.0秒", "差は0秒");
        says("0.6。", "コンマ6。");
    }

    #[test]
    fn seconds_come_before_the_tenths() {
        says("1.2秒差", "1秒2差");
        says("後ろまで12.4秒", "後ろまで12秒4");
        says("3.0秒", "3秒");
        says("2.25秒", "2秒ニーゴー");
        // Without the unit it stays a number.
        says("1.2詰めてる", "1.2詰めてる");
        says("5.0", "5");
        says("10秒", "10秒");
    }

    #[test]
    fn lap_times() {
        says("ベスト1:55.611", "ベスト1分55秒ロクイチイチ");
        says("1:30.600だった", "1分30秒6だった");
        says("1分30秒600", "1分30秒6");
        says("1分30秒6", "1分30秒6");
        says("2分03秒387", "2分3秒サンハチナナ");
        says("1:05.0", "1分5秒");
        says("1:30.6秒", "1分30秒6");
        says("残り12:05", "残り12分5秒");
        says("1分で", "1分で");
        says("3:1", "3:1");
    }

    #[test]
    fn other_units_keep_their_decimals() {
        says("20.7L足りない", "20.7リットル足りない");
        says("0.46リットル", "0.46リットル");
        says("54.5%", "54.5%");
        says("0.5パーセント", "0.5パーセント");
        says("90℃", "90度");
        says("水温88°C", "水温88度");
        says("138kPa", "138kPa");
        says("220km/h", "220キロ");
        says("2.5周", "2.5周");
    }

    #[test]
    fn signs_places_and_cars() {
        says("+0.4秒", "プラスコンマ4秒");
        says("差は−0.3", "差はマイナスコンマ3");
        says("5-6周", "5から6周");
        says("5〜6周", "5から6周");
        says("P20→P14", "20番手から14番手");
        says("今P4だ", "今4番手だ");
        says("P2P", "P2P");
        says("#12のIto", "12号車のIto");
        says("#12号車", "12号車");
        says("SF23", "SF23");
        says("-", "-");
        says("", "");
    }

    #[test]
    fn several_in_one_sentence() {
        says(
            "前のItoまで0.6。毎周0.4詰めてる、残り13周で1:30.6。",
            "前のItoまでコンマ6。毎周コンマ4詰めてる、残り13周で1分30秒6。",
        );
    }
}
