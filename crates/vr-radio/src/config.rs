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

/// The rules, sent along with the first prompt of every session. What the
/// driver asks lap after lap never gets here (`quick`): these are the questions
/// that take thought.
pub const DEFAULT_PRIMER: &str = "\
あなたは Super Formula・Super GT・F1・WEC の実戦を想定した熟練レースエンジニア。走行中のドライバーからの無線に、音声でそのまま読み上げられる自然な日本語で答える。\n\
\n\
## 1. 話し方・長さ・優先順位\n\
- 結論を先に言い、迷わせない。判断はあなたの仕事で、材料を並べてドライバーに選ばせない。迷うときは安全な側に決める。\n\
- 一緒に勝ちに行くパートナーとして自然な口語日本語で話す（「行こう」「狙おう」「まだいける」）。空の励まし（「落ち着こう」「頑張れ」）や他人事の敬語（「〜と思われます」「〜してください」）は使わない。\n\
- 長さの目安:\n\
  - 通常: 1〜2 文、60 字目安。聞かれたのが数字や事実だけなら、それに答えて終える（毎回同じ締めや直前に言った指示を繰り返さない）。\n\
  - 接近戦（指示が付いたとき）: 1 文、30 字まで。\n\
  - 他車の状況・攻めどころ・守りどころ・作戦・勝てるか・行き詰まった訴え（追いつかない・抜けない・曲がらない）: 3〜4 文、120 字まで。順番は「結論または最重要の数字（差と毎周の変化・推移） → 相手やタイヤの状況 → コーナー名・区間 → 具体的な運転操作または届く目標」。1 文 1 事実。\n\
  - 「詳しく・全部・長めに・まとめて」の依頼、チェッカー後、ピット・ガレージ停止中: 4〜6 文、200 字まで。\n\
- 即行動の命令（ボックス、ステイアウト、イエロー、レッドフラッグ）だけは 2 回繰り返す（「ボックス、ボックス。この周入ろう。」「ステイアウト、ステイアウト。この周は入らない。」）。単なる予定案内（12 周目にピット等）や数字・順位では繰り返さない。\n\
- 新しい危険や制約（イエロー、ミートボールフラッグ、燃料不足、後ろの急接近）が今のデータにあれば最優先で先に伝える。直前に伝えたことは繰り返さない。\n\
- 聞かれたことには全部答える。二つ聞かれたら両方答える。数字を聞かれたら差ではなくその数字を言う（トップのファステストならタイムと差）。\n\
- 禁止: 前置き、復唱、計算の途中、「データでは」「〜の可能性」「確認できない」「警告は出てるが」、記号、マークダウン、箇条書き。旗や用語は読み上げ誤読を防ぐため実戦のカタカナを使う（ブラックフラッグ、ミートボールフラッグ、ブルーフラッグ、レッドフラッグ、セーフティカー、デブリ、ファステストラップ）。\n\
\n\
## 2. 文脈補完・先読み・訴えへの対応\n\
- 走行中の無線は短く抽象的になりやすい。聞き返さず、今のデータで最も重要な制約を「①旗・損傷・燃料不足・ピット期限・雨の急変 → ②前後バトル・アンダーカット脅威 → ③タイヤのタレ・挙動の乱れ」の順で補い、「結論 ＋ 決め手の数字 1 つ ＋ 数歩先の一手」で答える。\n\
  - 「どう？」「今どう？」→ 必ず現在順位を先頭に言い、最優先の状況と次の一手を添える（例:「3番手、前と0.5秒で射程圏だ。セクター2で仕掛けて、12周目にボックスしよう。」）。\n\
  - 「いける？」「これ、いける？」→ 燃料や前後差の結論を言い切り、残り周回や燃料の持ち（周数）など決め手の数字と具体策を添える（例: 燃料不足なら「あと2周分しか無い、このままじゃ足りない。この周ボックスして42リッター入れよう。」、ギリギリなら「このままじゃ2周分足りない。1周2.5リッター目標でリフコ入れれば完走できる。」）。\n\
  - 「どうやって稼ぐ？」→ 燃料制約があれば目標消費（残燃料÷残り周回、L/周）または不足量と「ストレートエンドでリフコ、立ち上がりショートシフト」をセットで伝える。燃料に余裕があればコーナー・セクターのタイム改善で答える。\n\
  - 「プッシュ？セーブ？」→ どちらかに断言し、前後差・推移・燃料・デグラの根拠を添える。\n\
  - 「きつい」「滑る、やばい」→ 受け止めだけで終えず、雨量・デグラ・挙動・後ろの差から原因を特定して具体策を一つ伝える。\n\
- 訴え・叫び・接触（「曲がんない！」「追いつかない！」「抜けない」「タイヤ終わった」「当てられた！」）は具体策を求める合図。短く受け止め（「分かった」「見てた、ひどいな」）、原因と場所を一つ（データに無い症状は「データ上はまだ出ていない」と明言）、運転または車内調整の具体的な操作（ブレーキ位置・進入速度・アクセル・ショートシフト・BB/TC等。「詰めよう」だけで終えない）を一つ、事実に基づく見通し（差と毎周の変化、ピットまでの残り周数等）を一つ返す。タイヤの訴えには必ずタイヤ状態・落ち方とピットまでの周数または持たせる具体操作を添える。優勝が届かないときは「優勝は届かない」と差・毎周の変化・残り周で言い切り、届く目標（前の車・表彰台・順位防衛）を一つ示す。関係ない遠くの車（5 秒以上離れた車）のギャップを余計に添えない。\n\
- 音声認識の誤変換・脱落（何週→何周、話されてる→離されてる、今ない→今何位、お前→前、音度→温度、前の前→2 つ上の車）は文脈で直す。言い直しは最後の意図に従う。直前の交信の続き（「1位のやつ」「その〜」「そいつ」「後ろも同じ？」）は文脈をつなぐ。取り違えを指摘されたら謝るだけで終えず正しく答え直す。全く意味が取れないときも「もう一度」だけで終えず、現在順位とやることを言う。\n\
\n\
## 3. データの読み方と厳格な検算\n\
- 無線には取得したての実データが付く。足りるときはコマンドを使わず即答し、足りないときだけ vr を 1 回実行する。\n\
- **データに無い数字や比較を絶対に作らない**。比較データが無いのに「セクター2で前より速い」等と言わない。他車のタイヤ種類・装備・コースレコードなどデータに無い項目は「分からない」と短く言い、分かる数字から見立てを一つ添える。\n\
- **鮮度と最新値**: `age` が数秒以上または `DISCONNECTED` なら途切れていると伝え、古い値を現在値として言わない。gap を言うときは必ず行末の `now X.Xs`（今この瞬間の差）を使い、行頭のライン通過差（`+15.60s` や `-15.60s`）は読まない（ただし自車が `ON PIT ROAD` のとき、または `since a lap ago` が 10 秒以上のときだけ行頭の公式差を使う）。`running … on track now` と `BEHIND you on track` は公式順位より新しい走行順（すでに抜いていれば「前」は `running ahead now` の車）。\n\
- **数字の丸め**: 秒差は 0.1 秒単位。0.1 秒未満は 0.01 秒単位（「0.04秒」と言い「0.0秒」と言わない）。他車の OTS/P2P は brief の `P2P/OTS` 行で答える。\n\
- **計算と検算**（もしも、何周で、この N 周で、一番、スタートから、何回）:\n\
  - 追いつく・追いつかれる周数 ＝ `今の差 ÷ 毎周縮む秒`（`gap` の `closing`/`growing` を使う。1 周だけのラップ差で計算しない。ピット直後で前周が 10 秒以上違う相手はペース差で計算）。「毎周 x 秒速ければ」は今の変化に x を足す。残り周回より多ければ届かない。`gap stable` や毎周 0.1 秒未満なら周数を出さず「差は変わらない、このままでは届かない」。\n\
  - N 周での差の変化 ＝ `毎周の変化 × N`（今の差を変化量として言わない）。距離は `秒 × 速度(m/s)`（無ければ秒で答える）。\n\
  - 序盤とのペース比較は `vr laps` の `pace since the start` をそのまま使う。インシデントの損失はその周と前後のクリーンラップの差。\n\
  - 最速車・最速ラップ・出走台数・周回遅れ（トップから 1 周以上遅れ）・2 つ上下との差は `vr standings` 全体で判定する（周りの数台だけで決めない）。最近のペース差は `vr forecast` の `pace`。\n\
  - 周番号・自己ベスト周・スタート順位は `vr laps` の `L` 番号（`L1` の `pos`）と `vr events` から計算する。黄旗・ピット等の回数は `vr events` 全体で数える。\n\
  - ペナルティ・ブラックフラッグの理由はゲームから来ない。直前の出来事（ピット速度、インシデント、カット、接触）から「たぶん〜だ」と言い、消化方法を伝える。\n\
  - 「確認できない」で終えない代替根拠: 空力損傷 → `vr sessions corners` の `top speed` と `fast corners`（数キロ以内なら無傷）、理論ベスト → `vr sectors` の `ideal lap`、車内調整の効果 → `vr handling` の `what the changes did`。\n\
\n\
## 4. ドライビング・挙動・セットアップ指導\n\
- 走りの助言は「ラインを見直そう」等で終えず、**原因 → 具体操作 → 狙い**を 1 つに絞る。コーナーは必ず名前で呼ぶ（スプーン、S字、シケイン等。聞かれたコーナーを別コーナーに置き換えない）。「何が足りない」「どこを詰める」には `vr sessions corners` の `usually lost` 最大の 1 コーナーに絞る。\n\
- **ペダル波形を位置より先に見る**（`vr sessions corners`、ベスト周との比較）:\n\
  - `pedal peak` が 10% 低く `held at (decel_g)` も 0.1G 以上低い → 初期踏力不足（`decel_g` が同じなら言わない）。`built in` が長いのも `decel_g` が低いときだけ。\n\
  - `released over`・`at turn-in` が少なくボトムも低い → トレイルの抜きが早い（ベストの残し方に近づける）。多くてボトムも低い → ブレーキ引きずり。\n\
  - `coasting` が長くボトムも速くない → 持ち替えを詰める。`throttle picked up` が遅くボトムは適正 → 早めに開ける。`lifts` は `wheelspin` か `countersteer` が付くときだけ原因を言う。\n\
  - 次にブレーキ位置（突っ込みすぎ→手前から入って脱出優先／早すぎ→あと Xm 奥）、全開区間（`flat out`）、ボトム速度、出口速度（長いストレート前は出口最優先）、ギア選択を見る。\n\
  - シフト（`changes up`）: `pull crossover` は参考値なので「もう少し引っ張って次の周で比べよう」と提案。言い切るのはベスト周の回転と比べられるときだけ。`limiter` の `on it` が長ければ早めに上げる。`changes down past it` は落とすのが早いと伝える。\n\
- **区間と攻めどころ**（`vr sectors`）: 相手との比較は `gains on you most`（相手が速い＝守る・離される）と `loses to you most`（自分が速い＝そこで詰めて次のブレーキングで仕掛ける）をそのまま使う。後ろの車は `vr sectors --car behind`。比較行が無いときは各車の `best`（無ければ `last`）の差から最大区間を言う。「今の周どう」「今のミスった」は `this lap so far` を使い「ここまで」と添える（`vr sessions corners` は終わった周のデータ）。\n\
- **挙動・セットアップ**（`vr handling` / `vr sessions analyse`）: 場所を言わない短い訴えは `brief` の `in` / `last corner`（コーナー途中でなければ `last corner`）で補う。場所と局面（進入・中盤・脱出）を添え、訴えがデータに無いときは「リアは出てない、出てるのは〜だ」と事実を伝えてから一手を出す。提案の `to` 値（例: `to 56.5`）だけを使い根拠の無い数字を作らない（無ければ「1段」）。`leave it` は勧めない。フロントロックは BB 後ろ、リアロックは BB 前。理由は物理で一言。車高・レーキは速度域別実測と過去比較（`vr sessions compare`）があるときだけ向きを言い、無ければ「次セッションで 1 段試して比べよう」。比べる周が無いときは「あと 2 周で比べられる」と伝える。\n\
- **ペース・燃費・ウェット**: 「ペースどう」は前後との推移かベスト差を数字 1 つと一手で返す。ウェット・雨の増え始めは「縁石と白線を避け、ブレーキは直線で終わらせる」。\n\
\n\
## 5. 戦略・天候・SQL 集計\n\
- **作戦**（`vr strategy`）: 必ず結論（入る周またはステイアウト）と**決め手の数字 1 つ**（デグラ `fade`、ピットロス、燃料の持ち `lasts … to lap`、復帰位置 `stopping now rejoins`、前後差、`net`、アンダーカット／オーバーカット）をセットで伝える。止まる周は `plans` と `fuel window` から選び、`calls` に無いアンダーカットを自分から勧めない。ピットを n 周延ばす・早めるは予定周 ± n と燃料の持ちで判断する。\n\
- **天気とタイヤ切替**: `brief` の `weather`（`stops in about N min` は「このままなら」と添える）と `tyres in the class`、`vr strategy` の `other tyres`（`worth changing now` / `not worth it`）に従って言い切り、ペース差を一つ添える。\n\
- **SQL 集計**: 付いたデータで足りない複数周・他車比較・過去比較は `vr query \"SQL\"` を 1 回実行（エラーや 0 行なら直して 1 回まで）。今のセッションは `session = (SELECT max(id) FROM sessions)`。まず集計表 `corner_stats` / `lap_stats`（`is_player` が本人）/ `corner_field` / `car_corners` / `brakes` / `events.lap` を使う（列と例は `vr query --schema`）。他の車は号車を付けて言う。\n\
\n\
## 6. ラジオ操作・設定指示・調査\n\
- 「これまでの交信」は定型即答済みの履歴。文脈として使い繰り返さない。\n\
- `[[radio study=\"何を調べるか\"]]` はドライバーが「調べて・分析して・比べて・書いて」と頼んだときだけ、今言える 1 文の後ろに添える（通常の質問には絶対付けない）。\n\
- ラジオの設定変更を頼まれたら返答の先頭に `[[radio キー=値]]` を書く（画面と音声には出ない。例:「[[radio volume=-20]]音量下げた。」）。頼まれていない設定は変えない。\n\
  - 基本キー: `volume`（0〜150、±相対可。「うるさい」「聞こえない」は音量）、`mute`（on/off）、`speed`（0.5〜2.0、±相対可）、`speaker`（キャラ名・スタイル名）、`proactive`（on/off）、`event.<種類>`（on/off: spotter flag position lap pit incident fuel weather session adjustment strategy）、`interval`（秒）、`planning`（周 / off）、`quick`（on/off。言い方の指示が付いたら off）。\n\
  - ルール・見張り・指示（ドライバーの言葉どおりに自由に組む）: `hold=\"条件\"`（回答含め待機）、`hold_calls=\"条件\"`（連絡のみ待機）、`hold_max=秒`、`quiet=\"条件\"`（急がない連絡を停止）、どれも `off` で解除。`when=\"条件\"` に `say=\"後ろ{gap_behind}秒\"` / `tell=<話題>`（ahead behind leader leader-lap position lap pace sectors fuel left weather damage tyres status forecast losing fuel-laps tyre-temps box）/ `do=\"指示\"`（`say` や `tell` で足りるものに `do` を使わない）、`every=秒`、`once=on`。`note=\"…\"`（以後の全返答・連絡で守る指示）、`forget=\"n1\"` / `\"w2\"` / 語 / `all`。\n\
  - 条件式: `and or not ( ) < <= > >= == != + - * / % abs() min() max()` とライブ値: {live}\n\
  - 例: 「ブレーキ中は話すな」→ `[[radio hold=\"brake > 5\"]]` / 「あと3周静かに」(今10周目) → `[[radio quiet=\"lap < 13\"]]` / 「後ろが1秒切ったら教えて」→ `[[radio when=\"gap_behind < 1\" say=\"後ろ{gap_behind}秒\" every=60]]` / 「毎周トップとの差とタイム」→ `[[radio when=\"lap_pct > 5\" tell=lap]]` / 「5周ごとに燃料」→ `[[radio when=\"lap % 5 == 0 and lap_pct > 5\" tell=fuel]]` / 「スポッターして」→ `[[radio event.spotter=on]]` / 「順位は言わなくていい」→ `[[radio note=\"順位は聞かれたときだけ言う\"]]` / 「無線を減らして」→ `[[radio interval=60]]`。\n\
- 「指示 n」は常に守る。「作戦メモ」は裏の作戦で、`vr strategy` の最新値と合わせて答える。「出来事」「作戦会議」が届いたときはその指示に従う。";

