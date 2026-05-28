// =============================================================================
// # CMP Module Charter
//
// What:
//   Normalize the input string and pass it to subsequent modules (whitespace removal only).
//
// Invariants:
//   - Do not modify the input string destructively (the original input must be logged).
//   - Return an error if an empty string is received.
//
// Boundaries:
//   - Dependencies: None
//   - Dependents: tokenizer
//
// Extensible:
//   - Addition of normalization rules (e.g., full-width to half-width conversion, invisible character removal, etc.)
//
// Why:
//   Remove surface noise so that subsequent modules can focus on pure analysis.
//
// When modifying in Tier 1, the AI must never violate the above Invariants
// and Boundaries. Changes beyond the scope of "What" are treated as Tier 2.
// =============================================================================

// v1 実装範囲:
//   連続空白を単一空白に圧縮、前後空白をトリム。それ以外は素通し。

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

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("normalizer booting (v1)");

    let socket_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/normalizer.sock".to_string());

    // 以前のソケットファイルを削除
    let _ = std::fs::remove_file(&socket_path);
    // ディレクトリ作成
    if let Some(parent) = std::path::Path::new(&socket_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("Listening on {}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let (reader, mut writer) = stream.split();
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

                let (output, error) = match normalize(&request.input) {
                    Ok(out) => (Some(out), None),
                    Err(e) => (
                        None,
                        Some(ModuleError {
                            code: "SYNTAX_ERROR".to_string(), // normalizer doesn't have specific error codes in spec yet
                            message: e,
                            input_position: None,
                        }),
                    ),
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

/// 連続空白を単一空白に圧縮し、前後空白をトリムする。
/// 元の入力は破壊しない(参照を借りるだけ)。
/// 空文字列の場合はエラーを返す。
pub fn normalize(input: &str) -> Result<String, String> {
    if input.trim().is_empty() {
        return Err("Empty input".to_string());
    }
    Ok(input.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,normalizer=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_whitespace() {
        assert_eq!(normalize("3  +\n5 *  2").unwrap(), "3 + 5 * 2");
    }

    #[test]
    fn trims_edges() {
        assert_eq!(normalize("  3 + 5  ").unwrap(), "3 + 5");
    }

    #[test]
    fn empty_input_returns_error() {
        // Charter: 空文字列を受け取った場合はエラーを返す
        assert!(normalize("").is_err());
        assert!(normalize("   ").is_err());
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
