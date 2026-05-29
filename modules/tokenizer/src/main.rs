// =============================================================================
// # CMP Module Charter
//
// What:
//   Convert an input string into a sequence of tokens.
//
// Invariants:
//   - Must handle numbers, operators, and identifiers correctly.
//   - Must report position of lexical errors.
//
// Boundaries:
//   - Dependents: parser
//
// Extensible:
//   - New operators and symbols.
//
// Why:
//   Isolate lexical analysis.
// =============================================================================

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::net::SocketAddr;
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

fn is_identifier_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || 
    ('α'..='ω').contains(&c) || ('Α'..='Ω').contains(&c) ||
    c == '√' || c == '∛' || c == '∜' || c == '∞'
}

fn is_identifier_continue(c: char) -> bool {
    is_identifier_start(c) || c.is_ascii_digit()
}

pub fn tokenize(input: &str) -> std::result::Result<Vec<Token>, (String, usize)> {
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }

        match c {
            '+' => tokens.push(Token::Plus),
            '-' => tokens.push(Token::Minus),
            '*' => {
                if chars.peek().map_or(false, |&(_, next_c)| next_c == '*') {
                    chars.next();
                    tokens.push(Token::StarStar);
                } else {
                    tokens.push(Token::Star);
                }
            }
            '/' => {
                if chars.peek().map_or(false, |&(_, next_c)| next_c == '/') {
                    chars.next();
                    tokens.push(Token::DoubleSlash);
                } else {
                    tokens.push(Token::Slash);
                }
            }
            '^' => tokens.push(Token::Caret),
            '(' => tokens.push(Token::LParen),
            ')' => tokens.push(Token::RParen),
            '[' => tokens.push(Token::LBracket),
            ']' => tokens.push(Token::RBracket),
            '{' => tokens.push(Token::LBrace),
            '}' => tokens.push(Token::RBrace),
            ',' => tokens.push(Token::Comma),
            '!' => {
                if chars.peek().map_or(false, |&(_, next_c)| next_c == '=') {
                    chars.next();
                    tokens.push(Token::Ne);
                } else {
                    tokens.push(Token::Factorial);
                }
            }
            '?' => tokens.push(Token::Question),
            ':' => tokens.push(Token::Colon),
            '.' => {
                if chars.peek().map_or(false, |&(_, next_c)| next_c == '.') {
                    chars.next();
                    tokens.push(Token::DotDot);
                } else if chars.peek().map_or(false, |&(_, next_c)| next_c.is_ascii_digit()) {
                    let mut s = String::from("0.");
                    while let Some(&(_, next_c)) = chars.peek() {
                        if next_c.is_ascii_digit() || next_c == '_' {
                            let nc = chars.next().unwrap().1;
                            if nc != '_' { s.push(nc); }
                        } else {
                            break;
                        }
                    }
                    let n: f64 = s.parse().map_err(|e| (format!("Invalid number: {}", e), i))?;
                    tokens.push(Token::Number(n));
                } else {
                    tokens.push(Token::Dot);
                }
            }
            '=' => {
                if chars.peek().map_or(false, |&(_, next_c)| next_c == '=') {
                    chars.next();
                    tokens.push(Token::Eq);
                } else {
                    tokens.push(Token::Assign);
                }
            }
            '<' => {
                if chars.peek().map_or(false, |&(_, next_c)| next_c == '<') {
                    chars.next();
                    tokens.push(Token::LShift);
                } else if chars.peek().map_or(false, |&(_, next_c)| next_c == '=') {
                    chars.next();
                    tokens.push(Token::Le);
                } else {
                    tokens.push(Token::Lt);
                }
            }
            '>' => {
                if chars.peek().map_or(false, |&(_, next_c)| next_c == '>') {
                    chars.next();
                    tokens.push(Token::RShift);
                } else if chars.peek().map_or(false, |&(_, next_c)| next_c == '=') {
                    chars.next();
                    tokens.push(Token::Ge);
                } else {
                    tokens.push(Token::Gt);
                }
            }
            '%' => tokens.push(Token::Mod),
            ';' => tokens.push(Token::Semicolon),
            '√' => tokens.push(Token::Sqrt),
            '∛' => tokens.push(Token::Cbrt),
            '∜' => tokens.push(Token::Function("∜".to_string())),
            '@' => tokens.push(Token::At),
            '$' => tokens.push(Token::Dollar),
            '&' => {
                if chars.peek().map_or(false, |&(_, next_c)| next_c == '&') {
                    chars.next();
                    tokens.push(Token::LogicalAnd);
                } else {
                    tokens.push(Token::Ampersand);
                }
            }
            '|' => {
                if chars.peek().map_or(false, |&(_, next_c)| next_c == '|') {
                    chars.next();
                    tokens.push(Token::LogicalOr);
                } else {
                    tokens.push(Token::Pipe);
                }
            }
            'π' => tokens.push(Token::Pi),
            '∞' => tokens.push(Token::Infinity),
            '"' => {
                let mut s = String::new();
                let mut closed = false;
                while let Some((_, next_c)) = chars.next() {
                    if next_c == '"' {
                        closed = true;
                        break;
                    }
                    s.push(next_c);
                }
                if !closed {
                    return Err(("Unterminated string".to_string(), i));
                }
                tokens.push(Token::String(s));
            }
            '0'..='9' => {
                let mut s = String::new();
                let mut base = 10;
                
                if c == '0' {
                    match chars.peek() {
                        Some(&(_, 'x')) | Some(&(_, 'X')) => { base = 16; chars.next(); }
                        Some(&(_, 'o')) | Some(&(_, 'O')) => { base = 8; chars.next(); }
                        Some(&(_, 'b')) | Some(&(_, 'B')) => { base = 2; chars.next(); }
                        _ => { s.push('0'); }
                    }
                } else {
                    s.push(c);
                }

                let mut has_dot = false;
                while let Some(&(_, next_c)) = chars.peek() {
                    if next_c.is_digit(base) || next_c == '_' {
                        let nc = chars.next().unwrap().1;
                        if nc != '_' { s.push(nc); }
                    } else if next_c == '.' && !has_dot && base == 10 {
                        let mut temp_chars = chars.clone();
                        temp_chars.next();
                        if temp_chars.peek().map_or(false, |&(_, next_next_c)| next_next_c == '.') {
                            break;
                        }
                        has_dot = true;
                        s.push(chars.next().unwrap().1);
                    } else {
                        break;
                    }
                }
                
                if base == 10 {
                    if let Some(&(_, 'e')) | Some(&(_, 'E')) = chars.peek() {
                        let mut temp = chars.clone();
                        temp.next(); // consume e/E
                        let mut s_e = String::new();
                        if let Some(&(_, sign)) = temp.peek() {
                            if sign == '+' || sign == '-' {
                                s_e.push(temp.next().unwrap().1);
                            }
                        }
                        let mut valid_e = false;
                        while let Some(&(_, next_digit)) = temp.peek() {
                            if next_digit.is_ascii_digit() || next_digit == '_' {
                                let nc = temp.next().unwrap().1;
                                if nc != '_' { s_e.push(nc); valid_e = true; }
                            } else {
                                break;
                            }
                        }
                        if valid_e {
                            let e_char = chars.next().unwrap().1;
                            s.push(e_char);
                            s.push_str(&s_e);
                            chars = temp; 
                        }
                    }
                }

                let val = if base == 10 {
                    s.parse::<f64>().map_err(|e| (format!("Invalid number: {}", e), i))?
                } else {
                    u64::from_str_radix(&s, base).map_err(|e| (format!("Invalid integer: {}", e), i))? as f64
                };

                if let Some(&(_, 'i')) | Some(&(_, 'j')) = chars.peek() {
                    chars.next();
                    if val.is_nan() { tokens.push(Token::NaN); }
                    else if val.is_infinite() { tokens.push(Token::Infinity); }
                    else { tokens.push(Token::Imaginary(val)); }
                } else {
                    if val.is_nan() { tokens.push(Token::NaN); }
                    else if val.is_infinite() { tokens.push(Token::Infinity); }
                    else { tokens.push(Token::Number(val)); }
                }
            }
            _ if is_identifier_start(c) => {
                let mut s = String::from(c);
                while let Some(&(_, next_c)) = chars.peek() {
                    if is_identifier_continue(next_c) {
                        s.push(chars.next().unwrap().1);
                    } else {
                        break;
                    }
                }
                let s_lower = s.to_lowercase();
                let token = match s_lower.as_str() {
                    "pi" | "π" => Token::Pi,
                    "e" => Token::E,
                    "inf" | "infinity" | "∞" => Token::Infinity,
                    "nan" => Token::NaN,
                    "sin" => Token::Sin,
                    "cos" => Token::Cos,
                    "tan" => Token::Tan,
                    "asin" => Token::Asin,
                    "acos" => Token::Acos,
                    "atan" => Token::Atan,
                    "sinh" => Token::Sinh,
                    "cosh" => Token::Cosh,
                    "tanh" => Token::Tanh,
                    "log" => Token::Log,
                    "log10" => Token::Log10,
                    "log2" => Token::Log2,
                    "ln" => Token::Ln,
                    "exp" => Token::Exp,
                    "abs" => Token::Abs,
                    "sqrt" | "√" => Token::Sqrt,
                    "cbrt" | "∛" => Token::Cbrt,
                    "sum" => Token::Sum,
                    "integral" => Token::Integral,
                    "mod" | "rem" => Token::Mod,
                    "pow" | "power" => Token::Pow,
                    "xor" => Token::BitXor,
                    "and" => Token::LogicalAnd,
                    "or" => Token::LogicalOr,
                    "i" => Token::I,
                    "j" => Token::J,
                    _ => {
                        if s_lower.starts_with('d') && s_lower.len() > 1 {
                             Token::Differential(s[1..].to_string())
                        } else {
                             Token::Function(s)
                        }
                    }
                };
                tokens.push(token);
            }
            _ => return Err((format!("Unexpected character: {}", c), i)),
        }
    }
    Ok(tokens)
}

