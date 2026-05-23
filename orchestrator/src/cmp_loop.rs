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
use crate::charter_runtime::{enforce_hard_invariants, Action, Actor};
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
        let model_name = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "claude-cli".to_string());
        Self {
            claude,
            tier1_trigger,
            model_name,
        }
    }

    /// Tier 1: 同一エラーコードが閾値回数以上発生したら Claude に修復を依頼し、
    /// build → test → hot_swap → metadata 記録 まで行う。
    pub async fn maybe_repair(&self, ctx: RepairContext<'_>) -> Result<(RepairOutcome, Child)> {
        let RepairContext {
            error_code,
            error_count,
            module_name,
            module_source_path,
            hot_swapper,
            old_child,
            metadata,
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

        let response = self
            .claude
            .complete(&prompt)
            .await
            .context("Claude failed to generate repair proposal")?;

        info!(
            module = %module_name,
            chars = response.len(),
            "Tier 1: received repair proposal"
        );

        // markdown コードブロックがあれば剥がす
        let generated_code = strip_code_fence(&response);

        // Layer B ゲート: RepairAi によるモジュールソース書き込みを許可確認
        enforce_hard_invariants(
            Actor::RepairAi,
            &Action::FileWrite {
                path: std::path::PathBuf::from(module_source_path),
                size_bytes: generated_code.len(),
            },
        )
        .map_err(|e| anyhow::anyhow!("Charter violation before Tier1 write: {:?}", e))?;

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
                RepairOutcome::Rejected {
                    reason: "build failed".to_string(),
                },
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
                RepairOutcome::Rejected {
                    reason: "tests failed".to_string(),
                },
                old_child,
            ));
        }

        // hot_swap
        info!(module = %module_name, "Tier 1: build+test passed, initiating hot_swap");
        let adopted_at = Utc::now().to_rfc3339();
        let new_child = hot_swapper
            .swap(old_child)
            .await
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

    /// Tier 2: UNKNOWN_PATTERN が閾値回数以上発生したら Claude に判断を仰ぎ、
    /// 既存モジュール拡張 or 新モジュール追加を自律的に実行する。
    pub async fn maybe_tier2(&self, ctx: Tier2Context<'_>) -> Result<Tier2Outcome> {
        let Tier2Context {
            unknown_inputs,
            chain_module_names,
            module_source_paths,
            unknown_count,
            tier2_trigger,
            metadata,
        } = ctx;

        if unknown_count < tier2_trigger {
            return Ok(Tier2Outcome::BelowThreshold);
        }

        // --- 判定ステップ ---
        let chain_desc = chain_module_names.join(" → ");
        let examples = unknown_inputs
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let charters: Vec<String> = module_source_paths
            .iter()
            .zip(chain_module_names.iter())
            .map(|(path, name)| {
                let code = std::fs::read_to_string(path).unwrap_or_default();
                format!("[{}]\n{}", name, extract_charter(&code))
            })
            .collect();
        let charters_str = charters.join("\n\n");

        let judge_prompt = format!(
            "計算機が以下のパターンをどのモジュールでも処理できません。\n\n\
            未知パターン例: {examples}\n\
            現在のモジュールチェーン: {chain_desc}\n\n\
            各 Module Charter:\n{charters_str}\n\n\
            判定してください:\n\
            A) 既存モジュール (normalizer か tokenizer) の拡張で対応できる\n\
            B) 新しいモジュールが必要\n\n\
            回答形式: JSON のみ (説明不要)\n\
            {{\"approach\": \"extend\" | \"new\", \"target_module\": \"モジュール名\", \"reason\": \"理由\"}}"
        );

        info!("Tier 2: requesting judgment from claude");
        let judge_response = self
            .claude
            .complete(&judge_prompt)
            .await
            .context("Claude failed to respond to Tier 2 judgment")?;

        let judgment: serde_json::Value =
            parse_json_response(&judge_response).context("Failed to parse Tier 2 judgment")?;

        let approach = judgment["approach"].as_str().unwrap_or("extend");
        let target = judgment["target_module"].as_str().unwrap_or("normalizer");
        let reason = judgment["reason"].as_str().unwrap_or("").to_string();
        info!(approach, target, reason = %reason, "Tier 2: judgment received");

        if approach == "extend" {
            // --- 既存拡張パス (Tier 1 と同じフロー) ---
            let idx = chain_module_names
                .iter()
                .position(|n| n == target)
                .unwrap_or(0);
            let source_path = &module_source_paths[idx];
            let module_code = std::fs::read_to_string(source_path)
                .with_context(|| format!("Cannot read {}", source_path))?;
            let module_charter = extract_charter(&module_code);

            let repair_prompt = format!(
                "以下のモジュールを拡張して UNKNOWN_PATTERN エラーに対応してください。\n\n\
                未知パターン例: {examples}\n\n\
                Module Charter:\n{module_charter}\n\n\
                現在のコード:\n```rust\n{module_code}\n```\n\n\
                制約:\n\
                - Module CharterのInvariantsを破らないこと\n\
                - 修正範囲は最小限にすること\n\n\
                出力: 修正後のRustコード全体をコードブロックなしで出力してください。"
            );

            let repair_code = self.claude.complete(&repair_prompt).await?;
            let new_code = strip_code_fence(&repair_code);

            // Layer B ゲート: Tier 2 extend での書き込みを許可確認
            enforce_hard_invariants(
                Actor::RepairAi,
                &Action::FileWrite {
                    path: std::path::PathBuf::from(source_path.as_str()),
                    size_bytes: new_code.len(),
                },
            )
            .map_err(|e| anyhow::anyhow!("Charter violation before Tier2 extend write: {:?}", e))?;

            // バックアップ → 書き込み → build → test
            let backup = format!("{}.bak", source_path);
            std::fs::copy(source_path, &backup)?;
            std::fs::write(source_path, &new_code)?;

            let build_ok = tokio::process::Command::new("cargo")
                .args(["build", "-p", target])
                .output()
                .await?
                .status
                .success();
            let test_ok = build_ok
                && tokio::process::Command::new("cargo")
                    .args(["test", "-p", target])
                    .output()
                    .await?
                    .status
                    .success();

            if !test_ok {
                std::fs::copy(&backup, source_path)?;
                let rec = ModificationRecord {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    tier: 2,
                    module_name: target.to_string(),
                    trigger_type: "UNKNOWN_PATTERN".to_string(),
                    trigger_count: unknown_count as i32,
                    prompt_full: repair_prompt,
                    model_name: self.model_name.clone(),
                    generated_code: Some(new_code),
                    build_result: if build_ok { "success" } else { "failure" }.to_string(),
                    build_error: None,
                    test_result: Some("fail".to_string()),
                    decision: "rejected".to_string(),
                    rejection_reason: Some("build or test failed".to_string()),
                    adopted_at: None,
                };
                metadata.insert_modification(&rec)?;
                return Ok(Tier2Outcome::Rejected {
                    reason: "build or test failed".to_string(),
                });
            }

            let adopted_at = chrono::Utc::now().to_rfc3339();
            let rec = ModificationRecord {
                timestamp: chrono::Utc::now().to_rfc3339(),
                tier: 2,
                module_name: target.to_string(),
                trigger_type: "UNKNOWN_PATTERN".to_string(),
                trigger_count: unknown_count as i32,
                prompt_full: repair_prompt,
                model_name: self.model_name.clone(),
                generated_code: Some(new_code),
                build_result: "success".to_string(),
                build_error: None,
                test_result: Some("pass".to_string()),
                decision: "adopted".to_string(),
                rejection_reason: None,
                adopted_at: Some(adopted_at),
            };
            metadata.insert_modification(&rec)?;
            info!(target, "Tier 2: extension adopted");
            Ok(Tier2Outcome::Extended {
                module_name: target.to_string(),
            })
        } else {
            // --- 新モジュール追加パス ---
            let new_mod_prompt = format!(
                "新しいRustモジュールを生成してください。\n\n\
                目的: 以下の未知パターンを処理できるモジュールを追加する\n\
                未知パターン例: {examples}\n\n\
                現在のチェーン: {chain_desc}\n\n\
                要件:\n\
                - モジュールは UDS (Unix Domain Socket) 経由で JSON を受け取り返す\n\
                - 既存モジュール (modules/normalizer) の通信プロトコルと同一形式\n\
                - CMP Module Charter コメントを冒頭に書く\n\n\
                出力形式: JSON のみ (説明不要)\n\
                {{\n\
                  \"name\": \"モジュール名 (snake_case)\",\n\
                  \"insert_after\": \"チェーン内の挿入位置 (どのモジュールの後か)\",\n\
                  \"cargo_toml\": \"Cargo.toml の全文\",\n\
                  \"main_rs\": \"src/main.rs の全文\"\n\
                }}"
            );

            info!("Tier 2: requesting new module from claude");
            let new_mod_response = self.claude.complete(&new_mod_prompt).await?;
            let new_mod: serde_json::Value = parse_json_response(&new_mod_response)
                .context("Failed to parse new module spec")?;

            let mod_name = new_mod["name"]
                .as_str()
                .context("missing name")?
                .to_string();
            let insert_after = new_mod["insert_after"].as_str().map(|s| s.to_string());
            let cargo_toml = new_mod["cargo_toml"]
                .as_str()
                .context("missing cargo_toml")?
                .to_string();
            let main_rs = new_mod["main_rs"]
                .as_str()
                .context("missing main_rs")?
                .to_string();

            info!(mod_name = %mod_name, "Tier 2: creating new module");

            // ディレクトリ + ファイル作成
            let mod_dir = format!("modules/{}", mod_name);
            let src_dir = format!("{}/src", mod_dir);
            // Layer B ゲート: 新モジュールファイル書き込みを許可確認
            for write_path in [
                format!("{}/Cargo.toml", mod_dir),
                format!("{}/src/main.rs", mod_dir),
            ] {
                enforce_hard_invariants(
                    Actor::RepairAi,
                    &Action::FileWrite {
                        path: std::path::PathBuf::from(&write_path),
                        size_bytes: cargo_toml.len() + main_rs.len(),
                    },
                )
                .map_err(|e| {
                    anyhow::anyhow!("Charter violation before new module write: {:?}", e)
                })?;
            }

            std::fs::create_dir_all(&src_dir)
                .with_context(|| format!("Failed to create {}", src_dir))?;
            std::fs::write(format!("{}/Cargo.toml", mod_dir), &cargo_toml)?;
            std::fs::write(format!("{}/src/main.rs", mod_dir), &main_rs)?;

            // build + test
            let build_out = tokio::process::Command::new("cargo")
                .args(["build", "-p", &mod_name])
                .output()
                .await?;
            let build_ok = build_out.status.success();
            let build_error = if build_ok {
                None
            } else {
                Some(String::from_utf8_lossy(&build_out.stderr).to_string())
            };

            let test_ok = build_ok
                && tokio::process::Command::new("cargo")
                    .args(["test", "-p", &mod_name])
                    .output()
                    .await?
                    .status
                    .success();

            if !test_ok {
                // 失敗: ディレクトリごと削除
                let _ = std::fs::remove_dir_all(&mod_dir);
                let rec = ModificationRecord {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    tier: 2,
                    module_name: mod_name.clone(),
                    trigger_type: "UNKNOWN_PATTERN".to_string(),
                    trigger_count: unknown_count as i32,
                    prompt_full: new_mod_prompt,
                    model_name: self.model_name.clone(),
                    generated_code: Some(format!(
                        "# Cargo.toml\n{}\n\n# main.rs\n{}",
                        cargo_toml, main_rs
                    )),
                    build_result: if build_ok { "success" } else { "failure" }.to_string(),
                    build_error,
                    test_result: Some("fail".to_string()),
                    decision: "rejected".to_string(),
                    rejection_reason: Some("build or test failed".to_string()),
                    adopted_at: None,
                };
                metadata.insert_modification(&rec)?;
                return Ok(Tier2Outcome::Rejected {
                    reason: "new module build/test failed".to_string(),
                });
            }

            let socket_path = format!("/tmp/genesis-core/{}.sock", mod_name);
            let binary_path = format!("target/debug/{}", mod_name);

            let adopted_at = chrono::Utc::now().to_rfc3339();
            let rec = ModificationRecord {
                timestamp: chrono::Utc::now().to_rfc3339(),
                tier: 2,
                module_name: mod_name.clone(),
                trigger_type: "UNKNOWN_PATTERN".to_string(),
                trigger_count: unknown_count as i32,
                prompt_full: new_mod_prompt,
                model_name: self.model_name.clone(),
                generated_code: Some(format!(
                    "# Cargo.toml\n{}\n\n# main.rs\n{}",
                    cargo_toml, main_rs
                )),
                build_result: "success".to_string(),
                build_error: None,
                test_result: Some("pass".to_string()),
                decision: "adopted".to_string(),
                rejection_reason: None,
                adopted_at: Some(adopted_at),
            };
            metadata.insert_modification(&rec)?;

            info!(mod_name = %mod_name, "Tier 2: new module adopted");
            Ok(Tier2Outcome::NewModule(NewModuleSpec {
                name: mod_name,
                binary_path,
                socket_path,
                insert_after,
            }))
        }
    }

    /// §5 把握テスト: モジュールのソースを読んで Claude が要約を生成し、
    /// Charter の What と照合して comprehension_tests テーブルに記録する。
    pub async fn run_comprehension_test(
        &self,
        module_name: &str,
        source_code: &str,
        metadata: &MetadataStore,
    ) -> Result<()> {
        let charter_what = extract_charter_what(source_code);

        // Step 1: Claude にモジュールの1文要約を生成させる
        let summary_prompt = format!(
            "以下のRustモジュールを読んで、このモジュールが外部からどのように使われるか \
            (何を受け取り何を返すか) を1文で説明してください。説明のみを出力し、他の文字は不要です。\n\n\
            ```rust\n{source_code}\n```"
        );

        let generated_summary = self
            .claude
            .complete(&summary_prompt)
            .await
            .context("Claude failed to generate comprehension summary")?;
        let generated_summary = generated_summary.trim().to_string();

        // Step 2: 要約と Charter の What を比較させる
        let judge_prompt = format!(
            "以下の2つのテキストを比較してください。\n\n\
            [LLMが生成した要約]\n{generated_summary}\n\n\
            [Module Charterの What]\n{charter_what}\n\n\
            JSONのみで回答してください (説明不要):\n\
            {{\"match\": \"match\" or \"mismatch\", \"split_candidate\": 0 or 1}}\n\n\
            判定基準:\n\
            - match: 要約とCharterのWhatが意味的に一致する\n\
            - split_candidate=1: モジュールが複数の独立した責任を持ち、分割が検討されるべきなら1"
        );

        let judge_response = self
            .claude
            .complete(&judge_prompt)
            .await
            .context("Claude failed to judge comprehension")?;

        let judgment = parse_json_response(&judge_response)
            .unwrap_or_else(|_| serde_json::json!({"match": "mismatch", "split_candidate": 0}));

        let match_result = judgment["match"].as_str().unwrap_or("mismatch");
        let split_candidate = judgment["split_candidate"].as_i64().unwrap_or(0) as i32;

        info!(
            module = %module_name,
            match_result,
            split_candidate,
            "comprehension test completed"
        );
        if match_result == "mismatch" {
            warn!(
                module = %module_name,
                summary = %generated_summary,
                charter_what = %charter_what,
                "comprehension mismatch detected — module may have drifted from Charter"
            );
        }
        if split_candidate == 1 {
            warn!(
                module = %module_name,
                "split_candidate flagged — module may need to be split (Tier 3)"
            );
        }

        metadata.insert_comprehension_test(
            module_name,
            &self.model_name,
            &generated_summary,
            &charter_what,
            match_result,
            split_candidate,
        )?;

        Ok(())
    }
}

