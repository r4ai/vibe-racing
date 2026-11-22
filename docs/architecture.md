# アーキテクチャ

## 概要と基本概念

ゲームのテレメトリを解析し、エージェントの無線回答に必要な情報を返す CLI `vr`。
ゲーム固有の処理は入力アダプタに限定し、問い合わせは `RaceApi` に一本化。
音声無線アプリ `vr-radio` は、声をエージェントに渡し、返答を読み上げる。

### 基本モデル

- **正規化フレーム**: `Frame`（SI 単位、未対応項目は `None`）
- **テレメトリソース**: `Frame` を返す `TelemetrySource`。ライブ・記録再生・合成レースが実装
- **レース状態**: `RaceState` が周回・燃費・ギャップ推移・イベントを蓄積
- **問い合わせ API**: `RaceApi`。レスポンス型は JSON の互換契約

### 設計目標

- **応答性**: 1 コマンド・数十 ms で回答（`vr brief`）
- **拡張性**: ゲーム追加はアダプタ crate 1 つ、フロントエンド追加は `RaceApi` の利用のみ
- **正確さ**: 推定値と実測値を区別（`basis`・`mode`・`history`・`age_ms`）
- **ゲーム無しで検証可能**: 合成レースと記録再生

### 用語

| 用語               | 定義                                                                     |
| ------------------ | ------------------------------------------------------------------------ |
| 常駐コレクター     | `vr serve` 内でソースを読み続けるスレッド                                |
| ワンショット       | サーバー無しで共有メモリを 1 回だけ読む動作。履歴なし                    |
| クリーンラップ     | ピットレーン通過も黄旗も無い周。平均ペース・燃費の母集団                 |
| ギャップ推移       | 自車のライン通過ごとに記録した他車との差の変化量（秒/周）                |
| 近傍車             | 自車の ±2 位以内（マルチクラスではクラス内）。他車のピットイベントの対象 |
| discovery ファイル | `vr serve` が書く `server.json`。CLI はここから URL を取得               |

## 階層構造と実行時アーキテクチャ

### 全体の階層構造

```mermaid
flowchart TB
    subgraph Frontend["フロントエンド層"]
        CLI["vr-cli<br/>コマンド・出力形式・バックエンド選択"]
        Server["vr-server<br/>常駐コレクター・HTTP API"]
    end
    subgraph Analysis["解析層"]
        Api["RaceApi<br/>問い合わせ契約"]
        State["RaceState<br/>周回・燃費・ギャップ・イベント"]
        Render["render<br/>無線向けテキスト"]
        Api --> State
        Render --> Api
    end
    subgraph Adapter["入力アダプタ層"]
        IR["vr-iracing<br/>共有メモリ・セッション YAML"]
        Replay["vr-replay<br/>NDJSON 再生・合成レース"]
    end
    Core["vr-core<br/>Frame・TelemetrySource"]

    Frontend --> Analysis
    Frontend --> Adapter
    Analysis --> Core
    Adapter --> Core
```

- 解析層はアダプタに依存しない（`vr-engine` → `vr-replay` はテストのみ）
- アダプタは `crates/vr-cli/src/sources.rs` が `SourceFactory` として注入

### 実行時の構成

```mermaid
flowchart LR
    Game["iRacing<br/>（共有メモリ）"]
    subgraph Serve["vr serve プロセス"]
        Col["コレクタースレッド"]
        St[("RaceState<br/>RwLock")]
        Http["HTTP サーバー<br/>tokio・127.0.0.1"]
        Col -->|ingest| St
        Http -->|read| St
        Col -->|broadcast| Http
    end
    Radio["vr-radio<br/>（音声無線）"]
    Agent["Claude Code / Codex"]
    Vr["vr コマンド<br/>（短命プロセス）"]
    Disc[("server.json")]
    Rec[("記録 NDJSON")]

    Game -->|60 Hz| Col
    Col -.->|--record| Rec
    Http -.->|起動時に書込| Disc
    Radio -->|ACP stdio| Agent
    Agent -->|実行| Vr
    Vr -.->|URL 取得| Disc
    Vr -->|GET /v1/*| Http
    Vr -.->|サーバー無し| Game
```

| 実行単位           | 技術形態      | 役割                                              | 生存期間               |
| ------------------ | ------------- | ------------------------------------------------- | ---------------------- |
| `vr serve`         | 単一プロセス  | 収集・集計・API 提供                              | 走行セッション中ずっと |
| コレクタースレッド | `std::thread` | ソースの読取と `RaceState` 更新、切断時の再接続   | `vr serve` と同じ      |
| HTTP サーバー      | tokio + axum  | `RaceApi` の JSON 公開、SSE 配信                  | `vr serve` と同じ      |
| `vr <query>`       | 短命プロセス  | サーバーへの問い合わせ<br/>またはワンショット読取 | 1 コマンド             |
| `vr-radio`         | GUI プロセス  | 音声入力・エージェント呼出・読み上げ              | 利用者が閉じるまで     |

