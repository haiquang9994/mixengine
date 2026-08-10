//! The local endpoint the daemon listens on and every client dials.
//!
//! One shape, two implementations, exactly as `.claude/architecture/daemon-and-ipc.md` describes
//! them: a Unix domain socket in `run/` on Linux and macOS, a named pipe on Windows. Neither is a
//! network socket — the daemon opens no TCP port by default, so there is nothing for another
//! machine to reach and nothing for a firewall to have an opinion about.
//!
//! **Two gates, not one.** The endpoint's own permissions come first — mode `0600` on the socket,
//! a DACL naming this account and nobody else on the pipe — because they are enforced by the
//! kernel before any of our code runs. The peer check on top of them is what notices when the
//! first gate was not applied the way we think it was: a socket restored with somebody else's
//! mode, a pipe created by a build of MixEngine whose DACL was wrong. It answers "who is this",
//! never "what may they do" — every client of this daemon is the user, and the user may do
//! everything.
//!
//! This module deliberately does not speak HTTP or JSON-RPC. It hands back a byte stream
//! ([`Connection`] is `AsyncRead + AsyncWrite`); the protocol on top of it is the daemon's business
//! (roadmap task T8).

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::Result;
use crate::sys::ipc as sys;

/// Where the daemon listens, in the form this OS names an endpoint.
///
/// A filesystem path on Unix and a pipe name on Windows, which is why the inside is an [`OsString`]
/// and not a `PathBuf`: `\\.\pipe\…` is not a path anything should ever try to create, join or
/// canonicalise.
///
/// Both sides compute it from the same input — the `run/` directory of the home they were pointed
/// at — so a client started with `MIXENGINE_HOME` set to a sandbox reaches the sandbox's daemon and
/// not the real one. That is a property Unix gets for free, the socket being a file inside the
/// home, and Windows does not: the pipe namespace is flat and machine-wide, so the home is folded
/// into the name instead. See [`Endpoint::in_run_dir`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Endpoint(OsString);

impl Endpoint {
    /// The endpoint belonging to the home whose `run/` directory this is.
    ///
    /// The directory is not created or read — the endpoint can be computed before the home exists,
    /// which is what lets a client say where it *would* have connected when nothing is running.
    ///
    /// # Errors
    ///
    /// [`Error::Address`](crate::Error::Address) when the OS will not accept an address derived
    /// from this directory — on Unix a home so deeply nested that the socket path exceeds
    /// `sun_path`, which is 104 bytes on macOS and 108 on Linux. [`Error::Os`](crate::Error::Os)
    /// on Windows, where the name contains this account's SID and reading it can fail.
    pub fn in_run_dir(run: &Path) -> Result<Self> {
        sys::address(run).map(Self)
    }

    /// The address as the OS wants it: the argument to `bind`, or to `CreateFile`.
    pub(crate) fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

impl fmt::Display for Endpoint {
    /// Lossy, because this is for a log line and an error message and never for dialling: a path
    /// that is not valid UTF-8 should not stop the daemon from being able to say where it listens.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_string_lossy())
    }
}

/// Who is at the other end of a connection, as the OS describes them.
///
/// Descriptive rather than actionable: it exists so the daemon can say *whose* connection it
/// refused, in a log line somebody may have to read weeks later. Nothing branches on the contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    account: String,
    process: Option<u32>,
}

impl Peer {
    /// The account, in whatever form the OS identifies one: a numeric uid on Unix, a SID on
    /// Windows.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }

    /// The process that opened the connection, where the OS offers it.
    ///
    /// `None` on Windows, where the identity comes from impersonating the client rather than from
    /// looking up a process — see the peer check there for why that is the safer of the two.
    #[must_use]
    pub fn process(&self) -> Option<u32> {
        self.process
    }
}

impl fmt::Display for Peer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.process {
            Some(process) => write!(f, "{} (pid {process})", self.account),
            None => f.write_str(&self.account),
        }
    }
}

/// What one [`Listener::accept`] produced.
///
/// A rejected peer is an outcome and not an error, which is the whole reason this type exists: a
/// stranger knocking is something the daemon logs and carries on from, while an `Err` out of
/// `accept` means the listener itself is in trouble. Collapsing the two would make the accept loop
/// treat "somebody else tried" and "this daemon can no longer accept anything" the same way.
#[derive(Debug)]
pub enum Accepted {
    /// A connection from the account the daemon runs as.
    Trusted(Connection),

    /// A connection from somebody else, already closed. Carries who, for the log.
    Untrusted(Peer),
}

