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

    /// Give the address back, so that the service it belongs to can take it — **T70a**, design D4.
    ///
    /// **Only the database path calls this.** A pool's activator holds a permanent address of its
    /// own and never lets go; a database's activator holds *the database's* address, and the start
    /// it is about to ask for cannot succeed until this has returned.
    ///
    /// **Consuming `self` is the guarantee.** An `Activation` that has been released cannot be
    /// accepted on, so a caller cannot hold the address open in one place while believing it
    /// handed it over in another.
    ///
    /// # Errors
    ///
    /// [`Error::Os`](crate::Error::Os) when a socket file cannot be removed — which leaves the
    /// address unusable by the service too, and is why it is reported rather than swallowed.
    pub fn release(self) -> Result<()> {
        self.0.release()
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

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::atomic::{AtomicU16, Ordering};

    use super::*;

    /// An address of the shape this system's services use, chosen by binding rather than written
    /// down — a number this file merely hoped for is one another program is entitled to hold.
    ///
    /// **The counter is not decoration.** Two addresses chosen before either is bound are the same
    /// address: the first search has not taken its port yet when the second one looks, so both are
    /// handed the lowest free number and the second bind fails with `AddrInUse`. Each call
    /// therefore starts its search where the last one stopped.
    static NEXT: AtomicU16 = AtomicU16::new(0);

    fn somewhere(home: &std::path::Path, name: &str) -> Listen {
        if cfg!(windows) {
            let base = 25_800 + NEXT.fetch_add(32, Ordering::Relaxed);

            let port = (base..base + 32)
                .find(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok())
                .expect("a free port in the window");

            Listen::Tcp((Ipv4Addr::LOCALHOST, port).into())
        } else {
            Listen::Socket(home.join(name))
        }
    }

    /// **An address the activator gave back is an address the service can take** — T70a's D4.
    ///
    /// This is the whole of what the database path needs from the platform layer and the whole of
    /// what T70 never needed: a pool's activator holds an address of its own for as long as the
    /// daemon runs, and a database's activator holds *the database's*, which it must hand over.
    ///
    /// **Closing is not enough on a Unix socket.** The file survives the close, and a server asked
    /// to bind a path that already exists reports that it exists rather than taking it — so a
    /// release that only closed would hand the database an address it cannot have, and the
    /// database's own log would say the address is taken without saying what took it.
    #[tokio::test]
    async fn an_address_given_back_can_be_taken_again() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let listen = somewhere(home.path(), "given-back.sock");

        let activation = Activation::bind(&listen).await.expect("the address");
        activation.release().expect("giving the address back");

        as_a_server_would(&listen);
    }

    /// Take the address the way the *service* takes it, not the way [`Activation::bind`] does.
    ///
    /// **This is the whole point of the test.** `Activation::bind` clears a stale socket file
    /// before it binds, so asking *it* would answer that the address is free whether or not the
    /// release unlinked anything. A `mariadbd` does no such thing: it reports that the path exists
    /// and refuses to start. It is `mariadbd` that has to succeed here, so it is `mariadbd`'s bind
    /// that is made.
    #[cfg(unix)]
    fn as_a_server_would(listen: &Listen) {
        match listen {
            Listen::Socket(path) => {
                std::os::unix::net::UnixListener::bind(path)
                    .expect("the socket file was left behind, so no server could bind it");
            }

            Listen::Tcp(address) => {
                TcpListener::bind(address).expect("the port was not given back");
            }
        }
    }

    /// The same, on the system whose services all listen on ports — see the `windows` module note.
    #[cfg(windows)]
    fn as_a_server_would(listen: &Listen) {
        let Listen::Tcp(address) = listen else {
            unreachable!("`somewhere` chooses a port on this system");
        };

        TcpListener::bind(address).expect("the port was not given back");
    }
}