### 境界と永続化

| 境界           | 方式                                                                                  | 所有者                                           |
| -------------- | ------------------------------------------------------------------------------------- | ------------------------------------------------ |
| ゲーム → `vr`  | iRacing 共有メモリ `Local\IRSDKMemMapFileName`（読取専用）<br/>と新データ通知イベント | iRacing                                          |
| CLI → サーバー | HTTP/JSON、`127.0.0.1` 限定、既定ポート 47800                                         | `vr-server`                                      |
| イベント配信   | SSE `/v1/events/stream`（event 名 `race`）                                            | `vr-server`                                      |
| サーバー発見   | `%LOCALAPPDATA%\vibe-racing\server.json`<br/>（`VR_DISCOVERY` で上書き）              | 書いた `vr serve`。終了時に自 PID のものだけ削除 |
| 記録           | 正規化 `Frame` の NDJSON（既定 10 Hz に間引き）                                       | 利用者                                           |

`RaceState` はメモリ上のみ。`vr serve` の停止で履歴は消える。

## コードマップと依存関係

### 依存の向き

```mermaid
flowchart BT
    core[vr-core]
    iracing[vr-iracing] --> core
    replay[vr-replay] --> core
    engine[vr-engine] --> core
    server[vr-server] --> engine
    server --> replay
    server --> core
    cli[vr-cli] --> server
    cli --> engine
    cli --> iracing
    cli --> replay
    cli --> core
    radio[vr-radio]
```

- `vr-server` → `vr-replay` は記録（`Recorder`）用
- `vr-radio` は workspace の crate に依存しない。テレメトリはエージェントが `vr` で取得
- 許可する依存は [`crates/vr-cli/tests/architecture.rs`](../crates/vr-cli/tests/architecture.rs) が `cargo metadata` で検査（dev-dependencies は対象外）

### 構成要素の対応

| 層     | パス                                                                  | 責務                                                                  | 変更頻度 |
| ------ | --------------------------------------------------------------------- | --------------------------------------------------------------------- | -------- |
| 共通型 | [`crates/vr-core`](../crates/vr-core)                                 | `Frame`・`TelemetrySource`・`Capabilities`                            | 低       |
| 入力   | [`crates/vr-iracing`](../crates/vr-iracing)                           | ヘッダ・変数表の解析、YAML サニタイズ、`Frame` 変換、Win32 マッピング | 中       |
| 入力   | [`crates/vr-replay`](../crates/vr-replay)                             | NDJSON 記録・再生、ペーシング、合成レース                             | 低       |
| 解析   | [`crates/vr-engine/src/state.rs`](../crates/vr-engine/src/state.rs)   | 周回確定、燃費・ギャップ・近傍車、イベント検出                        | 高       |
| 解析   | [`crates/vr-engine/src/api.rs`](../crates/vr-engine/src/api.rs)       | `RaceApi` と引数型、`SharedState` への委譲                            | 中       |
| 解析   | [`crates/vr-engine/src/report.rs`](../crates/vr-engine/src/report.rs) | レスポンス型（JSON 契約）                                             | 低       |
| 解析   | [`crates/vr-engine/src/render.rs`](../crates/vr-engine/src/render.rs) | エージェント向けテキスト整形                                          | 中       |
| 配信   | [`crates/vr-server`](../crates/vr-server)                             | コレクター、HTTP・SSE、discovery                                      | 低       |
| CLI    | [`crates/vr-cli`](../crates/vr-cli)                                   | コマンド定義、ソース登録、バックエンド選択、`doctor`、スキル同梱      | 中       |
| スキル | [`skills/race-engineer`](../skills/race-engineer)                     | エージェントの手順と返答規範（`vr` バイナリに埋め込み）               | 中       |
| 評価   | [`crates/vr-eval`](../crates/vr-eval)                                 | 音声無線の端から端までの評価                                          | 中       |
| 音声   | [`crates/vr-radio`](../crates/vr-radio)                               | 録音・文字起こし・ACP クライアント・VOICEVOX 読み上げ・画面           | 中       |

## 設計上の不変条件

