# 03 Architecture — 実装アーキテクチャ概観

Genesis Core / CMP v0.2 / Lying Calculator の三本仕様を、実装レイヤに落とすときの全体像をまとめる。本ドキュメントは「どこに何があるか」と「なぜそこに置いたか」を一望するためのもので、詳細は各 SPEC を正とする。

---

## 1. 三層の意図と物理配置

| 概念層 | 出典 | 物理配置 | 改変権限 |
|---|---|---|---|
| 思想層 (Why) | Genesis Core v0.1 | `docs/00_genesis_core_meta.md` | 人間のみ |
| 仕様層 (What) | CMP v0.2 | `docs/01_cmp_v0.2.md` | 人間のみ |
| 実証層 (How) | Lying Calculator | `docs/02_lying_calculator.md` | Tier 3 (人間承認) |
| システム憲章 Layer A | CMP §2.3 | `charter/system.md` | 人間のみ |
| システム憲章 Layer B | CMP §2.3 | `charter/enforcement.rs` | 人間のみ |
| Orchestrator | CMP §8.3 | `orchestrator/` | 人間のみ |
| Cognitive Modules | CMP §2.1 | `modules/*/` | Tier 1 (AI自動) |
| Chain 定義 | Lying Calculator §5.3 | `chain.toml` | Tier 2 (AI自動) |
| メタデータ | CMP §6 | `metadata.db` (gitignore) | 追記のみ |

**鍵となる物理境界:** `charter/` と `orchestrator/` は AI 改変対象外。`modules/*/` と `chain.toml` のみが AI 自律改変の対象。これは README の「触ってよい / 触ってはいけない」表と一致する。

---

## 2. 起動からヒットまでのデータフロー

```
[攻撃AI(Gemini)]
       │ generate inputs
       ▼
[Orchestrator]
       │ read chain.toml
       ▼
input ─▶ normalizer ─▶ tokenizer ─▶ parser ─▶ evaluator ─▶ output
       │           │            │           │           │
       │ UDS+JSON  │ UDS+JSON   │ UDS+JSON  │ UDS+JSON  │
       └───────────┴────────────┴───────────┴───────────┘
                          │ all metrics
                          ▼
                  [Orchestrator]
                          │
            ┌─────────────┼──────────────┐
            ▼             ▼              ▼
      [Layer B]    [Metadata DB]   [CMP loop]
      enforcement     SQLite         (Tier 1/2)
                                        │
                                        ▼
                                 [修復AI(Claude)]
                                        │ patch
                                        ▼
                                 cargo build + test
                                        │
                                        ▼
                                    hot swap
```

通信は全て Unix Domain Socket + JSON。直接 import / 直接呼び出しは禁止。これは「Cognitive Module の物理的な独立性」を担保するための制約であり、§Hard Invariants で執行する。

---

## 3. Phase ロードマップ

```
Phase 1 (現在) : Pure Rust + プロセス分離
Phase 2        : Rust Host + Wasm Guest (wasmtime)
Phase 3        : ハードウェア最適化 (cache hit / memory bandwidth)
```

Phase 1 で固める通信プロトコル(`{request_id, input, timestamp}` / `{request_id, output, error, processing_ms}`)を Phase 2 でもそのまま使う。Phase 2 移行時に Wasm 化するのはモジュール本体のみで、Orchestrator は変えない。

---

## 4. なぜ Rust か (ADR-CMP-001 の要約)

- **物理的検閲:** AI 生成コードがメモリ安全性を破る場合、コンパイラが拒絶する。Tier 1 の最初のガードレール。
- **リソース制御:** cgroups / ulimit / Wasm fuel-based metering と相性が良い。
- **Hot swap 可能性:** プロセス分離方式でも Wasm 方式でも実現できる。

放棄した選択肢の理由(Elixir, Python, Go, C/C++)は `docs/01_cmp_v0.2.md` §8.1 を参照。

---

## 5. 「触ってよい / 触ってはいけない」の判定フロー

```
変更対象が...
├─ charter/ にある              → 改変禁止(Tier 3 で人間承認後のみ)
├─ orchestrator/ にある         → 改変禁止(同上)
├─ docs/00, 01 にある           → 改変禁止(仕様の出典)
├─ docs/02 (Lying Calculator)   → 議論・差分提案は可だが本文改変は Tier 3
├─ modules/*/src/ にある         → Tier 1 範囲(個別 Charter の Invariants 厳守)
├─ chain.toml                   → Tier 2 範囲
├─ 新 module の追加              → Tier 2 範囲(対照群評価必須)
└─ module の削除・統合・分割     → Tier 3(四案比較を人間に提示)
```

---

## 6. 観察可能性の最小単位

CMP §6.1「転写原則」に基づき、以下を **無編集で** SQLite に追記する:

- 改変時刻 / トリガ / 生成プロンプト全文 / 使用モデル名+バージョン
- 生成コード全文 / ビルド結果 / テスト結果 / Layer B 検閲結果
- 採否と理由 (Tier 3 なら四案比較結果)
- 攻撃ログ / 把握テスト結果 / モジュールスナップショット

要約や整形は禁止。「AI が次世代 AI にバトンを渡すときの一次資料」として保持する。

---

## 7. 失敗を予期しておく

CMP §9 にある6つの失敗シナリオはそのまま本プロジェクトの観察項目になる。先回りして仕様に書き込まない。実物を見てから判断する。