/// JSON レスポンスから値を抽出する (コードフェンスを剥がす)。
fn parse_json_response(s: &str) -> Result<serde_json::Value> {
    let s = s.trim();
    let inner = if let Some(rest) = s.strip_prefix("```json") {
        rest.trim_start_matches('\n').trim_end_matches("```").trim()
    } else if let Some(rest) = s.strip_prefix("```") {
        rest.trim_start_matches('\n').trim_end_matches("```").trim()
    } else {
        s
    };
    // { ... } を探す
    let start = inner.find('{').unwrap_or(0);
    let end = inner.rfind('}').map(|i| i + 1).unwrap_or(inner.len());
    serde_json::from_str(&inner[start..end]).context("JSON parse error")
}

pub struct Tier2Context<'a> {
    pub unknown_inputs: &'a [String],
    pub chain_module_names: &'a [String],
    pub module_source_paths: &'a [String],
    pub unknown_count: u32,
    pub tier2_trigger: u32,
    pub metadata: &'a MetadataStore,
}

pub enum Tier2Outcome {
    Extended { module_name: String },
    NewModule(NewModuleSpec),
    Rejected { reason: String },
    BelowThreshold,
}

pub struct NewModuleSpec {
    pub name: String,
    pub binary_path: String,
    pub socket_path: String,
    pub insert_after: Option<String>,
}

/// Charter コメントの What: 行を抽出する。
fn extract_charter_what(code: &str) -> String {
    for line in code.lines() {
        let trimmed = line.trim().trim_start_matches('/').trim();
        if trimmed.starts_with("What:") {
            return trimmed.trim_start_matches("What:").trim().to_string();
        }
    }
    "(no What section found)".to_string()
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
