//! Closes a stream's connection when its socket has refused writes for too long. A client that never reads can't be told anything, and hyper won't poll a response body while its last write is stuck, so only the socket can notice. Only connections that ask are watched, so a paused download is left alone.
use axum::serve::Listener;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Sleep, sleep};

/// Zero until the connection's handler asks to be watched.
#[derive(Default)]
struct Watch {
    stall_ms: AtomicU64,
}

/// Which connections are open, so a handler can find its own from its peer address.
#[derive(Default)]
pub(crate) struct ConnTable(Mutex<HashMap<SocketAddr, Weak<Watch>>>);

impl ConnTable {
    /// Writes on this peer's connection may now stall for this long, and then it closes. A request that didn't come through `WatchedListener`, as in tests, has nothing to watch.
    pub(crate) fn watch(&self, peer: SocketAddr, stall_ms: u64) {
        if let Some(watch) = self.0.lock().expect("lock").get(&peer).and_then(Weak::upgrade) {
            watch.stall_ms.store(stall_ms, Ordering::Relaxed);
        }
    }
}

pub(crate) struct WatchedListener {
    inner: TcpListener,
    table: Arc<ConnTable>,
}

impl WatchedListener {
    pub(crate) fn new(inner: TcpListener, table: Arc<ConnTable>) -> Self {
        Self { inner, table }
    }
}

impl Listener for WatchedListener {
    type Io = WatchedIo;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (WatchedIo, SocketAddr) {
        let (stream, peer) = Listener::accept(&mut self.inner).await;
        // A frame shouldn't wait for the acknowledgement of the one before it.
        let _ = stream.set_nodelay(true);
        let watch = Arc::new(Watch::default());
        self.table.0.lock().expect("lock").insert(peer, Arc::downgrade(&watch));
        (
            WatchedIo {
                inner: stream,
                watch,
                stuck: None,
                peer,
                table: self.table.clone(),
            },
            peer,
        )
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Listener::local_addr(&self.inner)
    }
}

pub struct WatchedIo {
    inner: TcpStream,
    watch: Arc<Watch>,
    stuck: Option<Pin<Box<Sleep>>>,
    peer: SocketAddr,
    table: Arc<ConnTable>,
}

impl Drop for WatchedIo {
    fn drop(&mut self) {
        self.table.0.lock().expect("lock").remove(&self.peer);
    }
}

impl WatchedIo {
    fn after_write<T>(&mut self, cx: &mut Context<'_>, polled: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        if polled.is_ready() {
            self.stuck = None;
            return polled;
        }
        let stall_ms = self.watch.stall_ms.load(Ordering::Relaxed);
        if stall_ms == 0 {
            return polled;
        }
        let stuck = self.stuck.get_or_insert_with(|| Box::pin(sleep(Duration::from_millis(stall_ms))));
        match stuck.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, "the client stopped reading"))),
            Poll::Pending => Poll::Pending,
        }
    }
}

use std::future::Future;

impl AsyncRead for WatchedIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for WatchedIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.after_write(cx, polled)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_flush(cx);
        self.after_write(cx, polled)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.after_write(cx, polled)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}
