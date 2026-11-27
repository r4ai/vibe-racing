//! Questions that need no model: what is asked lap after lap is answered by
//! `vr say`, at once and at no cost. Whatever asks for more than that, or is
//! not clearly one of them, is for the agent.

/// What a question that needs no model is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quick {
    /// A topic of `vr say`.
    Say(&'static str),
    /// Thanks, or a word of agreement.
    Ack,
    /// Say the last thing again.
    Again,
    /// The driver is about to ask, or to hear whether anyone is there.
    Here(&'static str),
    /// Too little was heard to make anything of it.
    Unclear,
}

/// The topics of `vr say`.
pub const TOPICS: [&str; 21] = [
    "status",
    "position",
    "ahead",
    "behind",
    "leader",
    "pace",
    "ahead-lap",
    "leader-lap",
    "lap",
    "sectors",
    "fuel",
    "left",
    "flags",
    "weather",
    "damage",
    "tyres",
    "forecast",
    "losing",
    "fuel-laps",
    "tyre-temps",
    "box",
];

/// Longer questions are about more than one thing.
const MAX_CHARS: usize = 22;
/// Less than this without any word that is known says nothing.
const UNCLEAR_CHARS: usize = 3;

/// What the transcriber makes of words that are always meant differently on
/// the radio. The whole text, or a part of it.
const WHOLE: &[(&str, &str)] = &[
    ("今ない", "今何位"),
    ("今ないか", "今何位か"),
    ("今なに", "今何位"),
    ("暇ない", "今何位"),
    ("いまない", "今何位"),
    ("今内", "今何位"),
    ("ダメにしない", "ダメージない"),
    ("ダメじゃない", "ダメージない"),
    ("ダメじゃないですか", "ダメージないですか"),
];
const PART: &[(&str, &str)] = &[
    ("何週", "何周"),
    ("毎週", "毎周"),
    ("前の週", "前の周"),
    ("話されて", "離されて"),
    ("話してる", "離してる"),
    ("音度", "温度"),
    ("お前のドライバー", "前のドライバー"),
    ("前のトライ", "前のドライバー"),
    ("1のラップ", "1位のラップ"),
    ("1との", "1位との"),
    ("お前との", "前との"),
    ("前回の前のやつ", "前の車"),
    ("追い付", "追いつ"),
];

/// Where the words begin or end with nothing.
const FILLERS: &[&str] = &[
    "えーっと",
    "えっと",
    "えーと",
    "あのー",
    "あの",
    "えー",
    "ねえ",
    "ちょっと",
    "すいません",
    "すみません",
    "じゃあ",
    "で、",
    "あ、",
];

/// Asks for more than a number: a judgement, a reason, something to be done.
const MORE: &[&str] = &[
    "なんで",
    "なぜ",
    "どうして",
    "どうすれ",
    "どうしたら",
    "どうやっ",
    "方法",
    "作戦",
    "戦略",
    "ピット",
    "ボックス",
    "カット",
    "持つ",
    "もつ",
    "替え",
    "交換",
    "適正",
    "動作",
    "勝て",
    "優勝",
    "なれ",
    "抜け",
    "守れ",
    "攻め",
    "名前",
    "誰",
    "番手の車",
    "その前",
    "前の前",
    "後ろの後ろ",
    "前回",
    "前のレース",
    "昨日",
    "予選の",
    "ベスト",
    "比べ",
    "調べ",
    "分析",
    "毎周",
    "毎回",
    "周ごと",
    "たら",
    "なったら",
    "知らせ",
    "報告",
    "広告",
    "言って",
    "言わな",
    "いらない",
    "うるさ",
    "黙",
    "静か",
    "音量",
    "声",
    "登録",
    "イシュー",
    "設定",
    "セットアップ",
    "バイアス",
    "トラコン",
    "二番手",
    "2番手",
    "2位",
    "二位",
    "3位",
    "三位",
    "#",
    "号車",
    // Someone's best lap is no gap, and no lap just driven.
    "ファステスト",
    "ファストラップ",
    "最速",
    // About an earlier answer, or asking for all of it.
    "その",
    "それ",
    "だから",
    "発言",
    "全部",
    "詳しく",
    "そいつ",
    "あいつ",
    "こいつ",
    "同じ",
    // What if: a sum to work out, not the report of now.
    "もし",
    "仮に",
    "計算",
    // How things changed over the race, or the whole field: no report of now.
    "スタート",
    "序盤",
    "何個",
    "上げた",
    "下げた",
    "何周目",
    "周前",
    "半分",
    "平均",
    "一番",
    "何台",
    "メートル",
    // Counted over the race, or a choice between two.
    "何回出",
    "何回入",
    "何回あ",
    "何回目",
    "回数",
    "どっち",
    "どちら",
    // Terse/abstract real-world radio calls that need live context & engineering judgement.
    "きつい",
    "厳しい",
    "やばい",
    "やば",
    "無理",
    "入る",
    "入ろ",
    "プッシュ",
    "セーブ",
    "抑え",
    "リフト",
    "リフコ",
    "滑る",
    "滑っ",
    "すべる",
    "巻く",
    "巻き",
    "タレ",
    "たれ",
    "重い",
    "跳ね",
    "ロック",
    "もっと速",
    "速く走",
    "伸びしろ",
    "次どう",
    "どうする",
    // Complaints, incidents, or observations about rivals that need judgement.
    "ミス",
    "ブロック",
    "幅寄せ",
    "抜かれ",
    "危な",
    "追いつかれ",
];

/// Short 2-3 character real radio calls that must go to the agent, never `Unclear`.
const TERSE: &[&str] = &[
    "どう",
    "今どう",
    "いける",
    "いけそ",
    "行ける",
    "行く",
    "押す",
    "押せ",
    "待つ",
    "攻め",
    "攻める",
    "守り",
    "守る",
    "熱い",
    "冷え",
    "濡れ",
    "乾い",
    "ガス",
    "どこ",
    "いつ",
    "誰",
    "なぜ",
    "何周",
    "何分",
];

/// Another car, or the cars: their tyres, fuel and damage are not ours.
const OTHERS: &[&str] = &[
    "前の車",
    "前のやつ",
    "前の人",
    "後ろの車",
    "後ろのやつ",
    "後ろの人",
    "トップの",
    "1位の",
    "一位の",
    "首位の",
    "相手",
    "他の車",
    "他車",
    "みんな",
];

const LEADER: &[&str] = &["トップ", "1位", "一位", "首位", "リーダー", "先頭"];
const GAP: &[&str] = &[
    "差",
    "ギャップ",
    "何秒",
    "離され",
    "追いつ",
    "遠い",
    "近い",
    "縮",
];
const LAP: &[&str] = &["ラップ", "何分", "タイム", "周回ペース"];

const ACK: &[&str] = &[
    "了解",
    "りょうかい",
    "オーケー",
    "ok",
    "おっけー",
    "ありがとう",
    "ありがと",
    "サンキュー",
    "はい",
    "うん",
    "おう",
    "わかった",
    "分かった",
    "コピー",
    "コピー、了解",
    "ナイス",
];

const AGAIN: &[&str] = &[
    "もう一回",
    "もう1回",
    "もういっかい",
    "もう一度",
    "もう1度",
    "聞こえなかった",
    "聞き取れなかった",
    "なんて言った",
    "何て言った",
    "なんて",
    "え",
    "リピート",
];

/// Exact short binary calls asking only whether to box now or stay out.
/// Any nuanced pit question (e.g. timing, undercut, traffic, fuel, tyres)
/// falls through to the agent with full `vr strategy` context.
const BOX_EXACT: &[&str] = &[
    "ピット",
    "ボックス",
    "ボックス、ボックス",
    "ボックスボックス",
    "ステイアウト",
    "入る",
    "今入る",
    "この周入る",
    "この周ピット",
    "この周ボックス",
    "入ろうか",
    "入っていい",
    "入るべき",
    "今入るべき",
    "入った方がいい",
    "入ったほうがいい",
    "今入った方がいい",
    "今入ったほうがいい",
    "この周入った方がいい",
    "この周入ったほうがいい",
    "ピット入る",
    "ピット入るべき",
    "今ピット入る",
    "この周ピット入る",
    "ボックスする",
    "今ボックス",
];

/// Complaints that something does not work: the driver wants a way out of it,
/// not the report of how things are.
const STUCK: &[&str] = &[
    "追いつかない",
    "追いつけない",
    "追いつかん",
    "追いつかね",
    "追いつけね",
    "抜けない",
    "抜けね",
    "抜けん",
    "届かない",
    "速すぎ",
];

/// Full-width letters and digits to ASCII, lower case, no spaces at the ends
/// and no punctuation.
fn plain(text: &str) -> String {
    let folded: String = text
        .chars()
        .map(|c| match c {
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            _ => c,
        })
        .collect::<String>()
        .to_lowercase();
    let mut s = folded.trim();
    while let Some(rest) = FILLERS.iter().find_map(|f| s.strip_prefix(f)) {
        s = rest.trim_start_matches(['、', ' ', '。', '…']);
    }
    let end = |c: char| {
        matches!(
            c,
            '。' | '?' | '!' | '、' | ' ' | '…' | '.' | ',' | 'ー' | '〜'
        )
    };
    s.trim_matches(end).to_string()
}

/// Calls that only want to know whether someone listens.
const HERE: &[(&str, &str)] = &[
    ("質問です", "どうぞ。"),
    ("質問", "どうぞ。"),
    ("質問があります", "どうぞ。"),
    ("質問いい", "どうぞ。"),
    ("聞いて", "どうぞ。"),
    ("聞こえてる", "聞こえてる。"),
    ("聞こえてる、これ", "聞こえてる。"),
    ("聞こえますか", "聞こえてる。"),
    ("聞こえる", "聞こえてる。"),
];

/// How the race ends: asked in words that otherwise ask for a judgement.
/// Tactical "can we win / podium?" calls go to the agent for full pace/target reasoning.
const END: &[&str] = &[
    "位になれ",
    "1になれ",
    "一になれ",
    "番手になれ",
    "トップになれ",
    "何位になり",
    "何位になる",
    "何位で終",
    "何位でゴール",
    "何位まで",
    "何位フィニッシュ",
    "何位でフィニッシュ",
    "このままだと何位",
    "このままいくと何位",
];
/// Whether the fuel lasts, in words that otherwise are about a stop.
const LASTS: &[&str] = &["持つ", "もつ", "持ちそう", "もちそう", "足り", "完走"];
/// Somebody ran into the car: is it all right?
const HIT: &[&str] = &[
    "当てられ",
    "かたられ",
    "ぶつけられ",
    "ぶつかった",
    "突っ込まれ",
    "追突され",
    "接触した",
];
/// Longer calls about it are a story.
const HIT_CHARS: usize = 14;
/// Whoever says one of these wants it looked into.
const LOOK: &[&str] = &[
    "調べ",
    "推測",
    "分析",
    "なんで",
    "なぜ",
    "ピット",
    "どうすれ",
    "どうしたら",
    "作戦",
    "戦略",
    "たら教え",
    "知らせ",
];

/// What was meant, where the transcriber always gets it wrong.
pub fn correct(text: &str) -> String {
    let mut s = plain(text);
    let bare = s.replace(['、', ' '], "");
    if let Some((_, meant)) = WHOLE.iter().find(|(heard, _)| *heard == bare) {
        s = (*meant).to_string();
    }
    for (heard, meant) in PART {
        // Not where it is right already.
        if !s.contains(meant) {
            s = s.replace(heard, meant);
        }
    }
    s
}

/// Whether `s` asks about more than one topic (「前との差は？後ろは？」): one
/// report answers only one of them.
fn several(s: &str) -> bool {
    let mut found: Vec<&str> = s
        .split(['？', '?', '。', '、', ' ', '!', '！'])
        .filter_map(|part| fuel_lasts(part).or_else(|| topic(part)))
        // 「今どう」 only asks for more of the same.
        .filter(|t| *t != "status")
        // 「前のラップは、前の車の」 is about one car.
        .map(|t| match t {
            "ahead-lap" => "ahead",
            "leader-lap" => "leader",
            "fuel-laps" => "fuel",
            "tyre-temps" => "tyres",
            "pace" => "lap",
            t => t,
        })
        .collect();
    found.dedup();
    found.len() > 1
}

/// Whether the fuel lasts, asked in words that otherwise are about a stop.
fn fuel_lasts(s: &str) -> Option<&'static str> {
    let any = |words: &[&str]| words.iter().any(|w| s.contains(w));
    let ours = !any(OTHERS);
    (ours && any(&["燃料", "ガソリン"]) && any(LASTS) || matches!(s, "完走できる" | "完走できそう"))
        .then_some("fuel")
}

