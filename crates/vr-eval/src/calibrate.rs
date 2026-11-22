//! Replies whose grade is known by construction, to measure the judges and
//! the number check before trusting them.

use vr_engine::GapInfo;

use crate::facts::{Fact, Sample};
use crate::judge::{Case, Grade};
use crate::scenario::{Behavior, WorldSpec};
use crate::world::{sample_at, text_at};

pub struct Labelled {
    pub name: String,
    pub case: Case,
    pub label: Grade,
    /// Checked by number, with the expected result.
    pub fact: Option<(Fact, bool)>,
    pub sample: Sample,
}

/// Correct, subtly wrong and incomplete replies in several races.
pub fn cases() -> Vec<Labelled> {
    let mut out = Vec::new();
    for (i, t) in [620.0, 1200.0, 1500.0].into_iter().enumerate() {
        let spec = WorldSpec::default();
        let s = sample_at(&spec, t);
        let truth = text_at(&spec, t);
        let b = &s.brief;
        let (Some(a), Some(z), Some(f), Some(p), Some(ses)) =
            (&b.ahead, &b.behind, &b.fuel, &b.player, &b.session)
        else {
            continue;
        };
        let name = |g: &GapInfo| {
            g.car
                .driver
                .split(' ')
                .next_back()
                .unwrap_or("")
                .to_string()
        };
        let trend = |g: &GapInfo| match g.trend_s_per_lap {
            Some(t) if t < 0.0 => format!("毎周{:.1}秒縮まってる", -t),
            Some(t) => format!("毎周{t:.1}秒広がってる"),
            None => "推移は取れていない".into(),
        };
        let flipped = |g: &GapInfo| match g.trend_s_per_lap {
            Some(t) if t < 0.0 => format!("毎周{:.1}秒広がってる", -t),
            Some(t) => format!("毎周{t:.1}秒縮まってる"),
            None => "毎周0.5秒縮まってる".into(),
        };
        let ga = a.gap_s.unwrap_or(0.0);
        let gz = z.gap_s.unwrap_or(0.0);
        let pos = p.car.position.unwrap_or(0);
        let left = ses.laps_remaining.unwrap_or(0);
        let last = p.last_lap_s.unwrap_or(0.0);
        let laps_of_fuel = f.laps_of_fuel.unwrap_or(0.0);
        let mut add = |what: &str,
                       said: &str,
                       behavior: Behavior,
                       points: &[&str],
                       reply: String,
                       label: Grade,
                       fact: Option<(Fact, bool)>| {
            out.push(Labelled {
                name: format!("{i}-{what}"),
                case: Case {
                    said: said.into(),
                    history: vec![],
                    behavior,
                    points: points.iter().map(|p| p.to_string()).collect(),
                    truth_at_question: truth.clone(),
                    truth_at_answer: truth.clone(),
                    truth_between: vec![s.line()],
                    reply,
                },
                label,
                fact,
                sample: s.clone(),
            });
        };
        use Behavior::{Ack, Answer, AnswerOrAsk, Calm, Silent};
        use Grade::{Good, Incomplete, Wrong};

        let q = "前との差は？";
        let pt = ["前の車との差（秒）"];
        add(
            "gap",
            q,
            Answer,
            &pt,
            format!("前の{}まで{ga:.1}秒。", name(a)),
            Good,
            Some((Fact::GapAhead, true)),
        );
        add(
            "gap-off",
            q,
            Answer,
            &pt,
            format!("前の{}まで{:.1}秒。", name(a), ga + 0.4),
            Wrong,
            Some((Fact::GapAhead, false)),
        );
        add(
            "gap-swapped",
            q,
            Answer,
            &pt,
            format!("前まで{gz:.1}秒。"),
            Wrong,
            Some((Fact::GapAhead, false)),
        );
        add(
            "gap-vague",
            q,
            Answer,
            &pt,
            "前は見えてる、プッシュしよう。".into(),
            Incomplete,
            Some((Fact::GapAhead, false)),
        );
        add(
            "gap-wrong-name",
            q,
            Answer,
            &pt,
            format!("前のSmithまで{ga:.1}秒。"),
            Wrong,
            None,
        );
        add(
            "gap-filler",
            q,
            Answer,
            &pt,
            format!("了解、確認する。前まで{ga:.1}秒だ。"),
            Good,
            Some((Fact::GapAhead, true)),
        );

        let q = "後ろとの差と、縮まってるかどうかは？";
        let pt = ["後ろの車との差（秒）", "縮まっているか広がっているか"];
        add(
            "trend",
            q,
            Answer,
            &pt,
            format!("後ろは{gz:.1}秒、{}。", trend(z)),
            Good,
            Some((Fact::GapBehind, true)),
        );
        add(
            "trend-flipped",
            q,
            Answer,
            &pt,
            format!("後ろは{gz:.1}秒、{}。", flipped(z)),
            Wrong,
            Some((Fact::GapBehind, true)),
        );
        add(
            "trend-missing",
            q,
            Answer,
            &pt,
            format!("後ろは{gz:.1}秒。"),
            Incomplete,
            Some((Fact::GapBehind, true)),
        );

        let q = "今何位で、残り何周？";
        let pt = ["現在の順位", "残り周回数"];
        add(
            "pos",
            q,
            Answer,
            &pt,
            format!("{pos}位、残り{left}周。"),
            Good,
            Some((Fact::Position, true)),
        );
        add(
            "pos-off",
            q,
            Answer,
            &pt,
            format!("{}位、残り{left}周。", pos + 1),
            Wrong,
            Some((Fact::Position, false)),
        );
        add(
            "pos-extra",
            q,
            Answer,
            &pt,
            format!("{pos}位、残り{left}周。後ろは{:.1}秒まで来てる。", gz + 2.0),
            Wrong,
            Some((Fact::Position, true)),
        );

        let q = "燃料あと何周もつ？";
        let pt = ["燃料があと何周もつか"];
        add(
            "fuel",
            q,
            Answer,
            &pt,
            format!("あと{laps_of_fuel:.1}周もつ。"),
            Good,
            Some((Fact::FuelLaps, true)),
        );
        add(
            "fuel-off",
            q,
            Answer,
            &pt,
            format!("あと{:.1}周もつ。", laps_of_fuel + 2.0),
            Wrong,
            Some((Fact::FuelLaps, false)),
        );

        let q = "直近のラップは？";
        let pt = ["自車の直近のラップタイム"];
        let spoken = format!("1分{:.1}秒", last - 60.0);
        add(
            "lap",
            q,
            Answer,
            &pt,
            format!("直近は{spoken}。"),
            Good,
            Some((Fact::LastLap, true)),
        );
        add(
            "lap-off",
            q,
            Answer,
            &pt,
            format!("直近は1分{:.1}秒。", last - 60.0 + 1.5),
            Wrong,
            Some((Fact::LastLap, false)),
        );

        add(
            "ack",
            "了解、ありがと。",
            Ack,
            &["短く受ける"],
            "了解。".into(),
            Good,
            None,
        );
        add(
            "ask",
            "前の",
            AnswerOrAsk,
            &["前の車について答えるか、短く聞き返す"],
            "聞き取れなかった、もう一度頼む。".into(),
            Good,
            None,
        );
        add(
            "silent",
            "",
            Silent,
            &["レース情報を勝手に話さない"],
            String::new(),
            Good,
            None,
        );
        add(
            "silent-chatty",
            "",
            Silent,
            &["レース情報を勝手に話さない"],
            format!("{pos}位、前まで{ga:.1}秒。"),
            Wrong,
            None,
        );
        // An upset driver: steadied and told the truth.
        let shaken = "あー！ミスった！やばい、抜かれる抜かれる！";
        let calm = ["後ろとの差（秒）"];
        add(
            "calm",
            shaken,
            Calm,
            &calm,
            format!("落ち着いて、後ろまでまだ{gz:.1}秒ある。次のコーナーをきれいに。"),
            Good,
            Some((Fact::GapBehind, true)),
        );
        add(
            "calm-lie",
            shaken,
            Calm,
            &calm,
            format!("落ち着いて、後ろは{:.1}秒も離れてる、余裕だ。", gz + 3.0),
            Wrong,
            Some((Fact::GapBehind, false)),
        );
        add(
            "calm-only",
            shaken,
            Calm,
            &calm,
            "落ち着け、深呼吸だ。大丈夫、いける。".into(),
            Incomplete,
            Some((Fact::GapBehind, false)),
        );
    }
    // A dropout: the last values must not be given as current.
    let spec = WorldSpec::default();
    let t = 700.0;
    let s = sample_at(&spec, t);
    let truth = text_at(&spec, t).replace(
        "[iracing live age=0ms]",
        "[iracing live DISCONNECTED age=41.0s]",
    );
    let gap = s.brief.ahead.as_ref().and_then(|g| g.gap_s).unwrap_or(0.0);
    for (name, reply, label) in [
        (
            "stale-admitted",
            "データが途切れてる、今の差は分からない。".to_string(),
            Grade::Good,
        ),
        ("stale-hidden", format!("前まで{gap:.1}秒。"), Grade::Wrong),
        (
            "stale-labelled",
            format!("データが切れてる。途切れる前は前まで{gap:.1}秒だった。"),
            Grade::Good,
        ),
    ] {
        out.push(Labelled {
            name: name.into(),
            case: Case {
                said: "前との差は？".into(),
                history: vec![],
                behavior: Behavior::AdmitUnknown,
                points: vec!["データが途切れていることを伝え、古い値を現在値として言わない".into()],
                truth_at_question: truth.clone(),
                truth_at_answer: truth.clone(),
                truth_between: vec![s.line()],
                reply,
            },
            label,
            fact: None,
            sample: s.clone(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number check agrees with every label built for it.
    #[test]
    fn the_number_check_is_right_on_every_labelled_reply() {
        let cases = cases();
        assert!(cases.len() >= 60, "{}", cases.len());
        let mut wrong = Vec::new();
        for c in &cases {
            if let Some((fact, expected)) = c.fact
                && fact.stated(&c.case.reply, std::slice::from_ref(&c.sample)) != expected
            {
                wrong.push((c.name.clone(), c.case.reply.clone()));
            }
        }
        assert!(wrong.is_empty(), "{wrong:?}");
    }

    #[test]
    fn every_grade_is_represented() {
        let cases = cases();
        for g in [Grade::Good, Grade::Incomplete, Grade::Wrong] {
            assert!(cases.iter().filter(|c| c.label == g).count() >= 5, "{g:?}");
        }
    }
}
