//! What a question is about, told from its words: which data to fetch beyond
//! what goes with every question.
//!
//! A command run by the agent costs two model round trips (about 3 s); a word
//! list costs nothing. A wrong guess only adds a little text, a missed one
//! leaves the agent to run the command itself.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    /// The balance of the car, its setup, settings changed while driving.
    Handling,
    /// Tyres, damage, temperatures.
    Car,
    /// Earlier sessions.
    Past,
    /// When to stop, and what the stops of the others mean.
    Strategy,
    /// How the laps are driven: corners, brake points, lines, time lost.
    Driving,
    /// The whole race: every car, the laps since the start, what if. Told by
    /// the words only, like `Driving`.
    Race,
}

impl Topic {
    pub const ALL: [Self; 6] = [
        Self::Handling,
        Self::Car,
        Self::Past,
        Self::Strategy,
        Self::Driving,
        Self::Race,
    ];

    fn commands(self) -> &'static [&'static str] {
        match self {
            // The setup itself only when it is asked about (`SETUP`): it is long.
            Self::Handling => &["vr handling", "vr sessions analyse"],
            Self::Car => &["vr car"],
            // The session being driven is the newest one.
            Self::Past => &[
                "vr sessions list --limit 5",
                "vr sessions show 1",
                "vr sessions show 2",
            ],
            Self::Strategy => &["vr strategy"],
            // The laps of the session being driven, corner by corner.
            Self::Driving => &["vr sessions corners", "vr sectors"],
            // Every car, every lap and what happened, and where it all goes.
            Self::Race => &[
                "vr standings",
                "vr laps --last 30",
                "vr events --limit 100",
                "vr forecast",
            ],
        }
    }

    /// As the transcriber writes them; Latin ones in lower case, matched as
    /// whole words.
    fn words(self) -> &'static [&'static str] {
        match self {
            Self::Handling => HANDLING,
            Self::Car => CAR,
            Self::Past => PAST,
            Self::Strategy => STRATEGY,
            Self::Driving => DRIVING,
            Self::Race => RACE,
        }
    }
}

const HANDLING: &[&str] = &[
    "セットアップ",
    "セッティング",
    "曲がら",
    "曲がん",
    "入らない",
    "入んない",
    "入りにく",
    // Not アンダーカット, a question of strategy.
    "アンダーステア",
    "アンダーが",
    "アンダー出",
    "アンダーで",
    "アンダーな",
    "アンダーだ",
    "アンダー気味",
    "アンダー強",
    "アンダーきつ",
    "アンダーひど",
    "オーバーステア",
    "オーバーが",
    "オーバー出",
    "オーバーで",
    "オーバーなん",
    "オーバー気味",
    "リアが",
    "リア出",
    "フロントが",
    "ロック",
    "ふらつ",
    "不安定",
    "跳ね",
    "滑る",
    "滑っ",
    "すべる",
    "すべっ",
    "流れる",
    "巻く",
    "巻き",
    "スピン",
    "挙動",
    "バランス",
    "バイアス",
    "ロックす",
    "ロックし",
    "空転",
    "ホイールスピン",
    "トラクション",
    "トラコン",
    "グリップ",
    "ウイング",
    "ウィング",
    "スタビ",
    "ダンパー",
    "車高",
    "キャンバー",
    "デフの",
    "デフ",
    "ブレーキング",
    "グレイニング",
    "ピックアップ",
    "バイブレーション",
    "振動",
    "トレイル",
    "突っ込",
    "オーバースピード",
    "ショートシフト",
    "ケツ出",
    "bb",
    "tc",
    "tcs",
    "abs",
    "arb",
];

/// The setup sheet is asked for.
const SETUP: &[&str] = &[
    "セットアップ",
    "セッティング",
    "ウイング",
    "ウィング",
    "車高",
    "キャンバー",
    "スタビ",
    "ダンパー",
    "スプリング",
    "バネ",
    "内圧",
    "空気圧",
    "トー",
    "arb",
];

