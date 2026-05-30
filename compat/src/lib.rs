
#[cfg(unix)]
pub use tokio::net::{UnixListener, UnixStream};

#[cfg(not(unix))]
pub use windows::*;

#[cfg(not(unix))]
mod windows {
    use std::net::SocketAddr;
    use std::path::Path;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::net::{TcpListener, TcpStream};

    pub fn path_to_port(path: impl AsRef<Path>) -> u16 {
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
        // 10000 〜 45000 の範囲にマッピング（Windowsの動的システム予約ポートを確実に避ける）
        (10000 + (hash % 35000)) as u16
    }

    pub struct UnixListener {
        inner: TcpListener,
    }

    impl UnixListener {
        pub fn bind(path: impl AsRef<Path>) -> std::io::Result<Self> {
            let port = path_to_port(path);
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
