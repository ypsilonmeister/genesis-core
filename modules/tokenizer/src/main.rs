// =============================================================================
// # CMP Module Charter
//
// What:
//   正規化済み文字列を数値・演算子・括弧のトークン列に分解する。
//
// Invariants:
//   - 認識できないトークンはエラーとして返す(サイレント無視禁止)
//   - トークン列の順序は入力順を保持する
//
// Boundaries:
//   - 依存先: normalizer
//   - 被依存先: parser
//
// Extensible:
//   - 認識トークンの種類追加 (関数名、定数、単位、etc.)
//
// Why:
//   parser が文法解析に集中できるよう、字句解析を分離する。
// =============================================================================

// v1 実装範囲:
//   ASCII 数字・小数点・四則演算子 (+ - * /)・括弧 ( ) のみ認識。
//   全角数字・全角演算子・自然言語等は UnknownToken エラー。

use anyhow::Result;
use compat::UnixListener;
use serde::{Deserialize, Serialize};
use std::env;
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

#[derive(Debug, thiserror::Error)]
pub enum TokenizeError {
    #[error("unknown token at position {position}: {character:?}")]
    UnknownToken { position: usize, character: char },
    #[error("empty input")]
    Empty,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, TokenizeError> {
    if input.is_empty() {
        return Err(TokenizeError::Empty);
    }

    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some(&(idx, c)) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' => {
                chars.next();
            }
            '0'..='9' | '.' => {
                let mut buf = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_ascii_digit() || c == '.' {
                        buf.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let n: f64 = buf.parse().map_err(|_| TokenizeError::UnknownToken {
                    position: idx,
                    character: c,
                })?;
                tokens.push(Token::Number(n));
            }
            '+' => {
                tokens.next_push(Token::Plus, &mut chars);
            }
            '-' => {
                tokens.next_push(Token::Minus, &mut chars);
            }
            '*' => {
                tokens.next_push(Token::Star, &mut chars);
            }
            '/' => {
                tokens.next_push(Token::Slash, &mut chars);
            }
            '(' => {
                tokens.next_push(Token::LParen, &mut chars);
            }
            ')' => {
                tokens.next_push(Token::RParen, &mut chars);
            }
            other => {
                return Err(TokenizeError::UnknownToken {
                    position: idx,
                    character: other,
                });
            }
        }
    }

    Ok(tokens)
}

// 小さなヘルパ: トークンを push して 1 文字進める
trait NextPush {
    fn next_push<I: Iterator>(&mut self, t: Token, chars: &mut std::iter::Peekable<I>);
}

impl NextPush for Vec<Token> {
    fn next_push<I: Iterator>(&mut self, t: Token, chars: &mut std::iter::Peekable<I>) {
        self.push(t);
        chars.next();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("tokenizer booting (v1)");

    let socket_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/tokenizer.sock".to_string());

    let _ = std::fs::remove_file(&socket_path);
    if let Some(parent) = std::path::Path::new(&socket_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("Listening on {}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(stream);
            let mut reader = tokio::io::BufReader::new(reader);
            let mut line = String::new();

            if let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    return;
                }
                let start = std::time::Instant::now();
                let request: ModuleRequest = match serde_json::from_str(&line) {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!("Failed to parse request: {}", e);
                        return;
                    }
                };

                let (output, error) = match tokenize(&request.input) {
                    Ok(tokens) => {
                        let json = serde_json::to_string(&tokens).unwrap();
                        (Some(json), None)
                    }
                    Err(e) => {
                        let (code, pos) = match e {
                            TokenizeError::UnknownToken { position, .. } => {
                                ("UNKNOWN_TOKEN", Some(position))
                            }
                            TokenizeError::Empty => ("SYNTAX_ERROR", None),
                        };
                        (
                            None,
                            Some(ModuleError {
                                code: code.to_string(),
                                message: e.to_string(),
                                input_position: pos,
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
                    let _ = writer.write_all(&payload).await;
                }
            }
        });
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tokenizer=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_simple_expr() {
        let tokens = tokenize("3 + 5 * 2").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Number(3.0),
                Token::Plus,
                Token::Number(5.0),
                Token::Star,
                Token::Number(2.0),
            ]
        );
    }

    #[test]
    fn rejects_full_width_digit() {
        let err = tokenize("３ + 5").unwrap_err();
        assert!(matches!(err, TokenizeError::UnknownToken { .. }));
    }
}
