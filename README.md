# genesis-core

> **Lying Calculator: Cognitive Module Protocol (CMP) v0.2 の実証実験**
>
> 「直すと壊れる、壊すと直る。AI が AI を直す環境を、まず壊れる側から作る。」

---

## これは何か

Genesis Core は、AI が自分でモジュール境界を引き直し、バグを修復し、未知の入力に対して新しい部品を追加していく **自律進化型ソフトウェア** の実証実験リポジトリ。題材として **意図的に壊れやすい計算機 (Lying Calculator)** を採用した。実用性ではなく観察可能性を最優先にしている。

検証する仮説は3つ:

- **Tier 1**: エラーを検出して AI が自動でモジュール内修復できるか
- **Tier 2**: 未知の入力パターンに対して AI が新モジュールを自動追加できるか
- **§5**: モジュール境界の自己検証 (把握テスト) が粒度判定に使えるか

---

## ドキュメント

| 順序 | パス | 内容 |
|---|---|---|
| 1 | `docs/02_lying_calculator.md` | **本実装の仕様。最初に読む** |
| 2 | `docs/03_architecture.md` | 物理配置と権限境界の一望 |
| 3 | `docs/04_workflow.md` | 開発と運用のフロー |
| 4 | `docs/01_cmp_v0.2.md` | CMP v0.2 原典 |
| 5 | `docs/00_genesis_core_meta.md` | Genesis Core メタ仕様 (思想層) |
| 6 | `charter/system.md` | システム憲章 Layer A (絶対遵守) |
| 7 | `charter/enforcement.rs` | システム憲章 Layer B (実行可能規則) |

AI エージェントが読むべきガイドは `CLAUDE.md` (Claude Code 向け) と `GEMINI.md` (Gemini CLI 向け) を参照。

---

## ディレクトリ構成

```
genesis-core/
├── CLAUDE.md                  # Claude Code 向け指示書
├── GEMINI.md                  # Gemini CLI 向け指示書
├── README.md                  # 本ファイル
├── Cargo.toml                 # workspace 定義
├── chain.toml                 # モジュール呼び出しチェーン (Tier 2 で AI が更新)
├── rust-toolchain.toml        # Rust バージョン固定
├── rustfmt.toml               # フォーマット設定
├── .editorconfig
├── .gitignore
├── docs/                      # 仕様書群 (本文改変は Tier 3)
│   ├── 00_genesis_core_meta.md
│   ├── 01_cmp_v0.2.md
│   ├── 02_lying_calculator.md
│   ├── 03_architecture.md
│   └── 04_workflow.md
├── charter/                   # 不可侵領域 (人間のみ編集可)
│   ├── system.md              # Layer A (自然言語)
│   ├── enforcement.rs         # Layer B (実行可能規則)
│   └── README.md
├── orchestrator/              # 不可侵領域 (CMP §8.3)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs
│   │   ├── attacker.rs        # 攻撃 AI 呼び出し (Gemini)
│   │   ├── chain.rs           # chain.toml 読み込み
│   │   ├── charter_runtime.rs # Layer B 橋渡し
│   │   ├── cmp_loop.rs        # Tier 1 / Tier 2 ループ
│   │   ├── executor.rs        # cargo/fs/hot_swap の抽象化
│   │   ├── hot_swap.rs        # 無停止プロセス入替
│   │   ├── ipc.rs             # UDS + JSON
│   │   ├── metadata.rs        # SQLite 追記
│   │   └── process.rs         # サブプロセス管理
│   └── tests/
│       └── ipc_chain_e2e.rs   # Layer 3: UDS E2E テスト (実プロセス + JSON 契約)
├── modules/                   # Cognitive Modules (Tier 1 で AI が改変)
│   ├── normalizer/
│   ├── tokenizer/
│   ├── parser/
│   └── evaluator/
├── archive/                   # 旧バイナリ退避 (gitignore)
└── .github/
    └── workflows/
        └── ci.yml             # build → test → clippy
```

---

## クイックスタート

### 前提

- Rust 1.75 以上 (`rust-toolchain.toml` で stable を固定)
- SQLite 3 (`rusqlite` の bundled 機能を使うので別途インストール不要)
- Unix Domain Socket が使える環境 (Linux / macOS / WSL2)
- Anthropic API キー (`ANTHROPIC_API_KEY`) — Week 2 以降に必要
- Google Gemini API キー (`GEMINI_API_KEY`) — Week 3 以降に必要

### ビルド

```bash
cargo build --workspace
```

### テスト

> **重要**: 統合テスト (`ipc_chain_e2e`) は実バイナリを必要とするため、
> `cargo test` の前に必ず `cargo build --workspace` を実行すること。
> CI も同じ順序で実行する (`ci.yml` 参照)。

```bash
# 1. モジュールバイナリをビルド (統合テストの前提条件)
cargo build --workspace

# 2. 全テスト実行
cargo test --workspace
```

### 起動 (Week 1 完了後)

```bash
cargo run -p orchestrator
```

---

## Tier 区分 (誰が何を変えてよいか)

| 対象 | Tier | 担当 |
|---|---|---|
| `modules/<name>/src/**` 内のコード変更 | Tier 1 | Claude Code (自動) |
| 新規 `modules/<name>/` の追加 | Tier 2 | Claude Code (対照群評価必須) |
| `chain.toml` の更新 | Tier 2 | Claude Code (自動) |
| モジュールの削除・統合・分割 | Tier 3 | 人間 (四案比較を見て承認) |
| `charter/**` の変更 | Tier 3 | 人間のみ |
| `orchestrator/**` の変更 | Tier 3 | 人間のみ |
| `docs/**` の本文改変 | Tier 3 | 人間 (提案は AI も可) |

---

## 開発フェーズ

- [x] **Week 1** — 4 モジュールが UDS で繋がり `"3 + 5 * 2"` → `13` が end-to-end で通る
- [x] **Week 2** — Tier 1 修復ループ (Claude API + cargo build + hot swap)
- [x] **Week 3** — Tier 2 ループ + 攻撃 AI Phase A 解放
- [x] **Week 4** — 把握テスト + 30 日連続運転開始
- [x] **テスト整備** — Layer 2 (Executor mock) + Layer 3 (IPC E2E) + CI

---

## 関連プロジェクト

- [Yui Protocol](https://github.com/yui-synth-lab/yui-protocol) — AI が問いを生み出し自己を形成できるかを探求する創作プロジェクト。Genesis Core はその技術探究の枝分かれ。

---

## ライセンス

MIT (予定。`LICENSE` ファイルは初回コミット時に追加する)