/// A span of laps (「この10周で」「5周の間」): how it changed, not how it is.
fn over_laps(s: &str) -> bool {
    let span = ["周で", "周間", "周の間"].iter().any(|w| s.contains(w));
    let count = |c: char| c.is_ascii_digit() || "二三四五六七八九十".contains(c);
    span && !s.contains("何周で") && s.chars().any(count)
}

/// What `text` is, if no model is needed for it.
pub fn read(text: &str) -> Option<Quick> {
    let s = correct(text);
    let chars = s.chars().count();
    if chars == 0 {
        return None;
    }
    if several(&s) || over_laps(&s) {
        return None;
    }
    if ACK.contains(&s.as_str()) {
        return Some(Quick::Ack);
    }
    if let Some((_, text)) = HERE.iter().find(|(h, _)| *h == s) {
        return Some(Quick::Here(text));
    }
    let any = |words: &[&str]| words.iter().any(|w| s.contains(w));
    if any(STUCK) {
        return None;
    }
    // Asked at speed in a word or two: box or stay out, at once.
    if BOX_EXACT.contains(&s.as_str()) {
        return Some(Quick::Say("box"));
    }
    if chars <= MAX_CHARS && !any(LOOK) {
        if any(HIT) && chars <= HIT_CHARS {
            return Some(Quick::Say("damage"));
        }
        if any(END) {
            return Some(Quick::Say("forecast"));
        }
        if let Some(fuel) = fuel_lasts(&s) {
            return Some(Quick::Say(fuel));
        }
    }
    let again = |a: &&str| s == **a || (a.chars().count() > 3 && chars <= 24 && s.contains(*a));
    if AGAIN.iter().any(again) && !MORE.iter().any(|m| s.contains(m)) {
        return Some(Quick::Again);
    }
    if chars <= MAX_CHARS
        && !MORE.iter().any(|m| s.contains(m))
        && let Some(topic) = topic(&s)
    {
        return Some(Quick::Say(topic));
    }
    let more = MORE.iter().any(|m| s.contains(m)) || TERSE.contains(&s.as_str());
    (chars <= UNCLEAR_CHARS && !more).then_some(Quick::Unclear)
}