const CAR: &[&str] = &[
    "タイヤ",
    "摩耗",
    "内圧",
    "空気圧",
    "表面温度",
    "ダメージ",
    "損傷",
    "壊れ",
    "当てら",
    "当たっ",
    "ぶつ",
    "接触",
    "追突",
    "クラッシュ",
    // As 当てられ is heard.
    "かたられ",
    "修理",
    "リペア",
    "水温",
    "油温",
    "オイル",
    "警告",
    "ピットサービス",
    "デルタ",
];

/// The corners drivers name, as the transcriber writes them.
pub const CORNERS: &[&str] = &[
    "最終コーナー",
    "ヘアピン",
    "シケイン",
    "スプーン",
    "s字",
    "エス字",
    "逆バンク",
    "ダンロップ",
    "デグナー",
    "200r",
    "130r",
    "ソース",
    "ルージュ",
    "オールージュ",
    "ラディオン",
    "コーム",
    "ブリュッセル",
    "プーオン",
    "プーホン",
    "ファニョー",
    "スタブロ",
    "ブランシモン",
    "バスストップ",
];

const DRIVING: &[&str] = &[
    "コーナー",
    "ヘアピン",
    "シケイン",
    "スプーン",
    "s字",
    "エス字",
    "逆バンク",
    "ダンロップ",
    "デグナー",
    "200r",
    "130r",
    "ソース",
    "ルージュ",
    "コーム",
    "プーオン",
    "スタブロ",
    "ブランシモン",
    "バスストップ",
    "ブレーキポイント",
    "ブレーキングポイント",
    "ブレーキ遅",
    "ブレーキを遅",
    "ブレーキ早",
    "進入",
    "立ち上がり",
    "脱出",
    "ライン",
    "クリップ",
    "失っ",
    "ロス",
    "詰め",
    "削れ",
    "タイム上げ",
    "タイムを上げ",
    "速く走",
    "攻め",
    "ミスっ",
    "ミスした",
    "ボトムスピード",
    "最低速",
    "何速",
    "ギア",
    "アクセル",
    "全開",
    "伸びしろ",
    "足りな",
    "追いつ",
    "追い付",
    "もっと速",
    "速くなる",
    "速くする",
    "今の周",
    "この周",
    "今周",
    // Against the others and the best lap: the sectors and the corners tell.
    "仕掛け",
    "抜け",
    "抜か",
    "抜き",
    "勝って",
    "負けて",
    "守れ",
    "守る",
    "固め",
    "弱点",
    "自己ベスト",
    "ベストと",
    "ベストから",
    "遅かった",
    "速いとこ",
    "遅いとこ",
    "どこで速",
    "どこで遅",
];

const PAST: &[&str] = &[
    "前回",
    "前のレース",
    "前のセッション",
    "この前",
    "こないだ",
    "過去の",
    "昨日",
    "先週",
    "今までのレース",
    "これまでのレース",
    "予選の",
    "練習のとき",
    "練習の時",
];

const STRATEGY: &[&str] = &[
    "作戦",
    "戦略",
    "ストラテジー",
    "ピット",
    "ボックス",
    "カット",
    "デグラ",
    "落ちて",
    "落ち方",
    "タレ",
    "垂れ",
    "スティント",
    "タイヤ交換",
    "タイヤ持",
    "タイヤもつ",
    "タイヤはもつ",
    "タイヤが持",
    "タイヤ何周",
    "何周もつ",
    "何周持つ",
    "引っ張",
    "引っぱ",
    "いつ入",
    "先に入",
    "次入る",
    "入るべき",
    "入った方",
    "入ったほう",
    "ストップ",
    "止まったら",
    // Tyres for the weather, and how it goes on.
    "タイヤ変え",
    "タイヤ替え",
    "タイヤを変え",
    "タイヤを替え",
    "履き替え",
    "スリック",
    "ドライタイヤ",
    "ウェットタイヤ",
    "レインタイヤ",
    "ドライで",
    "ドライに",
    "ウェットに",
    "レインに",
    "晴れ",
    "止む",
    "やむ",
    "乾い",
    "乾く",
    "ウィンドウ",
    "ウインドウ",
    "セーブ",
    "抑え",
    "リフト",
    "リフコ",
    "ステイ",
    "トラフィック",
    "渋滞",
    "詰まる",
    "インラップ",
    "アウトラップ",
    "稼ぐ",
    "次どう",
    "どうする",
    "入る",
    "入ろ",
];

