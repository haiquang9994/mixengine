//! A named pipe whose DACL names this account, with the client impersonated on every accept.
//!
//! Windows has no equivalent of a socket file sitting inside a directory we already locked down, so
//! both halves of the protection have to be stated explicitly here.
//!
//! **The name carries the home.** The pipe namespace is flat and machine-wide, so
//! `\\.\pipe\mixengine` alone would be one endpoint per machine: a sandbox daemon started with
//! `MIXENGINE_HOME` pointing somewhere disposable would collide with the real install, and two
//! tests would collide with each other. The name is therefore
//! `\\.\pipe\mixengine.<sid>.<fingerprint of run/>` — the SID because a second account signing in
//! runs its own daemon, and the fingerprint because one account can have several homes. This is a
//! correction to `.claude/architecture/daemon-and-ipc.md`, which described the SID alone.
//!
//! **The first instance is claimed, not joined.** `FILE_FLAG_FIRST_PIPE_INSTANCE` is what makes
//! [`Listener::bind`] refuse a name somebody else already created, rather than quietly adding an
//! instance to *their* pipe and serving whoever they attract. Every later instance must not carry
//! the flag, or it would refuse the pipe we ourselves are holding.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::io::AsRawHandle as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use std::{mem, ptr};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    PSECURITY_DESCRIPTOR, RevertToSelf, SECURITY_ATTRIBUTES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Pipes::ImpersonateNamedPipeClient;
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

// `super`, not `crate::windows`: `#[path]` maps whichever OS directory applies onto a single `sys`
// module, so the name this file is compiled under is not the name of the directory it sits in.
use super::sid;
use crate::ipc::{Accepted, Endpoint};
use crate::{Error, Result};

/// Everything before the part that identifies which daemon this is.
const PIPE_PREFIX: &str = r"\\.\pipe\mixengine.";

/// How long a client keeps trying while the daemon is between pipe instances.
///
/// The gap is the few microseconds in [`Listener::accept`] between a client being connected and the
/// replacement instance existing, so this is generous by orders of magnitude on purpose: it costs
/// nothing when nothing is wrong, and one second is short enough that a daemon which is genuinely
/// wedged is still reported quickly.
const BUSY_ATTEMPTS: u32 = 20;

/// How long to wait between those attempts.
const BUSY_PAUSE: Duration = Duration::from_millis(50);

/// The pipe belonging to the home whose `run/` directory this is.
pub(crate) fn address(run: &Path) -> Result<OsString> {
    let owner = sid::current_user()?;

    Ok(OsString::from(format!(
        "{PIPE_PREFIX}{owner}.{:016x}",
        fingerprint(run)
    )))
}

/// A short, stable stand-in for a home directory.
///
/// FNV-1a, written out rather than taken from a crate: this is a name, not a defence. Nothing is
/// kept secret by it and nothing is authenticated with it — an attacker who wants to know which
/// pipe a home maps to can simply run the same code. All it has to do is differ between two homes
/// and stay the same across two starts of the same one.
///
/// Case-folded first, because Windows paths are: a `--home C:\dev\sandbox` and a
/// `MIXENGINE_HOME=c:\dev\sandbox` name one directory and must reach one daemon.
fn fingerprint(path: &Path) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    path.to_string_lossy()
        .to_lowercase()
        .bytes()
        .fold(OFFSET, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
}

/// The daemon's end: the pipe instance currently waiting for a client.
#[derive(Debug)]
pub(crate) struct Listener {
    address: OsString,
    /// The SID every accepted client is compared against.
    owner: String,
    server: NamedPipeServer,
}

impl Listener {
    pub(crate) fn bind(endpoint: &Endpoint) -> Result<Self> {
        let address = endpoint.as_os_str().to_os_string();
        let owner = sid::current_user()?;

        let server = match create(&address, &owner, Instance::First) {
            Ok(server) => server,

            // `FILE_FLAG_FIRST_PIPE_INSTANCE` reports a name that is already taken as
            // `ERROR_ACCESS_DENIED` — the same answer a DACL we are not allowed to replace would
            // give, and an unhelpful one either way. Dialling the pipe is what separates them.
            Err(error) if refused_access(&error) => {
                if occupied(&address) {
                    return Err(Error::EndpointInUse {
                        address: address.to_string_lossy().into_owned(),
                    });
                }

                // Nothing answered, so the name is not the problem and the original refusal is the
                // truth we have. Reporting "already in use" here would send somebody looking for a
                // daemon that is not running.
                return Err(error);
            }

            Err(error) => return Err(error),
        };

        Ok(Self {
            address,
            owner,
            server,
        })
    }

