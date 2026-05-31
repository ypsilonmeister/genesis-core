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
//
// 公開するのは INSERT のみ。UPDATE / DELETE / DROP は生成しない (HI-3)。
// =============================================================================

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::charter_runtime::{enforce_hard_invariants, Action, Actor};

pub struct MetadataStore {
    conn: Connection,
}

impl MetadataStore {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open metadata db at {}", path))?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
            CREATE TABLE IF NOT EXISTS modifications (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                tier INTEGER NOT NULL,
                module_name TEXT NOT NULL,
                trigger_type TEXT NOT NULL,
                trigger_count INTEGER NOT NULL,
                prompt_full TEXT NOT NULL,
                model_name TEXT NOT NULL,
                generated_code TEXT,
                build_result TEXT NOT NULL,
                build_error TEXT,
                test_result TEXT,
                decision TEXT NOT NULL,
                rejection_reason TEXT,
                adopted_at TEXT,
                trigger_inputs TEXT
            );

            CREATE TABLE IF NOT EXISTS attacks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                attacker_model TEXT NOT NULL,
                inputs TEXT NOT NULL,
                phase TEXT NOT NULL,
                diversity_score REAL,
                results TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS module_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                module_name TEXT NOT NULL,
                version INTEGER NOT NULL,
                code TEXT NOT NULL,
                charter TEXT NOT NULL,
                modification_id INTEGER,
                FOREIGN KEY (modification_id) REFERENCES modifications(id)
            );

            CREATE TABLE IF NOT EXISTS comprehension_tests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                module_name TEXT NOT NULL,
                judge_model TEXT NOT NULL,
                generated_summary TEXT NOT NULL,
                charter_what TEXT NOT NULL,
                match_result TEXT NOT NULL,
                split_candidate INTEGER NOT NULL
            );
        ",
            )
            .context("Failed to initialize schema")?;
        Ok(())
    }

    pub fn insert_modification(&self, rec: &ModificationRecord) -> Result<i64> {
        enforce_hard_invariants(
            Actor::RepairAi,
            &Action::DbOperation {
                sql: "INSERT".to_string(),
            },
        )
        .map_err(|e| anyhow::anyhow!("Charter violation: {:?}", e))?;

        self.conn
            .execute(
                "INSERT INTO modifications (
                timestamp, tier, module_name, trigger_type, trigger_count,
                prompt_full, model_name, generated_code,
                build_result, build_error, test_result,
                decision, rejection_reason, adopted_at, trigger_inputs
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                params![
                    rec.timestamp,
                    rec.tier,
                    rec.module_name,
                    rec.trigger_type,
                    rec.trigger_count,
                    rec.prompt_full,
                    rec.model_name,
                    rec.generated_code,
                    rec.build_result,
                    rec.build_error,
                    rec.test_result,
                    rec.decision,
                    rec.rejection_reason,
                    rec.adopted_at,
                    rec.trigger_inputs,
                ],
            )
            .context("Failed to insert modification")?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_attack(
        &self,
        attacker_model: &str,
        inputs: &[String],
        phase: &str,
        diversity_score: Option<f64>,
        results: &serde_json::Value,
    ) -> Result<i64> {
        enforce_hard_invariants(
            Actor::RepairAi,
            &Action::DbOperation {
                sql: "INSERT".to_string(),
            },
        )
        .map_err(|e| anyhow::anyhow!("Charter violation: {:?}", e))?;

        let ts = chrono::Utc::now().to_rfc3339();
        let inputs_json = serde_json::to_string(inputs).context("Failed to serialize inputs")?;
        let results_json = results.to_string();

        self.conn.execute(
            "INSERT INTO attacks (timestamp, attacker_model, inputs, phase, diversity_score, results)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![ts, attacker_model, inputs_json, phase, diversity_score, results_json],
        ).context("Failed to insert attack")?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_comprehension_test(
        &self,
        module_name: &str,
        judge_model: &str,
        generated_summary: &str,
        charter_what: &str,
        match_result: &str,
        split_candidate: i32,
    ) -> Result<i64> {
        enforce_hard_invariants(
            Actor::RepairAi,
            &Action::DbOperation {
                sql: "INSERT".to_string(),
            },
        )
        .map_err(|e| anyhow::anyhow!("Charter violation: {:?}", e))?;

        let ts = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO comprehension_tests
             (timestamp, module_name, judge_model, generated_summary, charter_what, match_result, split_candidate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![ts, module_name, judge_model, generated_summary, charter_what, match_result, split_candidate],
        ).context("Failed to insert comprehension test")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// 指定モジュールの次のスナップショット version 番号を返す (現在の最大 + 1)。
    /// SELECT のみで破壊的操作ではないため HI-3 に抵触しない。
    pub fn next_snapshot_version(&self, module_name: &str) -> Result<i64> {
        let v: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) + 1 FROM module_snapshots WHERE module_name = ?1",
                params![module_name],
                |row| row.get(0),
            )
            .context("Failed to compute next snapshot version")?;
        Ok(v)
    }

    pub fn insert_module_snapshot(
        &self,
        module_name: &str,
        version: i64,
        code: &str,
        charter: &str,
        modification_id: Option<i64>,
    ) -> Result<()> {
        enforce_hard_invariants(
            Actor::RepairAi,
            &Action::DbOperation {
                sql: "INSERT".to_string(),
            },
        )
        .map_err(|e| anyhow::anyhow!("Charter violation: {:?}", e))?;

        let ts = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO module_snapshots (timestamp, module_name, version, code, charter, modification_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![ts, module_name, version, code, charter, modification_id],
        ).context("Failed to insert module snapshot")?;
        Ok(())
    }
}

/// `modifications` テーブルの 1 レコード
pub struct ModificationRecord {
    pub timestamp: String,
    pub tier: i32,
    pub module_name: String,
    pub trigger_type: String,
    pub trigger_count: i32,
    pub prompt_full: String,
    pub model_name: String,
    pub generated_code: Option<String>,
    pub build_result: String, // "success" | "failure"
    pub build_error: Option<String>,
    pub test_result: Option<String>, // "pass" | "fail"
    pub decision: String,            // "adopted" | "rejected"
    pub rejection_reason: Option<String>,
    pub adopted_at: Option<String>,
    /// この改変を発火させた実際の未知入力群 (JSON 配列文字列)。
    /// チャーン/収束分析のため、どの入力が何度修復対象になったかを追跡する。
    pub trigger_inputs: Option<String>,
}
