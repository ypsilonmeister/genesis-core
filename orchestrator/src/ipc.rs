// =============================================================================
// ipc.rs — Unix Domain Socket + JSON でモジュール間通信を行う
//
// 通信契約 (Lying Calculator §3.2):
//   要求:  { request_id, input, timestamp }
//   応答:  { request_id, output, error, processing_ms }
//
// 直接 import / 直接 socket は Hard Invariant HI-2 で禁止。
// 全ての通信は orchestrator を経由する。
// =============================================================================

use compat::UnixStream;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleRequest {
    pub request_id: String,
    pub input: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleResponse {
    pub request_id: String,
    pub output: Option<String>,
    pub error: Option<ModuleError>,
    pub processing_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleError {
    pub code: ErrorCode,
    pub message: String,
    pub input_position: Option<usize>,
}

/// Lying Calculator §3.3 のエラーコード定義
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    UnknownToken,
    SyntaxError,
    DivisionByZero,
    Overflow,
    UnknownPattern,
    ModuleCrash,
}

impl ErrorCode {
    /// CMP の Tier 区分。Tier 1 = モジュール内修復、Tier 2 = 新モジュール追加。
    #[allow(dead_code)]
    pub fn tier(&self) -> u8 {
        match self {
            ErrorCode::UnknownPattern => 2,
            _ => 1,
        }
    }
}

/// UDS 経由でリクエストを送り、レスポンスを待つ
pub async fn call_module(
    socket_path: &str,
    request: &ModuleRequest,
) -> anyhow::Result<ModuleResponse> {
    let mut stream = UnixStream::connect(socket_path).await?;

    // JSON L (Line-delimited JSON) で送受信
    let mut payload = serde_json::to_vec(request)?;
    payload.push(b'\n');
    stream.write_all(&payload).await?;

    let mut reader = tokio::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response: ModuleResponse = serde_json::from_str(&line)?;
    Ok(response)
}