    pub(crate) async fn accept(&mut self) -> Result<Accepted> {
        self.server
            .connect()
            .await
            .map_err(|source| failed("accept a connection on", &self.address, source))?;

        // The replacement instance is created before this connection is looked at, so that the
        // moment a client is taken there is already a free instance behind it: without that, every
        // second client that arrives while the first is being identified meets `ERROR_PIPE_BUSY`
        // and has to fall into the retry loop in `connect` to get anywhere.
        //
        // A failure here must not leave the connected instance sitting in `self.server`. It would
        // still be *this* listener's pipe, so the name stays claimed and nothing else can take it,
        // but `connect` on an already-connected instance returns `ERROR_PIPE_CONNECTED`, which mio
        // reports as success — so every later accept would come straight back here, hand out the
        // same stale peer or fail again, and never wait for anybody. Disconnecting first returns
        // the instance to the state the next accept expects and closes the client that provoked it.
        let connected = match create(&self.address, &self.owner, Instance::Additional) {
            Ok(next) => mem::replace(&mut self.server, next),

            Err(error) => {
                // Nothing useful to do with a failure to disconnect, and no honest way to report
                // two errors from one call: the one being returned is the one that explains why
                // this accept produced nothing.
                let _ = self.server.disconnect();
                return Err(error);
            }
        };

        let peer = peer_of(&connected)?;

        if peer == self.owner {
            return Ok(crate::ipc::trusted(Connection::Server(connected)));
        }

        // No pid: it would mean asking the pipe which process opened it and then opening that
        // process, by a number the OS is free to have reused in between. The SID comes from the
        // client's own token and cannot be about somebody else.
        Ok(Accepted::Untrusted(crate::ipc::peer(peer, None)))
    }
}

/// One client's byte stream, from whichever end of the pipe.
///
/// The two ends are different types on Windows, unlike the one `UnixStream` both ends of a socket
/// get, so the difference is absorbed here rather than being pushed into the public API.
#[derive(Debug)]
pub(crate) enum Connection {
    Server(NamedPipeServer),
    Client(NamedPipeClient),
}

pub(crate) async fn connect(endpoint: &Endpoint) -> Result<Connection> {
    let address = endpoint.as_os_str();

    for _ in 0..BUSY_ATTEMPTS {
        // The default security quality of service tokio applies —
        // `SECURITY_IDENTIFICATION | SECURITY_SQOS_PRESENT` — is what lets the daemon's peer check
        // work at all: it permits the server to learn who we are and nothing more. A client that
        // connected anonymously would be refused rather than trusted, which is the right way round.
        match ClientOptions::new().open(address) {
            Ok(client) => return Ok(Connection::Client(client)),

            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                tokio::time::sleep(BUSY_PAUSE).await;
            }

            Err(source) => return Err(failed("connect to", address, source)),
        }
    }

    Err(failed(
        "connect to",
        address,
        io::Error::from_raw_os_error(ERROR_PIPE_BUSY as i32),
    ))
}

impl AsyncRead for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Server(pipe) => Pin::new(pipe).poll_read(cx, buf),
            Self::Client(pipe) => Pin::new(pipe).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Server(pipe) => Pin::new(pipe).poll_write(cx, buf),
            Self::Client(pipe) => Pin::new(pipe).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Server(pipe) => Pin::new(pipe).poll_flush(cx),
            Self::Client(pipe) => Pin::new(pipe).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Server(pipe) => Pin::new(pipe).poll_shutdown(cx),
            Self::Client(pipe) => Pin::new(pipe).poll_shutdown(cx),
        }
    }
}

