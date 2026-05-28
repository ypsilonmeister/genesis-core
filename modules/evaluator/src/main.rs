// =============================================================================
// # CMP Module Charter
//
// What:
//   Receive an AST and return the calculation result (f64).
//
// Invariants:
//   - Division by zero must return an error (panics are prohibited).
//   - Overflow must return an error (silent ignores are prohibited).
//   - The calculation result must be returned with the same f64 precision as the input.
//
// Boundaries:
//   - Dependencies: parser
//   - Dependents: orchestrator (final output)
//
// Extensible:
//   - Evaluation of new node types (e.g., function calls, variable references).
//
// Why:
//   Isolate calculation logic so that changes to the parser do not propagate to the evaluator.
// =============================================================================

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleRequest {
    pub request_id: String,
    pub input: String,
    pub timestamp: DateTime<Utc>,
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
#[serde(tag = "type", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Expr {
    Number(f64),
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    FunctionCall {
        name: String,
        args: Vec<Expr>,
    },
    Sequence(Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Pow,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    Assign,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Range,
    At,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UnaryOp {
    Neg,
    Pos,
    Fact,
    Percent,
    Not,
}

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("division by zero")]
    DivisionByZero,
    #[error("overflow: {0}")]
    Overflow(String),
    #[error("unknown function: {0}")]
    UnknownFunction(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("stack error: {0}")]
    StackError(String),
}

fn check_result(v: f64) -> Result<f64, EvalError> {
    if v.is_infinite() {
        if v.is_sign_positive() {
            Err(EvalError::Overflow("result is positive infinity".to_string()))
        } else {
            Err(EvalError::Overflow("result is negative infinity".to_string()))
        }
    } else if v.is_nan() {
        Err(EvalError::Overflow("result is NaN".to_string()))
    } else {
        Ok(v)
    }
}

