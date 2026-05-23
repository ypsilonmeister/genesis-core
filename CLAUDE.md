# CLAUDE.md — Claude Code 向けプロジェクト指示

このリポジトリは **Genesis Core / CMP (Cognitive Module Protocol) v0.2** の実証実験 (Lying Calculator) を実装するための作業空間。Claude Code はここでは **修復AI (Repair AI)** として、Tier 1 と Tier 2 の自律改変を担当する。

> **最重要:** 本リポジトリには「触ってよい範囲」が物理的に区切られている。`charter/` と `orchestrator/` は不可侵領域。詳細は §不可侵領域を参照。

---

## このプロジェクトは何か

「壊れやすい計算機」を題材に、AI が自分でバグを修復し、未知のパターンに対応する新モジュールを追加していく実験。実用性ではなく **CMP ループが本当に機能するか** を観察対象とする。

詳細仕様は以下の順で読むこと:

1. `docs/02_lying_calculator.md` — 本実装の具体仕様 (最初に読む)
2. `docs/03_architecture.md` — 物理配置と権限境界の一望
3. `docs/01_cmp_v0.2.md` — モジュール化原理とTier制
4. `docs/00_genesis_core_meta.md` — 思想層の出典
5. `charter/system.md` — システム憲章 Layer A (絶対遵守)

---

## ビルドとテスト

| 目的 | コマンド |
|---|---|
| 全体ビルド | `cargo build --workspace` |
| 全体テスト | `cargo test --workspace` |
| orchestrator 起動 | `cargo run -p orchestrator` |
| 単一モジュール起動 | `cargo run -p normalizer` (デバッグ用) |
| フォーマット | `cargo fmt --all` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |

**コミット前に必ず:** `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

Phase 1 では Unix Domain Socket を使う。Windows ローカル開発時は WSL2 を推奨 (named pipe フォールバックは将来検討)。

---

## 不可侵領域 (Do NOT Modify)

| パス | 理由 |
|---|---|
| `charter/system.md` | システム憲章 Layer A。Tier 3 で人間のみ編集 |
| `charter/enforcement.rs` | システム憲章 Layer B。Tier 3 で人間のみ編集 |
| `charter/README.md` | 不可侵領域の説明 |
| `orchestrator/**` | CMP §8.3 の不可侵領域。人間のみ編集 |
| `docs/00_genesis_core_meta.md` | 思想層の出典。改変禁止 |
| `docs/01_cmp_v0.2.md` | 仕様層の出典。改変禁止 |
| `docs/02_lying_calculator.md` | 実証層の仕様。本文改変は Tier 3 |
| `docs/03_architecture.md` | 派生ドキュメント。本文改変は Tier 3 |
| `metadata.db` | 追記専用。`DELETE / UPDATE / DROP` を生成してはならない |

ユーザーが上記の編集を依頼してきた場合は、改変提案を Markdown で出力するに留め、実際の書き込みはユーザーに委ねること。提案理由は Layer B の `enforce_hard_invariants` に違反していないかを自分で先にチェックする。

---

## 触ってよい範囲と Tier 区分

| 対象 | Tier | 自動化レベル |
|---|---|---|
| `modules/<name>/src/**` 内のコード変更 | Tier 1 | 自動 (個別 Charter の Invariants 厳守) |
| 新規 `modules/<name>/` の追加 | Tier 2 | 自動 (対照群評価必須) |
| `chain.toml` の更新 | Tier 2 | 自動 (システム憲章 §4 Module Addition Criteria を満たすこと) |
| モジュールの削除・統合・分割 | Tier 3 | 人間承認必須 (四案比較を提示) |
| `Cargo.toml` (workspace 直下) の変更 | Tier 3 | 人間承認必須 |
| `rust-toolchain.toml` の変更 | Tier 3 | 人間承認必須 |

判断に迷ったら **常に上位 Tier に格上げ**して人間に確認。

---

## モジュール改変時の必須プロトコル

1. 該当モジュールの `src/main.rs` 冒頭にある **CMP Module Charter** をまず読む。
2. Invariants と Boundaries に違反しないかをコード生成前に自分で検証する。
3. 既存テストを壊さない。新規テストを追加する場合は、Charter の Invariants を property-based test で守る方向に揃える。
4. `cargo build` がコンパイル通過しない変更は破棄する (Rust コンパイラが Tier 1 の最初のガードレール)。
5. 生成コードは無編集でメタデータに転写されることを前提に、説明コメントを残す。
6. PR / コミットメッセージには変更したファイル一覧と Tier 判定根拠を必ず書く。

---

## 主要ライブラリと規約

- Rust edition: 2021、MSRV: 1.75
- 非同期ランタイム: `tokio`
- IPC: `tokio::net::UnixListener` / `UnixStream`
- シリアライズ: `serde` + `serde_json` (JSON 固定。仕様 §3.2 参照)
- SQLite: `rusqlite` (bundled feature 使用)
- HTTP: `reqwest` (rustls)
- エラー: モジュール内は `thiserror`、orchestrator のトップは `anyhow`
- ログ: `tracing` + `tracing-subscriber` (環境変数 `RUST_LOG`)

---

## やってはいけないこと (Negative Constraints)

- `unsafe` ブロックを書かない。Rust の物理的ガードレールを意図的に外す行為は HI-1 違反のリスクを生む。
- `panic!` / `unwrap()` / `expect()` を本番パスに残さない。`Result` を返すこと。テストでは可。
- `eprintln!` / `println!` をログとして使わない (`tracing` を使う)。
- モジュール同士で直接 socket 接続を作らない (HI-2)。必ず orchestrator を経由する。
- `metadata.db` に対して `DELETE / UPDATE / DROP / TRUNCATE / ALTER` を発行しない (HI-3)。追記のみ。
- 攻撃 AI として動作する場合 (Gemini からの委譲含む)、`modules/`, `orchestrator/`, `charter/` 配下のファイルを開かない (HI-4)。
- 仕様ドキュメント (`docs/00`, `docs/01`) の本文を勝手に変更しない。「v0.3 はこうあるべき」という提案は別ファイル (`docs/proposals/`) として出す。
- ユーザーが日本語で話しているときに英語で返さない (本プロジェクトは日本語が一次言語)。
- 絵文字を出力に使わない。ユーザーの user_preferences で明示的に禁止されている。

---

## サブエージェント活用の推奨

複雑なタスクは Task tool で分解すること:

- 仕様書全文の精読と要約 → `Explore` または `general-purpose`
- 大規模リファクタの設計レビュー → `code-reviewer` (利用可能なら)
- Layer B の安全性検証 → `general-purpose` で property-based testing シナリオ生成
- Tier 3 提案の四案生成 → 並列の `general-purpose` を 4 つ起動して独立に案を作らせる

---

## 開発フェーズの現在地

- [x] Week 1: 4 モジュールが UDS で繋がり `"3 + 5 * 2"` → `13` が通る
- [ ] Week 2: Tier 1 修復ループ (Claude API 呼び出し + hot swap)
- [ ] Week 3: Tier 2 ループ + 攻撃 AI Phase A 解放
- [ ] Week 4: 把握テスト + 30 日連続運転開始

**最初のコミットで動かすべきもの:** `"3 + 5 * 2"` → `13` が end-to-end で通ること。それだけ。攻撃 AI も CMP ループも後回し。