/// Whether this pipe instance is the one that claims the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Instance {
    /// Carries `FILE_FLAG_FIRST_PIPE_INSTANCE`: creation fails if the name already exists.
    First,
    /// Another instance of a pipe this process already owns.
    Additional,
}

/// Create one instance of the pipe, readable and writable by `owner` alone.
fn create(address: &OsStr, owner: &str, instance: Instance) -> Result<NamedPipeServer> {
    let descriptor = Descriptor::granting_full_control(owner)?;

    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        // The daemon spawns children — every managed service, and `mixengine-elevate`. None of them
        // has any business inheriting a handle to the API they are managed through.
        bInheritHandle: 0,
    };

    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(instance == Instance::First)
        // A named pipe is reachable over SMB by default. Nothing about MixEngine's API is meant to
        // leave this machine, and `.claude/architecture/security-model.md` says so.
        .reject_remote_clients(true);

    // `descriptor` is a local and is freed at the end of this function, which is all the contract
    // needs: `CreateNamedPipeW` copies the descriptor into the object it creates, so nothing below
    // this call reads through the pointer again.
    #[expect(
        unsafe_code,
        reason = "`attributes` is a fully initialised SECURITY_ATTRIBUTES pointing at a descriptor \
                  that is alive for the whole of this call, which is what the safety contract asks \
                  for"
    )]
    let server = unsafe {
        options.create_with_security_attributes_raw(address, (&raw mut attributes).cast())
    };

    server.map_err(|source| failed("create", address, source))
}

/// Is somebody already serving this pipe?
///
/// Blocking, like its Unix counterpart, and for the same reason: `bind` is not `async`. A pipe
/// answers instantly or not at all — `reject_remote_clients` means there is nothing here that can
/// wait on a remote host.
///
/// Only two answers mean "occupied": a connection that succeeded, and `ERROR_PIPE_BUSY`, which says
/// a server exists but every instance it has created is spoken for. Everything else, including a
/// refusal, leaves the caller's original failure standing.
fn occupied(address: &OsStr) -> bool {
    // The connection is dropped immediately. A daemon at the other end sees a client that connected
    // and closed, which is what a cancelled `mix status` looks like too.
    match ClientOptions::new().open(address) {
        Ok(_) => true,
        Err(error) => error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32),
    }
}

/// Did Windows refuse this because of permissions — or because the name is taken?
fn refused_access(error: &Error) -> bool {
    matches!(
        error,
        Error::Io { source, .. } if source.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32)
    )
}

/// The SID of whoever is at the other end of this pipe.
///
/// By impersonation, which is the only way of asking that cannot be answered about the wrong
/// process: `GetNamedPipeClientProcessId` hands back a number, and a number has to be turned back
/// into a process — by which time the client may have exited and something else may hold its pid.
/// The token adopted here belongs to the client that connected, whatever has become of it since.
///
/// The impersonation lasts from one synchronous call to the next, with nothing awaited in between,
/// because it is a property of the *thread* and tokio's worker threads are shared: a future that
/// yielded while impersonating would hand a stranger's identity to whatever ran next.
fn peer_of(pipe: &NamedPipeServer) -> Result<String> {
    let handle: HANDLE = pipe.as_raw_handle().cast();

    #[expect(
        unsafe_code,
        reason = "the handle is the connected pipe instance, borrowed for the duration of the call"
    )]
    let impersonating = unsafe { ImpersonateNamedPipeClient(handle) };

    if impersonating == 0 {
        return Err(Error::Os {
            action: "identify a client",
            source: io::Error::last_os_error(),
        });
    }

    let sid = thread_token().and_then(|token| sid::of_token(token.0));

    #[expect(
        unsafe_code,
        reason = "paired with the ImpersonateNamedPipeClient above, on every path out"
    )]
    let reverted = unsafe { RevertToSelf() };

    if reverted == 0 {
        // Documented as unable to fail in any way this code can cause. If it does anyway, this
        // thread is still carrying the client's identity and there is no second way to put it
        // down — and it is one of tokio's workers, so returning it to the pool would run the next
        // task, for any client, as whoever just connected. Neither an `Err` nor a panic prevents
        // that: both unwind back into the runtime on this same thread.
        //
        // So the process ends here, without unwinding. It is the harshest thing in this crate and
        // the only proportionate one: every managed service is a child that outlives us and can be
        // picked back up on the next start, while a daemon quietly acting as somebody else cannot
        // be undone after the fact.
        let failure = io::Error::last_os_error();
        eprintln!("mixengined: cannot stop impersonating a client ({failure}) — aborting");
        std::process::abort();
    }

    sid
}

