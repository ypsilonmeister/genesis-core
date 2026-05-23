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
//
// このファイルは Week 1 のスケルトン。各モジュールへ拡張していく。
// =============================================================================

mod attacker;
mod charter_runtime;
mod chain;
mod cmp_loop;
mod hot_swap;
mod ipc;
mod metadata;
mod process;

use anyhow::Result;
use tracing::{info, error};
use std::path::Path;
use uuid::Uuid;
use chrono::Utc;

use crate::chain::ChainConfig;
use crate::process::ModuleProcess;
use crate::ipc::{ModuleRequest, call_module};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    info!("genesis-core orchestrator booting (Week 1)");

    // 1. chain.toml を読み込む
    let chain_path = Path::new("chain.toml");
    let config = ChainConfig::load(chain_path)?;
    info!("Loaded chain configuration with {} modules", config.modules.len());

    // 2. modules/*/ の各バイナリをサブプロセスとして起動
    let mut processes = Vec::new();
    for m in &config.modules {
        info!("Spawning module: {}", m.name);
        let proc = ModuleProcess::spawn(&m.name, &m.binary, &m.socket).await?;
        processes.push(proc);
    }

    // モジュールの起動待ち (実際はヘルスチェックが必要だが、Week 1 なので 1秒待つ)
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // 3. UDS を使って順番にルーティング
    let input = "3 + 5 * 2";
    info!("Input: {:?}", input);

    let mut current_input = input.to_string();
    let request_id = Uuid::new_v4().to_string();

    for m in &config.modules {
        info!("Calling module: {}", m.name);
        let request = ModuleRequest {
            request_id: request_id.clone(),
            input: current_input.clone(),
            timestamp: Utc::now(),
        };

        match call_module(&m.socket, &request).await {
            Ok(response) => {
                if let Some(err) = response.error {
                    error!("Module {} returned error: {:?} ({})", m.name, err.code, err.message);
                    return Ok(());
                }
                if let Some(output) = response.output {
                    info!("Module {} output: {:?}", m.name, output);
                    current_input = output;
                } else {
                    error!("Module {} returned no output and no error", m.name);
                    return Ok(());
                }
            }
            Err(e) => {
                error!("Failed to call module {}: {}", m.name, e);
                return Ok(());
            }
        }
    }

    // 4. 結果の確認
    info!("Final Result: {}", current_input);
    assert_eq!(current_input, "13");
    info!("End-to-end calculation successful!");

    // 全プロセスを終了させる (実際は graceful shutdown が必要)
    for mut p in processes {
        let _ = p.child.kill().await;
    }

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
