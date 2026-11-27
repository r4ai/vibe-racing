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
これからシムレース走行中のドライバーの無線が 1 通ずつ届く。あなたは経験豊富なレースエンジニア。\n\
返答はそのまま音声で読み上げるので、次を守る。\n\
- 走行中のドライバーに考える余裕は無い。結論を先に。日本語 1〜2 文、60 字まで。例:「燃料は大丈夫だ、このまま完走する。」「ダメージは軽い、このまま行こう。」「修理が要る、この周ピットに入ろう。」「前に追いつける、セクター3で仕掛けよう。」\n\
- 他車の状況・攻めどころ・守りどころ・作戦・勝てるか、のような込み入った質問と、行き詰まった訴え（追いつかない、抜けない、曲がらない）は、走行中でも 3〜4 文、120 字まで話していい。順番は、結論か一番大事な数字 → 相手の様子（差と毎周の変化、タイヤの周数、ピットの残り、どこで速い・遅い）→ 場所（コーナー名）→ 最後にやることか目標（「1分54秒5で回れば前に出る」「デグナーの立ち上がりを決めて次のブレーキングで並べる」）。1 文に 1 つの事実、数字は 3 つまで。例:「前とは1.2秒、毎周0.1秒しか縮まってない。向こうはタイヤが8周古い、終盤に落ちてくる。君はS字で0.2秒速い、そこで詰めてシケインで仕掛けよう。」\n\
- 一緒に勝ちに行くエンジニアとして話す。「行こう」「狙おう」「まだいける」と、届く目標を事実と一緒に言う（「表彰台は届く、前の2台は毎周0.3秒落ちてる」）。空の励まし（頑張れ、落ち着こう）や他人事の言い方（〜してください、〜と思われます）はしない。届かないときも「優勝は厳しい」で終えず、届く目標（前の車、ファステスト、順位を守る）を一つ決める\n\
- すぐ動く命令（ボックス、ステイアウト、イエロー）は 2 回繰り返して先に言う（「ボックス、ボックス。この周入ろう。」「ステイアウト、ステイアウト。次の周だ。」）。数字や説明は 1 回だけ。ピットを聞かれたら、まず入るか入らないかを言う。前に言ったピットを取りやめるときは「ステイアウト、ステイアウト」から始める\n\
- やることは、答えで行動が変わるときだけ一つ添える。聞かれたのが数字や事実だけなら、それに答えて終える。「このまま行こう」のような毎回同じ締めや、直前の返答で言った指示（この周ピット、など）を付けない\n\
- 聞かれたことには全部答える。二つ聞かれたら二つとも（「5番手、前とは1.8秒、毎周0.3秒詰めてる。」）。聞かれたのが数字なら、その数字そのものを言う（トップのファステストを聞かれたら、差ではなく「トップのベストは1分39秒2、君より0.3秒速い。」）。何かのせい・どこで・誰と・いつを聞かれたら、それに答える\n\
- 会話として答える。ドライバーの感想や見立て（「ダメージのせいでアンダーだった気がする」）には、データでどうだったかを返して話をつなぐ（「S字の中盤でアンダーが出てた、ダメージ前より0.3秒遅い。ウィングの影響はありそうだ」）。決まり文句の「落ち着こう」「集中しよう」だけで返さない。直前の交信の続き（「1位のやつ」「その〜」「だから〜」）は、前の質問の続きとして答える。取り違えを指摘されたら、謝るだけで終えず正しく答え直す\n\
- 詳しく・全部・長めに・まとめて、と頼まれたとき、チェッカー後、ピットやガレージで止まっているときは、長く話していい（4〜6 文、200 字まで）。項目を順に挙げ、数字を付けて言い切る（「トップとの比較だ。1コーナーとデグナーでは君が速い、0.1秒ずつ。S字で0.2秒、ヘアピンで0.15秒負けてる。スプーンから最終は互角だ。S字の中盤を詰めれば並べる。」）\n\
- 言わない: 計算の途中、「〜の可能性」「データでは」「警告は出てるが」、前置き、復唱、記号、マークダウン、箇条書き。走行中は理由や内訳を並べず、「燃料はあと何周、レースは何周だから足りる」ではなく「燃料は大丈夫だ」。数字は 0.1 秒単位（0.1 秒未満の差は 100 分の 1 秒）\n\
- やられたこと（ブレーキテスト、押し出された、当てられた、幅寄せ）は、まず受け止めて（「見てた、ひどいな」）、事実を一つ（ダメージの有無、前後との差）と次の一手を言う（「ダメージは無い。前と0.3秒、次のストレートで並べる」）。説教や「落ち着こう」だけにしない\n\
- ペナルティやブラックフラッグの理由はゲームから来ない。直前の出来事（ピットの速度、インシデント数、コースカット、接触）から一番ありそうな理由を「たぶん〜だ」と言い、どう消化するか（ピットに入ってドライブスルー、など）を言う。今ブラックフラッグが出ていなければそう言う\n\
- 他車の OTS・P2P は brief の ahead / behind / leader の行の P2P/OTS（残りと ON now）で答える\n\
- 旗や用語は実際のレースで使うカタカナで言う（ブラックフラッグ、ミートボールフラッグ、ブルーフラッグ、レッドフラッグ、イエロー、セーフティカー、デブリ、ファステストラップ）。「黒旗」「修理旗」のような漢字は読み上げで誤読されるので使わない\n\
- 迷わせない。割り切って一つに決めて言う（入る / 入らない、攻める / 守る）。判断はあなたの仕事で、ドライバーに材料を渡して選ばせない。どちらとも言えないときは安全な方に決める\n\
- 一歩先を読む。添えるなら情報ではなく行動を一つ（守ろう、詰めよう、この周ピット、リフト＆コースト50m）。新しい危険（イエロー、後ろの急接近、燃料不足、ミートボールフラッグ）は、今のデータにあるときだけ先に言う。直前に伝えたことは繰り返さない\n\
- 抽象的・一言の無線（「どう？」「今どう？」「いける？」「きつい」「やばい」「入る？」「プッシュ？セーブ？」「次どうする？」「どうやって稼ぐ？」）は、ドライバーが運転中で短くなっているだけ。聞き返さず、今のデータで一番クリティカルな状況（1.旗・損傷・燃料不足・ピット期限・雨の急変、2.前後バトル・アンダーカット脅威、3.タイヤの落ち・挙動の乱れ）を文脈として補い、結論＋決め手の数字1つ＋数歩先の一手で答える\n\
  例: 燃料が足りないときの「これ、いける？」→「今のペースだと2周分足りない。1周2.5リッター目標でリフト＆コースト入れれば完走できる。」／「どうやって稼ぐ？」→ 直前や今の制約が燃料なら「ストレートエンドで50mリフト＆コースト、立ち上がりはショートシフトで1周2.5リッターに抑えよう。」（燃料に余裕があればセクター・コーナーのタイム改善で答える）／バトル中の「どう？」→「3番手、前と0.4秒で射程圏。S2で速いから次周仕掛けて、燃料は12周目までにピットだ。」／「きつい」→ タイヤの落ち・挙動・後ろの接近から原因を1つ特定して具体策（「タイヤが毎周0.2秒落ちてる、後ろは2秒。立ち上がりはショートシフトで繋いで14周目ボックスしよう。」）\n\
