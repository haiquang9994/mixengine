//! A Unix socket or a TCP port, whichever the service listens on — roadmap task **T70**.
//!
//! Identical on Linux and macOS, which is why it lives here rather than in either directory: tokio
//! spells both listeners the same way on both, and there is no peer check to differ over — see
//! [`crate::activation`] for why an activator has none.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

use crate::activation::Listen;
use crate::{Error, Result};

/// A held address.
#[derive(Debug)]
pub(crate) enum Activation {
    /// A socket in `run/`, beside the service's own.
    Socket {
        listener: UnixListener,
        listen: Listen,
    },

    /// A loopback port.
    Tcp {
        listener: TcpListener,
        listen: Listen,
    },
}

impl Activation {
    pub(crate) async fn bind(listen: &Listen) -> Result<Self> {
        match listen {
            Listen::Socket(path) => {
                clear_stale(path)?;

                let listener = UnixListener::bind(path).map_err(|source| Error::Os {
                    action: "bind the activation socket",
                    source,
                })?;

                Ok(Self::Socket {
                    listener,
                    listen: listen.clone(),
                })
            }

            Listen::Tcp(address) => {
                let listener = TcpListener::bind(address)
                    .await
                    .map_err(|source| Error::Os {
                        action: "bind the activation port",
                        source,
                    })?;

                Ok(Self::Tcp {
                    listener,
                    listen: listen.clone(),
                })
            }
        }
    }

    pub(crate) async fn accept(&self) -> Result<Incoming> {
        match self {
            Self::Socket { listener, .. } => listener
                .accept()
                .await
                .map(|(stream, _)| Incoming::Socket(stream))
                .map_err(|source| Error::Os {
                    action: "accept on the activation socket",
                    source,
                }),

            Self::Tcp { listener, .. } => listener
                .accept()
                .await
                .map(|(stream, _)| Incoming::Tcp(stream))
                .map_err(|source| Error::Os {
                    action: "accept on the activation port",
                    source,
                }),
        }
    }

    pub(crate) fn listening_on(&self) -> &Listen {
        match self {
            Self::Socket { listen, .. } | Self::Tcp { listen, .. } => listen,
        }
    }

    /// Close the listener, and remove the socket file the close leaves behind — **T70a**.
    ///
    /// **A `UnixListener` does not unlink its path when it is dropped**, and a server told to bind
    /// a path that already exists reports that it exists rather than taking it — so a release that
    /// only closed would hand the service an address it cannot have. There is nothing to remove
    /// for a port.
    pub(crate) fn release(self) -> Result<()> {
        match self {
            Self::Socket { listener, listen } => {
                // Before the unlink, so nothing can connect to a listener whose path is about to
                // go and be left holding a stream to an address that no longer names it.
                drop(listener);

                let Listen::Socket(path) = listen else {
                    return Ok(());
                };

                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(()),
                    // Already gone is the outcome asked for, not a failure to reach it.
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(source) => Err(Error::Os {
                        action: "remove the activation socket on the way back to the service",
                        source,
                    }),
                }
            }

            Self::Tcp { listener, .. } => {
                drop(listener);

                Ok(())
            }
        }
    }
}

/// Remove a socket file nothing is listening on, so the bind that follows can have the address.
///
/// **Only when nothing answers there.** Unlinking a path a live process is serving would take that
/// service off the air and leave it running with a socket nobody can reach, which is worse than
/// failing to bind — so the file is dialled first, and a refusal is what says it is stale.
fn clear_stale(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        // Somebody is serving it. Let `bind` refuse, and let the caller report that honestly.
        return Ok(());
    }

    std::fs::remove_file(path).map_err(|source| Error::Os {
        action: "remove a stale activation socket",
        source,
    })
}

pub(crate) async fn dial(listen: &Listen) -> Result<Incoming> {
    match listen {
        Listen::Socket(path) => UnixStream::connect(path)
            .await
            .map(Incoming::Socket)
            .map_err(|source| Error::Os {
                action: "connect to the service's socket",
                source,
            }),

        Listen::Tcp(address) => TcpStream::connect(address)
            .await
            .map(Incoming::Tcp)
            .map_err(|source| Error::Os {
                action: "connect to the service's port",
                source,
            }),
    }
}

/// One connection, of whichever kind the address was.
#[derive(Debug)]
pub(crate) enum Incoming {
    Socket(UnixStream),
    Tcp(TcpStream),
}

impl AsyncRead for Incoming {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Socket(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tcp(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Incoming {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Socket(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tcp(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Socket(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tcp(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Socket(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tcp(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}