pub fn evaluate(root: &Expr) -> Result<f64, EvalError> {
    enum Task<'a> {
        Eval(&'a Expr),
        ComputeBinOp(BinOp),
        ComputeUnaryOp(UnaryOp),
        ComputeFunction(String, usize),
        HandleSequence(usize),
    }

    let mut tasks = vec![Task::Eval(root)];
    let mut values = vec![];

    while let Some(task) = tasks.pop() {
        match task {
            Task::Eval(expr) => match expr {
                Expr::Number(n) => values.push(check_result(*n)?),
                Expr::BinOp { op, lhs, rhs } => {
                    tasks.push(Task::ComputeBinOp(*op));
                    tasks.push(Task::Eval(rhs));
                    tasks.push(Task::Eval(lhs));
                }
                Expr::UnaryOp { op, expr } => {
                    tasks.push(Task::ComputeUnaryOp(*op));
                    tasks.push(Task::Eval(expr));
                }
                Expr::FunctionCall { name, args } => {
                    tasks.push(Task::ComputeFunction(name.clone(), args.len()));
                    for arg in args.iter().rev() {
                        tasks.push(Task::Eval(arg));
                    }
                }
                Expr::Sequence(exprs) => {
                    if exprs.is_empty() {
                        values.push(0.0);
                    } else {
                        tasks.push(Task::HandleSequence(exprs.len()));
                        for expr in exprs.iter().rev() {
                            tasks.push(Task::Eval(expr));
                        }
                    }
                }
            },
            Task::ComputeBinOp(op) => {
                let rhs_val = values.pop().ok_or_else(|| EvalError::StackError("missing rhs".to_string()))?;
                let lhs_val = values.pop().ok_or_else(|| EvalError::StackError("missing lhs".to_string()))?;
                let res = match op {
                    BinOp::Add => lhs_val + rhs_val,
                    BinOp::Sub => lhs_val - rhs_val,
                    BinOp::Mul => lhs_val * rhs_val,
                    BinOp::Div | BinOp::FloorDiv => {
                        if rhs_val == 0.0 {
                            return Err(EvalError::DivisionByZero);
                        }
                        let res = lhs_val / rhs_val;
                        if op == BinOp::FloorDiv { res.floor() } else { res }
                    }
                    BinOp::Pow => {
                        if lhs_val == 0.0 && rhs_val < 0.0 {
                            return Err(EvalError::DivisionByZero);
                        }
                        lhs_val.powf(rhs_val)
                    }
                    BinOp::Mod => {
                        if rhs_val == 0.0 {
                            return Err(EvalError::DivisionByZero);
                        }
                        lhs_val % rhs_val
                    }
                    BinOp::Eq => if lhs_val == rhs_val { 1.0 } else { 0.0 },
                    BinOp::Ne => if lhs_val != rhs_val { 1.0 } else { 0.0 },
                    BinOp::Lt => if lhs_val < rhs_val { 1.0 } else { 0.0 },
                    BinOp::Gt => if lhs_val > rhs_val { 1.0 } else { 0.0 },
                    BinOp::Le => if lhs_val <= rhs_val { 1.0 } else { 0.0 },
                    BinOp::Ge => if lhs_val >= rhs_val { 1.0 } else { 0.0 },
                    _ => lhs_val, // TODO: Implement other ops
                };
                values.push(check_result(res)?);
            }
            Task::ComputeUnaryOp(op) => {
                let val = values.pop().ok_or_else(|| EvalError::StackError("missing operand".to_string()))?;
                let res = match op {
                    UnaryOp::Neg => -val,
                    UnaryOp::Pos => val,
                    UnaryOp::Fact => {
                        if val < 0.0 || val.fract() != 0.0 {
                            return Err(EvalError::InvalidArgument("factorial requires non-negative integer".to_string()));
                        }
                        if val > 170.0 {
                            return Err(EvalError::Overflow("factorial result exceeds f64 range".to_string()));
                        }
                        let mut r = 1.0;
                        for i in 1..=(val as u64) {
                            r *= i as f64;
                        }
                        r
                    }
                    UnaryOp::Percent => val / 100.0,
                    UnaryOp::Not => if val == 0.0 { 1.0 } else { 0.0 },
                };
                values.push(check_result(res)?);
            }
            Task::ComputeFunction(name, arg_count) => {
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count {
                    args.push(values.pop().ok_or_else(|| EvalError::StackError("missing function argument".to_string()))?);
                }
                args.reverse();

                let res = match name.as_str() {
                    "sin" => {
                        if args.len() != 1 { return Err(EvalError::InvalidArgument("sin takes 1 argument".to_string())); }
                        args[0].sin()
                    }
                    "cos" => {
                        if args.len() != 1 { return Err(EvalError::InvalidArgument("cos takes 1 argument".to_string())); }
                        args[0].cos()
                    }
                    "tan" => {
                        if args.len() != 1 { return Err(EvalError::InvalidArgument("tan takes 1 argument".to_string())); }
                        let c = args[0].cos();
                        if c == 0.0 { return Err(EvalError::DivisionByZero); }
                        args[0].tan()
                    }
                    "log" | "log10" => {
                        if args.len() != 1 { return Err(EvalError::InvalidArgument("log takes 1 argument".to_string())); }
                        if args[0] == 0.0 { return Err(EvalError::DivisionByZero); }
                        if args[0] < 0.0 { return Err(EvalError::InvalidArgument("log of negative number".to_string())); }
                        args[0].log10()
                    }
                    "ln" => {
                        if args.len() != 1 { return Err(EvalError::InvalidArgument("ln takes 1 argument".to_string())); }
                        if args[0] == 0.0 { return Err(EvalError::DivisionByZero); }
                        if args[0] < 0.0 { return Err(EvalError::InvalidArgument("ln of negative number".to_string())); }
                        args[0].ln()
                    }
                    "sqrt" => {
                        if args.len() != 1 { return Err(EvalError::InvalidArgument("sqrt takes 1 argument".to_string())); }
                        if args[0] < 0.0 { return Err(EvalError::InvalidArgument("sqrt of negative number".to_string())); }
                        args[0].sqrt()
                    }
                    "cbrt" => {
                        if args.len() != 1 { return Err(EvalError::InvalidArgument("cbrt takes 1 argument".to_string())); }
                        args[0].cbrt()
                    }
                    "abs" => {
                        if args.len() != 1 { return Err(EvalError::InvalidArgument("abs takes 1 argument".to_string())); }
                        args[0].abs()
                    }
                    "exp" => {
                        if args.len() != 1 { return Err(EvalError::InvalidArgument("exp takes 1 argument".to_string())); }
                        args[0].exp()
                    }
                    _ => return Err(EvalError::UnknownFunction(name)),
                };
                values.push(check_result(res)?);
            }
            Task::HandleSequence(count) => {
                let last_val = values.pop().ok_or_else(|| EvalError::StackError("missing sequence value".to_string()))?;
                for _ in 1..count {
                    values.pop().ok_or_else(|| EvalError::StackError("missing sequence value".to_string()))?;
                }
                values.push(last_val);
            }
        }
    }

    values.pop().ok_or_else(|| {
        EvalError::StackError("empty evaluation stack".to_string())
    })
}