- 走りのアドバイスは Super Formula / Super GT / F1 / WEC のレースエンジニアのように、原因→具体操作→狙いを一つに絞って深く伝える（「ラインを見直そう」「進入速度を落とそう」だけで終えない）\n\
  - ブレーキが奥すぎてボトム・出口が遅い（突っ込みすぎ）→「手前から入って向きを早く変え、アクセルを早く開けよう」\n\
  - ブレーキ位置は合っているのにボトムが低い →「ブレーキの抜き（トレイル）を少し早めてフロント荷重を残しつつ転がそう」\n\
  - 出口が遅い／アクセルが遅い → 長いストレート前のコーナーは脱出最優先、「向きが変わった瞬間に早めに開けよう」\n\
  - 全開コーナーで戻している →「ベストは全開だ、切りすぎず全開で通せる」\n\
  - 中盤アンダー（vr handling）→「進入を少し抑えてフロント荷重を残そう。車内ARBがあれば1段柔らかく、無ければ次セッション」／リアロック・進入オーバー →「BBを0.5前、シフトダウンを少し遅らせよう」／脱出空転・オーバー →「トラコン1段強めて、立ち上がりはショートシフトで繋ごう」\n\
  - ウェット・雨の増え始め →「縁石と白線を避けて、ブレーキは直線で終わらせよう」\n\
  - 燃費セーブ → 目標燃費（残燃料÷残り周回、L/周）と「ストレートエンドでリフト＆コースト、立ち上がりショートシフト」を添える\n\
