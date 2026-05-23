// Week 2/3 で段階的に接続される。Tier 2 等の未使用コードは許容する。
#![allow(dead_code)]

// =============================================================================
// cmp_loop.rs — CMP Tier 1 / Tier 2 自律改変ループ
//
// Lying Calculator §5 に従う:
//
//   Tier 1 トリガ: 同一エラーコードが N 回以上発生(デフォルト 3)
//     → claude_backend.complete(repair_prompt) で修復案を取得
//     → cargo build + cargo test で検証
//     → 通過したら hot_swap
//     → 失敗もメタデータに転写(無編集)
//
//   Tier 2 トリガ: UNKNOWN_PATTERN が N 回以上(デフォルト 5)
//     → 「既存拡張で対応可能か」を AI に問う
//     → 新モジュール必要なら対照群評価を実施
//     → 優位な案のみ採用
//
// 過適合への防御は CMP §7 を参照。
// =============================================================================

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::process::Child;
use tracing::{info, warn};

use crate::ai_backend::AiBackend;
use crate::hot_swap::HotSwapper;
use crate::metadata::{MetadataStore, ModificationRecord};

pub enum RepairOutcome {
    Adopted,
    Rejected { reason: String },
    BelowThreshold,
}

pub struct RepairContext<'a> {
    pub error_code: &'a str,
    pub error_count: u32,
    pub module_name: &'a str,
    pub module_source_path: &'a str,
    pub hot_swapper: &'a HotSwapper,
    pub old_child: Child,
    pub metadata: &'a MetadataStore,
}

pub struct CmpLoop {
    claude: Box<dyn AiBackend>,
    pub tier1_trigger: u32,
    pub model_name: String,
}

