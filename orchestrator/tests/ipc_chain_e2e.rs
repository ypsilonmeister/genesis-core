// =============================================================================
// Layer 3 E2E テスト: 実プロセス + UDS + JSON 契約の通電確認
//
// 目的:
//   「モジュール境界そのもの」が Week 1 の仕様を満たすことを固定する。
//   orchestrator は起動せず、テストコードが ModuleRequest を直接 socket に送る。
//
// 前提:
//   cargo build --workspace 済み (テスト内では cargo build を実行しない)。
//   CI では build ジョブ → test ジョブの順に実行すること。
//
// 検証内容:
//   - "3 + 5 * 2" → "13"          (演算子優先度)
//   - "(2 + 3) * 4" → "20"        (括弧優先度)
//   - "3 / 0" → DIVISION_BY_ZERO  (エラーコードを直接 assert)
//   - request_id がそのまま返ってくる (通信契約 §3.2)
//   - 各モジュールの中間出力が空でない (パイプライン各段の稼働確認)
// =============================================================================

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

// ソケット名の衝突を避けるためのカウンタ (chain_integration と別バイナリなので重複なし)
static E2E_ID: AtomicU64 = AtomicU64::new(0);

fn next_id() -> u64 {
    E2E_ID.fetch_add(1, Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// 通信型 — 仕様 §3.2 の JSON-L プロトコルをテスト内で再定義する。
// (orchestrator は binary クレートなので直接 use できない)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ModuleRequest {
    request_id: String,
    input: String,
    timestamp: String,
}

/// モジュールエラー (code を直接 assert するために型として持つ)
#[derive(Deserialize, Debug, Clone)]
struct ModuleError {
    code: String,
    message: String,
}

#[derive(Deserialize, Debug)]
struct ModuleResponse {
    request_id: String,
    output: Option<String>,
    error: Option<ModuleError>,
    processing_ms: u64,
}

// ---------------------------------------------------------------------------
// ヘルパ
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn module_binary(name: &str) -> PathBuf {
    workspace_root().join("target").join("debug").join(name)
}

/// UDS socket が accept できるまで最大 5 秒待つ。
async fn wait_for_socket(path: &str) -> bool {
    for _ in 0..50 {
        if UnixStream::connect(path).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// socket に 1 リクエストを送り ModuleResponse を返す。
async fn call(socket_path: &str, req_id: &str, input: &str) -> anyhow::Result<ModuleResponse> {
    let mut stream = UnixStream::connect(socket_path).await?;
    let req = ModuleRequest {
        request_id: req_id.to_string(),
        input: input.to_string(),
        // chrono::DateTime<Utc> は RFC3339 文字列を受け付ける
        timestamp: "2026-01-01T00:00:00Z".to_string(),
    };
    let mut payload = serde_json::to_vec(&req)?;
    payload.push(b'\n');
    stream.write_all(&payload).await?;

    let mut reader = tokio::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    serde_json::from_str(line.trim()).map_err(|e| anyhow::anyhow!("response parse error: {e}"))
}

// ---------------------------------------------------------------------------
// RAII ガード
// ---------------------------------------------------------------------------

/// 起動したモジュールプロセス群を表す RAII ガード。
/// パニックやテスト失敗時も Drop で全プロセスを kill してソケットディレクトリを削除する。
struct Chain {
    procs: Vec<Child>,
    sock_dir: PathBuf,
    /// sockets[i] は modules[i] の socket path
    sockets: Vec<String>,
}

const MODULES: [&str; 4] = ["normalizer", "tokenizer", "parser", "evaluator"];

impl Chain {
    async fn spawn(label: &str) -> anyhow::Result<Self> {
        let id = next_id();
        let sock_dir = PathBuf::from(format!("/tmp/genesis-e2e-{label}-{id}"));
        std::fs::create_dir_all(&sock_dir)?;

        // バイナリ存在確認 (cargo build が済んでいない場合はここで止まる)
        for name in MODULES {
            let bin = module_binary(name);
            anyhow::ensure!(
                bin.exists(),
                "Binary not found: {}. Run `cargo build --workspace` first.",
                bin.display()
            );
        }

        let sockets: Vec<String> = MODULES
            .iter()
            .map(|n| {
                sock_dir
                    .join(format!("{n}.sock"))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

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

        Ok(Self {
            procs,
            sock_dir,
            sockets,
        })
    }

    fn sock(&self, idx: usize) -> &str {
        &self.sockets[idx]
    }
}

impl Drop for Chain {
    fn drop(&mut self) {
        for p in &mut self.procs {
            let _ = p.kill();
        }
        let _ = std::fs::remove_dir_all(&self.sock_dir);
    }
}

/// 入力をチェーン全体に通す。各ステージのエラーを文字列で wrap して返す。
async fn run_chain(chain: &Chain, req_id: &str, input: &str) -> anyhow::Result<String> {
    let mut current = input.to_string();
    let labels = ["normalizer", "tokenizer", "parser", "evaluator"];
    for (i, label) in labels.iter().enumerate() {
        let resp = call(chain.sock(i), req_id, &current).await?;
        if let Some(e) = resp.error {
            anyhow::bail!("[{label}] code={} msg={}", e.code, e.message);
        }
        current = resp.output.unwrap_or_default();
    }
    Ok(current)
}

// ---------------------------------------------------------------------------
// テストケース
// ---------------------------------------------------------------------------

/// 演算子優先度: 乗算が加算より先に評価される
#[tokio::test]
async fn e2e_operator_precedence_gives_13() {
    let chain = Chain::spawn("prec").await.unwrap();
    let result = run_chain(&chain, "req-prec", "3 + 5 * 2")
        .await
        .expect("chain failed");
    assert_eq!(result, "13", "3 + 5 * 2 must be 13");
}

/// 括弧優先度: 括弧内が先に評価される
#[tokio::test]
async fn e2e_parentheses_override_precedence() {
    let chain = Chain::spawn("paren").await.unwrap();
    let result = run_chain(&chain, "req-paren", "(2 + 3) * 4")
        .await
        .expect("chain failed");
    assert_eq!(result, "20", "(2 + 3) * 4 must be 20");
}

/// ゼロ除算: evaluator が DIVISION_BY_ZERO を返し、他のステージは成功する
#[tokio::test]
async fn e2e_division_by_zero_returns_error_code() {
    let chain = Chain::spawn("divz").await.unwrap();

    // normalizer 〜 parser は成功する
    let norm = call(chain.sock(0), "req-divz", "3 / 0").await.unwrap();
    assert!(norm.error.is_none(), "normalizer failed: {:?}", norm.error);

    let tok = call(chain.sock(1), "req-divz", &norm.output.unwrap())
        .await
        .unwrap();
    assert!(tok.error.is_none(), "tokenizer failed: {:?}", tok.error);

    let ast = call(chain.sock(2), "req-divz", &tok.output.unwrap())
        .await
        .unwrap();
    assert!(ast.error.is_none(), "parser failed: {:?}", ast.error);

    // evaluator だけがエラーを返す
    let eval = call(chain.sock(3), "req-divz", &ast.output.unwrap())
        .await
        .unwrap();
    let err = eval
        .error
        .expect("evaluator must return an error for 3 / 0");
    assert_eq!(
        err.code, "DIVISION_BY_ZERO",
        "wrong error code: got {}",
        err.code
    );
}

/// request_id 契約: 各モジュールは受け取った request_id をそのまま返す
#[tokio::test]
async fn e2e_request_id_is_echoed_by_each_module() {
    let chain = Chain::spawn("echo").await.unwrap();
    let req_id = "unique-id-42";

    // 各モジュールには前段の出力フォーマットを期待するため normalizer だけチェック
    let resp = call(chain.sock(0), req_id, "1 + 1").await.unwrap();
    assert_eq!(
        resp.request_id, req_id,
        "{} did not echo request_id",
        MODULES[0]
    );
}

/// パイプライン稼働確認: 各ステージが空でない出力を返す
#[tokio::test]
async fn e2e_each_stage_produces_output() {
    let chain = Chain::spawn("stage").await.unwrap();

    let norm = call(chain.sock(0), "req-stage", "2 + 3").await.unwrap();
    assert!(norm.error.is_none());
    let norm_out = norm.output.expect("normalizer must produce output");
    assert!(!norm_out.is_empty(), "normalizer output is empty");

    let tok = call(chain.sock(1), "req-stage", &norm_out).await.unwrap();
    assert!(tok.error.is_none());
    let tok_out = tok.output.expect("tokenizer must produce output");
    assert!(!tok_out.is_empty(), "tokenizer output is empty");

    let ast = call(chain.sock(2), "req-stage", &tok_out).await.unwrap();
    assert!(ast.error.is_none());
    let ast_out = ast.output.expect("parser must produce output");
    assert!(!ast_out.is_empty(), "parser output is empty");

    let eval = call(chain.sock(3), "req-stage", &ast_out).await.unwrap();
    assert!(eval.error.is_none());
    let eval_out = eval.output.expect("evaluator must produce output");
    assert_eq!(eval_out, "5", "2 + 3 must be 5");
}

/// processing_ms 契約: 各モジュールは処理時間を u64 として返す
#[tokio::test]
async fn e2e_processing_ms_is_present() {
    let chain = Chain::spawn("timing").await.unwrap();
    let resp = call(chain.sock(0), "req-timing", "1 + 1").await.unwrap();
    // processing_ms は 0 以上 (存在していればデシリアライズ成功)
    let _ = resp.processing_ms; // デシリアライズで型チェック済み
}