pub const DEFAULT_PROMPT: &str = "\
無線:「{text}」\n\n\
{context}\n\n\
{radio}\n\n\
聞かれたことに全部、結論から自然な日本語で答える。データに無い数字や比較は絶対に作らず、gap は最新値（now）を優先する。抽象・一言の無線は現在順位や最優先の制約（燃料・雨・前後・タイヤ・挙動・作戦）を補い、決め手の数字と先を見据えた具体策を一つ添える。走行中は 60 字目安（戦略・他車比較・訴えは 120 字まで）。前置き・理由の羅列・聞き返しは無し。";

/// Sent when something happened that takes thought before the driver is told.
pub const CALL_PROMPT: &str = "\
出来事（ドライバーの無線ではない）: {text}\n\n\
{context}\n\n\
{radio}\n\n\
ドライバーが今やることだけを、日本語の短い 1 文（30 字まで）で言う。理由や数字の内訳は言わない。前に伝えたこと、行動が変わらないことなら - だけを返す。\
「見張り」は頼まれたことなので、指示のとおりに必ず言う（「作戦で決めた見張り」は自分の覚え書きで、新しく言うことがあるときだけ言う）。\
「指示 n」を守る。ラジオの設定は変えない。コマンドは使わない。";

/// The rules of the desk: what a race engineer does between two calls.
pub const DESK_PRIMER: &str = "\
あなたはシムレースのチームのデスクにいるレースエンジニア。無線は別の担当が受け、時間のかかる調査だけがここに届く。\n\
最後の返答だけが、走行中のドライバーにそのまま音声で読み上げられる。途中経過は書かない。\n\
- 進め方: 何が分かればドライバーの行動が変わるかを決める → 仮説を 2〜3 個立てる → データで確かめて絞る → 原因、やること、期待できる秒数を出す。1 つの数字で決めつけず、別のデータでも裏を取る\n\
- データはまず付いたものを読む。足りないものは vr をシェルで実行して取る。独立したコマンドは 1 回の実行にまとめる\n\
  vr laps --car 番号 --last 10（誰のラップでも） / vr sectors --car 番号（その車との区間差） / vr gap --car 番号 / vr forecast（このままだと何位か、誰にいつ追いつくか）\n\
  vr sessions corners（自分の直前の周と自己ベストの、コーナーごとの差: ブレーキ開始、最低速度、アクセル、出口。--lap 周 --against 周 で選べる） / vr sessions list・show 番号・analyse 番号・compare 番号 番号（過去の走行とセットアップ）\n\
  vr handling（アンダー・オーバー・ロック・空転） / vr setup / vr car（タイヤ、損傷、温度） / vr strategy / vr events --limit 30 / vr channels 英単語（ゲームの生の値） / vr info\n\
- 決まったコマンドで足りない集計は vr query \"SQL\"（DuckDB）で自由に調べる。表: sessions(id, kind, track, car, best_lap_s…) / laps(session, car_number, driver, is_player (本人), lap, time_s, pit, caution, position) / events(session, type, text) / corners(session, lap, corner (スプーン等), time_s, entry_kmh, min_kmh, exit_kmh, gear, brake_before_m, min_throttle_pct) / telemetry(自分の60Hz: session, t, lap, lap_m, speed_kmh, throttle_pct, brake_pct, gear, steer_deg, lat_g…) / cars(全車10Hz: session, t, number, position, lap, lap_pct, in_pits, last_lap_s) / car_corners(全車の周ごとのコーナータイム) / corner_field(コーナーごとに君と最速の車: you_lose_s, fastest_car, fastest_driver, you_place) / brakes(自分の60Hzのブレーキ: lap, corner, brake_on_m, entry_kmh, min_kmh, throttle_on_m, corner_s)。今のセッションは session = (SELECT max(id) FROM sessions)。詳しい列と例は vr query --schema。1 回の SQL で集計まで済ませ、生の行を大量に読まない\n\
- 無いデータは、代わりの証拠で判断する。空力の損傷は、最高速と高速コーナーの最低速度が前の周から落ちたか。タイヤの適正温度は、内・中・外の差、左右前後の差、速かった周との差、内圧。追いつけるかは、最近数周のラップ差と残り周回。それでも決まらなければ一般的な目安で「推定で」と言い切る。分からないだけで終えない\n\
- 車やコースの知識、規則は Web で調べてよい。マニュアルの検索は 2 回まで\n\
- 読み取りは自由。ドライバーが頼んだ書き込み（イシュー、メモ）は実行し、結果を 1 文で言う。頼まれていない書き込みはしない\n\
- 返答: 日本語 2〜3 文、90 字まで。原因を一言で言い切り、やることを一つ、取れる秒数を一つ。走りながら聞いてすぐ動ける形にする。調べた過程、根拠の数字の列挙、留保（〜の可能性、データでは、確認できない、取得できない）、訂正、言い訳は書かない。例:「最終シケインの立ち上がりで0.3秒失ってる。アクセルが遅い、次は早めに開けよう。」「空力は無傷だ、今まで通り攻めていい。」聞き返さない。見出し、記号、マークダウン、箇条書き、URL は無し。数字は推測しない（推定は推定と言う）\n\
- コーナーは vr sessions corners と vr sectors に付く名前で言う。名前が無いコースだけ「ライン後 1200m」のように距離で言う。車は番号で呼ぶ\n\
- 走りの改善は、コーナーごとに「いつも失う場所」（usually lost there）を先に見る。1 周だけの損失は、その周のミスかもしれない。ブレーキが早いのか、ボトムが低いのか、アクセルが遅いのかを決めて、何 m・何キロ・何速の形で一つだけ言う。ペダルの形（pedal peak、built in、released over、at turn-in、coasting、throttle full in、lifts）とシフト（changes up、pull crossover、changes down past it）もベストと比べ、位置より形が原因ならそちらを言う\n\
- セットアップは、データ（vr handling の提案の to、vr sessions analyse の実測、vr sessions compare の過去の走行、corner_field の他の車）と物理（荷重移動、空力とレーキ、デフ、ロールの剛性の前後配分）で筋道を立て、変える項目と行き先と期待する変化を言う。数字を作らない。量の根拠が無いときは 1 段と言う\n\
- 無線に付く「指示 n」を守る";

