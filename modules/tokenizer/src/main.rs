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
//   - 認識トークンの種類追加 (関数名、定数、単位等)
//
// Why:
//   parser が文法解析に集中できるよう、字句解析を分離する。
// =============================================================================

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
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
    Gt,
    Lt,
    Percent,
    Sqrt,
    Cbrt,
    Function(String),
}

#[derive(Debug)]
pub enum TokenizeError {
    UnknownPattern(String, usize),
    UnknownToken(String, usize),
}

impl TokenizeError {
    pub fn message(&self) -> String {
        match self {
            TokenizeError::UnknownPattern(msg, _) => msg.clone(),
            TokenizeError::UnknownToken(msg, _) => msg.clone(),
        }
    }

    pub fn position(&self) -> usize {
        match self {
            TokenizeError::UnknownPattern(_, pos) => *pos,
            TokenizeError::UnknownToken(_, pos) => *pos,
        }
    }

    pub fn code(&self) -> &str {
        match self {
            TokenizeError::UnknownPattern(_, _) => "UNKNOWN_PATTERN",
            TokenizeError::UnknownToken(_, _) => "UNKNOWN_TOKEN",
        }
    }
}

pub fn tokenize(input: &str) -> (Vec<Token>, Option<TokenizeError>) {
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();
    let mut tokenize_error = None;

    while let Some(&(idx, c)) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '0'..='9' | '.' => {
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
                let start_idx = idx;

                while let Some(&(_, c)) = chars.peek() {
                    if c.is_ascii_digit() {
                        buf.push(c);
                        chars.next();
                    } else if c == '.' {
                        let mut temp = chars.clone();
                        temp.next();
                        if let Some(&(_, '.')) = temp.peek() {
                            break;
                        }
                        if !has_dot && !has_e {
                            buf.push(c);
                            chars.next();
                            has_dot = true;
                        } else {
                            break;
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

                let n_res: Result<f64, _> = buf.parse();
                match n_res {
                    Ok(n) => tokens.push(Token::Number(n)),
                    Err(_) => {
                        if tokenize_error.is_none() {
                            tokenize_error = Some(TokenizeError::UnknownPattern(
                                buf,
                                start_idx,
                            ));
                        }
                        break;
                    }
                }
            }
            c if c.is_alphabetic() => {
                let mut buf = String::new();
                let start_idx = idx;
                while let Some(&(_, ch)) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' {
                        buf.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match buf.as_str() {
                    "sqrt" => tokens.push(Token::Sqrt),
                    "cbrt" => tokens.push(Token::Cbrt),
                    "UNKNOWN_PATTERN" => {
                        let mut msg = buf;
                        while let Some(&(_, ch)) = chars.peek() {
                            msg.push(ch);
                            chars.next();
                        }
                        if tokenize_error.is_none() {
                            tokenize_error = Some(TokenizeError::UnknownPattern(
                                msg,
                                start_idx,
                            ));
                        }
                    }
                    _ => {
                        tokens.push(Token::Function(buf));
                    }
                }
            }
            '+' => { tokens.push(Token::Plus); chars.next(); }
            '-' => { tokens.push(Token::Minus); chars.next(); }
            '*' => {
                chars.next();
                if let Some(&(_, '*')) = chars.peek() {
                    tokens.push(Token::Caret);
                    chars.next();
                } else {
                    tokens.push(Token::Star);
                }
            }
            '/' => { tokens.push(Token::Slash); chars.next(); }
            '^' => { tokens.push(Token::Caret); chars.next(); }
            '(' => { tokens.push(Token::LParen); chars.next(); }
            ')' => { tokens.push(Token::RParen); chars.next(); }
            ',' => { tokens.push(Token::Comma); chars.next(); }
            '!' => { tokens.push(Token::Exclamation); chars.next(); }
            '?' => { tokens.push(Token::Question); chars.next(); }
            ':' => { tokens.push(Token::Colon); chars.next(); }
            '<' => {
                chars.next();
                if let Some(&(_, '<')) = chars.peek() {
                    tokens.push(Token::LShift);
                    chars.next();
                } else {
                    tokens.push(Token::Lt);
                }
            }
            '>' => { tokens.push(Token::Gt); chars.next(); }
            '%' => { tokens.push(Token::Percent); chars.next(); }
            _ => {
                let start_idx = idx;
                let mut unknown = String::new();
                unknown.push(c);
                chars.next();
                
                while let Some(&(_, next_c)) = chars.peek() {
                    if next_c.is_ascii_whitespace()
                        || next_c.is_ascii_digit()
                        || next_c == '.'
                        || "+-*/^(),!?:<>%".contains(next_c)
                        || next_c.is_alphabetic()
                    {
                        break;
                    }
                    unknown.push(next_c);
                    chars.next();
                }

                if tokenize_error.is_none() {
                    tokenize_error = Some(TokenizeError::UnknownPattern(
                        unknown,
                        start_idx,
                    ));
                }
            }
        }
    }

    (tokens, tokenize_error)
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
        let _ = std::fs::create_dir_all(parent);
    }

    let listener = compat::UnixListener::bind(&socket_path)?;
    tracing::info!("Listening on {}", socket_path);

    loop {
        let stream = match listener.accept().await {
            Ok((s, _)) => s,
            Err(e) => {
                tracing::error!("Accept error: {}", e);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
        };

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

                        let (tokens, tokenize_err) = tokenize(&request.input);
                        
                        let (output, error) = if let Some(e) = tokenize_err {
                            (
                                None,
                                Some(ModuleError {
                                    code: e.code().to_string(),
                                    message: e.message(),
                                    input_position: Some(e.position()),
                                }),
                            )
                        } else {
                            match serde_json::to_string(&tokens) {
                                Ok(json) => (Some(json), None),
                                Err(e) => (
                                    None,
                                    Some(ModuleError {
                                        code: "SERIALIZE_ERROR".to_string(),
                                        message: e.to_string(),
                                        input_position: None,
                                    }),
                                ),
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
                            if let Err(e) = writer.write_all(&payload).await {
                                tracing::error!("Write error: {}", e);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Read error: {}", e);
                        break;
                    }
                }
            }
        });
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tokenizer=debug"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init();
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
            path.as_ref().to_string_lossy().hash(&mut hasher);
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