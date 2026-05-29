// =============================================================================
// # CMP Module Charter
//
// What:
//   Convert a sequence of tokens into an Abstract Syntax Tree (AST) considering operator precedence.
//
// Invariants:
//   - Operator precedence: * / is higher than + -
//   - Handle priority changes by parentheses correctly
//   - Return an error for invalid syntax (consecutive operators, mismatched parentheses, etc.)
//
// Boundaries:
//   - Dependencies: tokenizer
//   - Dependents: evaluator
//
// Extensible:
//   - Addition of new operators and function call syntax
//
// Why:
//   Isolate syntax parsing so that the evaluator can focus on pure calculation.
// =============================================================================

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::env;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

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
    Factorial,
    Question,
    Colon,
    Dot,
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
    E,
    At,
    Dollar,
    Ampersand,
    Pipe,
    BitXor,
    LogicalAnd,
    LogicalOr,
    Assign,
    Semicolon,
    Sum,
    Integral,
    Differential(String),
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Log,
    Log10,
    Log2,
    Ln,
    Exp,
    Abs,
    I,
    J,
    Imaginary(f64),
    Mod,
    Pow,
    Function(String),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Expr {
    Number(f64),
    Variable(String),
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
    Log {
        expr: Box<Expr>,
        base: Option<Box<Expr>>,
    },
    Sqrt {
        expr: Box<Expr>,
    },
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
    Sqrt,
    Log,
}

const MAX_RECURSION_DEPTH: usize = 256;

struct DepthGuard<'a> {
    depth: &'a Cell<usize>,
}

impl<'a> DepthGuard<'a> {
    fn new(depth: &'a Cell<usize>) -> std::result::Result<Self, String> {
        let d = depth.get();
        if d >= MAX_RECURSION_DEPTH {
            return Err("Maximum expression depth exceeded".to_string());
        }
        depth.set(d + 1);
        Ok(Self { depth })
    }
}

