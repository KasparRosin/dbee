//! [`TunnelStream`]: a TCP connection through the tunnel, safe to drop on any thread.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use russh::ChannelStream;
use russh::client::Msg;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::runtime::Handle;

/// A direct-tcpip channel of the tunnel as a byte stream, which can be dropped off its runtime.
pub struct TunnelStream {
    /// `None` only while it is dropped.
    inner: Option<ChannelStream<Msg>>,
    runtime: Handle,
}

impl TunnelStream {
    pub(crate) fn new(inner: ChannelStream<Msg>, runtime: Handle) -> TunnelStream {
        TunnelStream { inner: Some(inner), runtime }
    }

    fn inner(&mut self) -> io::Result<Pin<&mut ChannelStream<Msg>>> {
        self.inner.as_mut().map(Pin::new).ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected))
    }
}

impl std::fmt::Debug for TunnelStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelStream").finish_non_exhaustive()
    }
}

impl Drop for TunnelStream {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else { return };
        if Handle::try_current().is_ok() {
            drop(inner);
            return;
        }
        let _entered = self.runtime.enter();
        drop(inner);
    }
}

impl AsyncRead for TunnelStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut().inner() {
            Ok(inner) => inner.poll_read(cx, buf),
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncWrite for TunnelStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut().inner() {
            Ok(inner) => inner.poll_write(cx, buf),
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut().inner() {
            Ok(inner) => inner.poll_flush(cx),
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut().inner() {
            Ok(inner) => inner.poll_shutdown(cx),
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}