async fn send_response<W>(writer: &mut W, request_id: String, output: Option<String>, error: Option<ModuleError>, processing_ms: u64)
where W: AsyncWriteExt + Unpin {
    let response = ModuleResponse { request_id, output, error, processing_ms };
    if let Ok(payload) = serde_json::to_vec(&response) {
        let mut full_payload = payload;
        full_payload.push(b'\n');
        let _ = writer.write_all(&full_payload).await;
        let _ = writer.flush().await;
    }
}

#[cfg(not(unix))]
fn path_to_port(path: impl AsRef<std::path::Path>) -> u16 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    let file_name = path.as_ref().file_name().map(|f| f.to_string_lossy()).unwrap_or_else(|| path.as_ref().to_string_lossy());
    file_name.hash(&mut hasher);
    let hash = hasher.finish();
    (10000 + (hash % 35000)) as u16
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("tokenizer booting");

    let addr_or_path = env::args().nth(1).unwrap_or_else(|| "/tmp/genesis-core/tokenizer.sock".to_string());

    if addr_or_path.starts_with("tcp://") {
        let addr_str = addr_or_path.strip_prefix("tcp://").context("Invalid TCP address format")?;
        let listener = TcpListener::bind(addr_str).await.context(format!("Failed to bind to TCP {}", addr_str))?;
        loop {
            match listener.accept().await {
                Ok((stream, _)) => { tokio::spawn(async move { let _ = handle_client(stream).await; }); }
                Err(e) => { tracing::error!("accept error: {}", e); }
            }
        }
    } else {
        #[cfg(unix)]
        {
            let uds_path = addr_or_path.strip_prefix("uds://").unwrap_or(&addr_or_path);
            let _ = std::fs::remove_file(uds_path);
            if let Some(parent) = std::path::Path::new(uds_path).parent() { let _ = std::fs::create_dir_all(parent); }
            let listener = UnixListener::bind(uds_path).context(format!("Failed to bind to UDS {}", uds_path))?;
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => { tokio::spawn(async move { let _ = handle_client(stream).await; }); }
                    Err(e) => { tracing::error!("accept error: {}", e); }
                }
            }
        }
        #[cfg(not(unix))]
        {
            let uds_path = addr_or_path.strip_prefix("uds://").unwrap_or(&addr_or_path);
            let port = path_to_port(uds_path);
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            let socket = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::STREAM,
                None,
            )?;
            socket.set_reuse_address(true)?;
            socket.bind(&addr.into())?;
            socket.listen(128)?;
            let std_listener: std::net::TcpListener = socket.into();
            std_listener.set_nonblocking(true)?;
            let listener = TcpListener::from_std(std_listener)?;
            tracing::info!("tokenizer listening on TCP {}", addr);
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => { tokio::spawn(async move { let _ = handle_client(stream).await; }); }
                    Err(e) => { tracing::error!("accept error: {}", e); }
                }
            }
        }
    }
}

