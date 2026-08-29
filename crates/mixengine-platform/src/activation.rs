//! The address an activator holds, and the byte stream it hands back — roadmap task **T70**.
//!
//! A service that has been stopped for being idle is started again by the connection that needed
//! it. Something has to be listening where that connection arrives, and *where* is the one thing
//! about it that differs by operating system: a Unix domain socket on Linux and macOS, a TCP port
//! on Windows, because that is already what the service itself listens on.
//!
//! **This is not [`ipc`](crate::ipc), and the difference is not cosmetic.** That endpoint is the
//! daemon's own, there is exactly one of it, and it carries a peer check that answers *who is
//! this*. An activator holds one address per service, holds addresses a *user's* programs dial —
//! a browser, `mysql`, a php-fpm client — and has no business asking any of them who they are.
//! What the two share is the shape: an address value, a listener, a byte stream with no opinion
//! about what travels on it.
//!
//! **Nothing here parses anything**, which is the design's D1 and the reason one activator serves
//! FastCGI, the MySQL protocol and RESP alike. The client's first bytes are carried, not read — so
//! a client that speaks first and one that waits to be greeted are the same case.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::Result;
use crate::sys::activation as sys;

/// Where something listens, in whichever shape this system's services use.
///
/// **A value and not a string**, for [`crate::ipc::Endpoint`]'s reason and one more: the two arms
/// are not interchangeable even where both exist. A path is a file that has to be removed before it
/// can be bound again; a port is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listen {
    /// A Unix domain socket, by absolute path.
    Socket(PathBuf),

    /// A TCP address, which on Windows is what a service has instead.
    Tcp(SocketAddr),
}

impl std::fmt::Display for Listen {
    /// Lossy on a path that is not valid UTF-8, deliberately: this is for a log line and an error
    /// message, never for dialling.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Socket(path) => f.write_str(&path.to_string_lossy()),
            Self::Tcp(address) => write!(f, "{address}"),
        }
    }
}

/// A held address, waiting for whoever needs the service behind it.
#[derive(Debug)]
pub struct Activation(sys::Activation);

impl Activation {
    /// Take `listen`, so that a connection to it can start the service it belongs to.
    ///
    /// **A stale address is cleared first.** A daemon that was killed leaves its socket file
    /// behind, and `bind` on a path that still has one fails with `AddrInUse` even though nothing
    /// is listening — so every activator address in that home would stay unusable until somebody
    /// found out that a file in `run/` was why. There is nothing to clear for a TCP address.
    ///
    /// Must be called from inside a Tokio runtime.
    ///
    /// # Errors
    ///
    /// [`Error::Os`](crate::Error::Os) when the address cannot be taken — most often because the
    /// service itself, or something else, is already listening there.
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) for a socket path on
    /// Windows, which has services that listen on ports and no php-fpm that listens on a socket.
    pub async fn bind(listen: &Listen) -> Result<Self> {
        sys::Activation::bind(listen).await.map(Self)
    }

    /// Wait for somebody to need the service.
    ///
    /// # Errors
    ///
    /// [`Error::Os`](crate::Error::Os) when the accept fails.
    pub async fn accept(&self) -> Result<Incoming> {
        self.0.accept().await.map(Incoming)
    }

    /// The address being held, for a log line.
    #[must_use]
    pub fn listening_on(&self) -> &Listen {
        self.0.listening_on()
    }
}

/// Dial something listening at `listen`.
///
/// The other half of the splice: once the service is up, the activator connects to it and copies
/// the waiting client's bytes across.
///
/// # Errors
///
/// [`Error::Os`](crate::Error::Os) when nothing answers there — which for an activator means the
/// service did not come up after all, and is why a failed activation closes the connection rather
/// than holding it.
/// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) for a socket path on Windows.
pub async fn dial(listen: &Listen) -> Result<Incoming> {
    sys::dial(listen).await.map(Incoming)
}

/// One connection, in either direction, with no opinion about what travels on it.
#[derive(Debug)]
pub struct Incoming(sys::Incoming);

impl AsyncRead for Incoming {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Incoming {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
