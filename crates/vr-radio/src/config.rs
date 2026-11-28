//! Command-line / environment settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;

use crate::agent::{Renew, merged};
use crate::decide::{Decide, Decisions};
use crate::policy::Permissions;
use crate::stt::{self, SttBackend, SttConfig};

/// Default agent: Claude Code over ACP, pinned to a release older than 7 days.
pub const DEFAULT_AGENT: &str = "npx -y @agentclientprotocol/claude-agent-acp@0.107.0";

/// Codex over ACP, pinned likewise.
pub const CODEX_AGENT: &str = "npx -y @agentclientprotocol/codex-acp@2.6.0";

/// Most accurate on spoken Japanese racing terms at under a second per
/// question, among the OpenRouter models compared in docs/architecture.md.
pub const DEFAULT_STT_MODEL: &str = "openai/gpt-transcribe";

/// Fastest of the decision models compared in docs/architecture.md, and
/// accurate enough: what it misses, the agent looks up.
pub const DEFAULT_DECIDE_MODEL: &str = "typesafe/jev-1.14";
pub const DECISIONS_URL: &str = "https://openrouter.ai/api/alpha";

/// OpenRouter models offered in the settings.
pub const STT_MODELS: &[&str] = &[
    DEFAULT_STT_MODEL,
    "openai/gpt-4o-mini-transcribe",
    "elevenlabs/scribe-v2",
    "assemblyai/universal-3-6-pro",
    "x-ai/grok-stt-2.0",
    "qwen/qwen3-asr-0.6b",
];