fn topic(s: &str) -> Option<&'static str> {
    let any = |words: &[&str]| words.iter().any(|w| s.contains(w));
    // A corner is a question of driving: the agent has its data.
    if crate::topic::CORNERS
        .iter()
        .any(|w| s.to_lowercase().contains(w))
    {
        return None;
    }
    // A decision to make: the lap time is no answer to it.
    if any(&["ペース", "ラップ"]) && any(&["ていい", "でいい", "べき", "方がいい"])
    {
        return None;
    }
    let fuel_word = any(&["燃料", "ガソリン", "燃費", "フューエル"]);
    let losing = !fuel_word
        && (any(&["どこ", "どのセクター", "どの区間", "どのコーナー"])
            && any(&[
                "離され",
                "失",
                "遅",
                "負け",
                "詰め",
                "稼",
                "ロス",
                "削れ",
                "縮め",
            ])
            || any(&["ブレーキ"]) && any(&["遅らせ", "奥", "もっと我慢"]));
    let sectors = any(&["セクター", "区間"]);
    let gap = any(GAP);
    let lap = any(LAP) && !any(&["タイム差", "差"]);
    let ahead = any(&[
        "前の",
        "前と",
        "前は",
        "前まで",
        "前を",
        "前って",
        "前方",
        "前、",
    ]) || s == "前";
    let behind = any(&["後ろ", "うしろ", "後方", "背後", "後続"]);
    let hurt = any(&["ダメージ", "壊れ", "損傷"])
        || chars_at_most(s, 14) && any(&["ダウンフォース", "空力", "直った", "修理終わ"]);
    // Why, or what: a question the times do not answer.
    let why = any(&[
        "じゃない",
        "なんで",
        "なぜ",
        "原因",
        "スリップ",
        "どこ",
        "って何",
    ]);
    // The lap of the car behind is not ours.
    if behind && lap && !any(&["ペース", "上げ", "速"]) {
        return None;
    }
    // Where, when, with whom or what it does: a canned report does not answer.
    // Note: "いつ" inside "追いつく" is not "when".
    let when_word = s.contains("いつ") && !s.contains("追いつ");
    let cause = why || when_word || any(&["誰", "影響", "せい", "原因", "どれだけ遅"]);
    if losing {
        return Some("losing");
    }
    if cause {
        return None;
    }
    // The tyres, fuel or damage of another car: the reports are of ours.
    let theirs = any(OTHERS);
    Some(if sectors {
        "sectors"
    } else if hurt {
        if theirs {
            return None;
        }
        "damage"
    } else if any(&["タイヤ"]) {
        if theirs || !any(&["温度", "摩耗", "減り", "減って", "状態", "どう", "タイヤは"])
        {
            return None;
        }
        if s.contains("温度") {
            "tyre-temps"
        } else {
            "tyres"
        }
    } else if fuel_word {
        if theirs {
            return None;
        }
        if any(&[
            "何周",
            "何リッタ",
            "あと",
            "残り",
            "どれくらい",
            "どのくらい",
        ]) {
            "fuel-laps"
        } else {
            "fuel"
        }
    } else if any(&["イエロー", "旗", "フラッグ"]) {
        "flags"
    } else if any(&["雨", "天気", "天候", "路面温度", "気温"]) {
        "weather"
    } else if s.contains("前の周") {
        "lap"
    } else if any(LEADER) && (lap && !gap || any(&["ラップ"])) {
        "leader-lap"
    } else if any(LEADER) && (gap || s.ends_with(['は', 'と']) || chars_at_most(s, 5)) {
        "leader"
    } else if any(&["追いつ"]) && !behind && !s.contains("追いつかれ") {
        "ahead"
    } else if any(&[
        "何位",
        "何番手",
        "順位",
        "ポジション",
        "何番目",
        "今何番",
        "番手なの",
    ]) && !ahead
        && !behind
    {
        "position"
    } else if behind && (gap || chars_at_most(s, 10) || any(&["ペース", "上げ", "速"])) {
        "behind"
    } else if any(&["残り", "あと何", "何周ある", "何周残", "いつまで"])
        && any(&["周", "分", "時間", "残り"])
        && !ahead
    {
        "left"
    } else if ahead && lap && !gap {
        "ahead-lap"
    } else if ahead && (gap || lap || chars_at_most(s, 10)) {
        "ahead"
    } else if any(&["この周", "今の周"]) && any(&["縮", "タイム"]) {
        "lap"
    } else if any(&[
        "ペース",
        "何分で走",
        "自分のラップ",
        "俺のラップ",
        "今のラップ",
        "ラップタイム",
    ]) && !s.contains("このペース")
    {
        "pace"
    } else if any(&["ギャップ", "タイム差"])
        || (any(&["離され", "縮まって", "縮めて"]) && chars_at_most(s, 12))
    {
        "ahead"
    } else if any(&[
        "状況",
        "セッション情報",
        "どうなってる",
        "繋がって",
        "つながって",
    ]) {
        "status"
    } else {
        return None;
    })
}

