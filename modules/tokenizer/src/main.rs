// =============================================================================
// # CMP Module Charter
//
// What:
//   Deconstruct normalized strings into a sequence of numbers, operators, and parentheses tokens.
//
// Invariants:
//   - Unrecognized tokens must return an error (silent ignores are prohibited).
//   - The order of the token sequence must preserve the input order.
//
// Boundaries:
//   - Dependencies: normalizer
//   - Dependents: parser
//
// Extensible:
//   - Addition of recognized token types (e.g., function names, constants, units, etc.)
//
// Why:
//   Isolate lexical analysis so that the parser can focus on syntax parsing.
// =============================================================================

use crate::compat::UnixListener;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::env;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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
    NaN,
    Infinity,
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    Caret,
    LParen,
    RParen,
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
    /// ASCII アルファベット始まりの英数字・アンダースコア列 (sin, cos, log2 等)
    Function(String),
    /// 文字列リテラル
    String(String),
}

#[derive(Debug, thiserror::Error)]
pub enum TokenizeError {
    #[error("unknown pattern at position {position}")]
    UnknownPattern { position: usize },
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
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '0'..='9' | '.' => {
                // Check for '..' range operator
                if c == '.' {
                    let mut temp = chars.clone();
                    temp.next();
                    if let Some(&(_, '.')) = temp.peek() {
                        tokens.push(Token::DotDot);
                        chars.next();
                        chars.next();
                        continue;
                    }
                }

                let mut buf = String::new();
                let mut has_dot = false;
                let mut has_e = false;
                let mut has_exponent_dot = false;
                let start_idx = idx;

                while let Some(&(pos, c)) = chars.peek() {
                    if c.is_ascii_digit() {
                        buf.push(c);
                        chars.next();
                    } else if c == '_' {
                        chars.next(); // Skip underscore in numbers
                    } else if c == '.' {
                        // Check for range operator inside number context
                        let mut temp = chars.clone();
                        temp.next();
                        if let Some(&(_, '.')) = temp.peek() {
                            break;
                        }

                        if !has_dot && !has_e {
                            buf.push(c);
                            chars.next();
                            has_dot = true;
                        } else if has_e && !has_exponent_dot {
                            buf.push(c);
                            chars.next();
                            has_exponent_dot = true;
                        } else {
                            // Malformed number (e.g., 1.2.3 or 1e1.2.3)
                            return Err(TokenizeError::UnknownPattern { position: pos });
                        }
                    } else if (c == 'e' || c == 'E') && !has_e {
                        buf.push(c);
                        chars.next();
                        has_e = true;
                        if let Some(&(_, next_c)) = chars.peek() {
                            if next_c == '+' || next_c == '-' {
                                buf.push(next_c);
                                chars.next();
                            }
                        }
                    } else {
                        break;
                    }
                }

                let n: f64 = if has_exponent_dot {
                    let parts: Vec<&str> = buf.split(|c| ['e', 'E'].contains(&c)).collect();
                    if parts.len() == 2 {
                        let mantissa: f64 =
                            parts[0]
                                .parse()
                                .map_err(|_| TokenizeError::UnknownPattern {
                                    position: start_idx,
                                })?;
                        let exponent: f64 =
                            parts[1]
                                .parse()
                                .map_err(|_| TokenizeError::UnknownPattern {
                                    position: start_idx,
                                })?;
                        mantissa * 10.0_f64.powf(exponent)
                    } else {
                        return Err(TokenizeError::UnknownPattern {
                            position: start_idx,
                        });
                    }
                } else {
                    buf.parse().map_err(|_| TokenizeError::UnknownPattern {
                        position: start_idx,
                    })?
                };

                if n.is_nan() {
                    tokens.push(Token::NaN);
                } else if n.is_infinite() {
                    tokens.push(Token::Infinity);
                } else {
                    tokens.push(Token::Number(n));
                }
            }
            'a'..='z' | 'A'..='Z' => {
                let mut buf = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        buf.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }

