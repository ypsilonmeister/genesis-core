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
    Sin,
    Cos,
    Tan,
    Log,
    Ln,
    Exp,
    Abs,
    /// ASCII アルファベット始まりの英数字・アンダースコア列 (sin, cos, log2, x, y 等)
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

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("tokenizer booting");

    let socket_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/tokenizer.sock".to_string());

    let _ = std::fs::remove_file(&socket_path);
    if let Some(parent) = std::path::Path::new(&socket_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let listener = UnixListener::bind(&socket_path)?;
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

                        let (output, error) = match tokenize(&request.input) {
                            Ok(tokens) => {
                                let json = serde_json::to_string(&tokens).unwrap();
                                (Some(json), None)
                            }
                            Err(e) => {
                                let pos = if let TokenizeError::UnknownPattern { position } = e {
                                    Some(position)
                                } else {
                                    None
                                };
                                (
                                    None,
                                    Some(ModuleError {
                                        code: "UNKNOWN_PATTERN".to_string(),
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

pub fn tokenize(input: &str) -> Result<Vec<Token>, TokenizeError> {
    if input.is_empty() {
        return Err(TokenizeError::Empty);
    }

    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some(&(idx, c)) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '0'..='9' | '０'..='９' | '.' | '．' => {
                if c == '.' || c == '．' {
                    let mut temp = chars.clone();
                    temp.next();
                    if let Some(&(_, next_c)) = temp.peek() {
                        if next_c == '.' || next_c == '．' {
                            tokens.push(Token::DotDot);
                            chars.next();
                            chars.next();
                            continue;
                        }
                    }
                }

                if c == '0' || c == '０' {
                    let mut temp = chars.clone();
                    temp.next();
                    if let Some(&(_, next_c)) = temp.peek() {
                        match next_c {
                            'x' | 'X' | 'ｘ' | 'Ｘ' => {
                                chars.next(); chars.next();
                                let mut buf = String::new();
                                while let Some(&(_, nc)) = chars.peek() {
                                    if nc.is_ascii_hexdigit() || nc == '_' || ('ａ'..='ｆ').contains(&nc) || ('Ａ'..='Ｆ').contains(&nc) {
                                        if nc != '_' {
                                            let digit = if ('ａ'..='ｆ').contains(&nc) {
                                                std::char::from_u32(nc as u32 - 0xFEE0).unwrap()
                                            } else if ('Ａ'..='Ｆ').contains(&nc) {
                                                std::char::from_u32(nc as u32 - 0xFEE0).unwrap()
                                            } else {
                                                nc
                                            };
                                            buf.push(digit);
                                        }
                                        chars.next();
                                    } else { break; }
                                }
                                if buf.is_empty() { return Err(TokenizeError::UnknownPattern { position: idx }); }
                                let n = u64::from_str_radix(&buf, 16).map_err(|_| TokenizeError::UnknownPattern { position: idx })?;
                                tokens.push(Token::Number(n as f64));
                                continue;
                            }
                            'b' | 'B' | 'ｂ' | 'Ｂ' => {
                                chars.next(); chars.next();
                                let mut buf = String::new();
                                while let Some(&(_, nc)) = chars.peek() {
                                    if nc == '0' || nc == '1' || nc == '０' || nc == '１' || nc == '_' {
                                        let val = if nc == '０' { '0' } else if nc == '１' { '1' } else { nc };
                                        if val != '_' { buf.push(val); }
                                        chars.next();
                                    } else { break; }
                                }
                                if buf.is_empty() { return Err(TokenizeError::UnknownPattern { position: idx }); }
                                let n = u64::from_str_radix(&buf, 2).map_err(|_| TokenizeError::UnknownPattern { position: idx })?;
                                tokens.push(Token::Number(n as f64));
                                continue;
                            }
                            'o' | 'O' | 'ｏ' | 'Ｏ' => {
                                chars.next(); chars.next();
                                let mut buf = String::new();
                                while let Some(&(_, nc)) = chars.peek() {
                                    if ('0'..='7').contains(&nc) || ('０'..='７').contains(&nc) || nc == '_' {
                                        let val = if ('０'..='７').contains(&nc) { std::char::from_u32(nc as u32 - 0xFEE0).unwrap() } else { nc };
                                        if val != '_' { buf.push(val); }
                                        chars.next();
                                    } else { break; }
                                }
                                if buf.is_empty() { return Err(TokenizeError::UnknownPattern { position: idx }); }
                                let n = u64::from_str_radix(&buf, 8).map_err(|_| TokenizeError::UnknownPattern { position: idx })?;
                                tokens.push(Token::Number(n as f64));
                                continue;
                            }
                            _ => {}
                        }
                    }
                }

                let mut buf = String::new();
                let mut has_dot = false;
                let mut has_e = false;
                let start_idx = idx;

                while let Some(&(_pos, c)) = chars.peek() {
                    match c {
                        '0'..='9' => { buf.push(c); chars.next(); }
                        '０'..='９' => { buf.push(std::char::from_u32(c as u32 - 0xFEE0).unwrap()); chars.next(); }
                        '_' => { chars.next(); }
                        ',' | '，' => {
                            let mut temp = chars.clone();
                            temp.next();
                            if let Some(&(_, next_c)) = temp.peek() {
                                if next_c.is_ascii_digit() || ('０'..='９').contains(&next_c) {
                                    chars.next();
                                    continue;
                                }
                            }
                            break;
                        }
                        '.' | '．' => {
                            if has_dot || has_e { break; }
                            let mut temp = chars.clone();
                            temp.next();
                            if let Some(&(_, next_c)) = temp.peek() {
                                if next_c == '.' || next_c == '．' {
                                    break;
                                }
                            }
                            has_dot = true;
                            buf.push('.');
                            chars.next();
                        }
                        'e' | 'E' | 'ｅ' | 'Ｅ' => {
                            if has_e { break; }
                            has_e = true;
                            buf.push('e');
                            chars.next();
                            if let Some(&(_, next_c)) = chars.peek() {
                                if next_c == '+' || next_c == '-' || next_c == '＋' || next_c == '－' || next_c == '−' {
                                    buf.push(if next_c == '＋' { '+' } else if next_c == '－' || next_c == '−' { '-' } else { next_c });
                                    chars.next();
                                }
                            }
                        }
                        _ => break,
                    }
                }
                if buf == "." || buf.is_empty() {
                     return Err(TokenizeError::UnknownPattern { position: start_idx });
                }
                let val = buf.parse::<f64>().map_err(|_| TokenizeError::UnknownPattern { position: start_idx })?;
                tokens.push(Token::Number(val));
            }
            '+' | '＋' => { tokens.push(Token::Plus); chars.next(); }
            '-' | '－' | '−' | '–' | '—' => { tokens.push(Token::Minus); chars.next(); }
            '*' | '＊' | '×' | '⋅' | '·' => {
                chars.next();
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '*' || next_c == '＊' || next_c == '×' || next_c == '⋅' || next_c == '·' {
                        tokens.push(Token::StarStar);
                        chars.next();
                    } else {
                        tokens.push(Token::Star);
                    }
                } else {
                    tokens.push(Token::Star);
                }
            }
            '/' | '／' | '÷' | '∕' => {
                chars.next();
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '/' || next_c == '／' || next_c == '÷' || next_c == '∕' {
                        tokens.push(Token::DoubleSlash);
                        chars.next();
                    } else {
                        tokens.push(Token::Slash);
                    }
                } else {
                    tokens.push(Token::Slash);
                }
            }
            '^' | '＾' | '\u{02C6}' | '\u{2303}' => { tokens.push(Token::Caret); chars.next(); }
            '(' | '（' => { tokens.push(Token::LParen); chars.next(); }
            ')' | '）' => { tokens.push(Token::RParen); chars.next(); }
            '[' | '［' => { tokens.push(Token::LBracket); chars.next(); }
            ']' | '］' => { tokens.push(Token::RBracket); chars.next(); }
            '{' | '｛' => { tokens.push(Token::LBrace); chars.next(); }
            '}' | '｝' => { tokens.push(Token::RBrace); chars.next(); }
            ',' | '，' => { tokens.push(Token::Comma); chars.next(); }
            '!' | '！' => {
                chars.next();
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '=' || next_c == '＝' {
                        tokens.push(Token::Ne);
                        chars.next();
                    } else {
                        tokens.push(Token::Exclamation);
                    }
                } else {
                    tokens.push(Token::Exclamation);
                }
            }
            '?' | '？' => { tokens.push(Token::Question); chars.next(); }
            ':' | '：' => { tokens.push(Token::Colon); chars.next(); }
            '%' | '％' => { tokens.push(Token::Percent); chars.next(); }
            '@' | '＠' => { tokens.push(Token::At); chars.next(); }
            '$' | '＄' => { tokens.push(Token::Dollar); chars.next(); }
            '&' | '＆' => {
                chars.next();
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '&' || next_c == '＆' {
                        tokens.push(Token::LogicalAnd);
                        chars.next();
                    } else {
                        tokens.push(Token::Ampersand);
                    }
                } else {
                    tokens.push(Token::Ampersand);
                }
            }
            '|' | '｜' => {
                chars.next();
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '|' || next_c == '｜' {
                        tokens.push(Token::LogicalOr);
                        chars.next();
                    } else {
                        tokens.push(Token::Pipe);
                    }
                } else {
                    tokens.push(Token::Pipe);
                }
            }
            '=' | '＝' => {
                chars.next();
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '=' || next_c == '＝' {
                        tokens.push(Token::Eq);
                        chars.next();
                    } else {
                        tokens.push(Token::Assign);
                    }
                } else {
                    tokens.push(Token::Assign);
                }
            }
            '>' | '＞' => {
                chars.next();
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '>' || next_c == '＞' {
                        tokens.push(Token::RShift);
                        chars.next();
                    } else if next_c == '=' || next_c == '＝' {
                        tokens.push(Token::Ge);
                        chars.next();
                    } else {
                        tokens.push(Token::Gt);
                    }
                } else {
                    tokens.push(Token::Gt);
                }
            }
            '<' | '＜' => {
                chars.next();
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '<' || next_c == '＜' {
                        tokens.push(Token::LShift);
                        chars.next();
                    } else if next_c == '=' || next_c == '＝' {
                        tokens.push(Token::Le);
                        chars.next();
                    } else {
                        tokens.push(Token::Lt);
                    }
                } else {
                    tokens.push(Token::Lt);
                }
            }
            ';' | '；' => { tokens.push(Token::Semicolon); chars.next(); }
            '"' | '＂' | '\'' | '＇' => {
                let quote = c;
                chars.next();
                let mut buf = String::new();
                let mut closed = false;
                while let Some(&(_pos, nc)) = chars.peek() {
                    if nc == quote {
                        chars.next();
                        closed = true;
                        break;
                    }
                    buf.push(nc);
                    chars.next();
                }
                if !closed {
                    return Err(TokenizeError::UnknownPattern { position: idx });
                }
                tokens.push(Token::String(buf));
            }
            '√' | '∛' | '∜' => {
                tokens.push(match c {
                    '√' => Token::Sqrt,
                    '∛' => Token::Cbrt,
                    _ => Token::Function("root4".to_string()),
                });
                chars.next();
            }
            '±' | '∓' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '²' | '³' | '¹' => {
                tokens.push(Token::Caret);
                tokens.push(Token::Number(match c {
                    '²' => 2.0,
                    '³' => 3.0,
                    _ => 1.0,
                }));
                chars.next();
            }
            '½' | '¼' | '¾' | '⅓' | '⅔' | '⅛' | '⅜' | '⅝' | '⅞' => {
                tokens.push(Token::Number(match c {
                    '½' => 0.5,
                    '¼' => 0.25,
                    '¾' => 0.75,
                    '⅓' => 1.0/3.0,
                    '⅔' => 2.0/3.0,
                    '⅛' => 0.125,
                    '⅜' => 0.375,
                    '⅝' => 0.625,
                    '⅞' => 0.875,
                    _ => 0.0,
                }));
                chars.next();
            }
            c if c.is_alphabetic() || c == '_' || c == 'π' || c == 'τ' || c == 'φ' || c == '∞' => {
                let mut buf = String::new();
                while let Some(&(_pos, nc)) = chars.peek() {
                    if nc.is_alphanumeric() || nc == '_' || nc == 'π' || nc == 'τ' || nc == 'φ' || nc == '∞' {
                        let val = if ('ａ'..='ｚ').contains(&nc) {
                            std::char::from_u32(nc as u32 - 0xFEE0).unwrap()
                        } else if ('Ａ'..='Ｚ').contains(&nc) {
                            std::char::from_u32(nc as u32 - 0xFEE0).unwrap()
                        } else if ('０'..='９').contains(&nc) {
                            std::char::from_u32(nc as u32 - 0xFEE0).unwrap()
                        } else {
                            nc
                        };
                        buf.push(val);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match buf.to_lowercase().as_str() {
                    "nan" => tokens.push(Token::NaN),
                    "infinity" | "inf" | "∞" => tokens.push(Token::Infinity),
                    "sqrt" => tokens.push(Token::Sqrt),
                    "cbrt" => tokens.push(Token::Cbrt),
                    "pi" | "π" => tokens.push(Token::Pi),
                    "e" => tokens.push(Token::E),
                    "sum" => tokens.push(Token::Sum),
                    "sin" => tokens.push(Token::Sin),
                    "cos" => tokens.push(Token::Cos),
                    "tan" => tokens.push(Token::Tan),
                    "log" | "log10" => tokens.push(Token::Log),
                    "ln" => tokens.push(Token::Ln),
                    "exp" => tokens.push(Token::Exp),
                    "abs" => tokens.push(Token::Abs),
                    "xor" => tokens.push(Token::BitXor),
                    "mod" => tokens.push(Token::Percent),
                    _ => tokens.push(Token::Function(buf)),
                }
            }
            _ => {
                return Err(TokenizeError::UnknownPattern { position: idx });
            }
        }
    }
    Ok(tokens)
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tokenizer=debug"));
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