/// Terse/abstract radio calls where the engineer must infer what matters right now
/// from strategy, car, handling, and driving data together.
const ABSTRACT: &[&str] = &[
    "どう",
    "今どう",
    "どんな感じ",
    "今どんな感じ",
    "きつい",
    "厳しい",
    "やばい",
    "いける",
    "いけそう",
    "これいける",
    "これ、いける",
    "どうすればいい",
    "次どうする",
    "プッシュ？セーブ",
    "プッシュ?セーブ",
];

/// Beyond the cars around and the last laps: what goes with every question
/// does not answer these.
const RACE: &[&str] = &[
    // The whole field.
    "一番速",
    "一番ペース",
    "一番遅",
    "最速",
    "ファステスト",
    "ファストラップ",
    "何台",
    "周回遅れ",
    "全員",
    "みんな",
    "他の車",
    "後ろの後ろ",
    "前の前",
    "その前",
    "その後ろ",
    "3位",
    "三位",
    "4位",
    "5位",
    "誰が",
    // Since the start.
    "スタート",
    "序盤",
    "何周目",
    "周前",
    "平均",
    "これまで",
    "今まで",
    "上げた",
    "下げた",
    "何個",
    "広が",
    "何回出",
    "何回入",
    "何回あ",
    "何回目",
    "回数",
    // What if, and where it goes.
    "もし",
    "仮に",
    "計算",
    "半分",
    "捕まえ",
    "ゴールまで",
    "最後まで",
    "チェッカー",
];

/// What the desk looks at, whatever it was asked: the race, the pace of
/// everyone, where time is lost, how the car behaves.
pub const STUDY: &[&str] = &[
    "vr brief",
    "vr standings",
    "vr laps --last 60",
    "vr sectors",
    "vr forecast",
    "vr sessions corners",
    "vr handling",
    "vr car",
    "vr strategy",
];

/// Asks for more than a look at the data.
const LONG: &[&str] = &[
    "調べて",
    "調べと",
    "調査",
    "検索",
    "ネットで",
    "ウェブ",
    "イシュー",
    "メモし",
    "メモっ",
    "メモに",
    "まとめて",
    "分析",
    "解析",
    "詳しく",
    "くわしく",
    "細かく",
    "検討",
    "見積も",
    "推測",
    "予想して",
    "計算して",
    "じっくり",
    "徹底",
];

/// What goes with every question answers these.
const PLAIN: &[&str] = &[
    "との差",
    "ギャップ",
    "何位",
    "順位",
    "ポジション",
    "燃料",
    "燃費",
    "ガソリン",
    "リッター",
    "何周",
    "ラップ",
    "タイム",
    "ペース",
    "イエロー",
    "フラッグ",
    "旗",
    "雨",
    "天気",
    "音量",
    "うるさ",
    "聞こえ",
    "黙っ",
    "了解",
    "ありがと",
    "オーケー",
    "前は",
    "後ろは",
    "トップ",
    "オーバーテイク",
];
/// A longer call may be about more than its plain word.
const PLAIN_CHARS: usize = 20;

/// What a question is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    pub topics: Vec<Topic>,
    /// The answer will take its time.
    pub long: bool,
}

/// What the words of the question tell; `None` when they do not.
pub fn read(text: &str) -> Option<Found> {
    let text = fold(text);
    let trimmed = text.trim_matches(['。', '？', '?', '！', '!', '、', ' ', '…']);
    let any = |words: &[&str]| words.iter().any(|w| contains(&text, w));
    let mut topics: Vec<Topic> = Topic::ALL.into_iter().filter(|t| any(t.words())).collect();
    if ABSTRACT.contains(&trimmed) {
        for t in [Topic::Strategy, Topic::Car, Topic::Handling, Topic::Driving] {
            if !topics.contains(&t) {
                topics.push(t);
            }
        }
    }
    let found = Found {
        topics,
        long: any(LONG),
    };
    let chars = text.chars().count();
    let plain = chars == 0 || (chars <= PLAIN_CHARS && any(PLAIN));
    (found != Found::default() || plain || !car_numbers(&text).is_empty()).then_some(found)
}