/// Orders of a watch longer than this are about more than a topic.
const ORDER_CHARS: usize = 60;
/// Takes thought, whatever else the order is about.
const THOUGHT: &[&str] = &[
    "なら",
    "判断",
    "比べ",
    "提案",
    "作戦",
    "ピット",
    "アンダーカット",
    "左右",
    "名前",
];
/// What an order may be about, and the topic that says it.
const ORDERS: &[(&str, &[&str])] = &[
    ("leader", &["トップ"]),
    ("fuel", &["燃料", "燃費"]),
    ("position", &["順位"]),
    ("ahead", &["前の車", "前との差"]),
    ("behind", &["後ろ"]),
    ("sectors", &["セクター", "区間"]),
    ("tyres", &["タイヤ"]),
    ("weather", &["天気", "雨"]),
];
/// The report of a lap has them all.
const LAP_REPORT: &[&str] = &["ラップ", "前周", "タイム", "攻め"];

/// The topic that says what a watch has the engineer tell, if it takes no
/// thought: a call after every lap adds up.
pub fn order(act: &str) -> Option<&'static str> {
    let s = plain(act);
    let any = |words: &[&str]| words.iter().any(|w| s.contains(w));
    if s.chars().count() > ORDER_CHARS || any(THOUGHT) {
        return None;
    }
    let found: Vec<&str> = ORDERS
        .iter()
        .filter(|(_, words)| any(words))
        .map(|(topic, _)| *topic)
        .collect();
    // The order of a lap: the gaps, the time and where to find more of it.
    let in_lap = |t: &&str| matches!(*t, "leader" | "ahead" | "behind" | "sectors");
    if any(LAP_REPORT) && found.iter().all(in_lap) {
        return Some("lap");
    }
    match found[..] {
        [topic] if !any(LAP_REPORT) => Some(topic),
        _ => None,
    }
}