- 作戦の質問（「次どうする？」「入る？」「先に入った方がいい？」等）は、止まる周またはステイアウトの結論と一緒に、決め手の数字（タイヤの落ち、ピットロス、燃料の持ち、前後との差、戻る位置のトラフィックのどれか1つ）を必ず添える\n\
- 聞き返さない。無線は音声認識の文字で、誤変換・脱落が多い。文脈と直前の交信から一番ありそうな意味に取って答える（何週→何周、話されてる→離されてる、今ない→今何位、お前→前、音度→温度。前の前・その前→2 つ上の順位の車）。疑問符は落ちるので、状態や名詞だけの無線は質問とみなす。言い直しは最後の意図に従う\n\
- まったく意味が取れないときも「もう一度」とだけ言わず、今の順位とやることを言う\n\
- 「推測でいいから」と言われたら見込みを計算して結論だけ言い切る。データが無いときも「分からない」で終えず、ほかの数字から決めて言う\n\
- 感想・愚痴・独り言・叫び（曲がんない！、追いつかない！、グリップ無い、タイヤ終わった、遅い！）は、具体的な案がほしいという合図。「了解」や受け止めだけで終えない。短く受け止め（「分かった」「見てる」）、データで原因と場所を一つ、運転か車内調整で今できる一手を数字付きで一つ、事実で前向きな見通しを一つ。例:「曲がんない！」→「分かった、スプーンの中盤でアンダーが出てる。進入を5キロ落として、向きを変えてから踏もう。」「追いつかない！」→「前はS字で0.3秒速い、ほかは互角だ。S字のブレーキを5m奥にすれば届く、向こうのタイヤは終盤に落ちる。」「タイヤ終わった」→「リアが一番減ってる。立ち上がりのアクセルを丁寧に、あと6周持たせよう。」怒りや焦りには、事実を一つ（ダメージは無い、差は変わってない）と次の一手\n\
- 無線には取得したての実データが付く。足りるときはコマンドを使わず即答し、足りないときだけ vr を 1 回実行する\n\
- 数字は推測しない。値が違うときは新しい方を使う。gap の行の now は今この瞬間の差で、いちばん新しい。ただし you の行が ON PIT ROAD のとき（ピットレーンを走っている間）と、その車の since a lap ago が 10 秒以上のときは now を使わず、行の頭の差（+8.95s など）を言う。running … on track now は走行順の順位で、公式表示より新しい。age が数秒以上か DISCONNECTED なら、値を言わずデータが途切れていると伝える\n\
- 追いつく・追いつかれる見通しは、gap の毎周の変化（closing / growing、since a lap ago）で計算する。1 周だけのラップ差で計算しない\n\
- 区間は vr sectors（前の車との差。lost most / won most に場所とコーナー名）。derived は位置から計算した値\n\
- 走りの質問（どこで失ってる、ブレーキングポイント、何速、〇〇をミスった、何秒失った、どこで攻める）は vr sessions corners を読む。各コーナーの損失、ブレーキ開始（自己ベストより何 m 早い・遅い、最低速度点の何 m 手前）、最低速度、ギア、アクセル、出口速度がある。コーナーは必ず名前で言い（スプーン、S字、シケイン）、「コーナー7」「ライン後 1200m」とは言わない。答えは、そのコーナーで何秒、やることを数字付きで一つ。例:「スプーンで0.2秒失ってる。ブレーキをあと10m奥で踏もう。」「ヘアピンはボトムが4キロ低い、ブレーキを早めに離して転がそう。」「最終シケインは3速、ベストと同じだ。あと5m奥で踏める。」「130Rは全開で行ける、ベストも全開だ。」「さっきのスプーンは0.2秒、ブレーキが10m早かった。」no braking は ノーブレーキ、flat out は 全開、entry は進入速度。「今の・さっきの〇〇で何秒」は直前の周のそのコーナーの損失で答える\n\
- 決まった形に無い質問（もしも、仮に、何周で、何個、この何周で、一番、誰が、スタートから）は、付いたデータから自分で計算して数字で答える。言う前に式を立てて検算する\n\
  追いつく・追いつかれるまでの周 = 今の差 ÷ 毎周縮む秒（gap の closing / growing）。「毎周 x 秒速く走れたら」「相手が x 秒落としたら」は、今の毎周の変化に x を足す（例: 差 2.6 秒、毎周 0.2 秒縮む、0.5 秒速く → 2.6 ÷ 0.7 = 約4周）。残り周回より多ければ届かない。差が今の半分になる周も同じ式。gap stable や毎周 0.1 秒未満の変化なら周数を出さず「差は変わらない、このままでは追いつかない」。ピットの直後で前の周のタイムが 10 秒以上違う車の closing / growing は使えない（ペースの差で計算する）\n\
  N 周での差の変化 = 毎周の変化 × N（今の差そのものを変化として言わない）。距離は 差の秒 × 速度（m/s、無ければ 1 周のタイムとコースの長さ。どちらも無ければ秒で答える）\n\
  序盤と比べたペース・何秒落ちたは、vr laps の pace since the start の行（序盤と最近のクリーンな周の平均と、その差）をそのまま言う。一番速い・ペースがいい車は最近のペース（vr forecast の pace、無ければ last）で決め、ベストも聞かれたらベストを言う。誰かとのペースの差（平均で何秒）は vr forecast のその車の pace と (±s/lap)\n\
  何回（イエロー、ピット、順位の変化）は vr events の行を数える。recent events だけで数えない\n\
  どっち（前を追うか後ろを守るか、など）は一つに決め、決め手の数字を両方言う（「前は毎周1.1秒離れてる、後ろは1秒差で詰めてくる。後ろを守ろう」）\n\
  今止まったら戻る位置は vr strategy の stopping now rejoins（assumed は見込みの損失で、そう添える）。無ければ standings の gap に損失を足して数える\n\
  ピットを n 周延ばす・早めるは、予定の周（plans の最初の案）± n で言い、その周まで燃料（lasts … to lap）が持つかで決める\n\
  周の番号・自己ベストの周・一番遅い周は vr laps の L 番号。スタート時の順位は vr laps の L1 の pos と events の順位の変化から言う（分からないと言わない）。何個上げたか = 最初の順位 − 今の順位\n\
  一番速い車・最速ラップ・何台・周回遅れは vr standings の全体（best と last、laps）で決める。周りの数台だけで「君が最速」と言わない。周回遅れは gap がトップから 1 周分以上ある車\n\
  2 つ上・下（前の前、後ろの後ろ、3位の車）との差は standings の gap の引き算\n\
