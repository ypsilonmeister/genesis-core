// =============================================================================
// ipc.rs — Unix Domain Socket + JSON でモジュール間通信を行う
//
// 通信契約 (Lying Calculator §3.2):
//   要求:  { request_id, input, timestamp }
//   応答:  { request_id, output, error, processing_ms }
//
// 直接 import / 直接 socket は Hard Invariant HI-2 で禁止。
// 全ての通信は orchestrator を経由する。
// =============================================================================

use compat::UnixStream;
use serde::{Deserialize, Serialize};
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
    pub code: ErrorCode,
    pub message: String,
    pub input_position: Option<usize>,
}

/// Lying Calculator §3.3 のエラーコード定義
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    UnknownToken,
    SyntaxError,
    DivisionByZero,
    Overflow,
    UnknownPattern,
    ModuleCrash,
}

impl ErrorCode {
    /// CMP の Tier 区分。Tier 1 = モジュール内修復、Tier 2 = 新モジュール追加。
    #[allow(dead_code)]
    pub fn tier(&self) -> u8 {
        match self {
            ErrorCode::UnknownPattern => 2,
            _ => 1,
        }
    }
}

/// UDS 経由でリクエストを送り、レスポンスを待つ
pub async fn call_module(
    socket_path: &str,
    request: &ModuleRequest,
) -> anyhow::Result<ModuleResponse> {
    let mut stream = UnixStream::connect(socket_path).await?;

    // JSON L (Line-delimited JSON) で送受信
    let mut payload = serde_json::to_vec(request)?;
    payload.push(b'\n');
    stream.write_all(&payload).await?;

    let mut reader = tokio::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response: ModuleResponse = serde_json::from_str(&line)?;
    Ok(response)
}

#[allow(dead_code)]
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