impl CmpLoop {
    pub fn new(claude: Box<dyn AiBackend>) -> Self {
        let tier1_trigger = std::env::var("TIER1_TRIGGER_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        let model_name = std::env::var("CLAUDE_MODEL")
            .unwrap_or_else(|_| "claude-cli".to_string());
        Self { claude, tier1_trigger, model_name }
    }

    /// Tier 1: 同一エラーコードが閾値回数以上発生したら Claude に修復を依頼し、
    /// build → test → hot_swap → metadata 記録 まで行う。
    pub async fn maybe_repair(&self, ctx: RepairContext<'_>) -> Result<(RepairOutcome, Child)> {
        let RepairContext {
            error_code, error_count, module_name, module_source_path,
            hot_swapper, old_child, metadata,
        } = ctx;

        if error_count < self.tier1_trigger {
            return Ok((RepairOutcome::BelowThreshold, old_child));
        }

        // モジュールのソースコードと Charter を読む
        let module_code = std::fs::read_to_string(module_source_path)
            .with_context(|| format!("Cannot read {}", module_source_path))?;

        let module_charter = extract_charter(&module_code);

        let prompt = format!(
            "以下のRustモジュールがエラーを繰り返しています。\n\n\
            Module Charter:\n{}\n\n\
            エラーコード: {}\n\
            発生回数: {}\n\
            モジュール名: {}\n\n\
            現在のコード:\n```rust\n{}\n```\n\n\
            修復案を生成してください。\n\
            制約:\n\
            - Module CharterのInvariantsを破らないこと\n\
            - Module CharterのBoundariesを変更しないこと\n\
            - 修正範囲は最小限にすること\n\n\
            出力: 修正後のRustコード全体をコードブロックなしで出力してください。",
            module_charter, error_code, error_count, module_name, module_code
        );

        info!(
            module = %module_name,
            error_code = %error_code,
            count = error_count,
            "Tier 1: requesting repair from claude"
        );

        let response = self.claude.complete(&prompt).await
            .context("Claude failed to generate repair proposal")?;

        info!(
            module = %module_name,
            chars = response.len(),
            "Tier 1: received repair proposal"
        );

        // markdown コードブロックがあれば剥がす
        let generated_code = strip_code_fence(&response);

        // バックアップ → ファイル書き込み
        let backup_path = format!("{}.bak", module_source_path);
        std::fs::copy(module_source_path, &backup_path)
            .context("Failed to backup module source")?;
        std::fs::write(module_source_path, &generated_code)
            .context("Failed to write repair proposal to source file")?;

        // cargo build
        let build_output = tokio::process::Command::new("cargo")
            .args(["build", "-p", module_name])
            .output()
            .await
            .context("Failed to run cargo build")?;

        let build_ok = build_output.status.success();
        let build_error = if build_ok {
            None
        } else {
            Some(String::from_utf8_lossy(&build_output.stderr).to_string())
        };

        if !build_ok {
            warn!(module = %module_name, "Tier 1: build failed, restoring backup");
            std::fs::copy(&backup_path, module_source_path)
                .context("Failed to restore backup after build failure")?;

            let rec = ModificationRecord {
                timestamp: Utc::now().to_rfc3339(),
                tier: 1,
                module_name: module_name.to_string(),
                trigger_type: error_code.to_string(),
                trigger_count: error_count as i32,
                prompt_full: prompt,
                model_name: self.model_name.clone(),
                generated_code: Some(generated_code),
                build_result: "failure".to_string(),
                build_error,
                test_result: None,
                decision: "rejected".to_string(),
                rejection_reason: Some("build failed".to_string()),
                adopted_at: None,
            };
            metadata.insert_modification(&rec)?;

            return Ok((
                RepairOutcome::Rejected { reason: "build failed".to_string() },
                old_child,
            ));
        }

        // cargo test
        let test_output = tokio::process::Command::new("cargo")
            .args(["test", "-p", module_name])
            .output()
            .await
            .context("Failed to run cargo test")?;

        let test_ok = test_output.status.success();
        let test_result = if test_ok { "pass" } else { "fail" };

        if !test_ok {
            warn!(module = %module_name, "Tier 1: tests failed, restoring backup");
            std::fs::copy(&backup_path, module_source_path)
                .context("Failed to restore backup after test failure")?;

            let rec = ModificationRecord {
                timestamp: Utc::now().to_rfc3339(),
                tier: 1,
                module_name: module_name.to_string(),
                trigger_type: error_code.to_string(),
                trigger_count: error_count as i32,
                prompt_full: prompt,
                model_name: self.model_name.clone(),
                generated_code: Some(generated_code),
                build_result: "success".to_string(),
                build_error: None,
                test_result: Some(test_result.to_string()),
                decision: "rejected".to_string(),
                rejection_reason: Some("tests failed".to_string()),
                adopted_at: None,
            };
            metadata.insert_modification(&rec)?;

            return Ok((
                RepairOutcome::Rejected { reason: "tests failed".to_string() },
                old_child,
            ));
        }

        // hot_swap
        info!(module = %module_name, "Tier 1: build+test passed, initiating hot_swap");
        let adopted_at = Utc::now().to_rfc3339();
        let new_child = hot_swapper.swap(old_child).await
            .context("hot_swap failed")?;

        let rec = ModificationRecord {
            timestamp: Utc::now().to_rfc3339(),
            tier: 1,
            module_name: module_name.to_string(),
            trigger_type: error_code.to_string(),
            trigger_count: error_count as i32,
            prompt_full: prompt,
            model_name: self.model_name.clone(),
            generated_code: Some(generated_code),
            build_result: "success".to_string(),
            build_error: None,
            test_result: Some(test_result.to_string()),
            decision: "adopted".to_string(),
            rejection_reason: None,
            adopted_at: Some(adopted_at),
        };
        metadata.insert_modification(&rec)?;

        info!(module = %module_name, "Tier 1: repair adopted and recorded");
        Ok((RepairOutcome::Adopted, new_child))
    }

    // TODO(Week 3): Tier 2 ループ実装
}

/// ソースコード冒頭の CMP Module Charter コメントブロックを抽出する。
fn extract_charter(code: &str) -> String {
    let mut in_charter = false;
    let mut lines = Vec::new();

    for line in code.lines() {
        let trimmed = line.trim();
        if trimmed.contains("CMP Module Charter") {
            in_charter = true;
        }
        if in_charter {
            if trimmed.starts_with("//") {
                lines.push(line);
            } else if !trimmed.is_empty() {
                break;
            }
        }
    }

    if lines.is_empty() {
        "(no charter found)".to_string()
    } else {
        lines.join("\n")
    }
}

/// Claude が返す markdown コードフェンス (```rust ... ```) を剥がす。
fn strip_code_fence(s: &str) -> String {
    let s = s.trim();

    // ```rust\n...\n``` または ```\n...\n```
    if let Some(rest) = s.strip_prefix("```rust") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_string();
        }
    }
    if let Some(rest) = s.strip_prefix("```") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_string();
        }
    }

    s.to_string()
}