                let lower = buf.to_lowercase();
                if lower == "nan" {
                    tokens.push(Token::NaN);
                } else if lower == "infinity" || lower == "inf" {
                    tokens.push(Token::Infinity);
                } else {
                    tokens.push(Token::Function(buf));
                }
            }
            '+' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '-' => {
                tokens.push(Token::Minus);
                chars.next();
            }
            '*' => {
                chars.next();
                if let Some(&(_, '*')) = chars.peek() {
                    tokens.push(Token::StarStar);
                    chars.next();
                } else {
                    tokens.push(Token::Star);
                }
            }
            '/' => {
                tokens.push(Token::Slash);
                chars.next();
            }
            '^' => {
                tokens.push(Token::Caret);
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            ',' => {
                tokens.push(Token::Comma);
                chars.next();
            }
            '!' => {
                chars.next();
                if let Some(&(_, '=')) = chars.peek() {
                    tokens.push(Token::Ne);
                    chars.next();
                } else {
                    tokens.push(Token::Exclamation);
                }
            }
            '?' => {
                tokens.push(Token::Question);
                chars.next();
            }
            ':' => {
                tokens.push(Token::Colon);
                chars.next();
            }
            '>' => {
                chars.next();
                if let Some(&(_, '>')) = chars.peek() {
                    tokens.push(Token::RShift);
                    chars.next();
                } else if let Some(&(_, '=')) = chars.peek() {
                    tokens.push(Token::Ge);
                    chars.next();
                } else {
                    tokens.push(Token::Gt);
                }
            }
            '<' => {
                chars.next();
                if let Some(&(_, '<')) = chars.peek() {
                    tokens.push(Token::LShift);
                    chars.next();
                } else if let Some(&(_, '=')) = chars.peek() {
                    tokens.push(Token::Le);
                    chars.next();
                } else {
                    tokens.push(Token::Lt);
                }
            }
            '=' => {
                chars.next();
                if let Some(&(_, '=')) = chars.peek() {
                    tokens.push(Token::Eq);
                    chars.next();
                } else {
                    tokens.push(Token::Assign);
                }
            }
            '&' => {
                chars.next();
                if let Some(&(_, '&')) = chars.peek() {
                    tokens.push(Token::LogicalAnd);
                    chars.next();
                } else {
                    tokens.push(Token::Ampersand);
                }
            }
            '|' => {
                chars.next();
                if let Some(&(_, '|')) = chars.peek() {
                    tokens.push(Token::LogicalOr);
                    chars.next();
                } else {
                    tokens.push(Token::Pipe);
                }
            }
            '%' => {
                tokens.push(Token::Percent);
                chars.next();
            }
            '@' => {
                tokens.push(Token::At);
                chars.next();
            }
            ';' => {
                tokens.push(Token::Semicolon);
                chars.next();
            }
            '√' => {
                tokens.push(Token::Sqrt);
                chars.next();
            }
            '∛' => {
                tokens.push(Token::Cbrt);
                chars.next();
            }
            '∞' => {
                tokens.push(Token::Infinity);
                chars.next();
            }
            'π' => {
                tokens.push(Token::Pi);
                chars.next();
            }
            'Σ' => {
                tokens.push(Token::Sum);
                chars.next();
            }
            '$' => {
                tokens.push(Token::Dollar);
                chars.next();
            }
            '"' => {
                chars.next();
                let mut buf = String::new();
                let mut closed = false;
                while let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '"' {
                        closed = true;
                        chars.next();
                        break;
                    }
                    buf.push(next_c);
                    chars.next();
                }
                if !closed {
                    return Err(TokenizeError::UnknownPattern { position: idx });
                }
                tokens.push(Token::String(buf));
            }
            _ => {
                return Err(TokenizeError::UnknownPattern { position: idx });
            }
        }
    }

    Ok(tokens)
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("tokenizer booting (v1.2.1)");

    let addr_or_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/tokenizer.sock".to_string());

    if addr_or_path.starts_with("tcp://") {
        let addr = addr_or_path.strip_prefix("tcp://").unwrap();
        let listener = TcpListener::bind(addr).await?;
        tracing::info!("Listening on TCP {}", addr);
        loop {
            let (stream, _) = listener.accept().await?;
            tokio::spawn(async move {
                let _ = handle_client(stream).await;
            });
        }
    } else {
        let uds_path = addr_or_path.strip_prefix("uds://").unwrap_or(&addr_or_path);
        let _ = std::fs::remove_file(uds_path);
        if let Some(parent) = std::path::Path::new(uds_path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(uds_path)?;
        tracing::info!("Listening on UDS {}", uds_path);
        loop {
            let (stream, _) = listener.accept().await?;
            tokio::spawn(async move {
                let _ = handle_client(stream).await;
            });
        }
    }
}

