# 開発ガイド

ビルド・テスト・品質計測・記録データの扱いをまとめる。
構造と境界ルールは [architecture.md](architecture.md) を参照する。

## 前提環境

| ツール         | バージョン                                       | 用途                       |
| -------------- | ------------------------------------------------ | -------------------------- |
| Rust           | stable（`rust-version` 1.88 以上、edition 2024） | ビルド・テスト             |
| Node.js        | 22 以上                                          | Markdown 等の整形（oxfmt） |
| pnpm           | 12 系（`packageManager` で 12.20.0 に固定）      | 整形ツールの導入           |
| cargo-llvm-cov | 任意                                             | カバレッジ計測             |
| cargo-mutants  | 任意                                             | ミューテーションテスト     |

iRacing の実機接続は Windows のみ。
それ以外の OS でもビルドとテストは通り、`vr-iracing` は `Unsupported` を返す。

## ビルドと実行

```powershell
cargo build --workspace
```

```powershell
cargo run -q -p vr-cli -- brief --game synthetic --until 600
```

ゲーム無しで常駐構成を試す場合は、合成レースを等倍より速く流す。

```powershell
cargo run -q -p vr-cli -- serve --game synthetic --speed 10
```

## 整形

Rust は `cargo fmt`、Markdown・JSON・YAML は oxfmt で整形する。

```powershell
cargo fmt --all
```

```powershell
pnpm install --frozen-lockfile; pnpm fmt
```

- 対象外: `target/`、ロックファイル、`*.toml`、`*.ndjson`、`.agents/`、`.claude/`（`.oxfmtrc.json`）
- 文の途中では改行しない（`proseWrap: preserve`）。1 文 1 行の原稿をそのまま保つ
- CI は `pnpm fmt:check` で検査

### npm 依存の取り扱い

ルートの pnpm workspace は整形ツールのためだけに存在する。
サプライチェーン攻撃への備えとして、`pnpm-workspace.yaml` で次を強制している。

| 設定                        | 効果                                     |
| --------------------------- | ---------------------------------------- |
| `minimumReleaseAge: 10080`  | 公開から 7 日未満の版を解決しない        |
| `allowBuilds: {}`           | 依存のインストールスクリプトを実行しない |
| `strictDepBuilds: true`     | 未許可のスクリプトを持つ依存が入ると失敗 |
| `trustPolicy: no-downgrade` | 以前の版より発行元の信頼性が低い版を拒否 |
| `blockExoticSubdeps: true`  | 推移的依存に git・tarball URL を許さない |
| `savePrefix: ""`            | 追加時に完全一致の版で固定               |

- pnpm 自体も `packageManager` に版とハッシュを書いて固定
- 依存の追加・更新時は、版の公開日・インストールスクリプト・依存の有無を確認してから `pnpm add -D <name>@<version>`
- GitHub Actions は `pinact run` でコミット SHA に固定

## テスト

```powershell
cargo test --workspace
```

テストはゲーム無しで完結する。

| 対象            | 場所                                        | 方法                                                             |
| --------------- | ------------------------------------------- | ---------------------------------------------------------------- |
| iRacing 読取    | `crates/vr-iracing/src/*.rs` 内の単体テスト | ヘッダ・変数表・データバッファを合成したメモリイメージ           |
| セッション YAML | `crates/vr-iracing/src/session_info.rs`     | 不正なドライバー名を含むサンプル                                 |
| 解析（物語）    | `crates/vr-engine/tests/engine.rs`          | 合成レース 1 本を通した周回・燃費・ギャップ・イベント            |
| 解析（境界値）  | `crates/vr-engine/tests/scenarios.rs`       | 手組みのフレームによる練習走行・マルチクラス・時間制レース・閾値 |
| テキスト整形    | `crates/vr-engine/src/render.rs`            | 構築したレポートに対する期待文字列                               |
| 記録・再生      | `crates/vr-replay/tests/replay.rs`          | 往復、間引き、ペーシング、壊れた入力                             |
| サーバー        | `crates/vr-server/tests/http.rs`            | コレクター・HTTP 全エンドポイント・SSE・discovery                |
| CLI             | `crates/vr-cli/tests/cli.rs`                | 実バイナリを起動。`vr serve` 子プロセス経由も含む                |

CLI テストは `VR_DISCOVERY` を一時ファイルに向け、手元で動いている `vr serve` と干渉しない。

### 品質基準

- 新しい分岐や閾値には、境界の両側を値で検証するテストを書く（`> 0` ではなく期待値）
- 推定・欠損の扱い（`None`、`basis`、`mode`）も検証対象に含める
- 不具合を直すときは、先に再現テストを足す

## カバレッジ

```powershell
cargo llvm-cov --workspace --summary-only
```

CLI テストが起動する `vr` 子プロセスも計測対象に含まれる。
未到達として残るのは主に Win32 マッピングの実体（`crates/vr-iracing/src/shm.rs`）と、Ctrl-C で終わる経路である。

## ミューテーションテスト

カバレッジは「実行されたか」しか示さない。
テストが誤りを検出できるかは `cargo mutants` で確かめる。

```powershell
cargo mutants -p vr-engine -p vr-iracing -j 6
```

変更差分だけを対象にする場合:

```powershell
git diff main > mutants.diff; cargo mutants --in-diff mutants.diff -j 6
```

- 結果は `mutants.out/`（`missed.txt` が生存ミュータント）に出力
- 生存ミュータントは、検出するテストを足すか、等価ミュータントであることを確認する
- 等価ミュータントと、実機でしか検証できない関数は `.cargo/mutants.toml` の `exclude_re` に理由付きで登録

## 記録データ

実走のテレメトリを記録し、テストやデバッグで再生できる。

```powershell
vr serve --record race.ndjson
```

```powershell
vr brief --file race.ndjson --until 1200
```

- 形式は正規化 `Frame` の NDJSON（1 行 1 フレーム）
- 既定 10 Hz に間引き（`--record-hz`）。セッション切替の先頭フレームは必ず残す
- 記録にはドライバー名が含まれる。リポジトリに入れる場合は匿名化する（`.gitignore` は `fixtures/` 以外の `*.ndjson` を除外）

## ログ

`vr serve` と `vr record` は stderr に `info` 以上を出す。
`VR_LOG`（`tracing` の EnvFilter 書式）で変更できる。

```powershell
$env:VR_LOG = "debug"; vr serve
```