/// The impersonation token this thread is currently carrying.
fn thread_token() -> Result<sid::Token> {
    let mut token: HANDLE = ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "GetCurrentThread returns a pseudo-handle that needs no closing, and the token is \
                  written into a local this function hands to an owning guard"
    )]
    // `openasself` is TRUE: the access check for opening the token is made against this process's
    // own context rather than against the identity just adopted. Without it a plain user account —
    // which is what the daemon always runs as — cannot open the token it is impersonating.
    let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &raw mut token) };

    if opened == 0 {
        return Err(Error::Os {
            action: "open the access token of a client",
            source: io::Error::last_os_error(),
        });
    }

    Ok(sid::Token(token))
}

/// A security descriptor built from SDDL, released when it goes out of scope.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    /// `owner` may do everything with the pipe; nobody else appears in the DACL at all.
    ///
    /// Built from SDDL rather than with `SetEntriesInAclW`, which is the same decision
    /// `windows/access.rs` explains at length: composing an ACL by hand means computing sizes
    /// behind raw pointers, where a mistake yields a *wrong* ACL rather than a crash. One string,
    /// one call, and the parser is Microsoft's.
    ///
    /// `SYSTEM` and `Administrators` are on the directory ACL and deliberately not on this one.
    /// There they are unavoidable — an administrator can take ownership of a file whatever it says
    /// — and naming them keeps a repair tool working. A pipe has no such story: it exists only
    /// while the daemon runs, and nothing needs to reach it but the account being served.
    fn granting_full_control(owner: &str) -> Result<Self> {
        // `D:` a DACL, `P` protected from inheritance, then one allow-ACE granting GENERIC_ALL.
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{owner})")
            .encode_utf16()
            .chain(Some(0))
            .collect();

        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();

        #[expect(
            unsafe_code,
            reason = "the SDDL is NUL-terminated and lives for the call; the descriptor it \
                      allocates is owned by the returned guard"
        )]
        let built = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        };

        if built == 0 {
            return Err(Error::Os {
                action: "build the permissions for the pipe",
                source: io::Error::last_os_error(),
            });
        }

        Ok(Self(descriptor))
    }
}

impl Drop for Descriptor {
    #[expect(
        unsafe_code,
        reason = "the descriptor was allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW, \
                  is owned by this guard, and is freed exactly once"
    )]
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0.cast());
        }
    }
}

/// Windows services listen on TCP, not on a socket in the filesystem.
///
/// The `AF_UNIX` support Windows 10 gained is real but is not what any of the servers MixEngine
/// manages uses there: php-fpm's Windows build listens on a port, and a spec naming a socket path
/// on this system was written for another one.
pub(crate) const SERVICE_SOCKETS: bool = false;

/// There is no socket to reach; see [`SERVICE_SOCKETS`].
///
/// Answered rather than attempted, because the failure a caller needs is "this spec cannot work
/// here" and not "connection refused", which it would otherwise retry until its timeout ran out.
pub(crate) async fn reach_socket(path: &Path) -> Result<()> {
    Err(Error::UnsupportedPlatform {
        capability: "a service listening on a Unix domain socket",
        reason: format!(
            "nothing on Windows listens on {} — the same service listens on a TCP port here, and \
             the spec that named a socket path was written for another system",
            path.display()
        ),
    })
}

/// An operation on the pipe that Windows refused.
///
/// The pipe name goes into [`Error::Io`]'s `path`, which is a slight stretch of the field's name
/// and none at all of the message it produces: `\\.\pipe\…` is what `CreateFile` was handed, and
/// naming it is what makes "cannot connect to" mean anything.
fn failed(action: &'static str, address: &OsStr, source: io::Error) -> Error {
    Error::Io {
        action,
        path: PathBuf::from(address),
        source,
    }
}
