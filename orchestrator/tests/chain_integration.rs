// =============================================================================
// Layer 1 integration test: 実プロセス + UDS + JSON 契約の通電確認
//
// 前提: cargo test --workspace を使えばテスト前に自動ビルドされる。
//       cargo test -p orchestrator --test chain_integration を単独実行する場合は
//       事前に cargo build --workspace が必要。
//
// 検証内容:
//   - "3 + 5 * 2" => "13"  (演算子優先度 + 正確な計算)
//   - "6 / 0" => error      (ゼロ除算のエラー伝播)
//   - "(2 + 3) * 4" => "20" (括弧優先度)
// =============================================================================

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

static TEST_ID: AtomicU64 = AtomicU64::new(0);

fn next_test_id() -> u64 {
    TEST_ID.fetch_add(1, Ordering::SeqCst)
}

// IPC 型 (§3.2 の通信契約をテスト内で再定義)
#[derive(Serialize)]
struct Req {
    request_id: String,
    input: String,
    timestamp: String,
}

#[derive(Deserialize, Debug)]
struct Resp {
    output: Option<String>,
    error: Option<serde_json::Value>,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn module_binary(name: &str) -> PathBuf {
    workspace_root().join("target").join("debug").join(name)
}

/// UDS ソケットが listen 状態になるまで最大 5 秒待つ
async fn wait_for_socket(path: &str) -> bool {
    for _ in 0..50 {
        if UnixStream::connect(path).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// UDS 経由で 1 リクエストを送り応答を返す (§3.2 の JSON-L プロトコル)
async fn ipc_call(socket_path: &str, input: &str) -> anyhow::Result<String> {
    let mut stream = UnixStream::connect(socket_path).await?;
    let req = Req {
        request_id: "test".to_string(),
        input: input.to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
    };
    let mut payload = serde_json::to_vec(&req)?;
    payload.push(b'\n');
    stream.write_all(&payload).await?;

    let mut reader = tokio::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let resp: Resp = serde_json::from_str(line.trim())?;
    if let Some(err) = resp.error {
        anyhow::bail!("module error: {}", err);
    }
    resp.output
        .ok_or_else(|| anyhow::anyhow!("module returned no output"))
}

/// RAII guard: Drop 時に全プロセスを kill してソケットディレクトリを削除する
struct TestProcesses {
    children: Vec<Child>,
    sock_dir: PathBuf,
}

impl TestProcesses {
    async fn spawn(label: &str) -> anyhow::Result<(Self, Vec<String>)> {
        let id = next_test_id();
        let sock_dir = PathBuf::from(format!("/tmp/genesis-test-chain-{}-{}", label, id));
        std::fs::create_dir_all(&sock_dir)?;

        let modules = ["normalizer", "tokenizer", "parser", "evaluator"];

        // バイナリ存在確認
        for name in modules {
            let bin = module_binary(name);
            anyhow::ensure!(
                bin.exists(),
                "Binary not found: {}. Run `cargo build --workspace` first.",
                bin.display()
            );
        }

        let socks: Vec<String> = modules
            .iter()
            .map(|n| {
                sock_dir
                    .join(format!("{}.sock", n))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        let children: Vec<Child> = modules
            .iter()
            .zip(socks.iter())
            .map(|(name, sock)| {
                Command::new(module_binary(name))
                    .arg(sock)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap_or_else(|e| panic!("Failed to spawn {}: {}", name, e))
            })
            .collect();

        // 全ソケットが ready になるまで待機
        for sock in &socks {
            assert!(wait_for_socket(sock).await, "Socket timeout: {}", sock);
        }

        Ok((Self { children, sock_dir }, socks))
    }
}

impl Drop for TestProcesses {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
        }
        let _ = std::fs::remove_dir_all(&self.sock_dir);
    }
}

/// チェーン全体を通して入力 → 最終出力を得るヘルパ
async fn run_chain(socks: &[String], input: &str) -> anyhow::Result<String> {
    let normalized = ipc_call(&socks[0], input).await?;
    let tokens = ipc_call(&socks[1], &normalized).await?;
    let ast = ipc_call(&socks[2], &tokens).await?;
    ipc_call(&socks[3], &ast).await
}

// ---------------------------------------------------------------------------
// テストケース
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chain_basic_arithmetic_with_precedence() {
    let (_procs, socks) = TestProcesses::spawn("basic").await.unwrap();
    let result = run_chain(&socks, "3 + 5 * 2").await.expect("chain failed");
    assert_eq!(result, "13", "3 + 5 * 2 should be 13 (operator precedence)");
}

#[tokio::test]
async fn chain_parentheses_override_precedence() {
    let (_procs, socks) = TestProcesses::spawn("paren").await.unwrap();
    let result = run_chain(&socks, "(2 + 3) * 4")
        .await
        .expect("chain failed");
    assert_eq!(result, "20", "(2 + 3) * 4 should be 20");
}

#[tokio::test]
async fn chain_division_by_zero_propagates_error() {
    let (_procs, socks) = TestProcesses::spawn("divzero").await.unwrap();
    // normalizer と tokenizer と parser は成功するが evaluator でエラー
    let normalized = ipc_call(&socks[0], "6 / 0").await.expect("normalizer");
    let tokens = ipc_call(&socks[1], &normalized).await.expect("tokenizer");
    let ast = ipc_call(&socks[2], &tokens).await.expect("parser");
    let result = ipc_call(&socks[3], &ast).await;
    assert!(
        result.is_err(),
        "6 / 0 should return error, got: {:?}",
        result
    );
}

#[tokio::test]
async fn chain_whitespace_normalization() {
    let (_procs, socks) = TestProcesses::spawn("ws").await.unwrap();
    // 余分な空白は normalizer で除去されるべき
    let result = run_chain(&socks, "  3  +  5  ")
        .await
        .expect("chain failed");
    assert_eq!(result, "8");
}
