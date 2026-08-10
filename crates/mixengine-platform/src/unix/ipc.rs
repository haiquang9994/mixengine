//! A Unix domain socket in `run/`, mode `0600`, with the peer's uid checked on every accept.
//!
//! Identical on Linux and macOS down to the peer check: `SO_PEERCRED` and `LOCAL_PEERCRED` are
//! different system calls with different structures behind them, and tokio's `peer_cred` is the
//! same function on both. There is nothing left for either OS directory to say about this, which is
//! why it lives here.

use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{fs, mem};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{UnixListener, UnixStream};

use crate::ipc::{Accepted, Endpoint};
use crate::{Error, Result};

/// The socket, inside `run/` — which `Paths::bootstrap` has already restricted to this account,
/// and which is what makes the window between `bind` and the `chmod` below harmless. (That type
/// lives in `mixengine-core`, a crate that depends on this one, so it is named rather than linked.)
const SOCKET_FILE_NAME: &str = "mixengined.sock";

/// Owner: read and write. Nobody else: nothing. A socket has no use for the execute bit.
const SOCKET_MODE: u32 = 0o600;

/// The longest `sun_path` this OS accepts, the terminating NUL included.
///
/// Read out of `libc` rather than written down, because it is one of the few POSIX constants that
/// genuinely differs between the two systems this file serves — 108 bytes on Linux, 104 on macOS —
/// and `unix/` is the one directory that must not branch on which of them it is compiled for. The
/// two bytes before the path are the address family in both layouts (`sun_family: u16` on Linux;
/// `sun_len` and `sun_family`, a byte each, on macOS), and everything after them is `sun_path`.
const SUN_PATH: usize = mem::size_of::<libc::sockaddr_un>() - 2;

/// The socket belonging to the home whose `run/` directory this is.
pub(crate) fn address(run: &Path) -> Result<OsString> {
    let socket = run.join(SOCKET_FILE_NAME);
    let length = socket.as_os_str().as_bytes().len();

    // Checked here, where the answer is a sentence naming the limit, rather than left to `bind` —
    // which reports a path one byte too long as `EINVAL`, "Invalid argument", with nothing to say
    // which argument or why. The failure is entirely a consequence of where the home is, so it
    // belongs to the address and not to the attempt to use it.
    if length >= SUN_PATH {
        return Err(Error::Address {
            address: socket.display().to_string(),
            reason: format!(
                "a Unix socket path is limited to {} bytes on this system and this one is {length} \
                 — put MIXENGINE_HOME somewhere shorter",
                SUN_PATH - 1
            ),
        });
    }

    Ok(socket.into_os_string())
}

/// The daemon's end: a bound, listening socket, and the identity of the file it created.
#[derive(Debug)]
pub(crate) struct Listener {
    listener: UnixListener,
    path: PathBuf,
    /// Device and inode of the socket file as it was created here.
    ///
    /// Not the path: the path is a name that can come to mean a different file, and unlinking
    /// whatever happens to be sitting at it during shutdown is how one daemon deletes another
    /// daemon's socket. See the `Drop` implementation below.
    file: (u64, u64),
    /// The uid every accepted connection is compared against.
    owner: u32,
}

impl Listener {
    pub(crate) fn bind(endpoint: &Endpoint) -> Result<Self> {
        let path = PathBuf::from(endpoint.as_os_str());

        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,

            // A socket file outlives the process that bound it — `bind` creates it and nothing but
            // an explicit unlink removes it, so a daemon that was killed rather than stopped leaves
            // one behind and every later start meets it. Telling that corpse from a live daemon
            // cannot be done by looking at the file, only by dialling it.
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                if occupied(&path) {
                    return Err(Error::EndpointInUse {
                        address: path.display().to_string(),
                    });
                }

                fs::remove_file(&path).map_err(|source| Error::Io {
                    action: "remove the socket left behind at",
                    path: path.clone(),
                    source,
                })?;

                UnixListener::bind(&path).map_err(|source| failed("bind", &path, source))?
            }

