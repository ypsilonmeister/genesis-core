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
mod chain;
mod charter_runtime;
mod cmp_loop;
mod hot_swap;
mod ipc;
mod metadata;
mod process;

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::ai_backend::{build_claude_backend, build_gemini_backend};
use crate::attacker::{rand_attack_delay_secs, Attacker};
use crate::chain::{ChainConfig, ModuleSpec};
use crate::cmp_loop::{
    CmpLoop, NewModuleSpec, RepairContext, RepairOutcome, Tier2Context, Tier2Outcome,
};
use crate::hot_swap::HotSwapper;
use crate::ipc::{call_module, ModuleRequest};
use crate::metadata::MetadataStore;
use crate::process::ModuleProcess;

/// stdin 入力 vs 攻撃 AI 入力を区別するタグ
enum InputSource {
    Human(String),
    /// 攻撃 AI が生成した 1 バッチ分の入力 (diversity_score 付き)
    AttackBatch {
        inputs: Vec<String>,
        diversity: f64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    info!("genesis-core orchestrator booting");

    // 1. AI バックエンドを初期化
    let claude = build_claude_backend()?;
    let gemini = build_gemini_backend()?;
    let cmp = CmpLoop::new(claude);
    let attacker = Attacker::new(gemini);
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
    let mut processes: Vec<(ModuleSpec, ModuleProcess)> = Vec::new();
    for m in &config.modules {
        info!("spawning module: {}", m.name);
        let proc = ModuleProcess::spawn(&m.name, &m.binary, &m.socket).await?;
        processes.push((m.clone(), proc));
    }
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // 5. 共有状態 (Arc<Mutex<VecDeque<String>>>)
    let success_samples: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let recent_errors: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let recent_attack_inputs: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

    // 6. mpsc チャンネル (stdin & 攻撃 AI → main loop)
    let (tx, mut rx) = mpsc::unbounded_channel::<InputSource>();

    // 6a. stdin タスク
    let tx_stdin = tx.clone();
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_stdin.send(InputSource::Human(line));
        }
    });

    // 6b. 攻撃 AI バックグラウンドタスク
    let tx_attacker = tx.clone();
    let ss = Arc::clone(&success_samples);
    let re = Arc::clone(&recent_errors);
    let rai = Arc::clone(&recent_attack_inputs);
    tokio::spawn(async move {
        loop {
            let delay = rand_attack_delay_secs();
            info!(delay_secs = delay, "attacker: next attack in");
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;

            let ss_snap: Vec<String> = ss.lock().await.iter().cloned().collect();
            let re_snap: Vec<String> = re.lock().await.iter().cloned().collect();
            let rai_snap: Vec<String> = rai.lock().await.iter().cloned().collect();

            match attacker
                .generate_attacks(&ss_snap, &re_snap, &rai_snap)
                .await
            {
                Ok((inputs, diversity)) if !inputs.is_empty() => {
                    // 攻撃入力を recent_attack_inputs に追記
                    let mut rai_lock = rai.lock().await;
                    for i in &inputs {
                        rai_lock.push_back(i.clone());
                        while rai_lock.len() > 100 {
                            rai_lock.pop_front();
                        }
                    }
                    drop(rai_lock);
                    let _ = tx_attacker.send(InputSource::AttackBatch { inputs, diversity });
                }
                Ok(_) => warn!("attacker: empty batch, skipping"),
                Err(e) => warn!(err = %e, "attacker: generation failed"),
            }
        }
    });
    drop(tx); // 残った sender を落として tx_stdin / tx_attacker のみ残す

    // 7. エラーカウンタ
    let mut error_counts: HashMap<String, u32> = HashMap::new();
    let mut unknown_pattern_examples: VecDeque<String> = VecDeque::new();
    let tier1_trigger = cmp.tier1_trigger;
    let tier2_trigger: u32 = std::env::var("TIER2_TRIGGER_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    info!("ready — send expressions via stdin (Ctrl+D to quit)");

    // 8. メインループ
    while let Some(src) = rx.recv().await {
        let (inputs, is_attack, attack_diversity) = match src {
            InputSource::Human(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                (vec![line], false, None)
            }
            InputSource::AttackBatch { inputs, diversity } => (inputs, true, Some(diversity)),
        };

        let mut attack_results: Vec<serde_json::Value> = Vec::new();

        for input in &inputs {
            let request_id = Uuid::new_v4().to_string();
            let mut current = input.clone();
            let mut chain_error: Option<(String, String)> = None;

            'chain: for (m, _proc) in &processes {
                let req = ModuleRequest {
                    request_id: request_id.clone(),
                    input: current.clone(),
                    timestamp: Utc::now(),
                };

                match call_module(&m.socket, &req).await {
                    Ok(resp) => {
                        if let Some(err) = resp.error {
                            warn!(module = %m.name, code = ?err.code, "module error");
                            chain_error = Some((m.name.clone(), format!("{:?}", err.code)));
                            break 'chain;
                        }
                        if let Some(out) = resp.output {
                            current = out;
                        }
                    }
                    Err(e) => {
                        error!(module = %m.name, err = %e, "IPC failure");
                        chain_error = Some((m.name.clone(), "ModuleCrash".to_string()));
                        break 'chain;
                    }
                }
            }

            // 攻撃結果を収集
            if is_attack {
                let result_entry = if let Some((_, ref code)) = chain_error {
                    serde_json::json!({"input": input, "result": code})
                } else {
                    serde_json::json!({"input": input, "result": "success"})
                };
                attack_results.push(result_entry);
            }

            if let Some((module_name, error_code)) = chain_error {
                // 共有状態を更新
                {
                    let mut re = recent_errors.lock().await;
                    re.push_back(format!("[{}] {}", error_code, input));
                    while re.len() > 50 {
                        re.pop_front();
                    }
                }

                if error_code == "UnknownPattern" {
                    unknown_pattern_examples.push_back(input.clone());
                    while unknown_pattern_examples.len() > 20 {
                        unknown_pattern_examples.pop_front();
                    }
                }

                let count = error_counts.entry(error_code.clone()).or_insert(0);
                *count += 1;
                info!(input = %input, module = %module_name, error_code = %error_code,
                      count = *count, "error recorded");

                // Tier 1 判定
                if error_code != "UnknownPattern" && *count >= tier1_trigger {
                    info!(error_code = %error_code, "Tier 1 triggered");
                    if let Some(idx) = processes.iter().position(|(m, _)| m.name == module_name) {
                        let (m_cfg, proc) = processes.remove(idx);
                        let source_path = format!("modules/{}/src/main.rs", m_cfg.name);
                        let swapper = HotSwapper::new(&m_cfg.name, &m_cfg.binary, &m_cfg.socket);

                        let (outcome, new_child) = cmp
                            .maybe_repair(RepairContext {
                                error_code: &error_code,
                                error_count: *count,
                                module_name: &m_cfg.name,
                                module_source_path: &source_path,
                                hot_swapper: &swapper,
                                old_child: proc.child,
                                metadata: &metadata,
                            })
                            .await?;

                        let new_proc = ModuleProcess {
                            name: m_cfg.name.clone(),
                            child: new_child,
                        };
                        processes.insert(idx, (m_cfg, new_proc));

                        match outcome {
                            RepairOutcome::Adopted => {
                                info!(error_code = %error_code, "Tier 1: repair adopted");
                                error_counts.remove(&error_code);
                            }
                            RepairOutcome::Rejected { reason } => {
                                warn!(error_code = %error_code, reason = %reason, "Tier 1: repair rejected");
                            }
                            RepairOutcome::BelowThreshold => {}
                        }
                    }
                }

                // Tier 2 判定 (UNKNOWN_PATTERN)
                let unknown_count = *error_counts.get("UnknownPattern").unwrap_or(&0);
                if unknown_count >= tier2_trigger {
                    info!("Tier 2 triggered");
                    let names: Vec<String> =
                        processes.iter().map(|(m, _)| m.name.clone()).collect();
                    let paths: Vec<String> = names
                        .iter()
                        .map(|n| format!("modules/{}/src/main.rs", n))
                        .collect();
                    let examples: Vec<String> = unknown_pattern_examples.iter().cloned().collect();

                    let outcome = cmp
                        .maybe_tier2(Tier2Context {
                            unknown_inputs: &examples,
                            chain_module_names: &names,
                            module_source_paths: &paths,
                            unknown_count,
                            tier2_trigger,
                            metadata: &metadata,
                        })
                        .await?;

                    match outcome {
                        Tier2Outcome::Extended { module_name } => {
                            info!(module = %module_name, "Tier 2: extension adopted");
                            error_counts.remove("UnknownPattern");
                            unknown_pattern_examples.clear();
                            // 修正されたモジュールを hot_swap
                            if let Some(idx) =
                                processes.iter().position(|(m, _)| m.name == module_name)
                            {
                                let (m_cfg, proc) = processes.remove(idx);
                                let swapper =
                                    HotSwapper::new(&m_cfg.name, &m_cfg.binary, &m_cfg.socket);
                                match swapper.swap(proc.child).await {
                                    Ok(new_child) => {
                                        let new_proc = ModuleProcess {
                                            name: m_cfg.name.clone(),
                                            child: new_child,
                                        };
                                        processes.insert(idx, (m_cfg, new_proc));
                                    }
                                    Err(e) => warn!(err = %e, "Tier 2 hot_swap failed"),
                                }
                            }
                        }
                        Tier2Outcome::NewModule(spec) => {
                            spawn_and_insert_module(&mut processes, spec).await?;
                            error_counts.remove("UnknownPattern");
                            unknown_pattern_examples.clear();
                        }
                        Tier2Outcome::Rejected { reason } => {
                            warn!(reason = %reason, "Tier 2: rejected");
                        }
                        Tier2Outcome::BelowThreshold => {}
                    }
                }
            } else {
                // 成功
                println!("{} => {}", input, current);
                let mut ss = success_samples.lock().await;
                ss.push_back(input.clone());
                while ss.len() > 20 {
                    ss.pop_front();
                }
            }
        }

        // 攻撃バッチの結果を attacks テーブルに記録
        if is_attack && !attack_results.is_empty() {
            let all_inputs: Vec<String> = inputs.clone();
            let results_json = serde_json::Value::Array(attack_results);
            if let Err(e) = metadata.insert_attack(
                "gemini-cli",
                &all_inputs,
                &std::env::var("ATTACK_PHASE").unwrap_or_else(|_| "A".to_string()),
                attack_diversity,
                &results_json,
            ) {
                warn!(err = %e, "Failed to record attack to metadata");
            }
        }
    }

    // 全プロセスを終了
    for (_m, mut p) in processes {
        let _ = p.child.kill().await;
    }
    info!("orchestrator shutdown complete");
    Ok(())
}

/// 新規モジュールプロセスを起動して processes vec に挿入する。
async fn spawn_and_insert_module(
    processes: &mut Vec<(ModuleSpec, ModuleProcess)>,
    spec: NewModuleSpec,
) -> Result<()> {
    use std::process::Stdio;
    use tokio::process::Command;

    info!(name = %spec.name, "Tier 2: spawning new module");

    // chain 内の挿入位置を決定
    let insert_idx = if let Some(ref after) = spec.insert_after {
        processes
            .iter()
            .position(|(m, _)| &m.name == after)
            .map(|i| i + 1)
            .unwrap_or(processes.len())
    } else {
        0
    };

    let child = Command::new(&spec.binary_path)
        .arg(&spec.socket_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let module_spec = ModuleSpec {
        name: spec.name.clone(),
        binary: spec.binary_path,
        socket: spec.socket_path,
    };
    let proc = ModuleProcess {
        name: spec.name.clone(),
        child,
    };
    processes.insert(insert_idx, (module_spec, proc));
    info!(name = %spec.name, idx = insert_idx, "Tier 2: new module inserted into chain");

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
