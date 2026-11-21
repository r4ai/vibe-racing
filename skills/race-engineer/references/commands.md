# vr コマンドリファレンス

## 共通オプション

| オプション                                         | 内容                                                   |
| -------------------------------------------------- | ------------------------------------------------------ |
| `-f, --format text\|json`                          | 既定 `text`（簡潔テキスト）。`json` は 1 オブジェクト  |
| `--server <URL>` / `VR_SERVER`                     | 問い合わせ先の `vr serve`。既定は自動検出              |
| `--game iracing\|replay\|synthetic`                | データソースを明示（サーバーを使わずプロセス内で処理） |
| `--file <ndjson>` / `--speed <x>` / `--until <秒>` | 記録の再生、再生速度、停止するセッション時刻           |

## データの取り方

1. `--game` / `--file` 指定なし: 起動中の `vr serve`（履歴あり）
2. サーバー無し: iRacing の共有メモリを 1 回読む（`history=unavailable`）
3. `--file` / `--game synthetic`: 記録・デモレースを最後（または `--until`）まで流して回答

## コマンド

| コマンド                                                         | 内容                                                                                |
| ---------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `vr serve [--port 47800] [--record out.ndjson] [--record-hz 10]` | 常駐コレクターと `127.0.0.1` の HTTP API。ゲーム再起動時は自動再接続                |
| `vr brief`                                                       | 無線回答用サマリ（セッション、自車、前後、近くの車、燃料、直近イベント 5 件）       |
| `vr standings [--top N] [--around N] [--overall]`                | 順位表。マルチクラスでは既定でクラス内                                              |
| `vr gap [--car N] [--window 秒]`                                 | 前後・指定車との差、コース上の近くの車（`rel_s` 正 = 前）                           |
| `vr fuel`                                                        | 燃料残、1 周燃費、残り周回、ゴールまでの必要量                                      |
| `vr laps [--car N] [--last 10]`                                  | ラップ履歴。`pit` はピットレーン通過、`caution` は黄旗中（どちらも平均から除外）    |
| `vr events [--since SEQ] [--limit 30]`                           | フラッグ、順位変動、ピット、インシデント、ラップ、自己ベスト。`last_seq` で差分取得 |
| `vr watch`                                                       | イベントのストリーム（サーバーがあれば SSE）                                        |
| `vr snapshot`                                                    | 正規化テレメトリ 1 フレーム全量（JSON）                                             |
| `vr record --out x.ndjson [--hz 10]`                             | 記録のみ                                                                            |
| `vr doctor`                                                      | ゲーム接続・サーバー・スキル導入の診断                                              |
| `vr skill install [--target claude\|codex\|all] [--project]`     | スキルを Claude Code / Codex に導入                                                 |

終了コード: `0` 成功、`3` ゲーム未接続・データ無し、`4` 対象（車番など）無し、`1` その他。

## JSON の主なフィールド

共通 `meta`: `game`, `source`（`live|oneshot|replay|synthetic`）, `connected`, `age_ms`, `history`, `session_time_s`, `capabilities`。

`brief`:

- `session`: `kind`（`practice|qualify|warmup|race`）, `phase`, `track`, `lap`, `laps_total`, `laps_remaining`, `time_remaining_s`, `flags[]`
- `player`: `car{number,driver,class_name,position,class_position}`, `cars_total`, `cars_in_class`, `last_lap_s`, `best_lap_s`, `avg_lap_s`, `incidents`, `on_pit_road`
- `ahead` / `behind`: `car`, `gap_s`（正の秒）, `laps`（周回差）, `mode`（`race_time|track_progress|best_lap`）, `trend_s_per_lap`（負 = 縮小）, `their_last_lap_s`, `last_lap_delta_s`（負 = 相手が速い）, `on_pit_road`
- `nearby[]`: `car`, `rel_s`（正 = 前）, `same_class`, `lap_diff`（正 = 相手が周回で前）
- `fuel`: `fuel_l`, `capacity_l`, `per_lap_l`, `basis`（`history|instant|none`）, `laps_sampled`, `laps_of_fuel`, `laps_to_finish`, `to_finish_l`（正 = 不足、負 = 余り）, `add_with_margin_l`, `last_lap_on_fuel`
- `recent_events[]`: `seq`, `session_time_s`, `type`, `text`

## HTTP API（`vr serve`）

`GET /v1/health`, `/v1/brief`, `/v1/standings?top=&around=&class_only=`, `/v1/gaps?car=&window_s=`, `/v1/fuel`, `/v1/laps?car=&last=`, `/v1/events?since=&limit=`, `/v1/events/stream`（SSE、event 名 `race`）, `/v1/snapshot`。
レスポンスは `-f json` と同じ。