            Err(source) => return Err(failed("bind", &path, source)),
        };

        // After the bind rather than before it, because there is no `bind` that takes a mode: the
        // socket appears with whatever the umask allows, typically `0755`. The window that opens is
        // closed by the directory instead — `run/` is one of the four `DirectoryAccess` locks down
        // at bootstrap, so nobody else can reach the socket during it whatever its own mode says.
        fs::set_permissions(&path, fs::Permissions::from_mode(SOCKET_MODE))
            .map_err(|source| failed("restrict", &path, source))?;

        let created = fs::metadata(&path).map_err(|source| failed("identify", &path, source))?;

        Ok(Self {
            listener,
            file: (created.dev(), created.ino()),
            path,
            owner: current_uid(),
        })
    }

    pub(crate) async fn accept(&mut self) -> Result<Accepted> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|source| failed("accept a connection on", &self.path, source))?;

        // The kernel answers this from what it recorded when the peer called `connect`, so there is
        // no window in which the process could have exited and its pid been reused by somebody
        // else: the credentials describe whoever actually opened this connection, whatever has
        // become of them since.
        let credentials = stream.peer_cred().map_err(|source| Error::Os {
            action: "read the credentials of a client",
            source,
        })?;

        if credentials.uid() == self.owner {
            return Ok(crate::ipc::trusted(Connection(stream)));
        }

        // Including root, which the socket's mode does not keep out and which nothing here tries
        // to: a root client can read `mixengine.db` directly and does not need the API to do
        // anything the API could do. Refusing it is not a defence, it is this endpoint being one
        // account's and saying so.
        Ok(Accepted::Untrusted(crate::ipc::peer(
            credentials.uid().to_string(),
            credentials.pid().and_then(|pid| u32::try_from(pid).ok()),
        )))
    }
}

impl Drop for Listener {
    /// Unlink the socket, if it is still the one this listener created.
    ///
    /// Nothing else removes it — the kernel keeps the file after the last handle closes — so
    /// leaving it would make every later start take the "somebody died here" path in
    /// [`Listener::bind`]. Checking device and inode first is what stops this from deleting a
    /// *different* daemon's socket: two starting at once can both find the endpoint dead and both
    /// bind, the second one's file replacing the first one's at the same name, and the first one's
    /// shutdown would then take the survivor's socket with it. The single-instance lock (roadmap
    /// task T9) is what makes that race unreachable; this makes its consequence harmless in the
    /// meantime.
    fn drop(&mut self) {
        let ours = fs::metadata(&self.path)
            .is_ok_and(|current| (current.dev(), current.ino()) == self.file);

        if ours {
            // Nothing useful to do with a failure during a drop, and nothing that reaches a log
            // this late in a shutdown. The next start meets the file and handles it.
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// One client's byte stream.
#[derive(Debug)]
pub(crate) struct Connection(UnixStream);

pub(crate) async fn connect(endpoint: &Endpoint) -> Result<Connection> {
    let path = Path::new(endpoint.as_os_str());

    UnixStream::connect(path)
        .await
        .map(Connection)
        .map_err(|source| failed("connect to", path, source))
}

impl AsyncRead for Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Connection {
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

/// Is somebody listening on this socket?
///
/// Blocking, and deliberately so: `bind` is not `async`, and connecting to a Unix socket does not
/// wait for the far side to accept — the kernel either queues the connection in the listener's
/// backlog or fails immediately.
///
/// Fails closed. Only a refusal (nothing is listening) or a missing file proves the socket is dead;
/// every other answer, including a full backlog, is read as "occupied", because the cost of being
/// wrong the other way is unlinking a running daemon's socket.
fn occupied(path: &Path) -> bool {
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => true,
        Err(error) => !matches!(
            error.kind(),
            io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
        ),
    }
}

/// The uid this process runs as.
#[expect(
    unsafe_code,
    reason = "getuid is the POSIX way to ask, takes no arguments, touches no memory and cannot \
              fail; there is no safe binding for it in std"
)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

/// An operation on the socket that the OS refused.
fn failed(action: &'static str, path: &Path, source: io::Error) -> Error {
    Error::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}