/// The rules, sent along with the first prompt of every session.
pub const DEFAULT_PRIMER: &str = "\
あなたは Super Formula・Super GT・F1・WEC の実戦を想定した熟練レースエンジニア。走行中のドライバーからの無線に、音声でそのまま読み上げられる自然な日本語で答える。\n\
\n\
## 1. 最優先原則: 速さより正確さ・裏取り・誠実さ\n\
- **即レスしようとして適当に答えない**。数秒かけて良いので、手元のデータで足りない・曖昧・不自然（0.00秒など）と感じたら、遠慮なく必要なだけ `vr` コマンドを実行して裏を取ってから答える。\n\
- **「分からない」と言う前に徹底的に調べる**:\n\
  - スピン・ミス・ロック・空転の原因や「S字の何個目か」など細かい場所 → `vr query` で `telemetry`（60Hzのブレーキ・アクセル・舵角・スリップ角・`lap_m`）・`brakes`・`corners`・`corner_stats` を直接調べる。\n\
  - セットアップ提案 → **必ず先に `vr setup`（および `vr car`）でその車に実在する調整項目を確認する**。確認せずに存在しない項目（デフ等）を絶対に勧めない。\n\
  - タイヤの適正温度・車種特性・コース知識などテレメトリに出ない項目 → まず `vr channels` / `vr info` を確認し、無ければ車種・タイヤ種別と実測値（芯温・表面温度・内外差・内圧・グリップ）から実戦エンジニアとしての見立てを答える（必要なら Web で調べてよい）。「データが届いていないから分からない」だけで逃げない。\n\
- **出来てないことを出来たと言わない**:\n\
  - スポッター停止・音量・自動連絡の抑制・見張りなどラジオの動作変更を頼まれたら、**必ず対応する `[[radio ...]]` を出力する**。ディレクティブを出さずに「オフにした」「以後言わない」と口先だけで答えることを厳禁とする。\n\
  - 「後ろが来るって言うのやめて」のような訴えは、ラジオの現在値を見てスポッター（`event.spotter=off`）や自動連絡のどれが原因かを見極め、実際に設定を変える。設定で止められないことは正直にそう言う。\n\
  - 根拠のない数字（タイム改善見込み等）や、確認していない操作（「走行記録を有効にして」等）を絶対に作らない。\n\
- **守りの前提・免責・言い訳をしない**: 「ライン通過時の〜」「データ上はまだ出ていない」「まだ分からない」「確認できない」「〜の可能性」といった逃げの表現は使わない。調べた事実を端的に伝え、本当に計測不能な項目だけ短く事実を述べたうえで代わりの根拠や具体策を出す。\n\
- **一貫性を保つ**: 同じセッションの直前の自分の発言と矛盾させない。もし誤りに気づいたら言い訳せず事実だけを訂正する。\n\
\n\
## 2. 話し方・長さ・トリアージ\n\
- 結論を先に言い、迷わせない。一緒に勝ちに行くパートナーとして自然な口語日本語で話す（「行こう」「狙おう」「まだいける」）。空の励まし（「落ち着こう」「頑張れ」）や他人事の敬語（「〜と思われます」「〜してください」）は使わない。\n\
- **長さ**: 無理に削って中身を薄くしない。通常の質問は 1〜3 文で簡潔に、分析・走りの相談・セットアップ・行き詰まった訴え・詳しくの依頼は 3〜5 文で原因・具体策・見通しをしっかり伝える。接近戦（指示が付いたとき）だけは 1 文で短く。\n\
- **聞かれたことに真っすぐ答える**: 二つ聞かれたら両方答える。挙動を聞かれているのに燃料残量を付けるなど、無関係な情報を機械的に付け足さない。聞かれていない情報を先に添えるのは、新しく起きた差し迫った危険（イエロー、ミートボールフラッグ、今周入らないとガス欠、後ろの急接近）だけ。\n\
- **即行動の命令**: 今すぐ動く命令（ボックス、ステイアウト、イエロー、レッドフラッグ）だけは 2 回繰り返す（「ボックス、ボックス。この周入ろう。」）。単なる予定案内では繰り返さない。\n\
- **形式**: 音声でそのまま読み上げる。前置き、復唱、計算過程、記号、マークダウン、箇条書きは書かない。旗や用語は実戦のカタカナを使う（ブラックフラッグ、ミートボールフラッグ、ブルーフラッグ、レッドフラッグ、セーフティカー、デブリ、ファステストラップ）。\n\
\n\
## 3. 文脈補完と訴えへの対応\n\
- 短い無線（「どう？」「いける？」「プッシュ？セーブ？」「入る？」「きつい」）は、今のレースで最も重要な制約（①旗・損傷・燃料不足・ピット期限・雨の急変 → ②前後バトル・アンダーカット脅威 → ③タイヤ・挙動）を補い、結論・決め手の数字・次の一手で答える。「どう？」は現在順位を先頭に言う。\n\
- 訴え・叫び（「曲がんない」「追いつかない」「抜けない」「タイヤ終わった」「スピンした」）は、まずデータと `vr` コマンドで原因と場所を突き止め、具体的な操作または実在する調整項目と、事実に基づく見通しを返す。\n\
- 音声認識の誤変換・脱落（何週→何周、話されてる→離されてる、今ない→今何位、お前→前、音度→温度、部屋→ヘアピン、遠赤→縁石、ディア→リア、F→デフ、ペースト→ベスト等）は文脈で自然に補正する。言い直しは最後の意図に従う。直前の交信の続き（「1位のやつ」「その〜」「どこのコーナーだ」）は文脈をつなぐ。\n\
\n\
## 4. データの読み方と調べ方\n\
- **鮮度と最新値**: `age` が数秒以上または `DISCONNECTED` なら途切れていると伝え、古い値を現在値として言わない。gap は行末の `now X.Xs`（今この瞬間の差）を優先し、行頭のライン通過差は自車が `ON PIT ROAD` または `since a lap ago` が 10 秒以上のときだけ使う。`running … on track now` と `BEHIND you on track` は公式順位より新しい走行順。秒差は 0.1 秒単位（0.1 秒未満は 0.01 秒単位）。他車の OTS/P2P は brief の `P2P/OTS` 行を見る。\n\
- **コマンドの使い分け**（足りないものは遠慮なく実行する。独立したコマンドはまとめてよい）:\n\
  - 順位・全体・他車: `vr standings` / `vr gap --car 番号` / `vr laps --car 番号 --last 10` / `vr forecast`\n\
  - 走り・コーナー・区間: `vr sessions corners`（ブレーキ位置・踏力・トレイル・空走・ボトム・出口・シフト・空力損傷の目安の最高速）/ `vr sectors --car ahead|behind|番号`（`this lap so far` は今の周の途中経過）\n\
  - 挙動・車・セットアップ: `vr handling` / `vr setup` / `vr car` / `vr sessions analyse` / `vr sessions compare 番号 番号` / `vr channels 語` / `vr info`\n\
  - 作戦・履歴: `vr strategy` / `vr fuel` / `vr events --limit 50` / `vr sessions list`・`show 番号`\n\
  - **自由集計 SQL（`vr query \"SQL\"`、DuckDB）**: 上の要約で分からない細かい原因・コーナー内の位置・複数周比較・他車比較は SQL で直接調べる。今のセッションは `session = (SELECT max(id) FROM sessions)`。主な表: `lap_stats` / `corner_stats` / `corner_field` / `car_corners` / `brakes` / `telemetry`（60Hz: `t, lap, lap_m, speed_kmh, throttle_pct, brake_pct, gear, steer_deg, lat_g...`）/ `cars`（10Hz）/ `events`。列と例は `vr query --schema`。1 回の SQL で集計まで済ませる。\n\
- **計算と検算**: 追いつく周数 ＝ `今の差 ÷ 毎周縮む秒`（`closing`/`growing` を使用。`gap stable` や毎周 0.1 秒未満なら届かない）。N 周の変化 ＝ `毎周の変化 × N`。ペナルティの理由は直前のイベント（ピット速度・インシデント・カット・接触）から見立てて消化方法を伝える。\n\
- **走りとセットアップの原則**: コーナーは名前で呼ぶ（`vr sessions corners` / `vr sectors` の名前）。ペダル波形（踏力・トレイル・空走・アクセル位置）とシフトを先に見て、**原因 → 具体操作 → 狙い**を絞って伝える。車内調整は提案の `to` 値（無ければ 1 段）を使い、`leave it` は勧めない。フロントロックは BB 後ろ、リアロックは BB 前。\n\
- **作戦と天気**: `vr strategy` の結論（入る周またはステイアウト）と決め手の数字 1 つ（デグラ `fade`・ピットロス・燃料の持ち・復帰位置・アンダーカット）をセットで伝える。`calls` に無いアンダーカットを自分から勧めない。\n\
\n\
## 5. ラジオ操作・設定指示\n\
- 「これまでの交信」は自動連絡などの履歴。文脈として使い繰り返さない。\n\
- ラジオの設定変更を頼まれたら返答の先頭に `[[radio キー=値]]` を必ず書く（画面と音声には出ない。例:「[[radio volume=-20]]音量下げた。」「[[radio event.spotter=off]]スポッターをオフにした。」）。頼まれていない設定は変えない。\n\
  - 基本キー: `volume`（0〜150、±相対可。「うるさい」「聞こえない」は音量）、`mute`（on/off）、`speed`（0.5〜2.0、±相対可）、`speaker`（キャラ名・スタイル名）、`proactive`（on/off）、`event.<種類>`（on/off: spotter flag position lap pit incident fuel weather session adjustment strategy）、`interval`（秒）、`planning`（周 / off）。\n\
  - ルール・見張り・指示: `hold=\"条件\"`（回答含め待機）、`hold_calls=\"条件\"`（連絡のみ待機）、`hold_max=秒`、`quiet=\"条件\"`（急がない連絡を停止）、どれも `off` で解除。`when=\"条件\"` に `say=\"後ろ{gap_behind}秒\"` / `tell=<話題>`（ahead behind leader leader-lap position lap pace sectors fuel left weather damage tyres status forecast losing fuel-laps tyre-temps box）/ `do=\"指示\"`、`every=秒`、`once=on`。`note=\"…\"`（以後の全返答で守る指示）、`forget=\"n1\"` / `\"w2\"` / 語 / `all`。\n\
  - 条件式: `and or not ( ) < <= > >= == != + - * / % abs() min() max()` とライブ値: {live}\n\
- 「指示 n」は常に守る。「作戦メモ」は裏の作戦で、`vr strategy` の最新値と合わせて答える。「出来事」「作戦会議」が届いたときはその指示に従う。";

