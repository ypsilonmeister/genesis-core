# Lying Calculator — System Charter v1 (Layer A)

> **重要:** 本ドキュメントは「読み物としての憲章 (Reading Layer)」である。AI による直接改変は禁止。改変は人間が Layer A と Layer B (`enforcement.rs`) を同時に更新する Tier 3 プロセスでのみ行う。

---

## 1. Purpose

本システムは Cognitive Module Protocol (CMP) v0.2 の Tier 1 / Tier 2 自律修復ループを検証するための実証実験である。実用性より観察可能性を優先する。Lying Calculator という題材は「壊れやすさそのものを観察対象とする」ために選ばれた。

---

## 2. Hard Invariants

実行時に物理的にブロックされる不変条件。

1. **計算結果が数学的に正しくない場合、エラーを返す。** サイレント誤答禁止。
2. **モジュールはオーケストレータの通知なしに他モジュールと直接通信しない。** 直接 import / 直接 socket は Layer B で拒絶する。
3. **メタデータへの書き込みは追記専用。** 削除・更新を行うコードは Layer B が拒絶する。
4. **攻撃 AI は `modules/`, `orchestrator/`, `charter/` 配下のコードを読み書きしない。** ファイルシステムアクセスはサンドボックス化された一時ディレクトリのみに制限する。
5. **`charter/` 配下のファイルは AI による書き込み対象外。** Tier 3 でも人間がエディタで直接編集する。

違反は実行時に Layer B によってブロックされ、`modifications` テーブルに `decision='rejected', rejection_reason='charter_violation'` で転写される。

---

## 3. Soft Invariants

推奨される不変条件。違反する場合は記録と人間承認が必要。

1. 各モジュールのコードは 20K token 以内に収める (CMP §2.1 物理制約)。
2. 1 回の Tier 1 修復で変更する行数は元のコードの 30% 以内。
3. 修復失敗が連続 3 回の場合、人間に通知する。
4. 把握テストで不一致が連続 3 回出たモジュールは分割候補とフラグを立てる。

---

## 4. Module Addition Criteria (Tier 2 起動条件)

以下を全て満たす場合に Tier 2 を起動してよい:

1. 同一パターンの `UNKNOWN_PATTERN` が 5 回以上発生している。
2. 既存モジュール拡張案 (Tier 1) で対応できないと AI が判定している。
3. 対照群評価 (案 B: 既存モジュール拡張) と比較した結果、新モジュール案が「エラー率」で統計的優位を示している。
4. 新モジュールの Module Charter (What / Invariants / Boundaries / Extensible / Why) が AI によって自動生成されている。
5. システム全体テストが通る。

---

## 5. Module Deletion Criteria (Tier 3 起動条件)

以下のいずれかを満たすときに削除を「提案」してよい。実行は人間承認のみ。

1. 連続 72 時間呼び出されていない。
2. 依存する下流モジュールが削除済み。
3. 把握テストで「他モジュールと意味的に重複している」と複数モデルが判定した。

削除提案は §6 の Tier 3 プロトコル(四案比較)に従う。

---

## 6. Tier Boundaries

| Tier | 範囲 | 承認 |
|---|---|---|
| Tier 1 | Module Charter の Invariants を破らない範囲のコード変更 | AI 自動(全テスト通過 + Layer B 検閲通過) |
| Tier 2 | 新モジュールの追加 / `chain.toml` の変更 | AI 自動(対照群評価で優位 + システム憲章 §4 通過) |
| Tier 3 | モジュールの削除・統合・分割・境界再定義・システム憲章の変更 | 人間承認必須(四案比較を提示) |

曖昧ケースの分類規則:
- 「Invariants 違反か Extensible 拡張か」の判定が割れた場合は **常に上位 Tier に格上げ**して提案。
- Boundaries の変更を伴う改変は、たとえ表面的にはコード変更でも Tier 3。

---

## 7. Failure Mode Declarations

| 失敗パターン | フォールバック |
|---|---|
| モジュールクラッシュ | 直前バージョンに自動ロールバック |
| hot swap 失敗 | 現行バージョンを維持、`modifications` に記録 |
| 全モジュール同時障害 | 最後の既知正常スナップショット(`module_snapshots` の最新行)から復元 |
| Claude / Gemini API キー失効 | CMP ループを停止、人間に通知 |
| `cargo build` 連続失敗 5 回 | Tier 1 を 1 時間停止し、人間に通知 |
| メタデータ DB 破損 | CMP ループを停止、`archive/` のバックアップから復元を待つ |

---

## 8. External Dependencies

変更は人間承認必須。

| 依存先 | 用途 |
|---|---|
| Anthropic Claude API | Tier 1 / Tier 2 の修復案・新モジュール生成 AI |
| Google Gemini API | 攻撃 AI |
| SQLite (rusqlite) | メタデータストア |
| Rust toolchain (stable) | コンパイラ検閲 |
| `tokio` 非同期ランタイム | プロセス管理・IPC |
| Unix Domain Socket | モジュール間 IPC (Windows は将来検討) |

API キーは `.env` で管理し、`.gitignore` 済み。

---

## 9. Amendment Process

システム憲章 (本ドキュメント + `enforcement.rs`) の改訂は以下に従う:

1. AI は改訂提案のみ可能(本ドキュメントの直接編集は禁止)。
2. 人間が Layer A (`system.md`) を編集する。
3. 同じコミット内で Layer B (`enforcement.rs`) も整合的に編集する。
4. コミットメッセージに `[charter-amend]` prefix と、改訂の理由(現行憲章でシステムが死ぬ予測 vs. 改訂案で生きる予測)を記録する。
5. CMP §6.1 の通り、改訂の議論ログは `metadata.db` の `modifications` テーブルに `tier=3, module_name='charter'` として転写する。

**Layer A と Layer B の一貫性を保つ責任は人間にある。** AI はこの一貫性を保証しない。