fn chars_at_most(s: &str, n: usize) -> bool {
    s.chars().count() <= n
}

/// What the report of a topic tells, in English for the decision model:
/// whether it answers a call is asked before it is read out.
pub fn about(topic: &str) -> &'static str {
    match topic {
        "status" => "where the car is in the race now: position and the gaps around it",
        "position" => "the driver's current position",
        "ahead" => "the gap to the car directly ahead and whether it shrinks",
        "behind" => "the gap to the car directly behind and whether it shrinks",
        "leader" => "the gap to the race leader and whether it shrinks",
        "pace" => "the driver's own last lap time against their best",
        "ahead-lap" => "the last lap time of the car directly ahead",
        "leader-lap" => "the last lap time of the race leader",
        "lap" => "the driver's last lap time and the gap to the leader",
        "sectors" => "which sector of the last lap was slower than the driver's best",
        "fuel" => "whether the fuel lasts to the end",
        "left" => "how many laps or minutes of the race are left",
        "flags" => "which flags are out now",
        "weather" => "whether it rains and how wet the track is now",
        "damage" => "whether the car is damaged now and needs a repair",
        "tyres" => "how worn the tyres are",
        "forecast" => "where the driver will finish if everyone goes on as now",
        "losing" => "the corner where the driver loses most time and what to do there",
        "fuel-laps" => "how many laps the fuel lasts",
        "tyre-temps" => "the tyre temperatures",
        "box" => {
            "whether to come into the pits on this lap or stay out, and the lap of the planned stop"
        }
        _ => "something else",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calls of a race where the words answered something else.
    #[test]
    fn what_the_words_alone_answered_wrong() {
        for q in [
            "これどこでダメージ食らったんだ、俺。",
            "どこでダメージ食らったの。",
            "マシンダメージの影響は。",
            "これマシンダメージのせいでしょ。",
            "トップのファステストは何秒なの。",
            "そのファステストラップは1位のやつなの。",
            "じゃさっきのあの発言は何だったの。",
            "いつダメージ受けた？",
        ] {
            assert_eq!(say(q), None, "{q}");
        }
        // The plain questions still are answered at once.
        assert_eq!(say("ダメージは。"), Some("damage"));
        assert_eq!(say("これダメージ食らった。"), Some("damage"));
        assert_eq!(say("どこで失ってんの。"), Some("losing"));
        assert_eq!(say("トップ何秒。"), Some("leader"));
        assert!(TOPICS.iter().all(|t| about(t) != "something else"));
    }

    /// Calls no rule was written for: one report would answer only part of
    /// them, or something else.
    #[test]
    fn complex_calls_are_for_the_agent() {
        for q in [
            // Two questions at once.
            "前との差は？後ろは？",
            "燃料あと何周で、タイヤは？",
            "残り何周？燃料もつ？",
            // Another car's tyres, fuel or damage.
            "前の車のタイヤどう？",
            "トップのタイヤは？",
            "後ろの車、ダメージある？",
            "前のやつ燃料もつの？",
            // How it changed, or a sum to work out.
            "スタートから何個順位上げた？",
            "この10周でトップとの差どれくらい広がった？",
            "トップとの差、半分になるのいつ？",
            "後ろとの差、何周前から縮んでる？",
            "トップとの差、何メートルくらい？",
            "もし今入ったら何位？",
            "ここ5周の平均ってどのくらい？",
            "今一番速いの誰？",
            // Follow-ups to the call before.
            "後ろも同じ感じ？",
            "そいつのベストは？",
            // Counted, or a choice.
            "このレース、イエロー何回出た？",
            "前と後ろ、どっち優先？",
        ] {
            assert_eq!(read(q), None, "{q}");
        }
        // The same topic said twice, or with a word of no topic, is one question.
        assert_eq!(say("前との差、縮まってる？"), Some("ahead"));
        assert_eq!(say("前との差、今どう？"), Some("ahead"));
        assert_eq!(say("トップとの差、何秒？"), Some("leader"));
        assert_eq!(say("何周で追いつく？"), Some("ahead"));
        assert_eq!(say("タイヤどう？"), Some("tyres"));
        assert_eq!(say("燃料もつ？"), Some("fuel"));
        assert_eq!(say("前の周のタイムは？"), Some("lap"));
    }

    fn say(text: &str) -> Option<&'static str> {
        match read(text) {
            Some(Quick::Say(t)) => Some(t),
            _ => None,
        }
    }

    #[test]
    fn questions_asked_lap_after_lap() {
        for (topic, questions) in [
            (
                "position",
                &[
                    "今何位",
                    "今の順位。",
                    "今四番手なの。",
                    "今何番手ですか。",
                    "ポジションは？",
                    "今何位かって聞いてるの。",
                ][..],
            ),
            (
                "ahead",
                &[
                    "前との差は？",
                    "前とのタイム差。",
                    "前は",
                    "前の車は",
                    "前の車との差は。",
                    "前との。",
                    "前の車どう",
                    "前との差は何秒。",
                    "ギャップ",
                    "あの、前の車は？",
                    "あと何周で追いつく。",
                    "前のドライバー",
                ],
            ),
            (
                "behind",
                &["後ろは", "後ろとの差", "うしろ何秒", "後ろ迫ってる？"],
            ),
            (
                "leader",
                &[
                    "トップとの差は",
                    "1位とのタイム差を。",
                    "トップは",
                    "今、トップに離されてる",
                ],
            ),
            (
                "leader-lap",
                &[
                    "トップのラップタイムは。",
                    "1位のラップタイム。",
                    "トップは何分。",
                    "トップのラップタイム何秒。",
                ],
            ),
            (
                "ahead-lap",
                &[
                    "前は何分で走ってる。",
                    "前の車のラップタイムは",
                    "前のラップタイムは、前の車の。",
                ],
            ),
            (
                "pace",
                &["今何分で走ってる。", "ペースは", "自分のラップタイムは"],
            ),
            ("lap", &["この周で何秒縮めた。", "前の周は"]),
            (
                "sectors",
                &[
                    "今のセクターごとのタイムを教えて。",
                    "区間タイムは",
                    "セクターあたりのタイムとか見れないの。",
                ],
            ),
            (
                "fuel",
                &[
                    "燃料足りる",
                    "燃費は",
                    "燃料は",
                    "燃料もつ？",
                    "完走できる？",
                    "ガソリン最後まで持つ",
                ],
            ),
            (
                "fuel-laps",
                &["ガソリンあと何周", "燃料あとどれくらい", "燃料何リッター"],
            ),
            (
                "forecast",
                &[
                    "このペースで1位になれそうですか?",
                    "このペースで1になれそうですか?",
                    "このままだと何位",
                ],
            ),
            (
                "losing",
                &[
                    "どこで離されてんの。",
                    "どこで失ってる",
                    "どこ詰めればいい",
                    "どのセクターが遅い",
                ],
            ),
            (
                "left",
                &[
                    "あと何周。",
                    "あと何分。",
                    "残り時間は",
                    "あと何周ありますか。",
                    "残り何周",
                ],
            ),
            ("flags", &["イエロー出てる", "旗は"]),
            ("weather", &["雨降る？", "路面温度は"]),
            (
                "damage",
                &[
                    "マシンダメージない。",
                    "ダメージある？",
                    "ダメージないですか。",
                    "ダメージ受けたってことは。",
                    "Pさえもダメージないってこと。",
                    "ダウンフォースも大丈夫。",
                    "これ直ったの?",
                    "当てられた。",
                    "かたられたんだけど。",
                ],
            ),
            ("tyres", &["タイヤどう", "タイヤは"]),
            ("tyre-temps", &["タイヤの温度は。"]),
            (
                "status",
                &[
                    "アイレーシング、繋がってるこれ今。",
                    "セッション情報は。",
                    "状況は",
                ],
            ),
        ] {
            assert!(TOPICS.contains(&topic));
            for q in questions {
                assert_eq!(say(q), Some(topic), "{q}");
            }
        }
    }

    /// Heard wrong in a race of 22 November 2026.
    #[test]
    fn what_is_always_heard_wrong() {
        for q in ["今ない。", "今、ない。", "暇ない。"] {
            assert_eq!(say(q), Some("position"), "{q}");
        }
        for (q, topic) in [
            ("お前のドライバーは。", "ahead"),
            ("前のトライ。", "ahead"),
            ("1のラップタイムは。", "leader-lap"),
            ("前回の前のやつのタイムは。", "ahead-lap"),
            ("話されてるね。", "ahead"),
            ("えっと、これ話されてる。", "ahead"),
            ("今、何分話されてる。", "ahead"),
            ("あと何週。", "left"),
            ("どこで話されてんの。", "losing"),
            ("どのコーナーで詰められる？", "losing"),
            ("ブレーキどこまで遅らせられる？", "losing"),
            ("ブレーキもっと奥でいける？", "losing"),
            ("ちょっと疲れてきた、ペース落としていい？", ""),
            ("ペース抑えた方がいい？", ""),
            ("スプーンで何秒失ってる？", ""),
            ("セクター1はスリップじゃないのか。", ""),
            ("セクター4ってどこ、スプーンのとこ。", ""),
            ("後ろのラップタイムは。", ""),
            ("後ろのペース上げてきてないか。", "behind"),
            ("この周のセクターはどんな感じ、今。", "sectors"),
            ("最終コーナーのブレーキングポイントは", ""),
            ("ダメにしない。", "damage"),
            ("ダメじゃないですか。", "damage"),
        ] {
            assert_eq!(say(q).unwrap_or_default(), topic, "{q}");
        }
        assert_eq!(correct("それって動作音度内？"), "それって動作温度内");
        assert_eq!(correct("離されてる"), "離されてる");
        assert_eq!(correct("えっと、あの、前の週は。"), "前の周は");
    }

    #[test]
    fn whatever_asks_for_more_is_for_the_agent() {
        for q in [
            "前の車抜けそう",
            "前の車の名前は",
            "前のその前のタイムを教えて。",
            "二番手は何秒。",
            "このペースで1位になれるか調べて",
            "だから推測でいいから調べてちゃんと。",
            "燃料足りなかったら教えて",
            "このペースで。",
            "毎周トップとの差を報告して",
            "後ろが1秒切ったら教えて",
            "順位は言わなくていい",
            "タイヤ何周もつ",
            "それって適正温度に入ってる",
            "12号車との差は",
            "なんでイエロー",
            "温度を上げて。",
            "調べて。",
            "普通に走れるけど。",
            "これ入んないと失格。",
            "早いのとか遅いのとか、どこで失ってるのとか教えて。",
            // Terse/abstract calls go to the agent for contextual inference.
            "どう？",
            "今どう？",
            "今どんな感じ？",
            "いける？",
            "きつい",
            "プッシュ？セーブ？",
            "もっと速く走りたい、どこ詰めればいい？",
            // Misclassification guards: never hijack strategy/tactical/causal/terse calls.
            "ピットいつ入る？",
            "次の周入る？",
            "これ勝てる？",
            "優勝できそう？",
            "表彰台いける？",
            "追いつかれそう？",
            "どこで燃料稼ぐ？",
            "後ろ、どこで速い？",
            "前の車ミスった",
            "ブレーキどう？",
            "もう少し奥まで行ける？",
            "行く？",
            "無理",
        ] {
            assert_eq!(read(q), None, "{q}");
        }
        assert_eq!(say("入る？"), Some("box"));
    }

    /// Asked at speed: box or stay out, nothing else.
    #[test]
    fn whether_to_box_is_answered_at_once() {
        for q in [
            "ピット？",
            "ボックス？",
            "入る？",
            "今入る？",
            "この周入った方がいい？",
            "ステイアウト？",
            "ボックス、ボックス？",
        ] {
            assert_eq!(read(q), Some(Quick::Say("box")), "{q}");
        }
        for q in [
            // Something else about the stop.
            "ピットロスは何秒？",
            "ピットでタイヤ替える？",
            "前の車ピット入った？",
            "なんでピット？",
            "アンダーカットできる？",
            "ピット出たら何位？",
            "燃料、ピット入らずにもつ？",
            "もう一回ピット入る",
            // Nuanced pit questions must go to the agent, not canned `box`.
            "先に入った方がいい？",
            "今ボックスしたら詰まる？",
            "ボックスしたらどうなる？",
            "ピット開いてる？",
            // A complaint, not a question of the stop.
            "フロント入らない",
            // Longer: more than the call.
            "次のピットでフロントウイング直せる？",
        ] {
            assert_ne!(read(q), Some(Quick::Say("box")), "{q}");
        }
    }

    /// The driver is stuck: a way out is wanted, from the engineer.
    #[test]
    fn complaints_are_for_the_agent() {
        for q in [
            "追いつかない！",
            "追い付かない",
            "前に全然追いつけない",
            "抜けない！",
            "前の車速すぎ",
            "届かない",
        ] {
            assert_eq!(read(q), None, "{q}");
        }
        // The plain question still is the gap.
        assert_eq!(read("追いつける？"), Some(Quick::Say("ahead")));
    }

    #[test]
    fn orders_that_take_no_thought() {
        for (act, topic) in [
            // As kept in the settings of 22 November 2026.
            (
                "毎周トップとの差と前周のラップタイムを伝え、データに根拠があれば攻める場所と方法も助言する",
                "lap",
            ),
            ("トップとの差と前周のラップタイムを言う", "lap"),
            ("前周のタイムと攻めどころ", "lap"),
            ("トップとの差を言う", "leader"),
            ("燃料の残り周数と過不足を言う", "fuel"),
            ("順位を知らせる", "position"),
            ("後ろとの差を言う", "behind"),
            ("雨の様子", "weather"),
        ] {
            assert_eq!(order(act), Some(topic), "{act}");
        }
        for act in [
            "順位と燃料を言う",
            "燃料とラップタイム",
            "後ろが近いなら守り方を言う",
            "ピットに入るか判断する",
            "左右を確認してスポッターとして知らせる",
            "今の値を知らせる",
            "",
        ] {
            assert_eq!(order(act), None, "{act}");
        }
        let long = |n: usize| format!("順位{}", "を".repeat(n - 2));
        assert_eq!(order(&long(ORDER_CHARS)), Some("position"));
        assert_eq!(order(&long(ORDER_CHARS + 1)), None);
    }

    #[test]
    fn length_limits_are_exact() {
        let q = |n: usize| format!("{}何位", "今".repeat(n - 2));
        assert_eq!(say(&q(MAX_CHARS)), Some("position"));
        assert_eq!(say(&q(MAX_CHARS + 1)), None);
        assert_eq!(read("んん、"), Some(Quick::Unclear));
        assert_eq!(read("あああ"), Some(Quick::Unclear));
        assert_eq!(read("ああああ"), None);
        assert_eq!(read("  。"), None);
        assert_eq!(read(""), None);
    }

    #[test]
    fn words_of_agreement_and_repetition() {
        for q in ["了解。", "ＯＫ", "ありがとう！", "はい。"] {
            assert_eq!(read(q), Some(Quick::Ack), "{q}");
        }
        for q in [
            "もう一回",
            "え？",
            "ちょっとこっち聞こえなかったからもう一回。",
            "なんて？",
        ] {
            assert_eq!(read(q), Some(Quick::Again), "{q}");
        }
        assert_eq!(read("質問です。"), Some(Quick::Here("どうぞ。")));
        assert_eq!(
            read("聞こえてる、これ。"),
            Some(Quick::Here("聞こえてる。"))
        );
        // Not a repetition: an order.
        assert_eq!(read("もう一回ピット入る"), None);
        assert_eq!(read("前の車とのギャップ"), Some(Quick::Say("ahead")));
    }
}
