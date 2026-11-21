# 開発ガイド

構造と境界ルールは [architecture.md](architecture.md)。

## 前提環境

| ツール         | バージョン                                       | 用途                       |
| -------------- | ------------------------------------------------ | -------------------------- |
| Rust           | stable（`rust-version` 1.88 以上、edition 2024） | ビルド・テスト             |
| Node.js        | 22 以上                                          | Markdown 等の整形（oxfmt） |
| pnpm           | 12 系（`packageManager` で 12.20.0 に固定）      | 整形ツールの導入           |
| cargo-llvm-cov | 任意                                             | カバレッジ計測             |
| cargo-mutants  | 任意                                             | ミューテーションテスト     |

iRacing の実機接続は Windows のみ（他 OS では `Unsupported`）。

## ビルドと実行

```powershell
cargo build --workspace
```

```powershell
cargo run -q -p vr-cli -- brief --game synthetic --until 600
```

```powershell
cargo run -q -p vr-cli -- serve --game synthetic --speed 10
```

## 整形

```powershell
cargo fmt --all
```

```powershell
pnpm install --frozen-lockfile; pnpm fmt
```

- 対象外: `target/`、ロックファイル、`*.toml`、`*.ndjson`、`.agents/`、`.claude/`（`.oxfmtrc.json`）
- `proseWrap: preserve`（1 文 1 行を維持）

### npm 依存の取り扱い

ルートの pnpm workspace は整形ツール専用。
サプライチェーン攻撃対策として `pnpm-workspace.yaml` で次を強制。

| 設定                        | 効果                                     |
| --------------------------- | ---------------------------------------- |
| `minimumReleaseAge: 10080`  | 公開から 7 日未満の版を解決しない        |
| `allowBuilds: {}`           | 依存のインストールスクリプトを実行しない |
| `strictDepBuilds: true`     | 未許可のスクリプトを持つ依存が入ると失敗 |
| `trustPolicy: no-downgrade` | 以前の版より発行元の信頼性が低い版を拒否 |
| `blockExoticSubdeps: true`  | 推移的依存に git・tarball URL を許さない |
| `savePrefix: ""`            | 追加時に完全一致の版で固定               |

- pnpm 自体も `packageManager` で版とハッシュを固定
- 依存の追加前に公開日・インストールスクリプト・依存を確認
- GitHub Actions は `pinact run` でコミット SHA に固定

## テスト

```powershell
cargo test --workspace
```

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

CLI テストは `VR_DISCOVERY` を一時ファイルに向け、手元の `vr serve` と干渉しない。

### 品質基準

- 分岐・閾値は境界の両側を期待値で検証（`> 0` ではなく）
- 推定・欠損（`None`、`basis`、`mode`）も検証
- 不具合修正は再現テストから

## カバレッジ

```powershell
cargo llvm-cov --workspace --summary-only
```

主な未到達: Win32 マッピング（`crates/vr-iracing/src/shm.rs`）と Ctrl-C で終わる経路。

## ミューテーションテスト

```powershell
cargo mutants -p vr-engine -p vr-iracing -j 6
```

差分のみ:

```powershell
git diff main > mutants.diff; cargo mutants --in-diff mutants.diff -j 6
```

- 生存ミュータント: `mutants.out/missed.txt`
- 生存したらテストを追加。等価ミュータントと実機専用の関数は `.cargo/mutants.toml` に理由付きで除外

## 記録データ

```powershell
vr serve --record race.ndjson
```

```powershell
vr brief --file race.ndjson --until 1200
```

- 正規化 `Frame` の NDJSON、既定 10 Hz（`--record-hz`）
- ドライバー名を含むため、コミットする場合は匿名化（`fixtures/` 以外の `*.ndjson` は gitignore 済み）

## ログ

`vr serve` / `vr record` は stderr に `info` 以上。`VR_LOG`（EnvFilter 書式）で変更。

```powershell
$env:VR_LOG = "debug"; vr serve
```
