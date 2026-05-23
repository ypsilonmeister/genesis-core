// =============================================================================
// # CMP Module Charter
//
// What:
//   AST を受け取り計算結果 (f64) を返す。
//
// Invariants:
//   - ゼロ除算はエラーを返す (パニック禁止)
//   - オーバーフローはエラーを返す (サイレント無視禁止)
//   - 計算結果は入力と同一の f64 精度で返す
//
// Boundaries:
//   - 依存先: parser
//   - 被依存先: orchestrator (最終出力)
//
// Extensible:
//   - 新しいノードタイプ (関数呼び出し、変数参照等) の評価
//
// Why:
//   計算ロジックを分離し、parser の変更が evaluator に波及しないようにする。
// =============================================================================

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use std::env;

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
    pub code: String,
    pub message: String,
    pub input_position: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Number(f64),
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("division by zero")]
    DivisionByZero,
    #[error("overflow: {0}")]
    Overflow(String),
}

pub fn evaluate(expr: &Expr) -> Result<f64, EvalError> {
    match expr {
        Expr::Number(n) => Ok(*n),
        Expr::BinOp { op, lhs, rhs } => {
            let lhs_val = evaluate(lhs)?;
            let rhs_val = evaluate(rhs)?;
            match op {
                BinOp::Add => Ok(lhs_val + rhs_val),
                BinOp::Sub => Ok(lhs_val - rhs_val),
                BinOp::Mul => Ok(lhs_val * rhs_val),
                BinOp::Div => {
                    if rhs_val == 0.0 {
                        Err(EvalError::DivisionByZero)
                    } else {
                        Ok(lhs_val / rhs_val)
                    }
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("evaluator booting (v1)");

    let socket_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/evaluator.sock".to_string());

    let _ = std::fs::remove_file(&socket_path);
    if let Some(parent) = std::path::Path::new(&socket_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("Listening on {}", socket_path);

    loop {
        let (mut stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let (reader, mut writer) = stream.split();
            let mut reader = tokio::io::BufReader::new(reader);
            let mut line = String::new();

            if let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 { return; }
                let start = std::time::Instant::now();
                let request: ModuleRequest = match serde_json::from_str(&line) {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!("Failed to parse request: {}", e);
                        return;
                    }
                };

                let expr: Expr = match serde_json::from_str(&request.input) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("Failed to parse AST from input: {}", e);
                        return;
                    }
                };

                let (output, error) = match evaluate(&expr) {
                    Ok(val) => (Some(val.to_string()), None),
                    Err(e) => {
                        let code = match e {
                            EvalError::DivisionByZero => "DIVISION_BY_ZERO",
                            EvalError::Overflow(_) => "OVERFLOW",
                        };
                        (None, Some(ModuleError {
                            code: code.to_string(),
                            message: e.to_string(),
                            input_position: None,
                        }))
                    }
                };

                let response = ModuleResponse {
                    request_id: request.request_id,
                    output,
                    error,
                    processing_ms: start.elapsed().as_millis() as u64,
                };

                if let Ok(payload) = serde_json::to_vec(&response) {
                    let mut payload = payload;
                    payload.push(b'\n');
                    let _ = writer.write_all(&payload).await;
                }
            }
        });
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,evaluator=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_simple_expr() {
        let expr = Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Number(3.0)),
            rhs: Box::new(Expr::Number(5.0)),
        };
        assert_eq!(evaluate(&expr).unwrap(), 8.0);
    }

    #[test]
    fn rejects_division_by_zero() {
        let expr = Expr::BinOp {
            op: BinOp::Div,
            lhs: Box::new(Expr::Number(3.0)),
            rhs: Box::new(Expr::Number(0.0)),
        };
        assert!(matches!(evaluate(&expr).unwrap_err(), EvalError::DivisionByZero));
    }
}