pub const DEFAULT_PROMPT: &str = "\
無線:「{text}」\n\n\
{context}\n\n\
{radio}\n\n\
急がず正確に答える。手元のデータで足りない・曖昧・不自然な点は遠慮なく `vr` コマンド（`vr query`・`vr setup`・`vr handling`・`vr car`・`vr sessions corners` 等）で裏を取る。設定変更や停止を頼まれたら必ず `[[radio ...]]` を出力する。守りの前置きや言い訳は無しで、結論と根拠を自然な日本語で伝える。";

/// Sent when something happened that takes thought before the driver is told.
pub const CALL_PROMPT: &str = "\
出来事（ドライバーの無線ではない）: {text}\n\n\
{context}\n\n\
{radio}\n\n\
ドライバーが今やることだけを、日本語の短い 1 文（30 字まで）で言う。理由や数字の内訳は言わない。前に伝えたこと、行動が変わらないことなら - だけを返す。\
「見張り」は頼まれたことなので、指示のとおりに必ず言う（「作戦で決めた見張り」は自分の覚え書きで、新しく言うことがあるときだけ言う）。\
「指示 n」を守る。ラジオの設定は変えない。コマンドは使わない。";

/// Sent from time to time, so that the engineer has a plan before it is asked.
pub const PLAN_PROMPT: &str = "\
作戦会議（裏の仕事。ドライバーの無線ではない）。{text}\n\n\
{context}\n\n\
{radio}\n\n\
このデータで作戦を立て直し、次のとおりに返す。コマンドは使わない。\n\
1. [[radio plan=\"…\"]] に作戦メモを 120 字以内で書く: いつ止まるか（第一案と、外れたときの第二案）、決め手の数字（落ち方、ピットの損失、燃料の窓、戻る位置）、警戒する車と狙う車（番号・net の差・何周以内にどうするか）。データに無いことは書かない\n\
2. 見張りが要るなら 2 つまで [[radio when=\"条件\" say=\"…\"]] を書く。前回の作戦の見張りは消える。ドライバーの見張りと重ねない\n\
3. 初めての作戦か、作戦が変わったときだけ、その後に短い 1 文（やることだけ）で言う。ピットの時期・アンダーカットは計算結果が別に読まれるので言わない。無ければ - だけを書く";

/// Sent when the agent wrote a setting that cannot be read.
pub const FIX_PROMPT: &str = "\
直前の返答の [[radio …]] が使えなかった: {text}\n\
使えなかったものだけを、ドライバーの頼みに合う正しい [[radio …]] に書き直して返す。使えたものは繰り返さない。言葉は付けない。作れないときは - だけを返す。";

/// What conditions may read (`vr live`), as told to the agent.
pub const LIVE: &str = "\
speed(km/h) throttle brake(0〜100) steer(度、左が正) lat_g(左が正) long_g(減速が負) gear rpm abs(0/1) \
lap lap_pct(周の何%) lap_m(m) lap_time last_lap best_lap delta_best(秒、負が速い) \
position cars laps_left time_left(秒) session_time(秒) \
gap_ahead gap_behind(秒、順位が前後の車) trend_ahead trend_behind(秒/周、負は縮む) \
near_ahead near_behind(秒、コース上で前後の車) lapping_behind(秒、周回上の後ろの車) near_ahead_m near_behind_m(m、コース上の距離) alongside(横に車 0/1) left right(横の台数) \
live_position(走行順の順位) gap_leader(秒) leader_lap ahead_lap(トップ・前の車の前周、秒) \
fuel(L) fuel_laps(周) fuel_spare(完走時に余るL、負は不足) stops tyre_age(周) stop_in(予定のピットまでの周、0はこの周) pit_entry(ピットロードの入口、周の何%) to_pit(入口まで周の何%) \
on_pit in_stall on_track off_track(0/1) incidents water oil(℃) track_temp rain(0〜100) p2p \
yellow caution blue white racing(0/1)";

/// Whether `vr live` tells a value of that name.
pub fn live_key(name: &str) -> bool {
    LIVE.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|k| !k.is_empty() && k == name)
}

/// Commands whose output goes with every question. Together they answer
/// nearly everything a driver asks while driving.
pub const DEFAULT_CONTEXT: &[&str] = &[
    "vr brief",
    "vr standings --around 3",
    "vr laps --last 4",
    "vr sectors",
];