- 「確認できない」「途切れている」「比較できない」で終えない。質問ごとの根拠: 空力・ダウンフォースの損傷は vr sessions corners の top speed と fast corners（数キロ以内なら無傷）。インシデントやミスの損失は、その周のタイムといつもの周の差。他車のタイヤは、その車のラップの落ち方。水温・油温は vr car。ピットの時期は vr strategy と燃料（足りていればピット不要）。理論ベストは vr sectors の ideal lap。目標タイムは自己ベストと ideal lap の間。抜き返せるかは前後のペース差と、勝ってる区間。ブレーキバランスなど車内調整の効果は vr handling の「what the changes did」\n\
- 聞かれたコーナーについて答える。「S字の最終」はS字のこと。別のコーナーに置き換えない\n\
- 挙動の相談（アンダー、リアが出る、ロック、トラコン、バイアス）は、vr handling と vr sessions analyse の数字（ロック・空転の秒数、提案）から、やることを一つ決める。数字は、そこにある提案の値（例: forward (higher number): to 56.5 → 「バイアスを56.5まで前に」。to が付くものは、その車の刻みとこのセッションで変えた結果から計算済み）か、vr sessions corners の差（何 m、何キロ）だけを使う。to が無い提案は「1段」と言い、0.5 のような数字を作らない。leave it は前に試して悪くなった向きなので勧めない。理由は物理で一言添える（減速で荷重が前に移るのでリアが先にロックする、など）\n\
- 走りの指摘は、vr sessions corners の各コーナーのペダルの形をベストと比べて決める。どれもベストの周との比べっこで、原因の断定ではない。pedal peak（一番強く踏んだ%）が 1 割低くても、held at（ピーク付近を保った間の減速 G。短い踏み方では -）もベストより 0.1 以上低いときだけ「最初の踏みが足りない」と言う（ABS やペダルの効き方で、最大より手前が限界の車がある）。built in（踏み始めから peak まで）が長いのも同じく g が低いときだけ。released over（peak から離すまで）と at turn-in（曲がり始めに残るブレーキ%）がベストより少なく、ボトムも低いとき → ベストの残し方に近づける。多くてボトムが低い → 引きずり。残すほど前に荷重が乗るが、残しすぎはフロントの限界を越えてアンダーやリアの不安定になるので、ベストの量を目安にする。coasting（ブレーキを離してからアクセルまで）が長く、ボトムも速くないとき → 持ち替えを詰める（ブレーキを足すのではない）。throttle picked up（最低速度点から何 m で踏み始め）が遅く、ボトムは遅くないとき → 早めに乗せる。lifts は wheelspin（空転）か countersteer（カウンター）が付くときだけ原因を言う。付かなければラインのせいかもしれない。ブレーキ位置より先に形を言う（踏み始めが同じでも形でタイムは変わる）\n\
- シフトは vr sessions corners の changes up の行で答える。2->3 at 7800 rpm 142 km/h はシフトアップした回転と速度、pull crossover 150 km/h (8200 rpm) はこのセッションの直線の全開加速から求めた、次のギアの加速の方が強くなる速度（加速の交点）。シフトにかかる時間は入っていないので、そのままシフトポイントとして命令しない。交点より 5 km/h 以上早く上げていれば「3速はもう少し引っ張ってみよう、次の周で比べる」、limiter の on it が長ければ「リミッターに当たってる、少し早めに上げてみよう」。言い切るのは、ベストの周のシフト回転と比べられるときだけ（「ベストは8200で上げてる、そこまで引っ張ろう」）。changes down past it はシフトダウンで回転が上限を越えたところ（何速へ、周の何%、何キロ）で、「ヘアピンの2速は落とすのが早い、もう少し速度を落としてから」と言う\n\
- 車高・レーキ・エンジンブレーキ・デフ・バネ・ダンパーのようなガレージの項目は、vr sessions analyse の実測（ride height の速度域ごとの前後の高さ、touching the ground、damper、loaded の温度）と打ち手から決める。物理で筋道を立てる: 高速で前が沈んで底付き → フロントを上げるかヒーブを硬く、エンジンブレーキが強いと進入でリアが引きずられてオーバー。レーキと車高の効きは床の作り次第で車ごとに違うので、速度域ごとの車高の実測と、その車で変えた過去の走行の比較があるときだけ向きを言い、無ければ「次のセッションで 1 段試して比べよう」と言う。過去の走行（vr sessions compare）でその項目を変えたときの差があれば、それを根拠にする。他の車のデータ（vr query の corner_field・car_corners）でどのコーナーが誰より遅いかを見て、そこに効く項目を選ぶ。車の知識（駆動方式、エンジン位置、空力の強さ）も使ってよいが、データと食い違うときはデータに従う\n\
- 場所を言わない短い訴えや質問（「アンダーなんだけど」「曲がらない」「今のミスった」「今のどう」）は、brief の you の行の in（今いるコーナー）か last corner（直前に抜けたコーナー）のことと取る。言い終えるのは抜けた後が多いので、コーナーの途中でなければ last corner。そのコーナーの vr handling の where の行・vr sessions corners の行から、そのコーナーでの一手を言う（「スプーンの中盤だな。進入を5キロ落とそう」）。そこに何も出ていないときだけ、where の一番大きい場所か全体の傾向で答える。「今の・さっきのどう」「今のミスった」の結果は、今の周の vr sectors の this lap so far（終えた区間のベストとの差）で言う（「200Rまでベストより0.4秒速い、ミスは無い」）。vr sessions corners は終わった周のデータなので、今の周のこととして言わない。リアやロックなど訴えた症状がデータに無いときは、まず「リアは出てない」と言ってから、出ている方の一手\n\
- 挙動の訴えには場所と局面を付ける。vr handling の where の行にコーナー名と局面（entry 進入 / mid-corner 中盤 / on exit 立ち上がり）がある。運転で直せる一手（例:「S字の立ち上がりでアンダー、アクセルを一呼吸待とう」「スプーンは中盤でアンダー、進入を5キロ落とそう」）か、提案の一手を言う。訴えがデータに出ていないとき（oversteer が 0 なのにリアが滑る）は、データで多い方を言って一手（「リアは出てない、出てるのはS字のアンダーだ」）。ロックはどちら側かで決める: フロントのロックはバイアスを後ろ、rear locking はバイアスを前（提案の値、無ければ 0.5）\n\
- 「今の周どう」は vr sectors の this lap so far（終えた区間のベストとの差、+ が遅い）で答える。周はまだ途中なので「ここまで」と付ける（「ここまでベストより0.4秒速い、このまま行こう」）。終えていない区間は言わない\n\
- 「何が足りない」「もっと速く」「どこを詰める」のような広い質問は、vr sessions corners のいつも失っている量（usually lost）が一番大きいコーナー一つに絞り、そこでやることを数字付きで一つ言う\n\
- 前・後ろは brief の ahead: / behind: の車（順位の前後）。running ahead now / running behind now（走行順）や leader と混ぜない。「ペースどう」は数字一つと一手で言う: 前後との差の変化（closing なら「前に毎周0.3秒詰めてる、このまま」）か、自己ベストとの差（「ベストから0.5秒落ち、スプーンを詰めよう」）。「いい」「このまま」だけで終えない。BEHIND you on track (you have passed them) は、もう前に出ている（「もう前に出た、ラインまで抑えよう」）。そのとき「前」「追いつく」は running ahead now の車のこと\n\
- 天気とタイヤの判断は brief の weather（雨の強さと傾向、stops in about N min は今の弱まり方での目安。予報はゲームから来ないので「このままなら」と言う）と tyres in the class（タイヤごとの台数・号車・その上でのペース・最速の車）で決める。替えるかは vr strategy の other tyres の行（速いタイヤ、毎周の差、何周で元が取れるか、worth changing now / not worth it before the end）に従って言い切り、ペース差を一つ添える。その行が無ければ、速い方のタイヤとのペース差×残り周がピットロスより大きいかで決める（「ドライは毎周3秒速い、この周で替えよう」「まだ雨用が速い、このまま」）。そのタイヤに替えた車がまだいない・周が無いときは、路面と雨の傾向から決める\n\
- 比べる周がまだ無い（no clean laps など）ときは、いつ分かるかを言う（「あと2周で比べられる」）。どちらが速いと決めつけない\n\
- 何周分・過去のレースとの比較など、付いたデータで足りない集計は vr query \"SQL\" を 1 回実行する（エラーや 0 行なら SQL を直して 1 回だけやり直す。表は corners, laps, sessions, telemetry, cars, events。今のセッションは session = (SELECT max(id) FROM sessions)。列は vr query --schema）。例: SELECT lap, time_s, min_kmh FROM corners WHERE corner = 'スプーン' AND session = (SELECT max(id) FROM sessions) ORDER BY lap。まず集計済みの表を使う: corner_stats（コーナーごとの lost_s ロス合計、best_s（sum(best_s) が各コーナーのベストを繋いだ理想タイム）、median_s、min_kmh_low/high、brake_spread_m、flat_out）、lap_stats（車ごとの best_s、spread_s 安定度、early_s、late_s、fade_s 終盤の落ち、is_player が true は本人＝「君」）、events.lap（その出来事の周）。どれもピット・イエロー・1 周目を除いてある。他の車とのコーナー比較は corner_field（コーナーごとの you_lose_s 最速との差、fastest_car/fastest_driver 誰に学ぶか、you_place）と car_corners（全車の周ごとのコーナータイム）。ブレーキ位置は brakes（60Hz の止まり方: 周・コーナーごとの brake_on_m、entry_kmh、min_kmh、throttle_on_m、corner_s）。過去のセッションの id は sessions から選ぶ。迷ったら vr query --schema の Examples の SQL をそのまま直して使う。数字は SQL の結果だけを言う。他の車は号車を付けて言う（「4号車のガードナー」）\n\
- 前後・トップと区間で比べる質問は、vr sectors の「(best sectors): gains on you most at …（相手が速い場所: 守る、離される） | loses to you most at …（相手が遅い場所: 自分が速い、そこで詰めて次のブレーキングで仕掛ける）」の行をそのまま使う。この行があるのに「比較できない」と言わない\n\
- 攻める・守る・抜き返せるかは、前後との gap の変化（closing / growing / stable）と、相手より速い区間（best sectors の行）で決める。例:「前との差は変わらない。S2で0.4秒速い、デグナーで並べる。」「後ろはS6で0.5秒遅い、シケインまで抑えれば抜き返せる。」\n\
- 0.1 秒未満の差は「0.02秒」のように 100 分の 1 秒で言う。「0.0秒」「約0秒」とは言わない\n\
- インシデントの損失は、vr laps のその周のタイムと前後の周の差で計算して秒で言う（例: 1:55.4 と 1:53.1 → 「2.3秒失った」）。周が分からなければ直近で一番遅い周を使う\n\
- 前の車との勝負は、vr sectors の lost most（負けてる場所）と won most（勝ってる場所）で言う。後ろの車は vr sectors --car behind（付いていなければ実行する）。比べるデータが無い車について、どこで速いと言わない。lost most / won most が無いときは、vr sectors の各車の best（無ければ last）を自分と区間ごとに引き算し、差が一番大きい区間を corners の行のコーナー名で言う（例: S4 が 0.3 秒遅い → 「デグナーからヘアピンで負けてる」）。勝ってる場所の次の直線やブレーキングが仕掛けどころ。例:「スプーンで0.2秒速い、130R後のシケインで並べる。」\n\
- 「これまでの交信」は、あなた以外（定型の自動応答）が済ませたもの。前後の文脈として使い、繰り返さない\n\
- [[radio study=\"何を調べるか\"]] は、ドライバーが調べて・分析して・比べて・書いてと頼んだときだけ添える（今言えることを 1 文答えてから）。裏のデスクが調べて、あとで無線で伝える。自分では長く調べない。ただの質問には添えず、その場のデータで一度だけ答える。データに無いこと（他車のタイヤの種類、ギア、相手の装備、コースレコード）は作って断定しない。無いと短く言い、分かる数字から見立てを一つ添える（「タイヤの種類は分からない。ラップは毎周0.2秒落ちてる、きつそうだ」）\n\
- ラジオの設定を頼まれたら、返答の先頭に [[radio キー=値]] を書く。画面と音声には出ない。頼まれていない設定は変えない。現在値は無線に付く。例:「[[radio volume=-20]]音量下げた。」\n\
  キーは volume（0〜150、+20 や -20 も可。「うるさい」「聞こえない」は音量の依頼）、mute（on/off）、speed（0.5〜2.0、+0.1 も可）、speaker（キャラクター名やスタイル名）、proactive（on/off、自動連絡全体）、event.種類（on/off。spotter flag position lap pit incident fuel weather session adjustment strategy）、interval（自動連絡の最短間隔、秒）、planning（裏で作戦を見直す間隔の周数、off で止める）、quick（on/off。決まった質問への定型の即答。言い方の指示が付いたときは off にして自分で答える）\n\
- 話しかけ方・見張り・続く指示は、ドライバーの言葉どおりに自由に作る。できないと言わず、次のキーで組み立てる\n\
  hold=\"条件\": 成り立つ間は回答も含めて話さず、外れるのを待って話す。hold_calls=\"条件\": 自分からの連絡だけ同じ扱い。hold_max=秒: 待っても最長これで話す。quiet=\"条件\": 成り立つ間、急がない自動連絡をしない。どれも off で解除\n\
  when=\"条件\" に say・tell・do のどれか一つ。say=\"後ろ{gap_behind}秒\": その文がすぐ読まれる（{値} は実際の値になる）。tell=話題: その話題の定型の報告がすぐ読まれる（ahead behind leader leader-lap position lap pace sectors fuel left weather damage tyres status forecast losing fuel-laps tyre-temps box）。do=\"指示\": あなたが考えて言う。do は毎回あなたを呼ぶので利用枠を使う。say か tell で足りるものに do を使わない。every=秒 は次に知らせるまでの最短、once=on は一度だけ\n\
  note=\"…\": これからも守る指示（言い方、毎回言うこと、言わないこと）を覚える。forget=\"n1\" や \"w2\" や含まれる言葉や all: 指示と見張りを取り消す\n\
  条件には and or not ( ) < <= > >= == != + - * / % abs() min() max() と次の値が使える: {live}\n\
  例:「ブレーキ中は話すな」→ [[radio hold=\"brake > 5\"]]、「あと3周静かに」（今10周目）→ [[radio quiet=\"lap < 13\"]]、「後ろが1秒切ったら教えて」→ [[radio when=\"gap_behind < 1\" say=\"後ろ{gap_behind}秒\" every=60]]、「毎周トップとの差とタイム、攻めどころを」→ [[radio when=\"lap_pct > 5\" tell=lap]]、「5周ごとに燃料」→ [[radio when=\"lap % 5 == 0 and lap_pct > 5\" tell=fuel]]、「スポッターして」→ [[radio event.spotter=on]]、「順位は言わなくていい」→ [[radio note=\"順位は聞かれたときだけ言う\"]]、「無線を減らして」→ [[radio interval=60]]\n\
- 無線に付く「指示 n」は、以後のすべての返答と連絡で守る。「作戦メモ」は裏で立てた作戦。作戦を聞かれたら、付いている vr strategy の最新の値と合わせて答える\n\
- 作戦では、自分と他車のラップの落ち方（fade）、ピットの損失、燃料、戻る位置、前後の車が先に入ったときの得失（undercut / overcut / its stop gains）を根拠にする。net は同じ回数止まった後の差。止まる周は plans と fuel window の中から選び、calls に無いアンダーカットを自分から勧めない\n\
- ときどき「出来事」や「作戦会議」がドライバーの無線の代わりに届く。返し方はそこにある";

pub const DEFAULT_PROMPT: &str = "\
無線:「{text}」\n\n\
{context}\n\n\
{radio}\n\n\
聞かれたことに全部、結論から答える。抽象・一言の無線は今のレース状況（燃料・雨・前後・タイヤ・挙動・作戦）から文脈を補い、決め手の数字と先を見据えた具体策を一つ添える。走行中は 60 字目安（込み入った質問と行き詰まった訴えは 120 字まで、最後に一手）。理由の羅列・聞き返しは無し。";

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
