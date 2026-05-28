// # CMP Module Charter
//
// What:
//   Convert token sequences into an Abstract Syntax Tree (AST), taking operator precedence into account.
//
// Invariants:
//   - Operator Precedence: * and / have higher precedence than + and -.
//   - Correctly handle precedence changes caused by parentheses.
//   - Return an error for invalid syntax (e.g., consecutive operators, unmatched parentheses).
//
// Boundaries:
//   - Dependencies: tokenizer
//   - Dependents: evaluator
//
// Extensible:
//   - Addition of new operators or function call syntaxes.
//
// Why:
//   Isolate syntax parsing so that the evaluator can focus purely on computation.
// =============================================================================

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use tokio::io::AsyncBufReadExt;
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
#[serde(tag = "type", content = "value")]
pub enum Token {
    Number(f64),
    NaN,
    Infinity,
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    DoubleSlash,
    Caret,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Exclamation,
    Question,
    Colon,
    DotDot,
    LShift,
    RShift,
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
    Percent,
    Sqrt,
    Cbrt,
    Pi,
    At,
    Dollar,
    Ampersand,
    Pipe,
    LogicalAnd,
    LogicalOr,
    Assign,
    Semicolon,
    Sum,
    Function(String),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
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

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
}

const MAX_RECURSION_DEPTH: usize = 500;

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0, depth: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expression(&mut self) -> Result<Expr, String> {
        if self.depth >= MAX_RECURSION_DEPTH {
            return Err("Maximum recursion depth exceeded".to_string());
        }
        self.depth += 1;
        
        let mut exprs = Vec::new();
        loop {
            exprs.push(self.parse_assign()?);
            if let Some(Token::Semicolon) = self.peek() {
                self.next();
                if self.peek().is_none() { break; }
            } else {
                break;
            }
        }
        
        let res = if exprs.len() == 1 {
            Ok(exprs.remove(0))
        } else {
            Ok(Expr::Sequence(exprs))
        };
        
        self.depth -= 1;
        res
    }

    fn parse_assign(&mut self) -> Result<Expr, String> {
        let node = self.parse_logical_or()?;
        if let Some(Token::Assign) = self.peek() {
            self.next();
            let rhs = self.parse_assign()?;
            Ok(Expr::BinOp {
                op: BinOp::Assign,
                lhs: Box::new(node),
                rhs: Box::new(rhs),
            })
        } else {
            Ok(node)
        }
    }

    fn parse_logical_or(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_logical_and()?;
        while let Some(Token::LogicalOr) = self.peek() {
            self.next();
            let rhs = self.parse_logical_and()?;
            node = Expr::BinOp {
                op: BinOp::Or,
                lhs: Box::new(node),
                rhs: Box::new(rhs),
            };
        }
        Ok(node)
    }

    fn parse_logical_and(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_bitwise_or()?;
        while let Some(Token::LogicalAnd) = self.peek() {
            self.next();
            let rhs = self.parse_bitwise_or()?;
            node = Expr::BinOp {
                op: BinOp::And,
                lhs: Box::new(node),
                rhs: Box::new(rhs),
            };
        }
        Ok(node)
    }

    fn parse_bitwise_or(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_bitwise_xor()?;
        while let Some(Token::Pipe) = self.peek() {
            self.next();
            let rhs = self.parse_bitwise_xor()?;
            node = Expr::BinOp {
                op: BinOp::BitOr,
                lhs: Box::new(node),
                rhs: Box::new(rhs),
            };
        }
        Ok(node)
    }

    fn parse_bitwise_xor(&mut self) -> Result<Expr, String> {
        let node = self.parse_bitwise_and()?;
        // Standard math uses ^ for Pow, handled in parse_pow.
        // We could handle bitwise XOR here if a specific token like XOR exists.
        Ok(node)
    }

    fn parse_bitwise_and(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_comparison()?;
        while let Some(Token::Ampersand) = self.peek() {
            self.next();
            let rhs = self.parse_comparison()?;
            node = Expr::BinOp {
                op: BinOp::BitAnd,
                lhs: Box::new(node),
                rhs: Box::new(rhs),
            };
        }
        Ok(node)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_shift()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Eq => Some(BinOp::Eq),
                Token::Ne => Some(BinOp::Ne),
                Token::Lt => Some(BinOp::Lt),
                Token::Gt => Some(BinOp::Gt),
                Token::Le => Some(BinOp::Le),
                Token::Ge => Some(BinOp::Ge),
                Token::DotDot => Some(BinOp::Range),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_shift()?;
                node = Expr::BinOp {
                    op,
                    lhs: Box::new(node),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_shift(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_add_sub()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::LShift => Some(BinOp::Shl),
                Token::RShift => Some(BinOp::Shr),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_add_sub()?;
                node = Expr::BinOp {
                    op,
                    lhs: Box::new(node),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_add_sub(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_mul_div()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Plus => Some(BinOp::Add),
                Token::Minus => Some(BinOp::Sub),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_mul_div()?;
                node = Expr::BinOp {
                    op,
                    lhs: Box::new(node),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_mul_div(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_pow()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Star => Some(BinOp::Mul),
                Token::Slash => Some(BinOp::Div),
                Token::DoubleSlash => Some(BinOp::FloorDiv),
                Token::Percent => Some(BinOp::Mod),
                Token::At => Some(BinOp::At),
                // Implicit multiplication: followed by a primary expression or parenthesis or function
                Token::Number(_) | Token::LParen | Token::LBracket | Token::LBrace | Token::Function(_) | 
                Token::Pi | Token::NaN | Token::Infinity | Token::String(_) | Token::Sqrt | Token::Cbrt | Token::Sum => Some(BinOp::Mul),
                _ => None,
            };
            if let Some(op_type) = op {
                // If it's an explicit operator, consume it. If implicit, don't.
                if matches!(t, Token::Star | Token::Slash | Token::DoubleSlash | Token::Percent | Token::At) {
                    self.next();
                }
                let rhs = self.parse_pow()?;
                node = Expr::BinOp {
                    op: op_type,
                    lhs: Box::new(node),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_pow(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_unary()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::StarStar | Token::Caret => Some(BinOp::Pow),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                // Right-associative: 2^3^4 is 2^(3^4)
                let rhs = self.parse_pow()?;
                node = Expr::BinOp {
                    op,
                    lhs: Box::new(node),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if let Some(t) = self.peek() {
            let op = match t {
                Token::Minus => Some(UnaryOp::Neg),
                Token::Plus => Some(UnaryOp::Pos),
                Token::Exclamation => Some(UnaryOp::Not),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let expr = self.parse_unary()?;
                return Ok(Expr::UnaryOp {
                    op,
                    expr: Box::new(expr),
                });
            }
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut node = self.parse_primary()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Exclamation => Some(UnaryOp::Fact),
                Token::Percent => Some(UnaryOp::Percent),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                node = Expr::UnaryOp {
                    op,
                    expr: Box::new(node),
                };
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let t = self.next().ok_or_else(|| "Unexpected end of input".to_string())?;
        match t {
            Token::Number(n) => {
                // Mixed fraction support: Number Number
                // If the next token is also a number (like in "5 3/4"), 
                // but usually the tokenizer will handle ¾ as a single character if supported.
                // For now, handle as a simple number.
                Ok(Expr::Number(n))
            }
            Token::NaN => Ok(Expr::Number(f64::NAN)),
            Token::Infinity => Ok(Expr::Number(f64::INFINITY)),
            Token::Pi => Ok(Expr::Number(std::f64::consts::PI)),
            Token::LParen | Token::LBracket => {
                let closing = if matches!(t, Token::LParen) { Token::RParen } else { Token::RBracket };
                let expr = self.parse_expression()?;
                if self.next().as_ref() == Some(&closing) {
                    Ok(expr)
                } else {
                    Err(format!("Expected {:?}", closing))
                }
            }
            Token::LBrace => {
                let mut exprs = Vec::new();
                if !matches!(self.peek(), Some(Token::RBrace)) {
                    loop {
                        exprs.push(self.parse_expression()?);
                        match self.peek() {
                            Some(Token::Comma) | Some(Token::Semicolon) => {
                                self.next();
                            }
                            _ => break,
                        }
                    }
                }
                if let Some(Token::RBrace) = self.next() {
                    Ok(Expr::Sequence(exprs))
                } else {
                    Err("Expected '}'".to_string())
                }
            }
            Token::Sqrt | Token::Cbrt | Token::Sum | Token::Function(_) | Token::String(_) => {
                let name = match &t {
                    Token::Sqrt => "sqrt".to_string(),
                    Token::Cbrt => "cbrt".to_string(),
                    Token::Sum => "sum".to_string(),
                    Token::Function(s) => s.clone(),
                    Token::String(s) => s.clone(),
                    _ => unreachable!(),
                };
                
                // Handle both name(args) and name expression
                if let Some(Token::LParen) | Some(Token::LBracket) = self.peek() {
                    let opener = self.next().unwrap();
                    let closer = if matches!(opener, Token::LParen) { Token::RParen } else { Token::RBracket };
                    let mut args = Vec::new();
                    if self.peek() != Some(&closer) {
                        loop {
                            args.push(self.parse_expression()?);
                            if let Some(Token::Comma) = self.peek() {
                                self.next();
                                if self.peek() == Some(&closer) { break; }
                            } else {
                                break;
                            }
                        }
                    }
                    if self.next().as_ref() == Some(&closer) {
                        Ok(Expr::FunctionCall { name, args })
                    } else {
                        Err(format!("Expected {:?}", closer))
                    }
                } else {
                    // Try to parse next expression as argument for prefix-like functions
                    if matches!(t, Token::Sqrt | Token::Cbrt | Token::Sum) {
                        let arg = self.parse_unary()?;
                        Ok(Expr::FunctionCall { name, args: vec![arg] })
                    } else {
                        Ok(Expr::FunctionCall { name, args: vec![] })
                    }
                }
            }
            _ => Err(format!("Unexpected token: {:?}", t)),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("parser booting");
    let addr_or_path = env::args().nth(1).unwrap_or_else(|| "/tmp/genesis-core/parser.sock".to_string());
    if addr_or_path.starts_with("tcp://") {
        let addr = addr_or_path.strip_prefix("tcp://").unwrap();
        let listener = TcpListener::bind(addr).await?;
        loop {
            let (stream, _) = listener.accept().await?;
            tokio::spawn(async move { let _ = handle_client(stream).await; });
        }
    } else {
        #[cfg(unix)]
        {
            let uds_path = addr_or_path.strip_prefix("uds://").unwrap_or(&addr_or_path);
            let _ = std::fs::remove_file(uds_path);
            if let Some(parent) = std::path::Path::new(uds_path).parent() { std::fs::create_dir_all(parent)?; }
            let listener = UnixListener::bind(uds_path)?;
            loop {
                let (stream, _) = listener.accept().await?;
                tokio::spawn(async move { let _ = handle_client(stream).await; });
            }
        }
        #[cfg(not(unix))]
        {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            addr_or_path.hash(&mut hasher);
            let port = (49152 + (hasher.finish() % 16384)) as u16;
            let addr = format!("127.0.0.1:{}", port);
            tracing::info!("UDS simulation on TCP {}", addr);
            let listener = TcpListener::bind(&addr).await?;
            loop {
                let (stream, _) = listener.accept().await?;
                tokio::spawn(async move { let _ = handle_client(stream).await; });
            }
        }
    }
}

async fn handle_client<S>(stream: S) -> Result<()>
where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(reader);
    let mut line = String::new();
    while let Ok(n) = reader.read_line(&mut line).await {
        if n == 0 { break; }
        let start = std::time::Instant::now();
        let request: ModuleRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(_) => { line.clear(); continue; }
        };
        // Expect input to be a JSON array of tokens from tokenizer
        let tokens: Vec<Token> = match serde_json::from_str(&request.input) {
            Ok(t) => t,
            Err(e) => {
                send_response(&mut writer, request.request_id, None, Some(ModuleError { code: "UNKNOWN_PATTERN".to_string(), message: format!("Token parse error: {}", e), input_position: None }), start.elapsed().as_millis() as u64).await;
                line.clear();
                continue;
            }
        };
        let mut parser = Parser::new(tokens);
        let (output, error) = match parser.parse_expression() {
            Ok(expr) => {
                if parser.pos < parser.tokens.len() {
                    (None, Some(ModuleError { code: "UNKNOWN_PATTERN".to_string(), message: format!("Unexpected token at position {}: {:?}", parser.pos, parser.tokens[parser.pos]), input_position: Some(parser.pos) }))
                } else {
                    match serde_json::to_string(&expr) {
                        Ok(json) => (Some(json), None),
                        Err(e) => (None, Some(ModuleError { code: "SERIALIZE_ERROR".to_string(), message: e.to_string(), input_position: None })),
                    }
                }
            },
            Err(e) => (None, Some(ModuleError { code: "UNKNOWN_PATTERN".to_string(), message: e, input_position: Some(parser.pos) })),
        };
        send_response(&mut writer, request.request_id, output, error, start.elapsed().as_millis() as u64).await;
        line.clear();
    }
    Ok(())
}

async fn send_response<W>(writer: &mut W, request_id: String, output: Option<String>, error: Option<ModuleError>, processing_ms: u64)
where W: tokio::io::AsyncWriteExt + Unpin,
{
    let response = ModuleResponse { request_id, output, error, processing_ms };
    if let Ok(payload) = serde_json::to_vec(&response) {
        let mut payload = payload;
        payload.push(b'\n');
        let _ = writer.write_all(&payload).await;
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
