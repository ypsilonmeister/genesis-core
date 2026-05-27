// =============================================================================
// # CMP Module Charter
//
// What:
//   数学式の高度な変換を行う。scientific notation の展開、
//   modulo (%) などの標準的でないオペレーターの正規化を担当する。
//
// Invariants:
//   - 入力の意味を変えずに表現のみ変換する
//   - 処理できないパターンは UNKNOWN_PATTERN エラーで返す
//   - MODULE_CRASH / MATH_ERROR などの非標準コードを使用しない
//
// Boundaries:
//   - 依存先: math_expander (前段)
//   - 被依存先: tokenizer (後段)
//
// Extensible:
//   - 新たな数学表現の変換規則追加
//
// Tier 1 で AI が改変するときは、上記 Invariants と Boundaries を絶対に
// 破らないこと。What の範囲を超える変更は Tier 2 として扱う。
// =============================================================================

use crate::compat::UnixListener;
use anyhow::Result;
use serde_json::{json, Value};
use std::env;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    tracing::info!("advanced_math booting");

    let socket_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/advanced_math.sock".to_string());

    let uds_path = socket_path
        .strip_prefix("uds://")
        .unwrap_or(&socket_path)
        .to_string();
    let _ = std::fs::remove_file(&uds_path);
    if let Some(parent) = std::path::Path::new(&uds_path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let listener = UnixListener::bind(&uds_path)?;
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                continue;
            }
        };
        tokio::spawn(async move {
            let _ = handle_client(stream).await;
        });
    }
}

fn expand(input: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit()
            || (chars[i] == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let mut has_dot = false;
            let mut num_str = String::new();
            while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && !has_dot)) {
                if chars[i] == '.' {
                    has_dot = true;
                }
                num_str.push(chars[i]);
                i += 1;
            }
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let saved_i = i;
                i += 1;
                let mut exp_str = String::new();
                if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                    exp_str.push(chars[i]);
                    i += 1;
                }
                let exp_start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    exp_str.push(chars[i]);
                    i += 1;
                }
                if i > exp_start {
                    let base: f64 = num_str.parse().unwrap_or(0.0);
                    let exp: i32 = exp_str.parse().unwrap_or(0);
                    let val = base * 10f64.powi(exp);
                    if val.fract() == 0.0 && val.abs() < 1e15 {
                        result.push_str(&format!("{}", val as i64));
                    } else {
                        result.push_str(&format!("{}", val));
                    }
                    continue;
                } else {
                    result.push_str(&num_str);
                    i = saved_i;
                    continue;
                }
            }
            result.push_str(&num_str);
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

async fn handle_client<S>(stream: S) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                if let Ok(payload) = serde_json::from_str::<Value>(&line) {
                    let request_id = payload
                        .get("request_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();

                    let (output, error) = if let Some(input) = payload.get("input").and_then(|v| v.as_str()) {
                        let expanded = expand(input);
                        // Validate that result contains only chars tokenizer can handle
                        let mut ok = true;
                        for c in expanded.chars() {
                            if !c.is_ascii_digit()
                                && !c.is_alphabetic()
                                && !"+-*/^(). %,!?:<>".contains(c)
                            {
                                ok = false;
                                break;
                            }
                        }
                        if ok {
                            (Some(expanded), None)
                        } else {
                            (None, Some(json!({"code": "UNKNOWN_PATTERN", "message": "advanced_math: unhandled pattern", "input_position": null})))
                        }
                    } else {
                        (None, Some(json!({"code": "SYNTAX_ERROR", "message": "missing input field", "input_position": null})))
                    };

                    let response = json!({
                        "request_id": request_id,
                        "output": output,
                        "error": error,
                        "processing_ms": 0
                    });

                    if let Ok(mut res) = serde_json::to_vec(&response) {
                        res.push(b'\n');
                        let _ = writer.write_all(&res).await;
                        let _ = writer.flush().await;
                    }
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
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
            pub async fn connect(path: impl AsRef<Path>) -> std::io::Result<Self> {
                let port = path_to_port(path);
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                let inner = TcpStream::connect(addr).await?;
                Ok(Self { inner })
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
