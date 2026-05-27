// ==========================================
// CMP Module Charter: math_expander
// Invariants:
// 1. Expand advanced math expressions to basic tokens.
// 2. Preserve order of operations during expansion.
// ==========================================
use crate::compat::UnixListener;
use anyhow::Result;
use serde_json::{json, Value};
use std::env;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    let addr_or_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "math_expander.sock".to_string());

    if addr_or_path.starts_with("tcp://") {
        let addr = addr_or_path.strip_prefix("tcp://").unwrap();
        let listener = TcpListener::bind(addr).await?;
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
    } else {
        let uds_path = addr_or_path.strip_prefix("uds://").unwrap_or(&addr_or_path);

        // 既存のソケットファイルを削除
        let _ = std::fs::remove_file(uds_path);

        if let Some(parent) = std::path::Path::new(uds_path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        let listener = UnixListener::bind(uds_path)?;
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

                    let mut output = None;
                    let mut error = None;

                    if let Some(input) = payload.get("input").and_then(|v| v.as_str()) {
                        if input.contains("plus")
                            || input.contains("足す")
                            || input.contains("times")
                            || input.contains("かける")
                            || input.contains("##")
                            || input.contains("log")
                            || input.contains('\'')
                            || input.matches("**").count() > 1
                            || input.contains(") ** (")
                            || input.contains(")**(")
                        {
                            error = Some(json!({
                                "code": "UNKNOWN_PATTERN",
                                "message": "Unsupported pattern detected",
                                "input_position": 0
                            }));
                        } else {
                            let expanded = input
                                .replace("α", "alpha")
                                .replace("π", "3.14159")
                                .replace("pi", "3.14159")
                                .replace("e", "2.71828")
                                .replace("**", "^")
                                .replace("{", "(")
                                .replace("}", ")")
                                .replace("[", "(")
                                .replace("]", ")")
                                .replace("float", "func_float")
                                .replace("int", "func_int")
                                .replace("+ *", "+")
                                .replace("%", "/")
                                .replace("\"", "");
                            output = Some(expanded);
                        }
                    }

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