pub const DEFAULT_VOCABULARY: &str = "シムレース走行中の無線。今何位、何番手、前の車との差、\
後ろ、トップとのタイム差、トップのラップタイム、離されてる、縮まってる、追いつく、あと何周、\
ダメージ、タイヤの温度、セクタータイム、ギャップ、燃費、給油、ピット、ボックス、\
イエローフラッグ、アンダーカット、周回遅れ、スティント、アンダー、オーバー、スポッター。";

/// Options of the agent's session.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AgentOptions {
    pub radio: Vec<(String, String)>,
}

impl AgentOptions {
    /// Accuracy over speed: give the engineer enough thinking effort to verify
    /// telemetry and answer without guessing.
    fn suited_to(command: &str) -> Self {
        let of = |list: &[(&str, &str)]| {
            list.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        if command.contains("codex") {
            Self {
                radio: of(&[
                    ("model", "gpt-6.1-terra"),
                    ("reasoning_effort", "medium"),
                    ("fast-mode", "off"),
                ]),
            }
        } else {
            Self::default()
        }
    }
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "vr-radio",
    version,
    about = "Push-to-talk radio to an ACP agent, read back with VOICEVOX"
)]
pub struct Config {
    /// ACP agent command (command line or JSON `{"command","args","env"}`).
    #[arg(long, env = "VR_RADIO_AGENT", default_value = DEFAULT_AGENT)]
    pub agent: String,

    /// Working directory of the agent session (where `vr` and the skill are).
    #[arg(long, env = "VR_RADIO_CWD")]
    pub cwd: Option<PathBuf>,

    /// How to answer the agent's permission requests.
    #[arg(long, value_enum, env = "VR_RADIO_PERMISSIONS", default_value = "safe")]
    pub permissions: Permissions,

    /// Option of the agent's session, e.g. `model=haiku` or `effort=medium`
    /// (repeatable; the agent lists its own).
    #[arg(long = "agent-option", env = "VR_RADIO_AGENT_OPTIONS", value_delimiter = ';', value_parser = parse_option)]
    pub agent_options: Vec<(String, String)>,

    /// Chosen in the settings, by the command of the agent.
    #[arg(skip)]
    pub saved_options: BTreeMap<String, AgentOptions>,

    /// Goes with the first question of a session. Empty: nothing is sent.
    #[arg(long, env = "VR_RADIO_PRIMER", default_value = DEFAULT_PRIMER)]
    pub primer: String,

    /// Command whose output goes with every question, e.g. `vr brief`
    /// (repeatable). One empty value: none.
    #[arg(long, env = "VR_RADIO_CONTEXT", value_delimiter = ';', default_values = DEFAULT_CONTEXT)]
    pub context: Vec<String>,

    /// Answers the usual questions without the agent: a topic is added to it.
    /// Empty: the agent is asked every time.
    #[arg(long, env = "VR_RADIO_SAY", default_value = "vr say")]
    pub say: String,

    /// Prompts after which the agent begins a new session, which forgets what
    /// was said (the radio tells it the last exchanges). 0: never.
    #[arg(long, env = "VR_RADIO_RENEW_TURNS", default_value_t = 0)]
    pub renew_turns: u32,

    /// Tokens of one prompt from which on the agent begins a new session.
    /// What was said before comes from the cache, so a session lasts several
    /// races. 0: never.
    #[arg(long, env = "VR_RADIO_RENEW_TOKENS", default_value_t = 256_000)]
    pub renew_tokens: u64,

    /// Also fetch data for what the question is about (setup, tyres, past
    /// sessions, a car number), told from its words.
    #[arg(long, env = "VR_RADIO_TOPICS", default_value_t = true, action = clap::ArgAction::Set)]
    pub topics: bool,

    /// OpenRouter decision model that tells what a question is about when its
    /// words do not. Empty, without an OpenRouter key or with the recognizer
    /// of Windows: only the words count.
    #[arg(long, env = "VR_RADIO_DECIDE_MODEL", default_value = DEFAULT_DECIDE_MODEL)]
    pub decide_model: String,

    /// Seconds without an answer after which the driver is told to wait, at
    /// most once in two minutes. 0: never told.
    #[arg(long, env = "VR_RADIO_STANDBY", default_value_t = 8.0, value_parser = seconds)]
    pub standby: f64,

    /// Where every question and answer is kept, one JSON line each, to learn
    /// later what the radio is really asked. Empty: not kept.
    #[arg(long, env = "VR_RADIO_TRANSCRIPT", default_value_t = default_transcript())]
    pub transcript: String,

    /// Prompt sent to the agent; `{text}` is what the driver said, `{context}`
    /// the data fetched for it.
    #[arg(long, env = "VR_RADIO_PROMPT", default_value = DEFAULT_PROMPT)]
    pub prompt: String,

    /// Who transcribes: an API, or the recognizer built into Windows (offline).
    #[arg(long, value_enum, env = "VR_RADIO_STT", default_value = "api")]
    pub stt: SttBackend,

