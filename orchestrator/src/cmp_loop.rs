// Week 2/3 で段階的に接続される。Tier 2 等の未使用コードは許容する。
#![allow(dead_code)]

// =============================================================================
// cmp_loop.rs — CMP Tier 1 / Tier 2 自律改変ループ
//
// Lying Calculator §5 に従う:
//
//   Tier 1 トリガ: 同一エラーコードが N 回以上発生(デフォルト 3)
//     → repair_ai.complete(repair_prompt) で修復案を取得
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
use crate::executor::{Executor, SystemExecutor};
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
    repair_ai: Box<dyn AiBackend>,
    executor: Box<dyn Executor>,
    pub tier1_trigger: u32,
    pub model_name: String,
}

impl CmpLoop {
    pub fn new(repair_ai: Box<dyn AiBackend>) -> Self {
        Self::new_with_executor(repair_ai, Box::new(SystemExecutor))
    }

    pub fn new_with_executor(repair_ai: Box<dyn AiBackend>, executor: Box<dyn Executor>) -> Self {
        let tier1_trigger = std::env::var("TIER1_TRIGGER_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        let model_name = std::env::var("REPAIR_MODEL")
            .ok()
            .or_else(|| std::env::var("CLAUDE_MODEL").ok())
            .unwrap_or_else(|| "repair-ai".to_string());
        Self {
            repair_ai,
            executor,
            tier1_trigger,
            model_name,
        }
    }

    /// Tier 1: 同一エラーコードが閾値回数以上発生したら Claude に修復を依頼し、
    /// build → test (→ エラー時は修正ループ) → hot_swap → metadata 記録 まで行う。
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

        let module_code = self
            .executor
            .read_file(module_source_path)
            .with_context(|| format!("Cannot read {}", module_source_path))?;
        let module_charter = extract_charter(&module_code);

        let initial_prompt = format!(
            "以下のRustモジュールがエラーを繰り返しています。\n\n\
            Module Charter:\n{module_charter}\n\n\
            エラーコード: {error_code}\n\
            発生回数: {error_count}\n\
            モジュール名: {module_name}\n\n\
            現在のコード:\n```rust\n{module_code}\n```\n\n\
            修復案を生成してください。\n\
            制約:\n\
            - Module CharterのInvariantsを破らないこと\n\
            - Module CharterのBoundariesを変更しないこと\n\
            - 修正範囲は最小限にすること\n\n\
            出力: 修正後のRustコード全体を ```rust ブロックで出力してください。説明文は不要です。"
        );

        info!(module = %module_name, error_code = %error_code, count = error_count,
              "Tier 1: requesting repair from repair AI");
        let response = self.repair_ai.complete(&initial_prompt).await
            .context("Repair AI failed to generate repair proposal")?;
        info!(module = %module_name, chars = response.len(), "Tier 1: received repair proposal");

        let initial_code = strip_code_fence(&response);

        enforce_hard_invariants(
            Actor::RepairAi,
            &Action::FileWrite {
                path: std::path::PathBuf::from(module_source_path),
                size_bytes: initial_code.len(),
            },
        )
        .map_err(|e| anyhow::anyhow!("Charter violation before Tier1 write: {:?}", e))?;

        // バックアップは最初の 1 回だけ取る
        let backup_path = format!("{}.bak", module_source_path);
        self.executor.copy_file(module_source_path, &backup_path)
            .context("Failed to backup module source")?;

        // build/test ループ: 失敗時はエラー内容を渡して修正を依頼する
        let max_retries = fix_retry_count();
        let mut current_code = initial_code;
        let mut build_ok = false;
        let mut test_ok = false;
        let mut final_build_error: Option<String> = None;

        for attempt in 0..=max_retries {
            self.executor.write_file(module_source_path, &current_code)
                .context("Failed to write code")?;

            let build_result = self.executor.cargo_build(module_name).await?;
            build_ok = build_result.success;
            final_build_error = build_result.stderr.clone();

            if build_ok {
                test_ok = self.executor.cargo_test(module_name).await?;
                if test_ok {
                    break;
                }
            }

            if attempt < max_retries {
                let error_desc = if !build_ok {
                    format!("ビルドエラー:\n{}",
                        final_build_error.as_deref().unwrap_or("(不明)"))
                } else {
                    "テストが失敗しました。".to_string()
                };
                let fix_prompt = format!(
                    "以下のRustコードで{}が発生しました。修正してください。\n\n\
                    {error_desc}\n\n\
                    コード:\n```rust\n{current_code}\n```\n\n\
                    修正後のRustコード全体を ```rust ブロックで出力してください。説明文は不要です。",
                    if build_ok { "テスト失敗" } else { "ビルドエラー" }
                );
                info!(module = %module_name, attempt = attempt + 1, max = max_retries,
                      "Tier 1: requesting fix from repair AI");
                let fix_response = self.repair_ai.complete(&fix_prompt).await?;
                current_code = strip_code_fence(&fix_response);
            }
        }

        if !build_ok || !test_ok {
            warn!(module = %module_name, "Tier 1: build/test failed after all retries, restoring backup");
            self.executor.copy_file(&backup_path, module_source_path)
                .context("Failed to restore backup")?;

            let rejection_reason = if build_ok { "tests failed" } else { "build failed" };
            let rec = ModificationRecord {
                timestamp: Utc::now().to_rfc3339(),
                tier: 1,
                module_name: module_name.to_string(),
                trigger_type: error_code.to_string(),
                trigger_count: error_count as i32,
                prompt_full: initial_prompt,
                model_name: self.model_name.clone(),
                generated_code: Some(current_code),
                build_result: if build_ok { "success" } else { "failure" }.to_string(),
                build_error: final_build_error,
                test_result: if build_ok { Some("fail".to_string()) } else { None },
                decision: "rejected".to_string(),
                rejection_reason: Some(rejection_reason.to_string()),
                adopted_at: None,
            };
            metadata.insert_modification(&rec)?;
            return Ok((RepairOutcome::Rejected { reason: rejection_reason.to_string() }, old_child));
        }

        info!(module = %module_name, "Tier 1: build+test passed, initiating hot_swap");
        let adopted_at = Utc::now().to_rfc3339();
        let new_child = self.executor.hot_swap(hot_swapper, old_child).await
            .context("hot_swap failed")?;

        let rec = ModificationRecord {
            timestamp: Utc::now().to_rfc3339(),
            tier: 1,
            module_name: module_name.to_string(),
            trigger_type: error_code.to_string(),
            trigger_count: error_count as i32,
            prompt_full: initial_prompt,
            model_name: self.model_name.clone(),
            generated_code: Some(current_code),
            build_result: "success".to_string(),
            build_error: None,
            test_result: Some("pass".to_string()),
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
                let code = self.executor.read_file(path).unwrap_or_else(|e| {
                    warn!(path = %path, err = %e, "Tier 2: cannot read charter source");
                    String::new()
                });
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

        info!("Tier 2: requesting judgment from repair AI");
        let judge_response = self
            .repair_ai
            .complete(&judge_prompt)
            .await
            .context("Repair AI failed to respond to Tier 2 judgment")?;

        let judgment: serde_json::Value =
            parse_json_response(&judge_response).context("Failed to parse Tier 2 judgment")?;

        let approach = judgment["approach"].as_str().unwrap_or("extend");
        let raw_target = judgment["target_module"].as_str().unwrap_or("");
        // Claude が複数モジュール名を返すことがある ("tokenizer, parser, evaluator" など)。
        // chain_module_names の中で raw_target に含まれる最初のものを正とする。
        let target = chain_module_names
            .iter()
            .find(|n| raw_target == n.as_str() || raw_target.contains(n.as_str()))
            .map(|n| n.as_str())
            .unwrap_or_else(|| chain_module_names.first().map(|s| s.as_str()).unwrap_or("normalizer"));
        let reason = judgment["reason"].as_str().unwrap_or("").to_string();
        info!(approach, target, reason = %reason, "Tier 2: judgment received");

        if approach == "extend" {
            // --- 既存拡張パス (Tier 1 と同じフロー) ---
            let idx = chain_module_names
                .iter()
                .position(|n| n == target)
                .unwrap_or(0);
            let source_path = &module_source_paths[idx];
            let module_code = self
                .executor
                .read_file(source_path)
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
                出力: 修正後のRustコード全体を ```rust ブロックで出力してください。説明文は不要です。"
            );

            let repair_code = self.repair_ai.complete(&repair_prompt).await?;
            let initial_code = strip_code_fence(&repair_code);

            // Layer B ゲート: Tier 2 extend での書き込みを許可確認
            enforce_hard_invariants(
                Actor::RepairAi,
                &Action::FileWrite {
                    path: std::path::PathBuf::from(source_path.as_str()),
                    size_bytes: initial_code.len(),
                },
            )
            .map_err(|e| anyhow::anyhow!("Charter violation before Tier2 extend write: {:?}", e))?;

            // バックアップは最初の 1 回だけ取る
            let backup = format!("{}.bak", source_path);
            self.executor.copy_file(source_path, &backup)?;

            // build/test ループ: 失敗時はエラー内容を渡して修正を依頼する
            let max_retries = fix_retry_count();
            let mut current_code = initial_code;
            let mut build_ok = false;
            let mut test_ok = false;
            let mut final_build_error: Option<String> = None;

            for attempt in 0..=max_retries {
                self.executor.write_file(source_path, &current_code)?;

                let build_result = self.executor.cargo_build(target).await?;
                build_ok = build_result.success;
                final_build_error = build_result.stderr.clone();

                if build_ok {
                    test_ok = self.executor.cargo_test(target).await?;
                    if test_ok {
                        break;
                    }
                }

                if attempt < max_retries {
                    let error_desc = if !build_ok {
                        format!("ビルドエラー:\n{}",
                            final_build_error.as_deref().unwrap_or("(不明)"))
                    } else {
                        "テストが失敗しました。".to_string()
                    };
                    let fix_prompt = format!(
                        "以下のRustコードで{}が発生しました。修正してください。\n\n\
                        {error_desc}\n\n\
                        コード:\n```rust\n{current_code}\n```\n\n\
                        修正後のRustコード全体を ```rust ブロックで出力してください。説明文は不要です。",
                        if build_ok { "テスト失敗" } else { "ビルドエラー" }
                    );
                    info!(target, attempt = attempt + 1, max = max_retries,
                          "Tier 2: requesting fix from repair AI");
                    let fix_response = self.repair_ai.complete(&fix_prompt).await?;
                    current_code = strip_code_fence(&fix_response);
                }
            }

