// =============================================================================
// クロスプラットフォーム シード疎通テスト
//
// 目的:
//   compat::UnixStream を使って 4 モジュールのチェーンに直接 1 リクエストを通し、
//   Week 1 の最小要件 "3 + 5 * 2" → "13" が Windows (TCP) / Unix (UDS) の双方で
//   成立することを確認する。ipc_chain_e2e.rs は tokio::net::UnixStream 直結のため
//   #![cfg(unix)] でゲートされており Windows では走らない。本テストはその穴を埋める。
//
// 単一チェーンのみを起動する (compat はソケットの「ファイル名」のみを port に
// ハッシュするため、並列チェーンだと port 衝突する。1 チェーンなら各モジュールの
// ファイル名が異なるので衝突しない)。
//
// 前提: cargo build --workspace 済み。
// =============================================================================

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use compat::UnixStream;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[derive(Serialize)]
struct ModuleRequest {
    request_id: String,
    input: String,
    timestamp: String,
}

#[derive(Deserialize, Debug)]
struct ModuleError {
    code: String,
    #[allow(dead_code)]
    message: String,
}

#[derive(Deserialize, Debug)]
struct ModuleResponse {
    #[allow(dead_code)]
    request_id: String,
    output: Option<String>,
    error: Option<ModuleError>,
    #[allow(dead_code)]
    processing_ms: u64,
}

const MODULES: [&str; 4] = ["normalizer", "tokenizer", "parser", "evaluator"];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn module_binary(name: &str) -> PathBuf {
    let mut p = workspace_root().join("target").join("debug").join(name);
    if cfg!(windows) {
        p.set_extension("exe");
    }
    p
}

async fn wait_for_socket(path: &str) -> bool {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if UnixStream::connect(path).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .is_ok()
}

async fn call(socket_path: &str, input: &str) -> anyhow::Result<ModuleResponse> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = UnixStream::connect(socket_path).await?;
        let req = ModuleRequest {
            request_id: "seed-smoke".to_string(),
            input: input.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        };
        let mut payload = serde_json::to_vec(&req)?;
        payload.push(b'\n');
        stream.write_all(&payload).await?;

        let mut reader = tokio::io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        serde_json::from_str(line.trim()).map_err(|e| anyhow::anyhow!("response parse error: {e}"))
    })
    .await
    .map_err(|_| anyhow::anyhow!("call to {socket_path} timed out after 10s"))?
}

/// 起動したモジュール群の RAII ガード (Drop で kill)。
struct Chain {
    procs: Vec<Child>,
    sockets: Vec<String>,
}

impl Chain {
    async fn spawn() -> anyhow::Result<Self> {
        // 一意なファイル名 (compat は file_name のみを port にハッシュする)
        let sockets: Vec<String> = MODULES
            .iter()
            .map(|n| format!("/tmp/genesis-seed-smoke/{n}.sock"))
            .collect();

        for name in MODULES {
            let bin = module_binary(name);
            anyhow::ensure!(
                bin.exists(),
                "Binary not found: {}. Run `cargo build --workspace` first.",
                bin.display()
            );
        }

        let procs: Vec<Child> = MODULES
            .iter()
            .zip(&sockets)
            .map(|(name, sock)| {
                Command::new(module_binary(name))
                    .arg(sock)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap_or_else(|e| panic!("Failed to spawn {name}: {e}"))
            })
            .collect();

        for sock in &sockets {
            assert!(
                wait_for_socket(sock).await,
                "Socket timed out after 5s: {sock}"
            );
        }

        Ok(Self { procs, sockets })
    }
}

impl Drop for Chain {
    fn drop(&mut self) {
        for p in &mut self.procs {
            let _ = p.kill();
            let _ = p.wait();
        }
    }
}

async fn run_chain(chain: &Chain, input: &str) -> anyhow::Result<String> {
    let mut current = input.to_string();
    for (i, label) in MODULES.iter().enumerate() {
        let resp = call(&chain.sockets[i], &current).await?;
        if let Some(e) = resp.error {
            anyhow::bail!("[{label}] code={} msg={}", e.code, e.message);
        }
        current = resp.output.unwrap_or_default();
    }
    Ok(current)
}

/// シードが Week 1 の最小要件を満たすことを確認する。
#[tokio::test]
async fn seed_chain_computes_3_plus_5_times_2() {
    let chain = Chain::spawn().await.expect("failed to spawn module chain");

    let result = run_chain(&chain, "3 + 5 * 2")
        .await
        .expect("chain failed on '3 + 5 * 2'");
    assert_eq!(result, "13", "operator precedence: 3 + 5 * 2 must be 13");

    let paren = run_chain(&chain, "(2 + 3) * 4")
        .await
        .expect("chain failed on '(2 + 3) * 4'");
    assert_eq!(paren, "20", "parentheses: (2 + 3) * 4 must be 20");
}