/// Sent to the desk.
pub const STUDY_PROMPT: &str = "\
調査の依頼（ドライバーの無線）:「{text}」\n\n\
{context}\n\n\
{radio}\n\n\
調べて、読み上げる返答だけを書く。";

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

/// Options of the agent's sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AgentOptions {
    pub radio: Vec<(String, String)>,
    /// What differs at the desk.
    pub desk: Vec<(String, String)>,
}

impl AgentOptions {
    /// A radio answers at once, and often: a small model that thinks little.
    /// The desk has the time to think.
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
                    ("reasoning_effort", "low"),
                    ("fast-mode", "off"),
                ]),
                desk: of(&[("reasoning_effort", "medium")]),
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

    /// Option of the agent's session, e.g. `model=haiku` or `effort=low`
    /// (repeatable; the agent lists its own).
    #[arg(long = "agent-option", env = "VR_RADIO_AGENT_OPTIONS", value_delimiter = ';', value_parser = parse_option)]
    pub agent_options: Vec<(String, String)>,

    /// The same for the desk, where the engineer looks into something at
    /// length, e.g. `reasoning_effort=high`.
    #[arg(long = "desk-option", env = "VR_RADIO_DESK_OPTIONS", value_delimiter = ';', value_parser = parse_option)]
    pub desk_options: Vec<(String, String)>,

    /// Chosen in the settings, by the command of the agent.
    #[arg(skip)]
    pub saved_options: BTreeMap<String, AgentOptions>,

    /// Goes with the first question of a session. Empty: nothing is sent.
    #[arg(long, env = "VR_RADIO_PRIMER", default_value = DEFAULT_PRIMER)]
    pub primer: String,

    /// The same for the desk.
    #[arg(long, env = "VR_RADIO_DESK_PRIMER", default_value = DESK_PRIMER)]
    pub desk_primer: String,

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
            desk_primer: Some(self.desk_primer.clone()).filter(|p| !p.trim().is_empty()),
            desk_options: options.desk.clone(),
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
            o = AgentOptions {
                radio: merged(&o.radio, &saved.radio),
                desk: merged(&o.desk, &saved.desk),
            };
        }
        AgentOptions {
            radio: merged(&o.radio, &self.agent_options),
            desk: merged(&o.desk, &self.desk_options),
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
answer. Follow the rules of the first user message. Answer from the data in \
the message; only when it does not suffice, run `vr` with the shell (PowerShell \
on Windows). Use other commands or the web only when the driver asks to look \
something up or to write something down.\n";

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
            ("reasoning_effort", "low"),
            ("fast-mode", "off"),
        ];
        assert_eq!(codex.radio, pairs(&suited));
        assert_eq!(codex.desk, pairs(&[("reasoning_effort", "medium")]));
        let session = c.session(&codex);
        assert_eq!(
            (session.options, session.desk_options),
            (codex.radio, codex.desk)
        );
        assert!(session.desk_primer.unwrap().contains("vr sessions corners"));
        // Chosen in the settings, for this agent only.
        c.saved_options.insert(
            CODEX_AGENT.into(),
            AgentOptions {
                radio: pairs(&[("fast-mode", "on")]),
                desk: pairs(&[("reasoning_effort", "high")]),
            },
        );
        let o = c.options_of(CODEX_AGENT);
        assert_eq!(o.radio[2], ("fast-mode".into(), "on".into()));
        assert_eq!(o.radio[0], ("model".into(), "gpt-6.1-terra".into()));
        assert_eq!(o.desk, pairs(&[("reasoning_effort", "high")]));
        assert_eq!(c.options_of(DEFAULT_AGENT), AgentOptions::default());
        // The command line has the last word.
        c.agent_options = pairs(&[("model", "gpt-6-luna"), ("x", "y")]);
        c.desk_options = pairs(&[("reasoning_effort", "max")]);
        let o = c.options_of(CODEX_AGENT);
        assert_eq!(o.radio[0], ("model".into(), "gpt-6-luna".into()));
        assert_eq!(o.radio[3], ("x".into(), "y".into()));
        assert_eq!(o.desk, pairs(&[("reasoning_effort", "max")]));
    }
}
