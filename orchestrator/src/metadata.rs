// =============================================================================
// metadata.rs — SQLite メタデータストア
//
// CMP §6.1 「転写原則」: 全ての改変は無編集で追記する。
//                       編集・要約・削除は禁止 (Hard Invariant HI-3)。
//
// スキーマは docs/02_lying_calculator.md §6 を正とする。
//   - modifications        (全改変の転写ログ)
//   - attacks              (攻撃 AI の入力と結果)
//   - module_snapshots     (各バージョンのコードと Charter)
//   - comprehension_tests  (把握テスト結果)
// =============================================================================

// TODO(Week 2):
//   - rusqlite::Connection の wrapper
//   - スキーマ初期化 (CREATE TABLE IF NOT EXISTS ...)
//   - 追記専用ヘルパ (INSERT のみ公開)
//   - Layer B の Action::DbOperation を経由した呼び出し
