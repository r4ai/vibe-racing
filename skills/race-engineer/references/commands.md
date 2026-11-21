# vr コマンドリファレンス

全コマンド共通オプション:

- `-f, --format text|json` 既定 `text`（エージェント向けの簡潔テキスト）。`json` は 1 オブジェクト。
- `--server <URL>` / 環境変数 `VR_SERVER` 起動中の `vr serve`。未指定なら `vr serve` が書いた discovery ファイルから自動検出。
- `--game iracing|replay|synthetic`, `--file <ndjson>`, `--speed <x>`, `--until <秒>` データソースを明示（指定時はサーバーを使わずプロセス内で処理）。

データの取り方:

1. `--game`/`--file` 指定なし → 起動中の `vr serve` に問い合わせ（履歴あり）。
2. サーバー無し → iRacing の共有メモリを 1 回だけ読む（`history=unavailable`）。
3. `--game replay --file x.ndjson` / `--game synthetic` → 記録やデモレースを最後（または `--until`）まで流してから回答。

## コマンド

| コマンド                                                         | 内容                                                                                                                                      |
| ---------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `vr serve [--port 47800] [--record out.ndjson] [--record-hz 10]` | 常駐コレクター + `127.0.0.1` の HTTP API。ゲームの再起動にも自動再接続。                                                                  |
| `vr brief`                                                       | 無線回答用サマリ（セッション、自分、前後、近くの車、燃料、直近イベント 5 件）。                                                           |
| `vr standings [--top N] [--around N] [--overall]`                | 順位表。マルチクラスでは既定でクラス内。                                                                                                  |
| `vr gap [--car N] [--window 秒]`                                 | 前後との差、指定車との差、コース上で近い車（`rel_s` 正=前）。                                                                             |
| `vr fuel`                                                        | 燃料残、1 周燃費、残り周回、ゴールまでの必要量。                                                                                          |
| `vr laps [--car N] [--last 10]`                                  | ラップ履歴。`pit` はピットレーンを通った周、`caution` は黄旗中の周（平均から除外）。                                                      |
| `vr events [--since SEQ] [--limit 30]`                           | イベント（フラッグ、順位変動、ピットイン/アウト、インシデント、ラップ、自己ベスト）。`last_seq` を次の `--since` に渡すと差分だけ取れる。 |
| `vr watch`                                                       | イベントを 1 行ずつストリーム（サーバーがあれば SSE、無ければプロセス内）。                                                               |
| `vr snapshot`                                                    | 正規化テレメトリ 1 フレーム全量（JSON）。                                                                                                 |
| `vr record --out x.ndjson [--hz 10]`                             | テレメトリを記録だけする。                                                                                                                |
| `vr doctor`                                                      | ゲーム接続・サーバー・スキル導入状況の診断。                                                                                              |
| `vr skill install [--target claude                               | codex                                                                                                                                     | all] [--project]` | このスキルを Claude Code / Codex に導入。 |

終了コード: `0` 成功 / `3` ゲーム未接続・データ無し / `4` 指定対象が無い（車番など）/ `1` その他。

## JSON の主なフィールド

共通 `meta`: `game`, `source`(`live|oneshot|replay|synthetic`), `connected`, `age_ms`, `history`, `session_time_s`, `capabilities`。

`brief`:

- `session`: `kind`(`practice|qualify|warmup|race`), `phase`, `track`, `lap`, `laps_total`, `laps_remaining`, `time_remaining_s`, `flags[]`
- `player`: `car{number,driver,class_name,position,class_position}`, `cars_total`, `cars_in_class`, `last_lap_s`, `best_lap_s`, `avg_lap_s`, `incidents`, `on_pit_road`
- `ahead` / `behind`: `car`, `gap_s`（正の秒）, `laps`（周回差）, `mode`(`race_time|track_progress|best_lap`), `trend_s_per_lap`（負=差が縮小）, `their_last_lap_s`, `last_lap_delta_s`（負=相手が速い）, `on_pit_road`
- `nearby[]`: `car`, `rel_s`（正=コース上で前）, `same_class`, `lap_diff`（正=相手が周回で前）
- `fuel`: `fuel_l`, `capacity_l`, `per_lap_l`, `basis`(`history|instant|none`), `laps_sampled`, `laps_of_fuel`, `laps_to_finish`, `to_finish_l`（正=不足、負=余り）, `add_with_margin_l`, `last_lap_on_fuel`
- `recent_events[]`: `seq`, `session_time_s`, `type`, `text`

## HTTP API（`vr serve`）

`GET /v1/health`, `/v1/brief`, `/v1/standings?top=&around=&class_only=`, `/v1/gaps?car=&window_s=`, `/v1/fuel`, `/v1/laps?car=&last=`, `/v1/events?since=&limit=`, `/v1/events/stream`（SSE, event 名 `race`）, `/v1/snapshot`。レスポンスは CLI の `-f json` と同じ。