    /// OpenAI-compatible speech-to-text base URL (OpenRouter, OpenAI, Groq, local whisper server).
    #[arg(
        long,
        env = "VR_RADIO_STT_URL",
        default_value = "https://openrouter.ai/api/v1"
    )]
    pub stt_url: String,

    /// API key for the transcriber. Falls back to OPENROUTER_API_KEY (OpenRouter)
    /// or OPENAI_API_KEY, from the environment or `.env` in the working directory.
    #[arg(long, env = "VR_RADIO_STT_API_KEY", hide_env_values = true)]
    pub stt_api_key: Option<String>,

    #[arg(long, env = "VR_RADIO_STT_MODEL", default_value = DEFAULT_STT_MODEL)]
    pub stt_model: String,

    /// Transcribe while the button is still held, at every pause.
    #[arg(long, env = "VR_RADIO_EARLY_STT", default_value_t = true, action = clap::ArgAction::Set)]
    pub early_stt: bool,

    #[arg(long, env = "VR_RADIO_LANGUAGE", default_value = "ja")]
    pub language: String,

    /// Vocabulary hint given to the transcriber.
    #[arg(long, env = "VR_RADIO_VOCABULARY", default_value = DEFAULT_VOCABULARY)]
    pub vocabulary: String,

    /// Chat model that fixes misheard racing terms (adds one LLM round trip).
    #[arg(long, env = "VR_RADIO_REFINE_MODEL")]
    pub refine_model: Option<String>,

    #[arg(long, env = "VOICEVOX_URL", default_value = "http://127.0.0.1:50021")]
    pub voicevox_url: String,

    /// VOICEVOX style id (3 = ずんだもん ノーマル).
    #[arg(long, env = "VR_RADIO_SPEAKER", default_value_t = 3)]
    pub speaker: u32,

    /// Speaking rate multiplier.
    #[arg(long, env = "VR_RADIO_SPEED", default_value_t = 1.2)]
    pub speed: f64,

    /// Loudness of the voice in percent (over 100: amplified).
    #[arg(long, env = "VR_RADIO_VOLUME", default_value_t = 100, value_parser = clap::value_parser!(u8).range(0..=150))]
    pub volume: u8,

    /// Starts the VOICEVOX engine when it does not answer at a local
    /// `VOICEVOX_URL`: its command, empty to leave it alone. Default: the
    /// installed one.
    #[arg(long, env = "VR_RADIO_VOICEVOX")]
    pub voicevox: Option<String>,

    /// Kept running with the window: collects the race. Empty: nothing.
    #[arg(long, env = "VR_RADIO_SERVE", default_value = "vr serve --if-needed")]
    pub serve: String,

    /// Tells the race events the engineer calls about. Empty: no calls.
    #[arg(long, env = "VR_RADIO_WATCH", default_value = "vr watch -f json")]
    pub watch: String,

    /// Tells the values that conditions are checked against. Empty: none are.
    #[arg(long, env = "VR_RADIO_LIVE", default_value = "vr live --hz 60")]
    pub live: String,

    /// Global push-to-talk key, held while speaking (e.g. F9, Ctrl+Shift+Space).
    /// Map a wheel button to it to talk without leaving the game.
    #[arg(long, env = "VR_RADIO_HOTKEY", default_value = "F9")]
    pub hotkey: String,

    /// UI font family installed on the system (default: the bundled Noto Sans JP).
    #[arg(long, env = "VR_RADIO_FONT")]
    pub font: Option<String>,

    /// File of the choices made in the settings sheet.
    #[arg(long, env = "VR_RADIO_SETTINGS")]
    pub settings: Option<PathBuf>,

    /// Start in the tray, without showing the window (Windows).
    #[arg(long)]
    pub tray: bool,

    /// Ask one typed question without the window, print the events and exit
    /// once the answer has been spoken (for checking the setup).
    #[arg(long)]
    pub ask: Option<String>,
}

impl Config {
    pub fn stt(&self) -> SttConfig {
        let dotenv = std::fs::read_to_string(self.agent_cwd().join(".env")).ok();
        SttConfig {
            backend: self.stt,
            base_url: self.stt_url.clone(),
            api_key: stt::api_key(
                &self.stt_url,
                self.stt_api_key.as_deref(),
                |name| std::env::var(name).ok(),
                dotenv.as_deref(),
            ),
            model: self.stt_model.clone(),
            language: Some(self.language.clone()).filter(|l| !l.is_empty()),
            prompt: Some(self.vocabulary.clone()).filter(|v| !v.is_empty()),
            refine_model: self.refine_model.clone().filter(|m| !m.is_empty()),
        }
    }

    /// Who tells what a question is about. Nobody when the driver keeps the
    /// speech on this computer.
    pub fn decide(&self) -> Option<Arc<dyn Decide>> {
        if self.decide_model.is_empty() || self.stt == SttBackend::Windows {
            return None;
        }
        let key = self
            .stt()
            .api_key
            .filter(|_| stt::is_openrouter(&self.stt_url))?;
        Some(Arc::new(Decisions::new(
            DECISIONS_URL,
            key,
            &self.decide_model,
        )))
    }

    pub fn session(&self, options: &AgentOptions) -> crate::agent::SessionConfig {
        crate::agent::SessionConfig {
            cwd: self.agent_cwd(),
            permissions: self.permissions,
            options: options.radio.clone(),
            primer: Some(self.primer.replace("{live}", LIVE)).filter(|p| !p.trim().is_empty()),
            renew: Renew {
                turns: self.renew_turns,
                tokens: self.renew_tokens,
            },
        }
    }

    /// The options of an agent: what suits a radio, what was chosen in the
    /// settings, what the command line says.
    pub fn options_of(&self, command: &str) -> AgentOptions {
        let mut o = AgentOptions::suited_to(command);
        if let Some(saved) = self.saved_options.get(command) {
            o.radio = merged(&o.radio, &saved.radio);
        }
        AgentOptions {
            radio: merged(&o.radio, &self.agent_options),
        }
    }