- **ゲーム非依存の境界**: ゲーム固有の値（iRacing のビットフィールド、kg/h、`-1` や `604800` の番兵値）はアダプタ内で正規化
- **問い合わせ面の一本化**: フロントエンドは `RaceApi` 経由でのみ状態を読む（`HttpClient` と `RaceState` が実装）
- **JSON 互換**: `report.rs` の型は HTTP と `-f json` の契約。フィールドの削除・意味変更は禁止
- **推定の明示**: 推定値には根拠（`FuelBasis`・`GapMode`・`history`・`age_ms`）を付け、欠損は 0 で埋めず `None`
- **共有メモリの整合性**: バッファ複製後に tick を再確認し、変化していれば破棄
- **ローカル限定**: HTTP は `127.0.0.1` のみ
- **スキルの正本**: `skills/race-engineer/`。配布は `vr skill install` のみ
- **依存の向き**: 上図以外の crate 間依存は禁止。新しい crate は規則と本文書の更新が必要

## 主要な処理フロー

### ライブ走行中の無線質問

```mermaid
sequenceDiagram
    participant G as iRacing
    participant C as コレクター
    participant S as RaceState
    participant H as HTTP
    participant V as vr brief
    participant A as エージェント
    loop 60 Hz
        G->>C: 新データ通知
        C->>C: ヘッダ → 変数表 → YAML（更新時のみ）→ バッファ複製
        C->>S: ingest(Frame)
        S->>S: ライン通過・ピット・フラッグ・順位を検出
    end
    A->>V: 実行
    V->>V: server.json から URL を取得し health を確認
    V->>H: GET /v1/brief
    H->>S: brief()
    S-->>H: Brief
    H-->>V: JSON
    V-->>A: テキスト（無線向け）
```

### 周回の確定

1. `laps_completed` の増加で保留ラップを作成（ピット・黄旗・燃料差分を確定）
2. 新しいラップタイムの公開で記録
3. タイムが変わらなければ 3 秒後に表示値で確定

レース中は自車の周回確定ごとに全車とのギャップを標本化し、推移を計算。

### サーバー不在時のフォールバック

```mermaid
sequenceDiagram
    participant V as vr brief
    participant D as server.json
    participant G as iRacing
    V->>D: URL 取得
    alt サーバー応答あり
        V->>V: HTTP で問い合わせ
    else 応答なし
        V->>G: 共有メモリを開いて 1 フレーム読取
        alt セッション中
            V->>V: 新規 RaceState（oneshot, history=false）で回答
        else 未起動・セッション外
            V-->>V: 終了コード 3
        end
    end
```

`--game` / `--file` 指定時はサーバーを使わず、ソースを最後（または `--until`）まで流して回答。

## 横断的設計トピック

### 拡張方式

- **ゲーム追加**: `TelemetrySource` を実装する crate を作り、`crates/vr-cli/src/sources.rs` の `Game` に登録。未提供の値は `None`、可否は `Capabilities` で宣言
- **MCP 化**: `RaceApi` をツールとして公開する crate を追加。`SharedState`（プロセス内）と `HttpClient`（`vr serve` 経由）のどちらでも可。テキストは `to_text()`

### 障害と再接続

- ライブソースの切断: `connected=false` にして 2 秒後に再接続
- 有限ソース（再生・合成）: 終端で停止し、最終状態を提供し続ける
- 回復不能なエラー（`Unsupported` 等）: コレクター停止
- セッション番号の変化: 周回・フラッグ・順位・自己ベストの履歴を破棄
- 停止時の SSE 接続待ち: 1 秒で打ち切り

### セッション YAML

iRacing はドライバー名等を引用符なしで出力するため、`: ` や `#` を含む値で YAML が壊れる。
自由記述キーの値を単引用符で囲み、汎用 `Value` から必要なフィールドだけを読む。
解析に失敗した更新は無視し、直前の情報を使う。

### 性能

- 書込ロックはフレームごとに短時間、問い合わせは読込ロックのみ
- 記録は既定 10 Hz に間引き（1 フレーム数 KB〜十数 KB）
- 保持上限: イベント 1000 件、周回記録は車ごとに 200 周

## 関連文書

| 文書                                                                                          | 内容                                                           |
| --------------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| [development.md](development.md)                                                              | ビルド・テスト・カバレッジ・ミューテーションテスト・記録の扱い |
| [skills/race-engineer/references/commands.md](../skills/race-engineer/references/commands.md) | コマンド・JSON フィールド・HTTP エンドポイントの仕様           |
| [skills/race-engineer/SKILL.md](../skills/race-engineer/SKILL.md)                             | エージェントの手順と返答規範                                   |
| [crates/vr-radio/docs/architecture.md](../crates/vr-radio/docs/architecture.md)               | 音声無線のパイプライン・遅延・割り込み                         |