/// The commands that answer a question about what was `found`, and about the
/// cars named in `text`.
pub fn commands(found: &Found, text: &str) -> Vec<String> {
    let mut out: Vec<String> = found
        .topics
        .iter()
        .flat_map(|t| t.commands().iter().map(|c| c.to_string()))
        .collect();
    // An analysis is mostly one of laps; a few go with every question.
    if found.long && !found.topics.contains(&Topic::Past) {
        out.push("vr laps --last 60".into());
    }
    let folded = fold(text);
    if found.topics.contains(&Topic::Handling) && SETUP.iter().any(|w| contains(&folded, w)) {
        out.push("vr setup".into());
    }
    // Where the car behind or ahead is faster: our laps against its.
    if found.topics.contains(&Topic::Driving) {
        if ["後ろ", "後続", "うしろ"]
            .iter()
            .any(|w| folded.contains(w))
        {
            out.push("vr sectors --car behind".into());
        } else if ["前の", "前は", "前に", "前と", "前が", "前を"]
            .iter()
            .any(|w| folded.contains(w))
        {
            out.push("vr sectors --car ahead".into());
        }
    }
    for n in car_numbers(&folded) {
        out.push(format!("vr gap --car {n}"));
        out.push(format!("vr laps --car {n} --last 5"));
    }
    out
}

/// More cars than this were not meant to be compared.
const MAX_CARS: usize = 3;