async fn handle_client<S>(stream: S) -> Result<()>
where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let start = std::time::Instant::now();
                let request_val: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        send_response(&mut writer, "unknown".to_string(), None, Some(ModuleError { code: "INVALID_JSON".to_string(), message: format!("Failed to parse JSON: {}", e), input_position: None }), 0).await;
                        continue;
                    }
                };

                let request_id = request_val["request_id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| request_val["request_id"].to_string());
                let input = match request_val["input"].as_str() {
                    Some(s) => s,
                    None => {
                        send_response(&mut writer, request_id, None, Some(ModuleError { code: "INVALID_REQUEST".to_string(), message: "Missing input field".to_string(), input_position: None }), 0).await;
                        continue;
                    }
                };

                match tokenize(input) {
                    Ok(tokens) => {
                        match serde_json::to_string(&tokens) {
                            Ok(output) => { send_response(&mut writer, request_id, Some(output), None, start.elapsed().as_millis() as u64).await; }
                            Err(e) => { send_response(&mut writer, request_id, None, Some(ModuleError { code: "SERIALIZATION_ERROR".to_string(), message: e.to_string(), input_position: None }), start.elapsed().as_millis() as u64).await; }
                        }
                    }
                    Err((msg, pos)) => {
                        send_response(&mut writer, request_id, None, Some(ModuleError { code: "UNKNOWN_PATTERN".to_string(), message: msg, input_position: Some(pos) }), start.elapsed().as_millis() as u64).await;
                    }
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt().try_init();
}