async fn handle_client<S>(stream: S) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(reader);
    let mut line = String::new();

    if let Ok(n) = reader.read_line(&mut line).await {
        if n == 0 {
            return Ok(());
        }
        let start = std::time::Instant::now();
        let request: ModuleRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                tracing::error!("Failed to parse request: {}", e);
                return Ok(());
            }
        };

        let (output, error) = match tokenize(&request.input) {
            Ok(tokens) => match serde_json::to_string(&tokens) {
                Ok(json) => (Some(json), None),
                Err(e) => (
                    None,
                    Some(ModuleError {
                        code: "SERIALIZE_ERROR".to_string(),
                        message: e.to_string(),
                        input_position: None,
                    }),
                ),
            },
            Err(e) => {
                let (code, pos) = match e {
                    TokenizeError::UnknownPattern { position } => {
                        ("UNKNOWN_PATTERN", Some(position))
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
    Ok(())
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
        assert!(matches!(err, TokenizeError::UnknownPattern { .. }));
    }

    #[test]
    fn ignores_cr_lf() {
        let tokens = tokenize("3 + 5\r\n").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Number(3.0), Token::Plus, Token::Number(5.0)]
        );
    }

    #[test]
    fn tokenizes_scientific() {
        let tokens = tokenize("1.2e3 + 2.5e-1").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Number(1200.0), Token::Plus, Token::Number(0.25),]
        );
    }

    #[test]
    fn tokenizes_decimal_exponent() {
        let tokens = tokenize("3.14e1.5").unwrap();
        if let Token::Number(n) = tokens[0] {
            assert!((n - 99.2955).abs() < 0.001);
        } else {
            panic!("Expected Number token");
        }
    }

    #[test]
    fn tokenizes_comma_and_exclamation() {
        let tokens = tokenize("sum(1, 2)!").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Function("sum".to_string()),
                Token::LParen,
                Token::Number(1.0),
                Token::Comma,
                Token::Number(2.0),
                Token::RParen,
                Token::Exclamation,
            ]
        );
    }

    #[test]
    fn tokenizes_new_v1_2_tokens() {
        let tokens = tokenize("√144 + 3 << 2 ^ 1..10").unwrap();
        assert_eq!(tokens[0], Token::Sqrt);
        assert_eq!(tokens[4], Token::LShift);
        assert_eq!(tokens[8], Token::DotDot);
    }

    #[test]
    fn tokenizes_ternary_and_gt() {
        let tokens = tokenize("(10 > 5) ? 1 : 0").unwrap();
        assert!(tokens.contains(&Token::Gt));
        assert!(tokens.contains(&Token::Question));
        assert!(tokens.contains(&Token::Colon));
    }

    #[test]
    fn tokenizes_nan_and_infinity() {
        assert_eq!(tokenize("1.0e999").unwrap(), vec![Token::Infinity]);
        assert_eq!(tokenize("infinity").unwrap(), vec![Token::Infinity]);
        assert_eq!(tokenize("inf").unwrap(), vec![Token::Infinity]);
        assert_eq!(tokenize("NaN").unwrap(), vec![Token::NaN]);
    }

    #[test]
    fn rejects_malformed_numbers() {
        assert!(matches!(
            tokenize("1.2.3").unwrap_err(),
            TokenizeError::UnknownPattern { .. }
        ));
        assert!(matches!(
            tokenize("1e1.5.5").unwrap_err(),
            TokenizeError::UnknownPattern { .. }
        ));
    }

    #[test]
    fn tokenizes_expanded_patterns() {
        // Now supporting logical operators and equality
        assert!(tokenize("a && b").is_ok());
        assert!(tokenize("x = 10;").is_ok());
        assert!(tokenize("5! + 10 == 60").is_ok());
        assert!(tokenize("Σ(x^2)").is_ok());
        assert!(tokenize("10 <= 20 && 30 != 40").is_ok());
        assert!(tokenize("∛27").is_ok());
    }

    #[test]
    fn tokenizes_expanded_patterns_extra() {
        assert!(tokenize("log(0)").is_ok());
        assert!(tokenize("10 ** 100").is_ok());
        assert!(tokenize("3.14**2 / 0.1").is_ok());
        assert!(tokenize("sqrt(-1)").is_ok());
        assert!(tokenize("a + b * c / d").is_ok());
        assert!(tokenize("10 / (2 - 2) + ∞").is_ok());

        // Newly added tokens
        assert!(tokenize("3 + 5ππ2").is_ok());
        assert!(tokenize("5 * (2 + log(10)) + 3").is_ok());
        assert!(tokenize("3 + 5 * 2 % 3").is_ok());
        assert!(tokenize("4.5 * (2 + 1) ** 3").is_ok());
        assert!(tokenize("(1 + (2 * 3)) + $5").is_ok());
        assert!(tokenize("√(16) + π").is_ok());

        // String literal tests
        assert!(tokenize("3 + \"5\" * 2.5").is_ok());
    }
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
                std_listener.set_nonblocking(true)?;
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

        // Standard poll_read matching Tokio's trait
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