/// Full-width letters, digits and signs to ASCII, lower case.
fn fold(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// Latin words must stand alone: `abs` is not in `absolutely`.
fn contains(text: &str, word: &str) -> bool {
    if !word.is_ascii() {
        return text.contains(word);
    }
    let free = |c: Option<char>| c.is_none_or(|c| !c.is_ascii_alphanumeric());
    text.match_indices(word).any(|(i, _)| {
        free(text[..i].chars().next_back()) && free(text[i + word.len()..].chars().next())
    })
}

/// `#12`, `12番`, `12号車`, `カーナンバー12`; not `12番手`, a position.
fn car_numbers(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if start == i {
            i += 1;
            continue;
        }
        let before: String = chars[..start].iter().collect();
        let after: String = chars[i..].iter().collect();
        let named = before.ends_with('#')
            || before.ends_with("カーナンバー")
            || before.ends_with("ゼッケン")
            || after.starts_with("号車")
            || (after.starts_with('番') && !after.starts_with("番手"));
        // Longer ones are not on a car.
        if named && i - start <= 3 {
            let n: String = chars[start..i].iter().collect();
            if !out.contains(&n) && out.len() < MAX_CARS {
                out.push(n);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topics(text: &str) -> Vec<Topic> {
        read(text)
            .unwrap_or_else(|| panic!("unclear: {text}"))
            .topics
    }

    #[test]
    fn the_whole_race_comes_with_its_questions() {
        for q in [
            "今このレースで一番ペースいいのって誰？",
            "後ろの後ろの車って何秒後ろ？",
            "スタートから何個順位上げた？",
            "俺の自己ベストって何周目に出したっけ？",
            "毎周0.5秒速く走れたら、ゴールまでに前の車捕まえられる？",
            "今のレース何台走ってる？周回遅れいる？",
            "このレース、イエロー何回出た？",
        ] {
            assert!(topics(q).contains(&Topic::Race), "{q}");
        }
        assert_eq!(
            commands(&read("トップとの差、半分になるのいつ").unwrap(), ""),
            [
                "vr standings",
                "vr laps --last 30",
                "vr events --limit 100",
                "vr forecast"
            ]
        );
        // What goes with every question answers these.
        for q in ["前との差は", "今何位", "あと何周"] {
            assert!(!topics(q).contains(&Topic::Race), "{q}");
        }
    }

    #[test]
    fn topics_follow_the_words() {
        for q in [
            "セットアップのリアウイング何度",
            "フロント入らないんだけど",
            "ブレーキバイアスどうしたらいい",
            "ＢＢいくつにすればいい",
            "BBは",
            "TCSを変えたほうがいい",
        ] {
            assert_eq!(topics(q), [Topic::Handling], "{q}");
        }
        assert_eq!(
            topics("立ち上がりでリア滑る"),
            [Topic::Handling, Topic::Driving]
        );
        for q in [
            "スプーンミスったかも",
            "130R全開で行ける？",
            "S字でどれだけ失ってる",
            "ヘアピン何速",
            "どこでタイム上げられる",
        ] {
            assert_eq!(topics(q), [Topic::Driving], "{q}");
        }
        for q in [
            "タイヤどう",
            "ダメージある",
            "水温は",
            "当てられた",
            "かたられたんだけど",
        ] {
            assert_eq!(topics(q), [Topic::Car], "{q}");
        }
        for q in [
            "作戦どうする",
            "ピットいつ入る",
            "アンダーカットできる",
            "次の周ボックス？",
            "デグラどれくらい",
            "もう少し引っ張る？",
        ] {
            assert_eq!(topics(q), [Topic::Strategy], "{q}");
        }
        assert_eq!(topics("タイヤ何周もつ"), [Topic::Car, Topic::Strategy]);
        assert_eq!(
            topics("最終コーナーのブレーキングポイントを教えて"),
            [Topic::Handling, Topic::Driving]
        );
        let f = read("スプーンで何秒失った").unwrap();
        assert_eq!(commands(&f, ""), ["vr sessions corners", "vr sectors"]);
        let q = "後ろの車はどこで速い、どこ攻められてる";
        assert_eq!(
            commands(&read(q).unwrap(), q),
            [
                "vr sessions corners",
                "vr sectors",
                "vr sectors --car behind"
            ]
        );
        for q in ["前のレースのベストラップは", "昨日の自分より速い"] {
            assert_eq!(topics(q), [Topic::Past], "{q}");
        }
        assert_eq!(
            topics("昨日のセットアップとタイヤ"),
            [Topic::Handling, Topic::Car, Topic::Past]
        );
    }

    #[test]
    fn the_usual_questions_need_nothing_more() {
        for q in [
            "前との差は",
            "後ろは",
            "今何位",
            "燃料足りる",
            "あと何周",
            "前のやつのラップは",
            "オーバーテイクあと何回",
            "音量上げて",
            "了解",
            "",
        ] {
            assert_eq!(read(q), Some(Found::default()), "{q}");
            assert!(commands(&Found::default(), q).is_empty(), "{q}");
        }
    }

    #[test]
    fn other_words_do_not_tell() {
        for q in ["ケツが出る", "後ろが落ち着かない"] {
            assert_eq!(read(q), None, "{q}");
        }
        // Passing is a question of where each car is faster.
        assert_eq!(topics("追い抜けそう"), [Topic::Driving]);
        assert_eq!(topics("前の車にどこで勝ってる"), [Topic::Driving]);
        // 20 characters are plain, 21 may be about more.
        let q = "今何位かあのーえーっと教えてくれないかな";
        assert_eq!(q.chars().count(), PLAIN_CHARS);
        assert_eq!(read(q), Some(Found::default()));
        assert_eq!(read(&format!("{q}あ")), None);
    }

    #[test]
    fn complaints_and_open_questions_bring_their_data() {
        for q in [
            "アンダーなんだけど",
            "リアが滑る",
            "ブレーキでロックする",
            "オーバー気味",
        ] {
            assert!(topics(q).contains(&Topic::Handling), "{q}");
        }
        for q in [
            "何が足りない？",
            "前に追いつくにはどうすればいい？",
            "今の周どう？",
            "もっと速く走るには？",
        ] {
            assert!(topics(q).contains(&Topic::Driving), "{q}");
        }
        // The car the question is about.
        let ahead = commands(&about(&[Topic::Driving], false), "前はどこで遅い？");
        assert_eq!(
            ahead.last().map(String::as_str),
            Some("vr sectors --car ahead")
        );
        let behind = commands(&about(&[Topic::Driving], false), "後ろに対してどこを守る？");
        assert_eq!(
            behind.last().map(String::as_str),
            Some("vr sectors --car behind")
        );
        let none = commands(&about(&[Topic::Driving], false), "どこで攻める？");
        assert!(!none.iter().any(|c| c.contains("--car")), "{none:?}");
    }

    #[test]
    fn weather_and_tyre_calls_bring_the_strategy() {
        for q in [
            "いつ晴れるの？",
            "ドライで走ってるやついる？",
            "タイヤ変えなくていい？",
            "雨いつ止む？",
            "路面乾いてきた？",
            "プッシュ？セーブ？",
            "入る？",
            "今ボックスしたら詰まる？",
        ] {
            assert!(topics(q).contains(&Topic::Strategy), "{q}");
        }
        for q in ["どう？", "今どう？", "きつい", "いける？", "次どうする？"] {
            let t = topics(q);
            assert!(
                t.contains(&Topic::Strategy)
                    && t.contains(&Topic::Car)
                    && t.contains(&Topic::Handling)
                    && t.contains(&Topic::Driving),
                "{q}: {t:?}"
            );
        }
    }

    #[test]
    fn long_look_ups_are_announced() {
        for q in ["コースレコードを調べて", "イシュー作っといて"] {
            let f = read(q).unwrap();
            assert!(f.long && f.topics.is_empty(), "{q}");
        }
        let f = read("過去のレースをまとめて").unwrap();
        assert_eq!(
            (f.topics.as_slice(), f.long),
            ([Topic::Past].as_slice(), true)
        );
    }

    fn about(topics: &[Topic], long: bool) -> Found {
        Found {
            topics: topics.to_vec(),
            long,
        }
    }

    #[test]
    fn commands_of_the_topics() {
        assert_eq!(commands(&about(&[], true), ""), ["vr laps --last 60"]);
        assert_eq!(
            commands(&about(&[Topic::Handling, Topic::Car], false), ""),
            ["vr handling", "vr sessions analyse", "vr car"]
        );
        // The setup sheet only when it is asked about.
        let q = "リアウイング何度にする";
        assert_eq!(
            commands(&read(q).unwrap(), q),
            ["vr handling", "vr sessions analyse", "vr setup"]
        );
        assert_eq!(
            commands(&about(&[Topic::Strategy], false), ""),
            ["vr strategy"]
        );
        assert_eq!(
            commands(&about(&[Topic::Past], true), ""),
            [
                "vr sessions list --limit 5",
                "vr sessions show 1",
                "vr sessions show 2"
            ]
        );
    }

    #[test]
    fn cars_by_number() {
        for q in ["12番の様子", "#12は", "１２号車どこ", "カーナンバー12"] {
            assert_eq!(read(q), Some(Found::default()), "{q}");
            assert_eq!(
                commands(&Found::default(), q),
                ["vr gap --car 12", "vr laps --car 12 --last 5"],
                "{q}"
            );
        }
        for q in ["今12番手", "12位", "12周目", "1234番", "番"] {
            assert!(commands(&Found::default(), q).is_empty(), "{q}");
        }
        assert_eq!(commands(&Found::default(), "12番と12番").len(), 2);
        assert_eq!(commands(&Found::default(), "1番と2番と3番と4番").len(), 6);
        assert_eq!(
            commands(&Found::default(), "1番と2番と3番")[4],
            "vr gap --car 3"
        );
        assert_eq!(commands(&Found::default(), "999番").len(), 2);
    }

    #[test]
    fn latin_words_stand_alone() {
        assert!(contains("abs は", "abs"));
        assert!(contains("今absを", "abs"));
        assert!(!contains("absolute", "abs"));
        assert!(!contains("crabs", "abs"));
    }
}