async fn send_response<W>(
    writer: &mut W,
    request_id: String,
    output: Option<String>,
    error: Option<ModuleError>,
    processing_ms: u64,
) where
    W: AsyncWriteExt + Unpin,
{
    let response = ModuleResponse {
        request_id,
        output,
        error,
        processing_ms,
    };
    if let Ok(payload) = serde_json::to_vec(&response) {
        let mut payload = payload;
        payload.push(b'\n');
        let _ = writer.write_all(&payload).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("evaluator booting (v2.8 - supporting Sequence and extended ops)");

    let addr_or_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/evaluator.sock".to_string());

    if addr_or_path.starts_with("tcp://") {
        let addr = addr_or_path.strip_prefix("tcp://").unwrap();
        let listener = TcpListener::bind(addr).await?;
        tracing::info!("Listening on TCP {}", addr);
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to accept TCP connection: {}", e);
                    continue;
                }
            };
            tokio::spawn(async move {
                let _ = handle_client(stream).await;
            });
        }
    } else {
        #[cfg(unix)]
        {
            let uds_path = addr_or_path.strip_prefix("uds://").unwrap_or(&addr_or_path);
            let _ = std::fs::remove_file(uds_path);
            if let Some(parent) = std::path::Path::new(uds_path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let listener = UnixListener::bind(uds_path)?;
            tracing::info!("Listening on UDS {}", uds_path);
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to accept UDS connection: {}", e);
                        continue;
                    }
                };
                tokio::spawn(async move {
                    let _ = handle_client(stream).await;
                });
            }
        }
        #[cfg(not(unix))]
        {
            use std::path::Path;
            fn path_to_port(path: impl AsRef<Path>) -> u16 {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                let file_name = path
                    .as_ref()
                    .file_name()
                    .map(|f| f.to_string_lossy())
                    .unwrap_or_else(|| path.as_ref().to_string_lossy());
                file_name.hash(&mut hasher);
                let hash = hasher.finish();
                (49152 + (hash % 16384)) as u16
            }

            let addr = if !addr_or_path.contains('/') && !addr_or_path.contains('\\') && addr_or_path.contains(':') {
                addr_or_path.clone()
            } else {
                let port = path_to_port(&addr_or_path);
                let default_addr = format!("127.0.0.1:{}", port);
                tracing::warn!("UDS not supported on Windows. Falling back to TCP {}", default_addr);
                default_addr
            };
            let listener = TcpListener::bind(&addr).await?;
            tracing::info!("Listening on TCP {}", addr);
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to accept TCP connection: {}", e);
                        continue;
                    }
                };
                tokio::spawn(async move {
                    let _ = handle_client(stream).await;
                });
            }
        }
    }
}

async fn handle_client<S>(stream: S) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let start = std::time::Instant::now();
                let request: ModuleRequest = match serde_json::from_str(&line) {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!("Failed to parse request envelope: {}", e);
                        send_response(
                            &mut writer,
                            "unknown".to_string(),
                            None,
                            Some(ModuleError {
                                code: "SYNTAX_ERROR".to_string(),
                                message: format!("Failed to parse request: {}", e),
                                input_position: None,
                            }),
                            start.elapsed().as_millis() as u64,
                        )
                        .await;
                        continue;
                    }
                };

                let expr: Expr = match serde_json::from_str(&request.input) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("Failed to parse AST: {}", e);
                        send_response(
                            &mut writer,
                            request.request_id,
                            None,
                            Some(ModuleError {
                                code: "SYNTAX_ERROR".to_string(),
                                message: format!("Failed to parse AST: {}", e),
                                input_position: None,
                            }),
                            start.elapsed().as_millis() as u64,
                        )
                        .await;
                        continue;
                    }
                };

                match evaluate(&expr) {
                    Ok(val) => {
                        let val_str = if val.fract() == 0.0 {
                            format!("{:.0}", val)
                        } else {
                            val.to_string()
                        };
                        send_response(
                            &mut writer,
                            request.request_id,
                            Some(val_str),
                            None,
                            start.elapsed().as_millis() as u64,
                        )
                        .await;
                    }
                    Err(e) => {
                        let code = match e {
                            EvalError::DivisionByZero => "DIVISION_BY_ZERO",
                            EvalError::Overflow(_) => "OVERFLOW",
                            EvalError::UnknownFunction(_) | EvalError::InvalidArgument(_) => "SYNTAX_ERROR",
                            _ => "SYNTAX_ERROR",
                        };
                        send_response(
                            &mut writer,
                            request.request_id,
                            None,
                            Some(ModuleError {
                                code: code.to_string(),
                                message: e.to_string(),
                                input_position: None,
                            }),
                            start.elapsed().as_millis() as u64,
                        )
                        .await;
                    }
                }
            }
            Err(e) => {
                tracing::error!("Socket read error: {}", e);
                break;
            }
        }
    }
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,evaluator=debug"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
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
    fn evaluates_sequence() {
        let expr = Expr::Sequence(vec![
            Expr::Number(1.0),
            Expr::Number(2.0),
            Expr::Number(3.0),
        ]);
        assert_eq!(evaluate(&expr).unwrap(), 3.0);
    }
}
