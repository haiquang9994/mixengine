//! A loopback port — roadmap task **T70**.
//!
//! **There is no socket arm here, and that is upstream's shape rather than an omission.** Windows
//! has had `AF_UNIX` since 1803, but the services MixEngine starts on this system listen on ports:
//! there is no php-fpm on Windows at all — `php-cgi.exe` is what serves a pool, on
//! `127.0.0.1:<services.port>` — and both databases and both caches are the same. A socket path
//! reaching this file therefore means a home built on another system, so it is answered with
//! [`Error::UnsupportedPlatform`] naming the address rather than with a bind that would fail
//! obscurely.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};

use crate::activation::Listen;
use crate::{Error, Result};

/// A held port.
#[derive(Debug)]
pub(crate) struct Activation {
    listener: TcpListener,
    listen: Listen,
}

impl Activation {
    pub(crate) async fn bind(listen: &Listen) -> Result<Self> {
        let address = tcp(listen)?;

        let listener = TcpListener::bind(address)
            .await
            .map_err(|source| Error::Os {
                action: "bind the activation port",
                source,
            })?;

        Ok(Self {
            listener,
            listen: listen.clone(),
        })
    }

    pub(crate) async fn accept(&self) -> Result<Incoming> {
        self.listener
            .accept()
            .await
            .map(|(stream, _)| Incoming(stream))
            .map_err(|source| Error::Os {
                action: "accept on the activation port",
                source,
            })
    }

    pub(crate) fn listening_on(&self) -> &Listen {
        &self.listen
    }
}

pub(crate) async fn dial(listen: &Listen) -> Result<Incoming> {
    let address = tcp(listen)?;

    TcpStream::connect(address)
        .await
        .map(Incoming)
        .map_err(|source| Error::Os {
            action: "connect to the service's port",
            source,
        })
}

/// The address, or the refusal that says why a path cannot be one here.
fn tcp(listen: &Listen) -> Result<std::net::SocketAddr> {
    match listen {
        Listen::Tcp(address) => Ok(*address),
        Listen::Socket(path) => Err(Error::UnsupportedPlatform {
            capability: "activation on a Unix socket",
            reason: format!(
                "no service MixEngine runs on Windows listens on a socket, so `{}` is an address \
                 from a home built on another system",
                path.display()
            ),
        }),
    }
}

/// One connection.
#[derive(Debug)]
pub(crate) struct Incoming(TcpStream);

impl AsyncRead for Incoming {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Incoming {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
