// =============================================================================
// # CMP Module Charter
//
// What:
//   繝医・繧ｯ繝ｳ蛻励ｒ貍皮ｮ怜ｭ仙━蜈亥ｺｦ繧定・・縺励◆謚ｽ雎｡讒区枚譛ｨ (AST) 縺ｫ螟画鋤縺吶ｋ縲・//
// Invariants:
//   - 貍皮ｮ怜ｭ仙━蜈亥ｺｦ: * / 縺ｯ + - 繧医ｊ鬮倥＞
//   - 諡ｬ蠑ｧ縺ｫ繧医ｋ蜆ｪ蜈亥ｺｦ螟画峩繧呈ｭ｣縺励￥蜃ｦ逅・☆繧・//   - 荳肴ｭ｣縺ｪ譁・ｳ・貍皮ｮ怜ｭ宣｣邯壹∵峡蠑ｧ荳堺ｸ閾ｴ遲・縺ｯ繧ｨ繝ｩ繝ｼ繧定ｿ斐☆
//
// Boundaries:
//   - 萓晏ｭ伜・: tokenizer
//   - 陲ｫ萓晏ｭ伜・: evaluator
//
// Extensible:
//   - 譁ｰ縺励＞貍皮ｮ怜ｭ舌・髢｢謨ｰ蜻ｼ縺ｳ蜃ｺ縺玲ｧ区枚縺ｮ霑ｽ蜉
//
// Why:
//   evaluator 縺檎ｴ皮ｲ九↑險育ｮ励↓髮・ｸｭ縺ｧ縺阪ｋ繧医≧縲∵枚豕戊ｧ｣譫舌ｒ蛻・屬縺吶ｋ縲・// =============================================================================

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
#[serde(tag = "type", content = "value")]
pub enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
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
pub enum ParseError {
    #[error("syntax error: {0}")]
    Syntax(String),
    #[error("unexpected token: {0:?}")]
    UnexpectedToken(Token),
    #[error("unexpected end of input")]
    UnexpectedEof,
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_expr()?;
        if self.pos < self.tokens.len() {
            return Err(ParseError::Syntax(format!("Unexpected token at {}", self.pos)));
        }
        Ok(expr)
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_add_sub()
    }

    fn parse_add_sub(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul_div()?;
        while let Some(token) = self.peek() {
            match token {
                Token::Plus => {
                    self.consume();
                    let rhs = self.parse_mul_div()?;
                    lhs = Expr::BinOp { op: BinOp::Add, lhs: Box::new(lhs), rhs: Box::new(rhs) };
                }
                Token::Minus => {
                    self.consume();
                    let rhs = self.parse_mul_div()?;
                    lhs = Expr::BinOp { op: BinOp::Sub, lhs: Box::new(lhs), rhs: Box::new(rhs) };
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_mul_div(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_primary()?;
        while let Some(token) = self.peek() {
            match token {
                Token::Star => {
                    self.consume();
                    let rhs = self.parse_primary()?;
                    lhs = Expr::BinOp { op: BinOp::Mul, lhs: Box::new(lhs), rhs: Box::new(rhs) };
                }
                Token::Slash => {
                    self.consume();
                    let rhs = self.parse_primary()?;
                    lhs = Expr::BinOp { op: BinOp::Div, lhs: Box::new(lhs), rhs: Box::new(rhs) };
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let token = self.peek().ok_or(ParseError::UnexpectedEof)?.clone();
        match token {
            Token::Number(n) => {
                self.consume();
                Ok(Expr::Number(n))
            }
            Token::LParen => {
                self.consume();
                let expr = self.parse_expr()?;
                match self.peek() {
                    Some(Token::RParen) => {
                        self.consume();
                        Ok(expr)
                    }
                    _ => Err(ParseError::Syntax("Expected ')'".to_string())),
                }
            }
            token => Err(ParseError::UnexpectedToken(token)),
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn consume(&mut self) {
        self.pos += 1;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("parser booting (v1)");

    let socket_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/parser.sock".to_string());

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

                let tokens: Vec<Token> = match serde_json::from_str(&request.input) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!("Failed to parse tokens from input: {}", e);
                        return;
                    }
                };

                let (output, error) = match Parser::new(tokens).parse() {
                    Ok(expr) => {
                        let json = serde_json::to_string(&expr).unwrap();
                        (Some(json), None)
                    }
                    Err(e) => (None, Some(ModuleError {
                        code: "SYNTAX_ERROR".to_string(),
                        message: e.to_string(),
                        input_position: None,
                    })),
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
        .unwrap_or_else(|_| EnvFilter::new("info,parser=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_expr() {
        let tokens = vec![
            Token::Number(3.0),
            Token::Plus,
            Token::Number(5.0),
            Token::Star,
            Token::Number(2.0),
        ];
        let expr = Parser::new(tokens).parse().unwrap();
        assert!(matches!(expr, Expr::BinOp { op: BinOp::Add, .. }));
    }
}
