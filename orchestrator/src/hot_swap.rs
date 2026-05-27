// =============================================================================
// hot_swap.rs — モジュールバイナリの無停止差し替え
//
// CMP §8.2 Phase 1 の手順:
//   1. archive/ に旧バイナリを退避
//   2. 旧プロセスを kill
//   3. 新プロセスを同じ socket_path で起動
//   4. ヘルスチェック通過後、新 Child を返す
//
// 失敗時のロールバック規律は charter/system.md §7 を参照。
// =============================================================================

use anyhow::{bail, Context, Result};
use compat::UnixStream;
use std::path::Path;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tracing::{info, warn};

pub struct HotSwapper {
    pub module_name: String,
    pub binary_path: String,
    pub socket_path: String,
}

impl HotSwapper {
    pub fn new(module_name: &str, binary_path: &str, socket_path: &str) -> Self {
        Self {
            module_name: module_name.to_string(),
            binary_path: binary_path.to_string(),
            socket_path: socket_path.to_string(),
        }
    }

    /// 旧プロセスを archive に退避し、新プロセスに差し替える。
    /// 成功すると新プロセスの Child を返す。
    pub async fn swap(&self, mut old_child: Child) -> Result<Child> {
        info!(module = %self.module_name, "hot_swap: starting");

        // 1. 旧バイナリを archive/ に退避
        self.archive_old_binary()?;

        // 2. 旧プロセスを kill
        old_child
            .kill()
            .await
            .context("Failed to kill old process")?;
        old_child
            .wait()
            .await
            .context("Failed to wait for old process")?;
        info!(module = %self.module_name, "hot_swap: old process terminated");

        // 3. 既存の socket ファイルを削除 (残留すると bind できない)
        if Path::new(&self.socket_path).exists() {
            std::fs::remove_file(&self.socket_path).context("Failed to remove stale socket")?;
        }

        // 4. 新プロセスを起動
        let new_child = Command::new(&self.binary_path)
            .arg(&self.socket_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("Failed to spawn new process: {}", self.binary_path))?;

        info!(module = %self.module_name, "hot_swap: new process spawned");

        // 5. ヘルスチェック (socket に繋がるまで最大 5 秒)
        self.health_check().await?;

        info!(module = %self.module_name, "hot_swap: completed successfully");
        Ok(new_child)
    }

    fn archive_old_binary(&self) -> Result<()> {
        let binary = Path::new(&self.binary_path);
        if !binary.exists() {
            return Ok(());
        }

        std::fs::create_dir_all("archive").context("Failed to create archive/")?;

        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();
        let archive_path = format!("archive/{}_{}", self.module_name, ts);

        std::fs::copy(&self.binary_path, &archive_path)
            .with_context(|| format!("Failed to archive binary to {}", archive_path))?;

        info!(module = %self.module_name, dest = %archive_path, "hot_swap: old binary archived");
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        const MAX_RETRIES: u32 = 10;
        const RETRY_INTERVAL_MS: u64 = 500;

        for attempt in 1..=MAX_RETRIES {
            match UnixStream::connect(&self.socket_path).await {
                Ok(_) => {
                    info!(module = %self.module_name, attempt, "hot_swap: health check passed");
                    return Ok(());
                }
                Err(_) => {
                    warn!(
                        module = %self.module_name,
                        attempt,
                        max = MAX_RETRIES,
                        "hot_swap: waiting for socket"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_INTERVAL_MS)).await;
                }
            }
        }

        bail!(
            "hot_swap health check failed: {} did not become ready within {}ms",
            self.module_name,
            MAX_RETRIES as u64 * RETRY_INTERVAL_MS
        );
    }
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