            if !build_ok || !test_ok {
                self.executor.copy_file(&backup, source_path)?;
                let rejection_reason = if build_ok { "tests failed" } else { "build failed" };
                let rec = ModificationRecord {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    tier: 2,
                    module_name: target.to_string(),
                    trigger_type: "UNKNOWN_PATTERN".to_string(),
                    trigger_count: unknown_count as i32,
                    prompt_full: repair_prompt,
                    model_name: self.model_name.clone(),
                    generated_code: Some(current_code),
                    build_result: if build_ok { "success" } else { "failure" }.to_string(),
                    build_error: final_build_error,
                    test_result: if build_ok { Some("fail".to_string()) } else { None },
                    decision: "rejected".to_string(),
                    rejection_reason: Some(rejection_reason.to_string()),
                    adopted_at: None,
                };
                metadata.insert_modification(&rec)?;
                return Ok(Tier2Outcome::Rejected {
                    reason: rejection_reason.to_string(),
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
                generated_code: Some(current_code),
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
                - CMP Module Charter コメントを冒頭に書く\n\
                - 出力サイズ制限（トークン切れ）を防ぐため、コードおよび Cargo.toml は極めて簡潔かつ最小限に実装してください。\n\
                - Cargo.toml の dependencies は、必要最小限の依存関係 (tokio, serde, serde_json, thiserror, anyhow, tracing) のみに限定し、不要なパッケージを含めないでください。\n\n\
                出力形式: JSON のみ (説明不要)\n\
                重要: \"cargo_toml\" や \"main_rs\" などの複数行のソースコードを JSON 内に埋め込む際、\n\
                改行は \\n に、ダブルクォーテーション「\"」は \\\" に、バックスラッシュ「\\」は \\\\ に、\n\
                規格に従って正しくエスケープしてください。有効な JSON 文字列を出力してください。\n\
                {{\n\
                  \"name\": \"モジュール名 (snake_case)\",\n\
                  \"insert_after\": \"チェーン内の挿入位置 (どのモジュールの後か)\",\n\
                  \"cargo_toml\": \"Cargo.toml の全文\",\n\
                  \"main_rs\": \"src/main.rs の全文\"\n\
                }}"
            );

            let mut attempts = 0;
            let mut new_mod: Option<serde_json::Value> = None;
            let mut current_prompt = new_mod_prompt.clone();

            while attempts < 2 {
                info!(
                    attempt = attempts + 1,
                    "Tier 2: requesting new module from repair AI"
                );
                let new_mod_response = self.repair_ai.complete(&current_prompt).await?;
                match parse_json_response(&new_mod_response) {
                    Ok(parsed) => {
                        new_mod = Some(parsed);
                        break;
                    }
                    Err(e) => {
                        warn!(
                            attempt = attempts + 1,
                            error = %e,
                            "Failed to parse new module JSON"
                        );
                        attempts += 1;
                        if attempts < 2 {
                            current_prompt = format!(
                                "{}\n\n前回の出力は以下のパースエラーになりました:\n{}\n\n\
                                上記のエラーを修正し、正しくエスケープされた有効な JSON のみを出力してください。",
                                new_mod_prompt,
                                e
                            );
                        }
                    }
                }
            }

            let new_mod = new_mod.context("Failed to parse new module spec after retries")?;

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

            self.executor
                .create_dir_all(&src_dir)
                .with_context(|| format!("Failed to create {}", src_dir))?;
            self.executor
                .write_file(&format!("{}/Cargo.toml", mod_dir), &cargo_toml)?;
            self.executor
                .write_file(&format!("{}/src/main.rs", mod_dir), &main_rs)?;

            // build + test
            let build_out = self.executor.cargo_build(&mod_name).await?;
            let build_ok = build_out.success;
            let build_error = build_out.stderr;

            let test_ok = build_ok && self.executor.cargo_test(&mod_name).await?;

            if !test_ok {
                // 失敗: ディレクトリごと削除
                let _ = self.executor.remove_dir_all(&mod_dir);
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
            .repair_ai
            .complete(&summary_prompt)
            .await
            .context("Repair AI failed to generate comprehension summary")?;
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
            .repair_ai
            .complete(&judge_prompt)
            .await
            .context("Repair AI failed to judge comprehension")?;

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

/// ビルド/テスト失敗時に 修復 AI に修正を依頼する最大追加試行回数。
/// FIX_RETRY_COUNT 環境変数で上書き可能 (デフォルト 2)。
fn fix_retry_count() -> u32 {
    std::env::var("FIX_RETRY_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
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
    let target = &inner[start..end];
    serde_json::from_str(target).map_err(|e| {
        anyhow::anyhow!(
            "JSON parse error: {}\n--- Raw string attempted to parse ---\n{}\n-------------------------------------",
            e,
            target
        )
    })
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

/// Charter コメントの What: セクションを抽出する (複数行対応)。
fn extract_charter_what(code: &str) -> String {
    let mut lines_iter = code.lines().peekable();
    while let Some(line) = lines_iter.next() {
        let trimmed = line.trim().trim_start_matches('/').trim();
        if trimmed.starts_with("What:") {
            let inline = trimmed.trim_start_matches("What:").trim();
            if !inline.is_empty() {
                return inline.to_string();
            }
            // What: の内容が次行以降にある場合、インデントされたコメント行を収集する
            let mut parts = Vec::new();
            while let Some(next) = lines_iter.peek() {
                let nc = next.trim().trim_start_matches('/').trim();
                // 次のセクション見出し (例: "Invariants:") が来たら終了
                if nc.ends_with(':') && !nc.starts_with('-') {
                    break;
                }
                if nc.is_empty() {
                    break;
                }
                parts.push(nc.to_string());
                lines_iter.next();
            }
            if !parts.is_empty() {
                return parts.join(" ");
            }
            return "(no What content)".to_string();
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

/// LLM が返す markdown コードフェンス (```rust ... ```) を剥がす。
fn strip_code_fence(s: &str) -> String {
    let s = s.trim();

    // 1. コードフェンス (```rust / ```) がレスポンス中にあれば、そこから抽出する。
    //    日本語の説明文がコードの前に付いていても正しく抽出できる。
    for fence in ["```rust", "```"] {
        if let Some(start) = s.find(fence) {
            let after_open = &s[start + fence.len()..];
            let code_start = after_open.trim_start_matches('\n');
            if let Some(end) = code_start.find("```") {
                return code_start[..end].trim().to_string();
            }
        }
    }

    // 2. コードフェンスがない場合: Rust コードらしい行頭を探してそこ以降を返す。
    //    Claude がコードブロックなしで説明文 + コードを出力した場合のフォールバック。
    const RUST_MARKERS: &[&str] = &[
        "// # CMP", "// CMP", "use ", "pub use ", "pub fn ", "pub struct ",
        "pub enum ", "fn main", "#[derive", "#![", "mod ",
    ];
    for marker in RUST_MARKERS {
        // 行頭にあるか確認 (pos==0 または直前が改行)
        let mut search_from = 0;
        while let Some(pos) = s[search_from..].find(marker) {
            let abs_pos = search_from + pos;
            if abs_pos == 0 || s.as_bytes().get(abs_pos - 1) == Some(&b'\n') {
                return s[abs_pos..].trim().to_string();
            }
            search_from = abs_pos + 1;
            if search_from >= s.len() {
                break;
            }
        }
    }

    s.to_string()
}

// =============================================================================
// Layer 2: Tier 1 修復ループの模擬テスト
//
// FakeExecutor で cargo/fs/hot_swap を差し替え、実プロセス・実 cargo 不使用で
// maybe_repair の条件分岐 (Adopted / Rejected / BelowThreshold) を検証する。
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    // --- Fake AI ---

    struct FakeAi {
        response: String,
    }

    #[async_trait]
    impl AiBackend for FakeAi {
        async fn complete(&self, _prompt: &str) -> Result<String> {
            Ok(self.response.clone())
        }
    }

    // --- Fake Executor ---

    struct FakeExecutor {
        build_ok: bool,
        test_ok: bool,
        source: String,
        /// 書き込まれた (path, content) のリスト
        writes: Arc<Mutex<Vec<(String, String)>>>,
    }

    #[async_trait]
    impl Executor for FakeExecutor {
        fn read_file(&self, _path: &str) -> Result<String> {
            Ok(self.source.clone())
        }
        fn write_file(&self, path: &str, content: &str) -> Result<()> {
            self.writes
                .lock()
                .unwrap()
                .push((path.to_string(), content.to_string()));
            Ok(())
        }
        fn copy_file(&self, _src: &str, _dst: &str) -> Result<()> {
            Ok(())
        }
        fn create_dir_all(&self, _path: &str) -> Result<()> {
            Ok(())
        }
        fn remove_dir_all(&self, _path: &str) -> Result<()> {
            Ok(())
        }
        async fn cargo_build(&self, _pkg: &str) -> Result<crate::executor::BuildResult> {
            Ok(crate::executor::BuildResult {
                success: self.build_ok,
                stderr: None,
            })
        }
        async fn cargo_test(&self, _pkg: &str) -> Result<bool> {
            Ok(self.test_ok)
        }
        async fn hot_swap(&self, _swapper: &HotSwapper, mut old: Child) -> Result<Child> {
            let _ = old.kill().await;
            let child = tokio::process::Command::new("sleep")
                .arg("9999")
                .spawn()
                .expect("sleep must exist");
            Ok(child)
        }
    }

    fn fake_swapper() -> HotSwapper {
        HotSwapper::new("test_module", "/dev/null", "/tmp/fake_test_cmp.sock")
    }

    async fn dummy_child() -> Child {
        tokio::process::Command::new("sleep")
            .arg("9999")
            .spawn()
            .expect("sleep must exist on this platform")
    }

    fn make_metadata() -> MetadataStore {
        MetadataStore::open(":memory:").unwrap()
    }

    const FAKE_SRC: &str = "// # CMP Module Charter\n// What: テスト用モジュール\nfn main() {}\n";

    // tier1_trigger は env から読む。テスト中は環境変数を設定しないので デフォルト 3 になる。
    // error_count >= 3 → Tier 1 発動。

    #[tokio::test]
    async fn tier1_adopted_when_build_and_test_pass() {
        let writes = Arc::new(Mutex::new(vec![]));
        let cmp = CmpLoop::new_with_executor(
            Box::new(FakeAi {
                response: "fn repaired() {}".to_string(),
            }),
            Box::new(FakeExecutor {
                build_ok: true,
                test_ok: true,
                source: FAKE_SRC.to_string(),
                writes: Arc::clone(&writes),
            }),
        );
        let metadata = make_metadata();
        let old_child = dummy_child().await;

        let (outcome, mut new_child) = cmp
            .maybe_repair(RepairContext {
                error_code: "PARSE_ERROR",
                error_count: 99,
                module_name: "normalizer",
                module_source_path: "/fake/src/main.rs",
                hot_swapper: &fake_swapper(),
                old_child,
                metadata: &metadata,
            })
            .await
            .unwrap();

        let _ = new_child.kill().await;
        assert!(
            matches!(outcome, RepairOutcome::Adopted),
            "build+test pass → Adopted"
        );
        // AI の提案コードが書き込まれた
        let w = writes.lock().unwrap();
        assert!(
            w.iter().any(|(_, c)| c == "fn repaired() {}"),
            "repair code should be written; got: {:?}",
            w
        );
    }

    #[tokio::test]
    async fn tier1_rejected_when_build_fails() {
        let cmp = CmpLoop::new_with_executor(
            Box::new(FakeAi {
                response: "broken".to_string(),
            }),
            Box::new(FakeExecutor {
                build_ok: false,
                test_ok: true,
                source: FAKE_SRC.to_string(),
                writes: Arc::new(Mutex::new(vec![])),
            }),
        );
        let metadata = make_metadata();
        let old_child = dummy_child().await;

        let (outcome, mut returned_child) = cmp
            .maybe_repair(RepairContext {
                error_code: "BUILD_ERROR",
                error_count: 5,
                module_name: "tokenizer",
                module_source_path: "/fake/src/main.rs",
                hot_swapper: &fake_swapper(),
                old_child,
                metadata: &metadata,
            })
            .await
            .unwrap();

        let _ = returned_child.kill().await;
        assert!(
            matches!(outcome, RepairOutcome::Rejected { .. }),
            "build fail → Rejected"
        );
    }

    #[tokio::test]
    async fn tier1_rejected_when_test_fails() {
        let cmp = CmpLoop::new_with_executor(
            Box::new(FakeAi {
                response: "fn ok() {}".to_string(),
            }),
            Box::new(FakeExecutor {
                build_ok: true,
                test_ok: false,
                source: FAKE_SRC.to_string(),
                writes: Arc::new(Mutex::new(vec![])),
            }),
        );
        let metadata = make_metadata();
        let old_child = dummy_child().await;

        let (outcome, mut returned_child) = cmp
            .maybe_repair(RepairContext {
                error_code: "PARSE_ERROR",
                error_count: 3,
                module_name: "parser",
                module_source_path: "/fake/src/main.rs",
                hot_swapper: &fake_swapper(),
                old_child,
                metadata: &metadata,
            })
            .await
            .unwrap();

        let _ = returned_child.kill().await;
        assert!(
            matches!(outcome, RepairOutcome::Rejected { .. }),
            "test fail → Rejected"
        );
    }

    #[tokio::test]
    async fn tier1_below_threshold_returns_old_child() {
        let cmp = CmpLoop::new_with_executor(
            Box::new(FakeAi {
                response: String::new(),
            }),
            Box::new(FakeExecutor {
                build_ok: true,
                test_ok: true,
                source: FAKE_SRC.to_string(),
                writes: Arc::new(Mutex::new(vec![])),
            }),
        );
        let metadata = make_metadata();
        let old_child = dummy_child().await;

        // tier1_trigger = 3 (デフォルト), count = 2 → BelowThreshold
        let (outcome, mut returned_child) = cmp
            .maybe_repair(RepairContext {
                error_code: "PARSE_ERROR",
                error_count: 2,
                module_name: "normalizer",
                module_source_path: "/fake/src/main.rs",
                hot_swapper: &fake_swapper(),
                old_child,
                metadata: &metadata,
            })
            .await
            .unwrap();

        let _ = returned_child.kill().await;
        assert!(
            matches!(outcome, RepairOutcome::BelowThreshold),
            "count < threshold → BelowThreshold"
        );
    }

    // =========================================================================
    // Layer 4: Tier 2 モックテスト
    //
    // SequencedAi (事前キューから順番に応答) + TrackingExecutor (Op 記録) で
    // maybe_tier2 の条件分岐をすべて副作用なしに検証する。
    // =========================================================================

    use std::collections::VecDeque;

    // --- Sequenced AI: 事前に積んだ応答を順番に返す ---

    struct SequencedAi {
        responses: Arc<Mutex<VecDeque<String>>>,
    }

    impl SequencedAi {
        fn new(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses.into_iter().map(Into::into).collect())),
            }
        }
    }

    #[async_trait]
    impl AiBackend for SequencedAi {
        async fn complete(&self, _prompt: &str) -> Result<String> {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("SequencedAi: no more responses queued"))
        }
    }

    // --- Op: TrackingExecutor が記録する副作用の種別 ---

    #[derive(Debug, Clone, PartialEq)]
    enum Op {
        WriteFile(String),
        CopyFile(String, String),
        CreateDirAll(String),
        RemoveDirAll(String),
        CargoBuild(String),
        CargoTest(String),
    }

    // --- Tracking Executor: すべての副作用を Op として記録する ---

    struct TrackingExecutor {
        build_ok: bool,
        test_ok: bool,
        source: String,
        ops: Arc<Mutex<Vec<Op>>>,
    }

    #[async_trait]
    impl Executor for TrackingExecutor {
        fn read_file(&self, _path: &str) -> Result<String> {
            Ok(self.source.clone())
        }
        fn write_file(&self, path: &str, _content: &str) -> Result<()> {
            self.ops
                .lock()
                .unwrap()
                .push(Op::WriteFile(path.to_string()));
            Ok(())
        }
        fn copy_file(&self, src: &str, dst: &str) -> Result<()> {
            self.ops
                .lock()
                .unwrap()
                .push(Op::CopyFile(src.to_string(), dst.to_string()));
            Ok(())
        }
        fn create_dir_all(&self, path: &str) -> Result<()> {
            self.ops
                .lock()
                .unwrap()
                .push(Op::CreateDirAll(path.to_string()));
            Ok(())
        }
        fn remove_dir_all(&self, path: &str) -> Result<()> {
            self.ops
                .lock()
                .unwrap()
                .push(Op::RemoveDirAll(path.to_string()));
            Ok(())
        }
        async fn cargo_build(&self, pkg: &str) -> Result<crate::executor::BuildResult> {
            self.ops
                .lock()
                .unwrap()
                .push(Op::CargoBuild(pkg.to_string()));
            Ok(crate::executor::BuildResult {
                success: self.build_ok,
                stderr: if self.build_ok {
                    None
                } else {
                    Some("fake build error".to_string())
                },
            })
        }
        async fn cargo_test(&self, pkg: &str) -> Result<bool> {
            self.ops
                .lock()
                .unwrap()
                .push(Op::CargoTest(pkg.to_string()));
            Ok(self.test_ok)
        }
        async fn hot_swap(&self, _swapper: &HotSwapper, mut old: Child) -> Result<Child> {
            let _ = old.kill().await;
            let child = tokio::process::Command::new("sleep")
                .arg("9999")
                .spawn()
                .expect("sleep must exist");
            Ok(child)
        }
    }

    // Tier 2 テスト共通の入力データ
    fn tier2_modules() -> (Vec<String>, Vec<String>) {
        let names = vec![
            "normalizer".to_string(),
            "tokenizer".to_string(),
            "parser".to_string(),
            "evaluator".to_string(),
        ];
        let paths = vec![
            "modules/normalizer/src/main.rs".to_string(),
            "modules/tokenizer/src/main.rs".to_string(),
            "modules/parser/src/main.rs".to_string(),
            "modules/evaluator/src/main.rs".to_string(),
        ];
        (names, paths)
    }

    const JUDGE_EXTEND: &str =
        r#"{"approach": "extend", "target_module": "normalizer", "reason": "既存で対応可能"}"#;
    const JUDGE_NEW: &str =
        r#"{"approach": "new", "target_module": "", "reason": "新規モジュール必要"}"#;
    const NEW_MOD_SPEC: &str = r#"{
        "name": "unicode_norm",
        "insert_after": "normalizer",
        "cargo_toml": "[package]\nname = \"unicode_norm\"\nversion = \"0.1.0\"\nedition = \"2021\"",
        "main_rs": "// # CMP Module Charter\n// What: Unicode 正規化モジュール\nfn main() {}"
    }"#;

    /// extend 成功: build + test が通れば Extended を返し、CopyFile/WriteFile/Build/Test が走る
    #[tokio::test]
    async fn tier2_extend_adopted_when_build_and_test_pass() {
        let ops = Arc::new(Mutex::new(vec![]));
        let (module_names, module_paths) = tier2_modules();
        let inputs = vec!["３ + ５".to_string()];
        let metadata = make_metadata();

        let cmp = CmpLoop::new_with_executor(
            Box::new(SequencedAi::new([JUDGE_EXTEND, "fn repaired() {}"])),
            Box::new(TrackingExecutor {
                build_ok: true,
                test_ok: true,
                source: FAKE_SRC.to_string(),
                ops: Arc::clone(&ops),
            }),
        );

        let outcome = cmp
            .maybe_tier2(Tier2Context {
                unknown_inputs: &inputs,
                chain_module_names: &module_names,
                module_source_paths: &module_paths,
                unknown_count: 5,
                tier2_trigger: 5,
                metadata: &metadata,
            })
            .await
            .unwrap();

        assert!(
            matches!(outcome, Tier2Outcome::Extended { ref module_name } if module_name == "normalizer"),
            "build+test pass → Extended(normalizer)"
        );
        let recorded = ops.lock().unwrap().clone();
        assert!(
            recorded.iter().any(|o| matches!(o, Op::CopyFile(..))),
            "backup CopyFile must be recorded; ops={:?}",
            recorded
        );
        assert!(
            recorded.iter().any(|o| matches!(o, Op::WriteFile(..))),
            "WriteFile must be recorded; ops={:?}",
            recorded
        );
        assert!(
            recorded.contains(&Op::CargoBuild("normalizer".to_string())),
            "CargoBuild(normalizer) must be recorded; ops={:?}",
            recorded
        );
        assert!(
            recorded.contains(&Op::CargoTest("normalizer".to_string())),
            "CargoTest(normalizer) must be recorded; ops={:?}",
            recorded
        );
    }

    /// extend 失敗 (build): Rejected を返し、バックアップからの CopyFile (ロールバック) が走る
    #[tokio::test]
    async fn tier2_extend_rejected_when_build_fails() {
        let ops = Arc::new(Mutex::new(vec![]));
        let (module_names, module_paths) = tier2_modules();
        let inputs = vec!["３ + ５".to_string()];
        let metadata = make_metadata();

        let cmp = CmpLoop::new_with_executor(
            // 修正ループ (FIX_RETRY_COUNT=2) で合計 2 回 fix 呼び出しが発生するため
            // JUDGE + initial + fix×2 = 4 レスポンスを用意する
            Box::new(SequencedAi::new([
                JUDGE_EXTEND, "broken code", "still broken 1", "still broken 2",
            ])),
            Box::new(TrackingExecutor {
                build_ok: false,
                test_ok: true,
                source: FAKE_SRC.to_string(),
                ops: Arc::clone(&ops),
            }),
        );

        let outcome = cmp
            .maybe_tier2(Tier2Context {
                unknown_inputs: &inputs,
                chain_module_names: &module_names,
                module_source_paths: &module_paths,
                unknown_count: 5,
                tier2_trigger: 5,
                metadata: &metadata,
            })
            .await
            .unwrap();

        assert!(
            matches!(outcome, Tier2Outcome::Rejected { .. }),
            "build fail → Rejected"
        );
        let recorded = ops.lock().unwrap().clone();
        let copy_count = recorded
            .iter()
            .filter(|o| matches!(o, Op::CopyFile(..)))
            .count();
        assert!(
            copy_count >= 2,
            "backup + rollback CopyFile must both be recorded; ops={:?}",
            recorded
        );
        assert!(
            !recorded.contains(&Op::CargoTest("normalizer".to_string())),
            "CargoTest must NOT be called when build fails; ops={:?}",
            recorded
        );
    }

    /// extend 失敗 (test): Rejected を返し、ロールバックと CargoTest が両方記録される
    #[tokio::test]
    async fn tier2_extend_rejected_when_test_fails() {
        let ops = Arc::new(Mutex::new(vec![]));
        let (module_names, module_paths) = tier2_modules();
        let inputs = vec!["３ + ５".to_string()];
        let metadata = make_metadata();

        let cmp = CmpLoop::new_with_executor(
            // 修正ループ (FIX_RETRY_COUNT=2) で合計 2 回 fix 呼び出しが発生するため
            // JUDGE + initial + fix×2 = 4 レスポンスを用意する
            Box::new(SequencedAi::new([
                JUDGE_EXTEND, "fn ok() {}", "fn ok() {} // fix1", "fn ok() {} // fix2",
            ])),
            Box::new(TrackingExecutor {
                build_ok: true,
                test_ok: false,
                source: FAKE_SRC.to_string(),
                ops: Arc::clone(&ops),
            }),
        );

        let outcome = cmp
            .maybe_tier2(Tier2Context {
                unknown_inputs: &inputs,
                chain_module_names: &module_names,
                module_source_paths: &module_paths,
                unknown_count: 5,
                tier2_trigger: 5,
                metadata: &metadata,
            })
            .await
            .unwrap();

        assert!(
            matches!(outcome, Tier2Outcome::Rejected { .. }),
            "test fail → Rejected"
        );
        let recorded = ops.lock().unwrap().clone();
        assert!(
            recorded.contains(&Op::CargoBuild("normalizer".to_string())),
            "CargoBuild must be recorded; ops={:?}",
            recorded
        );
        assert!(
            recorded.contains(&Op::CargoTest("normalizer".to_string())),
            "CargoTest must be recorded even on failure; ops={:?}",
            recorded
        );
        let copy_count = recorded
            .iter()
            .filter(|o| matches!(o, Op::CopyFile(..)))
            .count();
        assert!(
            copy_count >= 2,
            "backup + rollback CopyFile must both be recorded; ops={:?}",
            recorded
        );
    }

    /// new 成功: CreateDirAll / WriteFile×2 / CargoBuild / CargoTest が順に走り NewModule を返す
    #[tokio::test]
    async fn tier2_new_module_adopted_creates_files_and_builds() {
        let ops = Arc::new(Mutex::new(vec![]));
        let (module_names, module_paths) = tier2_modules();
        let inputs = vec!["三 + 五".to_string()];
        let metadata = make_metadata();

        let cmp = CmpLoop::new_with_executor(
            Box::new(SequencedAi::new([JUDGE_NEW, NEW_MOD_SPEC])),
            Box::new(TrackingExecutor {
                build_ok: true,
                test_ok: true,
                source: FAKE_SRC.to_string(),
                ops: Arc::clone(&ops),
            }),
        );

        let outcome = cmp
            .maybe_tier2(Tier2Context {
                unknown_inputs: &inputs,
                chain_module_names: &module_names,
                module_source_paths: &module_paths,
                unknown_count: 5,
                tier2_trigger: 5,
                metadata: &metadata,
            })
            .await
            .unwrap();

        assert!(
            matches!(outcome, Tier2Outcome::NewModule(ref spec) if spec.name == "unicode_norm"),
            "build+test pass → NewModule(unicode_norm)"
        );
        let recorded = ops.lock().unwrap().clone();
        assert!(
            recorded
                .iter()
                .any(|o| matches!(o, Op::CreateDirAll(p) if p.contains("unicode_norm"))),
            "CreateDirAll must be recorded; ops={:?}",
            recorded
        );
        let write_count = recorded
            .iter()
            .filter(|o| matches!(o, Op::WriteFile(..)))
            .count();
        assert!(
            write_count >= 2,
            "Cargo.toml + main.rs must both be written; ops={:?}",
            recorded
        );
        assert!(
            recorded.contains(&Op::CargoBuild("unicode_norm".to_string())),
            "CargoBuild(unicode_norm) must be recorded; ops={:?}",
            recorded
        );
        assert!(
            recorded.contains(&Op::CargoTest("unicode_norm".to_string())),
            "CargoTest(unicode_norm) must be recorded; ops={:?}",
            recorded
        );
    }

    /// new 失敗: build が通らない場合は RemoveDirAll でロールバックされ Rejected を返す
    #[tokio::test]
    async fn tier2_new_module_failed_build_removes_directory() {
        let ops = Arc::new(Mutex::new(vec![]));
        let (module_names, module_paths) = tier2_modules();
        let inputs = vec!["三 + 五".to_string()];
        let metadata = make_metadata();

        let cmp = CmpLoop::new_with_executor(
            Box::new(SequencedAi::new([JUDGE_NEW, NEW_MOD_SPEC])),
            Box::new(TrackingExecutor {
                build_ok: false,
                test_ok: true,
                source: FAKE_SRC.to_string(),
                ops: Arc::clone(&ops),
            }),
        );

        let outcome = cmp
            .maybe_tier2(Tier2Context {
                unknown_inputs: &inputs,
                chain_module_names: &module_names,
                module_source_paths: &module_paths,
                unknown_count: 5,
                tier2_trigger: 5,
                metadata: &metadata,
            })
            .await
            .unwrap();

        assert!(
            matches!(outcome, Tier2Outcome::Rejected { .. }),
            "build fail → Rejected"
        );
        let recorded = ops.lock().unwrap().clone();
        assert!(
            recorded
                .iter()
                .any(|o| matches!(o, Op::RemoveDirAll(p) if p.contains("unicode_norm"))),
            "RemoveDirAll must be called on build failure; ops={:?}",
            recorded
        );
    }

    /// BelowThreshold: unknown_count < tier2_trigger のとき AI も executor も呼ばれない
    #[tokio::test]
    async fn tier2_below_threshold_no_ai_or_executor_calls() {
        let ops = Arc::new(Mutex::new(vec![]));
        let (module_names, module_paths) = tier2_modules();
        let inputs = vec!["３ + ５".to_string()];
        let metadata = make_metadata();

        let cmp = CmpLoop::new_with_executor(
            Box::new(SequencedAi::new(Vec::<String>::new())),
            Box::new(TrackingExecutor {
                build_ok: true,
                test_ok: true,
                source: FAKE_SRC.to_string(),
                ops: Arc::clone(&ops),
            }),
        );

        let outcome = cmp
            .maybe_tier2(Tier2Context {
                unknown_inputs: &inputs,
                chain_module_names: &module_names,
                module_source_paths: &module_paths,
                unknown_count: 4,
                tier2_trigger: 5,
                metadata: &metadata,
            })
            .await
            .unwrap();

        assert!(
            matches!(outcome, Tier2Outcome::BelowThreshold),
            "count < trigger → BelowThreshold"
        );
        let recorded = ops.lock().unwrap().clone();
        assert!(
            recorded.is_empty(),
            "no executor ops should be recorded below threshold; ops={:?}",
            recorded
        );
    }
}