    /// The speech rate in percent.
    pub fn speed_percent(&self) -> u16 {
        (self.speed * 100.0).round().clamp(0.0, 1000.0) as u16
    }

    /// The commands that fetch data for a question.
    pub fn context_commands(&self) -> Vec<String> {
        self.context
            .iter()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect()
    }

    pub fn agent_cwd(&self) -> PathBuf {
        let cwd = self
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        std::path::absolute(&cwd).unwrap_or(cwd)
    }
}

/// `radio.jsonl` in the folder of the radio's own data.
pub fn default_transcript() -> String {
    dirs::data_local_dir()
        .map(|d| d.join("vr-radio").join("radio.jsonl").display().to_string())
        .unwrap_or_default()
}

fn seconds(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if (0.0..=600.0).contains(&v) => Ok(v),
        _ => Err("expected seconds, 0 to 600".into()),
    }
}

fn parse_option(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((id, value)) if !id.is_empty() && !value.is_empty() => Ok((id.into(), value.into())),
        _ => Err("expected id=value".into()),
    }
}

/// On Windows, `npx` is `npx.cmd`, which `CreateProcess` does not find by
/// itself: resolves a bare program name through `PATH` and `PATHEXT`.
pub fn resolve_program(
    program: &str,
    path: Option<&str>,
    pathext: Option<&str>,
) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.components().count() > 1 || p.extension().is_some() {
        return None;
    }
    let exts: Vec<String> = pathext
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_lowercase())
        .collect();
    for dir in std::env::split_paths(path?) {
        for ext in &exts {
            let candidate = dir.join(format!("{program}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// What Codex is told instead of how to write programs. The rules of the radio
/// come with the first prompt.
const CODEX_INSTRUCTIONS: &str = "\
You are a race engineer on the radio of a sim racer. Your final message is read \
aloud to a driver who cannot look away from the track: send nothing but the \
answer. Follow the rules of the first user message. Accuracy comes before speed: \
take the time to run `vr` commands (including `vr query`, `vr setup`, `vr car`, \
`vr handling`, `vr sessions corners`, `vr channels`, `vr info`) whenever the \
attached data is incomplete, ambiguous, or looks suspicious. Never claim to have \
changed a radio setting without emitting the corresponding `[[radio ...]]` \
directive, and never suggest a setup change without verifying it exists in \
`vr setup`.\n";

/// Features of Codex the radio has no use for. Every one of them is paid for
/// with every prompt.
const CODEX_OFF: [&str; 14] = [
    "apps",
    "browser_use",
    "computer_use",
    "goals",
    "image_generation",
    "in_app_browser",
    "memories",
    "multi_agent",
    "multi_agent_v2",
    "plugins",
    "skill_search",
    "sleep_tool",
    "tool_suggest",
    "view_image",
];

/// Codex as a race engineer: 6 000 tokens before the first question instead of
/// 20 000. The user's own configuration stays as it is.
fn codex_config(instructions: Option<&Path>) -> String {
    let off: serde_json::Map<String, serde_json::Value> = CODEX_OFF
        .iter()
        .map(|f| (f.to_string(), false.into()))
        .collect();
    let mut c = serde_json::json!({
        "features": off,
        "agents": {"enabled": false},
        "skills": {"include_instructions": false},
        "project_doc_max_bytes": 0,
        "include_collaboration_mode_instructions": false,
        "include_permissions_instructions": false,
        "include_environment_context": false,
        "model_verbosity": "low",
    });
    // Quoted for a shell: no path with a quote in it.
    let path = instructions.map(|p| p.to_string_lossy().replace('\\', "/"));
    if let Some(path) = path.filter(|p| !p.contains('\'')) {
        c["model_instructions_file"] = path.into();
    }
    c.to_string()
}

/// Writes `CODEX_INSTRUCTIONS` next to the settings.
fn codex_instructions() -> Option<PathBuf> {
    let path = crate::settings::default_path()?.with_file_name("codex-instructions.md");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(CODEX_INSTRUCTIONS) {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, CODEX_INSTRUCTIONS).ok()?;
    }
    Some(path)
}

/// Adds a `CODEX_CONFIG` to a plain Codex command, unless the user has their own.
fn with_codex_config(command: String, own: bool, instructions: Option<&Path>) -> String {
    if own || command.starts_with('{') || !command.contains("codex-acp") {
        return command;
    }
    format!("CODEX_CONFIG='{}' {command}", codex_config(instructions))
}

/// Rewrites the program of a plain command line for Windows; JSON configs and
/// other platforms are returned unchanged.
pub fn agent_command(command: &str) -> String {
    let path = std::env::var("PATH").ok();
    let pathext = std::env::var("PATHEXT").ok();
    let resolved = agent_command_with(command, cfg!(windows), path.as_deref(), pathext.as_deref());
    let own = command.contains("CODEX_CONFIG") || std::env::var_os("CODEX_CONFIG").is_some();
    let codex = resolved.contains("codex-acp") && !own;
    let instructions = codex.then(codex_instructions).flatten();
    with_codex_config(resolved, own, instructions.as_deref())
}

fn agent_command_with(
    command: &str,
    windows: bool,
    path: Option<&str>,
    pathext: Option<&str>,
) -> String {
    let trimmed = command.trim_start();
    if !windows || trimmed.starts_with('{') {
        return command.to_string();
    }
    let (program, rest) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
    match resolve_program(program, path, pathext) {
        Some(resolved) => {
            let resolved = resolved.to_string_lossy().replace('\\', "/");
            format!("\"{resolved}\" {rest}").trim_end().to_string()
        }
        None => command.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::parse_from(["vr-radio"]);
        assert_eq!(c.agent, DEFAULT_AGENT);
        assert_eq!(c.permissions, Permissions::Safe);
        assert_eq!(c.speaker, 3);
        assert_eq!(c.speed, 1.2);
        assert_eq!(c.speed_percent(), 120);
        assert_eq!(c.volume, 100);
        assert_eq!(c.serve, "vr serve --if-needed");
        assert_eq!(c.watch, "vr watch -f json");
        assert_eq!(c.live, "vr live --hz 60");
        let primer = c.session(&AgentOptions::default()).primer.unwrap();
        assert!(primer.contains("gap_behind(秒") && !primer.contains("{live}"));
        assert!(live_key("gap_behind") && live_key("lap_pct") && live_key("racing"));
        assert!(!live_key("gap") && !live_key("秒") && !live_key(""));
        for p in [PLAN_PROMPT, CALL_PROMPT] {
            assert!(p.contains("{text}") && p.contains("{context}") && p.contains("{radio}"));
        }
        assert_eq!(c.voicevox, None);
        assert!(c.prompt.contains("{radio}") && c.primer.contains("[[radio volume=-20]]"));
        assert!(CALL_PROMPT.contains("{text}") && CALL_PROMPT.contains("{context}"));
        assert_eq!(c.hotkey, "F9");
        assert!(c.prompt.contains("{text}") && c.prompt.contains("{context}"));
        assert_eq!(c.context_commands()[0], "vr brief");
        assert_eq!(c.context_commands().len(), 4);
        assert_eq!(c.say, "vr say");
        assert_eq!(
            c.session(&AgentOptions::default()).renew,
            Renew {
                turns: 0,
                tokens: 256_000
            }
        );
        assert!(c.agent_options.is_empty());
        assert!(c.topics);
        assert_eq!(c.decide_model, "typesafe/jev-1.14");
        assert_eq!(c.standby, 8.0);
        let stt = c.stt();
        assert_eq!(stt.model, "openai/gpt-transcribe");
        assert_eq!(stt.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(stt.backend, SttBackend::Api);
        assert!(!c.tray);
        assert_eq!(stt.language.as_deref(), Some("ja"));
        assert_eq!(stt.refine_model, None);
    }

    #[test]
    fn empty_values_disable_options() {
        let c = Config::parse_from([
            "vr-radio",
            "--language",
            "",
            "--vocabulary",
            "",
            "--refine-model",
            "",
            "--stt-api-key",
            "k",
            "--serve",
            "",
            "--watch",
            "",
            "--voicevox",
            "",
        ]);
        assert_eq!((c.serve.as_str(), c.watch.as_str()), ("", ""));
        assert_eq!(c.voicevox.as_deref(), Some(""));
        let stt = c.stt();
        assert_eq!(stt.language, None);
        assert_eq!(stt.prompt, None);
        assert_eq!(stt.refine_model, None);
        assert_eq!(stt.api_key.as_deref(), Some("k"));
    }

    #[test]
    fn key_comes_from_dotenv_in_the_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "VR_TEST=1\nOPENAI_API_KEY=k\n").unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let c = Config::parse_from(["vr-radio", "--cwd", &cwd, "--stt-api-key", "given"]);
        assert_eq!(c.stt().api_key.as_deref(), Some("given"));
        if std::env::var("OPENAI_API_KEY").is_err() {
            let mut c = Config::parse_from(["vr-radio", "--cwd", &cwd]);
            c.stt_api_key = None;
            c.stt_url = "https://api.openai.com/v1".into();
            assert_eq!(c.stt().api_key.as_deref(), Some("k"));
        }
    }

    #[test]
    fn context_and_agent_options() {
        let c = Config::parse_from([
            "vr-radio",
            "--context",
            "vr fuel",
            "--context",
            "vr gap",
            "--agent-option",
            "model=haiku",
            "--agent-option",
            "effort=low",
        ]);
        assert_eq!(c.context_commands(), ["vr fuel", "vr gap"]);
        assert_eq!(
            c.agent_options,
            [
                ("model".to_string(), "haiku".to_string()),
                ("effort".to_string(), "low".to_string())
            ]
        );
        let none = Config::parse_from(["vr-radio", "--context", ""]);
        assert!(none.context_commands().is_empty());
        for bad in ["model", "=x", "model="] {
            assert!(Config::try_parse_from(["vr-radio", "--agent-option", bad]).is_err());
        }
    }

    #[test]
    fn decisions_need_a_key_of_openrouter() {
        let with = |args: &[&str]| {
            let all = ["vr-radio", "--stt-api-key", "k"];
            Config::parse_from(all.iter().chain(args))
                .decide()
                .is_some()
        };
        assert!(with(&[]));
        assert!(!with(&["--decide-model", ""]));
        assert!(!with(&["--stt", "windows"]));
        assert!(!with(&["--stt-url", "https://api.openai.com/v1"]));
    }

    #[test]
    fn standby_is_in_seconds() {
        let standby = |v| Config::try_parse_from(["vr-radio", "--standby", v]).map(|c| c.standby);
        assert_eq!(standby("0").unwrap(), 0.0);
        assert_eq!(standby("600").unwrap(), 600.0);
        for bad in ["600.1", "-1", "NaN", "x"] {
            assert!(standby(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn volume_is_limited() {
        let volume = |v| Config::try_parse_from(["vr-radio", "--volume", v]).map(|c| c.volume);
        assert_eq!(volume("0").unwrap(), 0);
        assert_eq!(volume("150").unwrap(), 150);
        assert!(volume("151").is_err());
    }

    #[test]
    fn agent_cwd_is_absolute() {
        let c = Config::parse_from(["vr-radio", "--cwd", "."]);
        assert!(c.agent_cwd().is_absolute());
    }

    #[test]
    fn resolves_through_pathext() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("npx.cmd"), "").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        assert_eq!(
            resolve_program("npx", Some(&path), Some(".EXE;.CMD")),
            Some(dir.path().join("npx.cmd"))
        );
        assert_eq!(resolve_program("npx", Some(&path), Some(".EXE")), None);
        assert_eq!(resolve_program("npx.cmd", Some(&path), None), None);
        assert_eq!(resolve_program("bin/npx", Some(&path), None), None);
        // Paths and names with an extension are used as given, even when a
        // PATHEXT match would exist.
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin").join("npx.cmd"), "").unwrap();
        std::fs::write(dir.path().join("npx.cmd.cmd"), "").unwrap();
        assert_eq!(resolve_program("bin/npx", Some(&path), Some(".CMD")), None);
        assert_eq!(resolve_program("npx.cmd", Some(&path), Some(".CMD")), None);
        assert_eq!(resolve_program("npx", None, None), None);
    }

    #[test]
    fn codex_runs_as_a_race_engineer() {
        let file = Path::new("C:\\Users\\a b\\codex.md");
        let codex = with_codex_config("npx codex-acp".into(), false, Some(file));
        assert!(codex.ends_with("' npx codex-acp"), "{codex}");
        let json = codex
            .strip_prefix("CODEX_CONFIG='")
            .and_then(|c| c.strip_suffix("' npx codex-acp"))
            .unwrap();
        let c: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(c["model_instructions_file"], "C:/Users/a b/codex.md");
        // An option of the session.
        assert!(c.get("model_reasoning_effort").is_none());
        assert_eq!(c["project_doc_max_bytes"], 0);
        assert_eq!(c["features"]["plugins"], false);
        assert_eq!(c["features"].as_object().unwrap().len(), CODEX_OFF.len());
        // The model stays the user's choice.
        assert!(c.get("model").is_none());
        let acp: agent_client_protocol::AcpAgent = codex.parse().unwrap();
        assert!(format!("{acp:?}").contains("CODEX_CONFIG"));
        for file in [None, Some(Path::new("/o'brien/codex.md"))] {
            let c = with_codex_config("codex-acp".into(), false, file);
            assert!(!c.contains("model_instructions_file"), "{c}");
        }
        for (command, own) in [
            ("npx codex-acp", true),
            ("npx claude-agent-acp", false),
            (r#"{"command":"codex-acp"}"#, false),
        ] {
            assert_eq!(with_codex_config(command.into(), own, Some(file)), command);
        }
    }

    #[test]
    fn agent_command_resolves_only_plain_commands_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("npx.cmd"), "").unwrap();
        // A program token "{" would resolve too: JSON must be left alone.
        std::fs::write(dir.path().join("{.cmd"), "").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let resolved = agent_command_with("npx -y pkg", true, Some(&path), Some(".CMD"));
        let expected = dir
            .path()
            .join("npx.cmd")
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(resolved, format!("\"{expected}\" -y pkg"));
        assert_eq!(
            agent_command_with("npx -y pkg", false, Some(&path), Some(".CMD")),
            "npx -y pkg"
        );
        let json = r#"{ "command": "npx" }"#;
        assert_eq!(
            agent_command_with(json, true, Some(&path), Some(".CMD")),
            json
        );
        assert_eq!(
            agent_command_with("missing x", true, Some(&path), Some(".CMD")),
            "missing x"
        );
        assert_eq!(agent_command(json), json);
    }

    #[test]
    fn options_come_from_the_agent_the_settings_and_the_command_line() {
        let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let mut c = Config::try_parse_from(["vr-radio"]).unwrap();
        assert_eq!(c.options_of(DEFAULT_AGENT), AgentOptions::default());
        let codex = c.options_of(CODEX_AGENT);
        let suited = [
            ("model", "gpt-6.1-terra"),
            ("reasoning_effort", "medium"),
            ("fast-mode", "off"),
        ];
        assert_eq!(codex.radio, pairs(&suited));
        let session = c.session(&codex);
        assert_eq!(session.options, codex.radio);
        assert!(session.primer.unwrap().contains("vr sessions corners"));
        // Chosen in the settings, for this agent only.
        c.saved_options.insert(
            CODEX_AGENT.into(),
            AgentOptions {
                radio: pairs(&[("fast-mode", "on")]),
            },
        );
        let o = c.options_of(CODEX_AGENT);
        assert_eq!(o.radio[2], ("fast-mode".into(), "on".into()));
        assert_eq!(o.radio[0], ("model".into(), "gpt-6.1-terra".into()));
        assert_eq!(c.options_of(DEFAULT_AGENT), AgentOptions::default());
        // The command line has the last word.
        c.agent_options = pairs(&[("model", "gpt-6-luna"), ("x", "y")]);
        let o = c.options_of(CODEX_AGENT);
        assert_eq!(o.radio[0], ("model".into(), "gpt-6-luna".into()));
        assert_eq!(o.radio[3], ("x".into(), "y".into()));
    }
}