impl<'a> Drop for DepthGuard<'a> {
    fn drop(&mut self) {
        self.depth.set(self.depth.get() - 1);
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: Cell<usize>,
    depth: Cell<usize>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: Cell::new(0), depth: Cell::new(0) }
    }

    fn check_depth(&self) -> std::result::Result<DepthGuard<'_>, String> {
        DepthGuard::new(&self.depth)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos.get())
    }

    fn peek_next(&self) -> Option<&Token> {
        self.tokens.get(self.pos.get() + 1)
    }

    fn next(&self) -> Option<Token> {
        let p = self.pos.get();
        let t = self.tokens.get(p).cloned();
        if t.is_some() {
            self.pos.set(p + 1);
        }
        t
    }

    fn is_function_like(&self, t: &Token) -> bool {
        matches!(t,
            Token::Function(_) | Token::Sin | Token::Cos | Token::Tan |
            Token::Log | Token::Log10 | Token::Log2 | Token::Ln | Token::Exp | Token::Abs | Token::Sqrt |
            Token::Cbrt | Token::Sum | Token::Asin | Token::Acos | Token::Atan | Token::Sinh | Token::Cosh | Token::Tanh
        )
    }

    fn token_to_function_name(&self, t: &Token) -> String {
        match t {
            Token::Function(s) => s.clone(),
            Token::Sin => "sin".to_string(),
            Token::Cos => "cos".to_string(),
            Token::Tan => "tan".to_string(),
            Token::Log => "log".to_string(),
            Token::Log10 => "log10".to_string(),
            Token::Log2 => "log2".to_string(),
            Token::Ln => "ln".to_string(),
            Token::Exp => "exp".to_string(),
            Token::Abs => "abs".to_string(),
            Token::Sqrt => "sqrt".to_string(),
            Token::Cbrt => "cbrt".to_string(),
            Token::Sum => "sum".to_string(),
            Token::Asin => "asin".to_string(),
            Token::Acos => "acos".to_string(),
            Token::Atan => "atan".to_string(),
            Token::Sinh => "sinh".to_string(),
            Token::Cosh => "cosh".to_string(),
            Token::Tanh => "tanh".to_string(),
            _ => format!("{:?}", t).to_lowercase(),
        }
    }

    fn can_start_primary(&self, t: &Token) -> bool {
        matches!(t,
            Token::Number(_) | Token::Pi | Token::E | Token::Infinity | Token::NaN |
            Token::LParen | Token::LBracket | Token::LBrace |
            Token::Function(_) | Token::Sin | Token::Cos | Token::Tan |
            Token::Log | Token::Log10 | Token::Log2 | Token::Ln | Token::Exp | Token::Abs | Token::Sqrt |
            Token::Cbrt | Token::Sum | Token::String(_) | Token::I | Token::J | Token::Imaginary(_) |
            Token::Integral | Token::Differential(_) | Token::Asin | Token::Acos | Token::Atan |
            Token::Sinh | Token::Cosh | Token::Tanh
        )
    }

    fn can_start_expr(&self, t: &Token) -> bool {
        self.can_start_primary(t) || matches!(t, Token::Plus | Token::Minus | Token::Exclamation)
    }

    fn parse_expression(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let mut exprs = Vec::new();
        loop {
            while matches!(self.peek(), Some(Token::Semicolon | Token::Comma)) {
                self.next();
            }
            if self.peek().is_none() { break; }
            
            exprs.push(self.parse_assign()?);
            
            if matches!(self.peek(), Some(Token::Semicolon | Token::Comma)) {
                self.next();
                if self.peek().is_none() { break; }
            } else {
                break;
            }
        }
        
        if exprs.is_empty() {
            Err("Empty expression".to_string())
        } else if exprs.len() == 1 {
            Ok(exprs.remove(0))
        } else {
            Ok(Expr::Sequence(exprs))
        }
    }

    fn parse_assign(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
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

    fn parse_logical_or(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
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

    fn parse_logical_and(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
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

    fn parse_bitwise_or(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
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

    fn parse_bitwise_xor(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let mut node = self.parse_bitwise_and()?;
        while let Some(t) = self.peek() {
            if matches!(t, Token::BitXor) {
                self.next();
                let rhs = self.parse_bitwise_and()?;
                node = Expr::BinOp {
                    op: BinOp::BitXor,
                    lhs: Box::new(node),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_bitwise_and(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
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

    fn parse_comparison(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let mut node = self.parse_range()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Eq => Some(BinOp::Eq),
                Token::Ne => Some(BinOp::Ne),
                Token::Lt => Some(BinOp::Lt),
                Token::Gt => Some(BinOp::Gt),
                Token::Le => Some(BinOp::Le),
                Token::Ge => Some(BinOp::Ge),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_range()?;
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

    fn parse_range(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let mut node = self.parse_shift()?;
        while let Some(Token::DotDot) = self.peek() {
            self.next();
            let rhs = self.parse_shift()?;
            node = Expr::BinOp {
                op: BinOp::Range,
                lhs: Box::new(node),
                rhs: Box::new(rhs),
            };
        }
        Ok(node)
    }

    fn parse_shift(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let mut node = self.parse_term()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::LShift => Some(BinOp::Shl),
                Token::RShift => Some(BinOp::Shr),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_term()?;
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

    fn parse_term(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let mut node = self.parse_factor()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Plus => Some(BinOp::Add),
                Token::Minus => Some(BinOp::Sub),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_factor()?;
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

    fn parse_factor(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let mut node = self.parse_power()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Star => {
                    if self.peek_next() == Some(&Token::Star) {
                        None // handled in parse_power
                    } else {
                        Some(BinOp::Mul)
                    }
                }
                Token::Slash => {
                    if self.peek_next() == Some(&Token::Slash) {
                        Some(BinOp::FloorDiv)
                    } else {
                        Some(BinOp::Div)
                    }
                }
                Token::DoubleSlash => Some(BinOp::FloorDiv),
                Token::Percent | Token::Mod => {
                    let is_binary = if let Some(next_t) = self.peek_next() {
                        self.can_start_expr(next_t)
                    } else {
                        false
                    };
                    if is_binary { Some(BinOp::Mod) } else { None }
                }
                Token::At => Some(BinOp::At),
                _ if self.can_start_primary(t) => Some(BinOp::Mul),
                _ => None,
            };
            if let Some(op) = op {
                if matches!(t, Token::Star | Token::Slash | Token::DoubleSlash | Token::Percent | Token::Mod | Token::At) {
                    self.next();
                    if matches!(t, Token::Slash) && op == BinOp::FloorDiv {
                        self.next();
                    }
                }
                let rhs = self.parse_power()?;
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

    fn parse_power(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let node = self.parse_unary()?;
        if let Some(t) = self.peek() {
            let op = match t {
                Token::StarStar | Token::Caret | Token::Pow => Some(BinOp::Pow),
                Token::Star if self.peek_next() == Some(&Token::Star) => Some(BinOp::Pow),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                if matches!(t, Token::Star) {
                    self.next();
                }
                let rhs = self.parse_power()?; // right-associative
                return Ok(Expr::BinOp {
                    op,
                    lhs: Box::new(node),
                    rhs: Box::new(rhs),
                });
            }
        }
        Ok(node)
    }

    fn parse_unary(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
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

    fn parse_postfix(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let mut node = self.parse_primary()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Exclamation | Token::Factorial => Some(UnaryOp::Fact),
                Token::Percent => {
                    let is_binary = if let Some(next_t) = self.peek_next() {
                        self.can_start_expr(next_t)
                    } else {
                        false
                    };
                    if is_binary { None } else { Some(UnaryOp::Percent) }
                }
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

    fn parse_primary(&self) -> std::result::Result<Expr, String> {
        let _guard = self.check_depth()?;
        let t = self.next().ok_or("Unexpected end of tokens")?;
        
        match &t {
            Token::Number(n) => Ok(Expr::Number(*n)),
            Token::Pi => Ok(Expr::Number(std::f64::consts::PI)),
            Token::E => Ok(Expr::Number(std::f64::consts::E)),
            Token::Infinity => Ok(Expr::Number(f64::INFINITY)),
            Token::NaN => Ok(Expr::Number(f64::NAN)),
            Token::I => Ok(Expr::Variable("i".to_string())),
            Token::J => Ok(Expr::Variable("j".to_string())),
            Token::Imaginary(n) => Ok(Expr::BinOp {
                op: BinOp::Mul,
                lhs: Box::new(Expr::Number(*n)),
                rhs: Box::new(Expr::Variable("i".to_string())),
            }),
            Token::String(s) => Ok(Expr::Variable(s.clone())),
            Token::LParen | Token::LBracket | Token::LBrace => {
                let closing = match t {
                    Token::LParen => Token::RParen,
                    Token::LBracket => Token::RBracket,
                    Token::LBrace => Token::RBrace,
                    _ => unreachable!(),
                };
                let node = self.parse_expression()?;
                if self.next() != Some(closing.clone()) {
                    return Err(format!("Expected {:?}", closing));
                }
                Ok(node)
            }
            _ if self.is_function_like(&t) => {
                let name = self.token_to_function_name(&t);
                if matches!(self.peek(), Some(Token::LParen)) {
                    self.parse_function_call(name)
                } else {
                    // Check if it's a known constant/keyword first
                    match name.to_lowercase().as_str() {
                        "pi" | "π" => Ok(Expr::Number(std::f64::consts::PI)),
                        "e" => Ok(Expr::Number(std::f64::consts::E)),
                        "inf" | "infinity" | "∞" => Ok(Expr::Number(f64::INFINITY)),
                        "nan" => Ok(Expr::Number(f64::NAN)),
                        "i" | "j" => Ok(Expr::Variable(name.to_lowercase())),
                        _ => Ok(Expr::Variable(name)),
                    }
                }
            }
            _ => Err(format!("Unexpected token in primary: {:?}", t)),
        }
    }

    fn parse_function_call(&self, name: String) -> std::result::Result<Expr, String> {
        self.next(); // consume '('
        let mut args = Vec::new();
        if !matches!(self.peek(), Some(Token::RParen)) {
            args.push(self.parse_assign()?);
            while matches!(self.peek(), Some(Token::Comma)) {
                self.next();
                args.push(self.parse_assign()?);
            }
        }
        if self.next() != Some(Token::RParen) {
            return Err("Expected ')' after function arguments".to_string());
        }
        
        // Automatic translation as requested in markdown
        match name.to_lowercase().as_str() {
            "log10" => {
                if args.len() != 1 { return Err("log10 requires exactly 1 argument".to_string()); }
                Ok(Expr::Log {
                    expr: Box::new(args.remove(0)),
                    base: Some(Box::new(Expr::Number(10.0))),
                })
            }
            "log2" => {
                if args.len() != 1 { return Err("log2 requires exactly 1 argument".to_string()); }
                Ok(Expr::Log {
                    expr: Box::new(args.remove(0)),
                    base: Some(Box::new(Expr::Number(2.0))),
                })
            }
            "log" | "ln" => {
                if args.len() == 1 {
                    Ok(Expr::Log { expr: Box::new(args.remove(0)), base: None })
                } else if args.len() == 2 {
                    let expr = args.remove(0);
                    let base = args.remove(0);
                    Ok(Expr::Log { expr: Box::new(expr), base: Some(Box::new(base)) })
                } else {
                    Err("log requires 1 or 2 arguments".to_string())
                }
            }
            "sqrt" => {
                if args.len() != 1 { return Err("sqrt requires exactly 1 argument".to_string()); }
                Ok(Expr::Sqrt { expr: Box::new(args.remove(0)) })
            }
            _ => Ok(Expr::FunctionCall { name, args }),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("parser booting");

    let socket_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/parser.sock".to_string());

    let _ = std::fs::remove_file(&socket_path);
    if let Some(parent) = std::path::Path::new(&socket_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = compat::UnixListener::bind(&socket_path)?;
    tracing::info!("Listening on {}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let (reader, mut writer) = stream.split();
            let mut reader = tokio::io::BufReader::new(reader);
            
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let start = std::time::Instant::now();
                        let request: ModuleRequest = match serde_json::from_str(&line) {
                            Ok(req) => req,
                            Err(e) => {
                                tracing::error!("Failed to parse request: {}", e);
                                break;
                            }
                        };

                        let tokens: Vec<Token> = match serde_json::from_str(&request.input) {
                            Ok(t) => t,
                            Err(e) => {
                                let response = ModuleResponse {
                                    request_id: request.request_id,
                                    output: None,
                                    error: Some(ModuleError {
                                        code: "UNKNOWN_PATTERN".to_string(),
                                        message: format!("Failed to parse tokens: {}", e),
                                        input_position: None,
                                    }),
                                    processing_ms: start.elapsed().as_millis() as u64,
                                };
                                let _ = writer.write_all(&(serde_json::to_vec(&response).unwrap_or_default())).await;
                                let _ = writer.write_all(b"\n").await;
                                continue;
                            }
                        };

                        let parser = Parser::new(tokens);
                        let (output, error) = match parser.parse_expression() {
                            Ok(expr) => {
                                if parser.pos.get() < parser.tokens.len() {
                                    (
                                        None,
                                        Some(ModuleError {
                                            code: "UNKNOWN_PATTERN".to_string(),
                                            message: format!("Unexpected token at end: {:?}", parser.tokens.get(parser.pos.get())),
                                            input_position: Some(parser.pos.get()),
                                        }),
                                    )
                                } else {
                                    let json = serde_json::to_string(&expr).unwrap();
                                    (Some(json), None)
                                }
                            }
                            Err(e) => {
                                (
                                    None,
                                    Some(ModuleError {
                                        code: "UNKNOWN_PATTERN".to_string(),
                                        message: e,
                                        input_position: Some(parser.pos.get()),
                                    }),
                                )
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
                            if writer.write_all(&payload).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,parser=debug"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

pub mod compat {
    #[cfg(windows)]
    pub use windows::*;

    #[cfg(unix)]
    pub use tokio::net::{UnixListener, UnixStream};

    #[cfg(windows)]
    mod windows {
        use std::net::SocketAddr;
        use std::path::Path;
        use std::pin::Pin;
        use std::task::{Context, Poll};
        use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
        use tokio::net::{TcpListener, TcpStream};

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

        pub struct UnixListener {
            inner: TcpListener,
        }

        impl UnixListener {
            pub fn bind(path: impl AsRef<Path>) -> std::io::Result<Self> {
                let port = path_to_port(path);
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                let std_listener = std::net::TcpListener::bind(addr)?;
                let inner = TcpListener::from_std(std_listener)?;
                Ok(Self { inner })
            }

            pub async fn accept(&self) -> std::io::Result<(UnixStream, SocketAddr)> {
                let (stream, addr) = self.inner.accept().await?;
                Ok((UnixStream { inner: stream }, addr))
            }
        }

        pub struct UnixStream {
            inner: TcpStream,
        }

        impl UnixStream {
            pub async fn connect(path: impl AsRef<Path>) -> std::io::Result<Self> {
                let port = path_to_port(path);
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                let inner = TcpStream::connect(addr).await?;
                Ok(Self { inner })
            }

            pub fn split(self) -> (tokio::io::ReadHalf<Self>, tokio::io::WriteHalf<Self>) {
                tokio::io::split(self)
            }
        }

        impl AsyncRead for UnixStream {
            fn poll_read(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.inner).poll_read(cx, buf)
            }
        }

        impl AsyncWrite for UnixStream {
            fn poll_write(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                Pin::new(&mut self.inner).poll_write(cx, buf)
            }

            fn poll_flush(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.inner).poll_flush(cx)
            }

            fn poll_shutdown(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Pin::new(&mut self.inner).poll_shutdown(cx)
            }
        }
    }
}
