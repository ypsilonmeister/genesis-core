# 04 Workflow — 開発と運用のフロー

本ドキュメントは「人間 (Yuya) と AI (Claude Code / Gemini CLI) がこのリポジトリで具体的にどう動くか」を記述する。CMP の創成期 (Genesis Phase) と保守期 (Stewardship Phase) を時系列に並べたもの。

---

## 1. 創成期 (Genesis Phase) のワークフロー

### 1.1 Week 1: 骨格通電

| Day | 担当 | アウトプット |
|---|---|---|
| 1-2 | 人間 + Claude Code | Cargo workspace ビルド通過、`cargo run -p orchestrator` でプロセス4本起動 |
| 3 | Claude Code | UDS で normalizer ↔ tokenizer ↔ parser ↔ evaluator が JSON を流せる |
| 4 | Claude Code | `"3 + 5 * 2"` → `13` が end-to-end で通る |
| 5 | 人間 + Claude Code | システム憲章 Layer A レビュー、Hard Invariants 確定 |
| 6 | Claude Code | Layer B 最小実装 (`enforcement.rs`) |
| 7 | 人間 | Week 1 レビューと v1 タグ |

**Week 1 完了条件:** 攻撃 AI も CMP ループも無しで、4 モジュールが固定 chain で動くこと。

### 1.2 Week 2: Tier 1 ループ

- Claude API 呼び出しの抽象化 (`orchestrator/src/cmp_loop.rs`)
- `cargo build` をサブプロセスで実行するサンドボックス
- メタデータ転写 (`metadata.db` の `modifications` テーブル)
- hot swap (`orchestrator/src/hot_swap.rs`)
- 手動でエラーを起こして Tier 1 が回ることを確認

### 1.3 Week 3: 攻撃 AI と Tier 2 ループ

- Gemini API での攻撃 AI 実装 (`orchestrator/src/attacker.rs`)
- Phase A の攻撃のみ解放
- Tier 2 の対照群評価ロジック
- chain.toml の動的書き換え

### 1.4 Week 4: 自己検証と観察

- 把握テスト (Comprehension Test) 実装
- 統合テスト (Cohesion Test) 実装
- 30 日連続運転を開始

**v1 リリース宣言の瞬間に保守期へ移行。**

---

## 2. 保守期 (Stewardship Phase) のワークフロー

### 2.1 AI が自律的に行うこと

- Tier 1: モジュール内のバグ修正・リファクタ・パフォーマンス改善
- Tier 2: 新モジュールの追加 / chain.toml の更新

### 2.2 人間が必ず関与すること

- Tier 3 (モジュール削除・統合・分割・境界再編) の承認
- システム憲章の改訂 (Layer A + Layer B 同時更新)
- 過適合警報への応答
- 週 1 回 5 モジュール程度のサンプリングレビュー (§5.3)

### 2.3 Tier 3 提案を受け取ったときの人間の手順

1. オーケストレータから通知 (将来的に Slack や Email 経由)
2. 四案比較を読む: A=現状維持 / B=AI 提案 / C=ランダム / D=逆方向
3. シミュレーション結果(`metadata.db` のテーブル参照)を確認
4. B が A・C に対して明確な優位を示せていなければ却下
5. 承認する場合は、コミットメッセージに四案比較への参照を残す

---

## 3. AI への作業依頼テンプレート

### 3.1 Claude Code への依頼

```
タスク: {何をしてほしいか1文}
対象モジュール: {modules/normalizer/ など、または「unscoped」}
Tier: {1 / 2 / 3} — 自分で判定して、不確かなら判定根拠を述べてから着手
参照すべきドキュメント:
  - 必須: CLAUDE.md, charter/system.md, docs/02_lying_calculator.md
  - 該当モジュールの Module Charter (src/main.rs のヘッダ)
完了条件:
  - cargo build が通る
  - 既存テスト全通過
  - 個別 Charter の Invariants を破っていない
  - 変更したファイル一覧と意図を report
```

### 3.2 Gemini CLI への依頼

```
タスク: {攻撃入力の生成 / Layer B 設計レビュー / etc.}
役割: 攻撃 AI または 第三者レビュアー
参照すべきドキュメント:
  - 必須: GEMINI.md, charter/system.md, docs/02_lying_calculator.md §4
完了条件:
  - 出力は JSON 配列 (攻撃モード時) または Markdown レビュー (レビューモード時)
  - 過去の攻撃ログとの diversity score を意識する
```

---

## 4. コミットと PR の作法

- コミットメッセージは英語または日本語、prefix は `[orchestrator] / [modules/normalizer] / [docs] / [charter] / [chain]` のいずれか
- Tier 1 自動コミットは `[tier1-auto]` prefix
- Tier 2 自動コミットは `[tier2-auto]` prefix
- 人間のコミットには prefix 不要
- 改変メタデータ(`metadata.db`)は git 管理外。必要に応じてダンプを `archive/` に置く

---

## 5. ローカル開発のコマンド一覧

| 目的 | コマンド |
|---|---|
| 全体ビルド | `cargo build --workspace` |
| 全体テスト | `cargo test --workspace` |
| orchestrator 起動 | `cargo run -p orchestrator` |
| 単一モジュール起動 (デバッグ用) | `cargo run -p normalizer` |
| フォーマット | `cargo fmt --all` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| メタデータ閲覧 | `sqlite3 metadata.db ".tables"` |

---

## 6. 緊急停止 (Kill Switch)

CMP ループが暴走した場合、以下のいずれかで止める:

1. `orchestrator` プロセスを SIGTERM
2. `charter/enforcement.rs` の `EMERGENCY_HALT` フラグを true にしてリビルド
3. `chain.toml` を空配列にして次回起動を骨抜きに

復旧後、`metadata.db` の最新スナップショットから当該モジュールを元に戻す。