/// The daemon's end of the local endpoint.
///
/// Dropping it releases the endpoint: the socket file is removed on Unix, and on Windows the pipe
/// ceases to exist once the last handle to it closes. Neither happens if the process is killed
/// outright — see [`Listener::bind`] for what the next start does about the leftovers.
#[derive(Debug)]
pub struct Listener {
    inner: sys::Listener,
}

impl Listener {
    /// Take the endpoint over, with permissions that admit this account and nothing else.
    ///
    /// Must be called from inside a Tokio runtime: the returned listener registers with its
    /// reactor, exactly as `tokio::net::UnixListener::bind` does.
    ///
    /// **A leftover endpoint is cleaned up, a live one is not.** The two look identical from
    /// outside — a socket file is left behind by a daemon that was killed rather than stopped, and
    /// Windows answers `ERROR_ACCESS_DENIED` both when somebody else's pipe of that name exists and
    /// when the permissions are genuinely wrong. So the question is asked the only way that
    /// distinguishes them: by dialling the endpoint. Something answers, and this is
    /// [`Error::EndpointInUse`](crate::Error::EndpointInUse) on both systems.
    ///
    /// What "nothing answers" leads to is where they part, because only one of them leaves a corpse
    /// to clear. On Unix the socket file is unlinked and the bind retried once. On Windows there is
    /// nothing to remove — a pipe ceases to exist with the last handle to it, so a name that
    /// answers nothing was never the reason `bind` failed — and the original refusal is returned
    /// unchanged rather than being relabelled as a daemon that is not there.
    ///
    /// That probe is not a substitute for the single-instance lock in roadmap task T9, and does not
    /// try to be. Two daemons starting at the same instant can both find the endpoint dead, and on
    /// Unix the second one's `bind` then replaces the first one's socket file while the first is
    /// still listening on it. The lock is what makes that unreachable; this only handles the far
    /// commoner case of one daemon starting after another one died.
    ///
    /// # Errors
    ///
    /// [`Error::EndpointInUse`](crate::Error::EndpointInUse) when a daemon is already there,
    /// [`Error::Io`](crate::Error::Io) when the endpoint cannot be created or its permissions
    /// cannot be set, and [`Error::Os`](crate::Error::Os) for the Windows security calls that build
    /// the pipe's DACL.
    pub fn bind(endpoint: &Endpoint) -> Result<Self> {
        sys::Listener::bind(endpoint).map(|inner| Self { inner })
    }

    /// Wait for the next client and find out who it is.
    ///
    /// Cancel safe, so it can sit in a `select!` arm next to the daemon's shutdown signal: both
    /// implementations await exactly one call that tokio documents as cancel safe, and everything
    /// after it — taking the next pipe instance, reading the peer's credentials — is synchronous.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) if the endpoint itself failed, and
    /// [`Error::Os`](crate::Error::Os) if the OS would not say who connected. Neither should end
    /// the accept loop on its own: a connection that dies between the kernel queueing it and us
    /// asking about it fails here and means nothing.
    pub async fn accept(&mut self) -> Result<Accepted> {
        self.inner.accept().await
    }
}

/// One client's byte stream.
///
/// `AsyncRead + AsyncWrite`, and nothing else: this is what T8 hands to `hyper`, and what a client
/// writes a JSON-RPC request into.
#[derive(Debug)]
pub struct Connection(sys::Connection);

impl Connection {
    /// Dial the daemon.
    ///
    /// Must be called from inside a Tokio runtime, as [`Listener::bind`] must.
    ///
    /// Nothing is retried past the moment of connecting: a daemon that is not running is an answer
    /// this returns rather than waits for, because the caller is the one that knows whether to
    /// start one (roadmap task T10) or to report that none is up.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) with a `NotFound` or `ConnectionRefused` cause when no
    /// daemon is listening, which is the case worth telling apart from the rest.
    pub async fn connect(endpoint: &Endpoint) -> Result<Self> {
        sys::connect(endpoint).await.map(Self)
    }
}

impl AsyncRead for Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Connection {
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

/// Build a [`Peer`] from what an implementation managed to learn.
///
/// Here rather than in each of them so the two cannot drift into describing the same idea
/// differently — the account is the OS's own identifier for one, verbatim, and never a name
/// resolved from it.
pub(crate) fn peer(account: String, process: Option<u32>) -> Peer {
    Peer { account, process }
}

/// Wrap a connection an implementation has already decided to trust.
pub(crate) fn trusted(connection: sys::Connection) -> Accepted {
    Accepted::Trusted(Connection(connection))
}
