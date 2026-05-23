// =============================================================================
// orchestrator — Lying Calculator の不可侵領域
//
// 重要: 本クレートは CMP v0.2 §7.3 「自己改変への過適合への対処」により
//       AI による直接改変対象外。改変は Tier 3 として人間が行う。
//
// 役割 (CMP §8.3):
//   - モジュールプロセスのライフサイクル管理
//   - モジュール間通信のルーティング (UDS + JSON)
//   - Layer B (charter/enforcement.rs) の執行
//   - 改変提案 AI (Claude) の呼び出し
//   - メタデータストアへの書き込み
//   - 自己検証ループ (CMP §5) の実行
//   - Tier 3 提案時の人間への通知
// =============================================================================

mod ai_backend;
mod attacker;
mod charter_runtime;
mod chain;
mod cmp_loop;
mod hot_swap;
mod ipc;
mod metadata;
mod process;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::ai_backend::{build_claude_backend, build_gemini_backend};
use crate::attacker::Attacker;
use crate::chain::ChainConfig;
use crate::cmp_loop::{CmpLoop, RepairContext, RepairOutcome};
use crate::hot_swap::HotSwapper;
use crate::ipc::{call_module, ModuleRequest};
use crate::metadata::MetadataStore;
use crate::process::ModuleProcess;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    info!("genesis-core orchestrator booting");

    // 1. AI バックエンドを初期化
    let claude = build_claude_backend()?;
    let gemini = build_gemini_backend()?;
    let cmp = CmpLoop::new(claude);
    let _attacker = Attacker::new(gemini);
    info!(
        claude_backend = %std::env::var("CLAUDE_BACKEND").unwrap_or_else(|_| "cli".to_string()),
        gemini_backend = %std::env::var("GEMINI_BACKEND").unwrap_or_else(|_| "cli".to_string()),
        "AI backends initialized"
    );

    // 2. SQLite メタデータストアを開く
    let metadata = MetadataStore::open("metadata.db")?;
    info!("metadata.db opened");

    // 3. chain.toml を読み込む
    let chain_path = Path::new("chain.toml");
    let config = ChainConfig::load(chain_path)?;
    info!("chain config loaded: {} modules", config.modules.len());

    // 4. 各モジュールをサブプロセスとして起動
    let mut processes: Vec<(crate::chain::ModuleSpec, ModuleProcess)> = Vec::new();
    for m in &config.modules {
        info!("spawning module: {}", m.name);
        let proc = ModuleProcess::spawn(&m.name, &m.binary, &m.socket).await?;
        processes.push((m.clone(), proc));
    }
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // 5. エラーカウンタ (error_code → 発生回数)
    let mut error_counts: HashMap<String, u32> = HashMap::new();
    let tier1_trigger = cmp.tier1_trigger;

    // 6. stdin から入力を 1 行ずつ受け取ってチェーンに流すメインループ
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    info!("ready — send expressions via stdin (Ctrl+D to quit)");

    while let Some(line) = reader.next_line().await? {
        let input = line.trim().to_string();
        if input.is_empty() {
            continue;
        }

        let request_id = Uuid::new_v4().to_string();
        let mut current = input.clone();
        let mut chain_error: Option<(String, String)> = None; // (module_name, error_code)

        // チェーンを順番に呼ぶ
        'chain: for (m, _proc) in &processes {
            let req = ModuleRequest {
                request_id: request_id.clone(),
                input: current.clone(),
                timestamp: Utc::now(),
            };

            match call_module(&m.socket, &req).await {
                Ok(resp) => {
                    if let Some(err) = resp.error {
                        warn!(
                            module = %m.name,
                            code = ?err.code,
                            msg = %err.message,
                            "module error"
                        );
                        chain_error = Some((m.name.clone(), format!("{:?}", err.code)));
                        break 'chain;
                    }
                    if let Some(out) = resp.output {
                        current = out;
                    }
                }
                Err(e) => {
                    error!(module = %m.name, err = %e, "IPC failure");
                    chain_error = Some((m.name.clone(), "MODULE_CRASH".to_string()));
                    break 'chain;
                }
            }
        }

        if let Some((module_name, error_code)) = chain_error {
            // エラーカウントを更新
            let count = error_counts.entry(error_code.clone()).or_insert(0);
            *count += 1;
            info!(
                input = %input,
                module = %module_name,
                error_code = %error_code,
                count = *count,
                threshold = tier1_trigger,
                "error recorded"
            );

            // Tier 1 トリガ判定
            if *count >= tier1_trigger {
                info!(
                    error_code = %error_code,
                    "Tier 1 triggered"
                );

                // 対象モジュールのインデックスを探す
                if let Some(idx) = processes.iter().position(|(m, _)| m.name == module_name) {
                    let (m_cfg, proc) = processes.remove(idx);
                    let source_path = format!("modules/{}/src/main.rs", m_cfg.name);
                    let swapper = HotSwapper::new(&m_cfg.name, &m_cfg.binary, &m_cfg.socket);

                    let (outcome, new_child) = cmp.maybe_repair(RepairContext {
                        error_code: &error_code,
                        error_count: *count,
                        module_name: &m_cfg.name,
                        module_source_path: &source_path,
                        hot_swapper: &swapper,
                        old_child: proc.child,
                        metadata: &metadata,
                    }).await?;

                    let new_proc = ModuleProcess { name: m_cfg.name.clone(), child: new_child };
                    processes.insert(idx, (m_cfg, new_proc));

                    match outcome {
                        RepairOutcome::Adopted => {
                            info!(error_code = %error_code, "Tier 1: repair adopted, resetting counter");
                            error_counts.remove(&error_code);
                        }
                        RepairOutcome::Rejected { reason } => {
                            warn!(error_code = %error_code, reason = %reason, "Tier 1: repair rejected");
                        }
                        RepairOutcome::BelowThreshold => {}
                    }
                }
            }
        } else {
            println!("{} => {}", input, current);
        }
    }

    // 全プロセスを終了
    for (_m, mut p) in processes {
        let _ = p.child.kill().await;
    }

    info!("orchestrator shutdown complete");
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,orchestrator=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}
