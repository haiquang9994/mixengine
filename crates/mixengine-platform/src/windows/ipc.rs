//! A named pipe this account owns and alone may open, with each end checking who the other is.
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
//!
//! **And the client asks who it reached.** Refusing to join somebody else's pipe keeps this daemon
//! from serving strangers; it does nothing for a `mix` that dials one. The name is derivable — the
//! SID is public and [`fingerprint`] is written out just above — the namespace is flat, and
//! `CreateNamedPipeW` needs no privilege, so another account can hold the name before the daemon
//! comes up and be handed every request, `elevation.*` included. So [`dial`] reads the owner of the
//! pipe object it opened and hangs up on one this account does not own, before the first byte. It
//! is the mirror of the peer check in [`Listener::accept`] and the reason [`create`] states the
//! owner rather than letting the token supply one.

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
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, ERROR_SUCCESS, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, RevertToSelf, SECURITY_ATTRIBUTES,
    TOKEN_QUERY,
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
            Err(error) if refused_access(&error) => match occupant(&address, &owner) {
                // Nothing answered, so the name is not the problem and the original refusal is the
                // truth we have. Reporting "already in use" here would send somebody looking for a
                // daemon that is not running.
                Occupant::Nothing => return Err(error),

                Occupant::Theirs(account) => {
                    return Err(Error::EndpointNotOurs {
                        address: address.to_string_lossy().into_owned(),
                        account,
                    });
                }

                Occupant::Ours | Occupant::Unidentified => {
                    return Err(Error::EndpointInUse {
                        address: address.to_string_lossy().into_owned(),
                    });
                }
            },

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
    dial(endpoint.as_os_str(), &sid::current_user()?).await
}

/// Dial the pipe, and hang up on one that `owner` is not serving.
///
/// The owner is a parameter for the same reason [`create`]'s is: it is one question to the OS,
/// asked once by the caller, and passing it makes both halves of the check testable against a real
/// pipe without a second account on the machine to be refused from.
async fn dial(address: &OsStr, owner: &str) -> Result<Connection> {
    for _ in 0..BUSY_ATTEMPTS {
        // The default security quality of service tokio applies —
        // `SECURITY_IDENTIFICATION | SECURITY_SQOS_PRESENT` — is what lets the daemon's peer check
        // work at all: it permits the server to learn who we are and nothing more. A client that
        // connected anonymously would be refused rather than trusted, which is the right way round.
        match ClientOptions::new().open(address) {
            Ok(client) => {
                let serving = owner_of(&client)?;

                if serving != owner {
                    // Closed before anything is written to it, which is the whole of the remedy:
                    // the danger is not that a stranger holds the name, it is that a request gets
                    // sent to them.
                    drop(client);

                    return Err(Error::EndpointNotOurs {
                        address: address.to_string_lossy().into_owned(),
                        account: serving,
                    });
                }

                return Ok(Connection::Client(client));
            }

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

/// The account serving the pipe this handle is connected to.
///
/// The **owner of the pipe object**, not the process that created it.
/// `GetNamedPipeServerProcessId` and then `OpenProcess` would be the same mistake [`peer_of`]
/// refuses to make in the other direction: it hands back a number, and by the time a number has
/// been turned back into a process the OS is free to have reused it. An owner is stamped on the
/// object when it is created, cannot be set to an account the creator does not hold — that needs
/// `SeRestorePrivilege`, which is not something a standard account has — and is still true however
/// long this takes to read.
///
/// Readable because `ClientOptions` opens with `GENERIC_READ`, whose generic mapping includes
/// `READ_CONTROL`; a server whose DACL withheld that would have refused the open as well. Any
/// failure here is therefore a refusal and not a shrug: the caller propagates it and dials nobody.
fn owner_of(pipe: &NamedPipeClient) -> Result<String> {
    let handle: HANDLE = pipe.as_raw_handle().cast();
    let mut owner: PSID = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "the handle is the connected pipe, borrowed for the call; `owner` points into the \
                  descriptor written back beside it, which the guard below frees exactly once"
    )]
    let read = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };

    if read != ERROR_SUCCESS {
        return Err(Error::Os {
            action: "find out which account is serving the pipe",
            #[expect(
                clippy::cast_possible_wrap,
                reason = "a WIN32_ERROR is what `from_raw_os_error` takes; the two agree on every \
                          code Windows produces and disagree only on a bit pattern that is not one"
            )]
            source: io::Error::from_raw_os_error(read as i32),
        });
    }

    // Taken before anything reads through `owner`, which points inside it: an early return between
    // the two would leak the descriptor, and rendering the SID is the only thing left that can fail.
    let descriptor = Descriptor(descriptor);
    let sid = sid::render(owner);

    drop(descriptor);

    sid
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

/// Who is already serving this pipe.
///
/// Four answers rather than the two [`Listener::bind`] needs to decide whether to fail, because
/// they are not the same message. A name held by a daemon of this account's is the ordinary case
/// and says "one is already running"; a name held by somebody else is the squat [`dial`] exists to
/// catch, and telling the user that as "another process is already listening" would send them
/// hunting for a daemon of their own to stop while a stranger holds the name they are about to
/// dial.
enum Occupant {
    /// Nothing answered. The name is not why `create` was refused.
    Nothing,

    /// A daemon this account is running.
    Ours,

    /// Somebody else's, named for the message.
    Theirs(String),

    /// Something answered and could not be asked who: every instance was already spoken for
    /// (`ERROR_PIPE_BUSY`), or its owner could not be read off the handle. Reported as [`Self::Ours`]
    /// is — the start is refused either way, and a daemon of ours under load is much the commoner
    /// reason to meet it.
    Unidentified,
}

/// Dial the pipe to find out.
///
/// Blocking, like its Unix counterpart, and for the same reason: `bind` is not `async`. A pipe
/// answers instantly or not at all — `reject_remote_clients` means there is nothing here that can
/// wait on a remote host.
///
/// Only two answers mean the name is taken: a connection that succeeded, and `ERROR_PIPE_BUSY`,
/// which says a server exists but every instance it has created is spoken for. Everything else,
/// including a refusal, leaves the caller's original failure standing.
fn occupant(address: &OsStr, owner: &str) -> Occupant {
    // The connection is dropped immediately. A daemon at the other end sees a client that connected
    // and closed, which is what a cancelled `mix status` looks like too.
    match ClientOptions::new().open(address) {
        Ok(pipe) => match owner_of(&pipe) {
            Ok(serving) if serving == owner => Occupant::Ours,
            Ok(serving) => Occupant::Theirs(serving),
            Err(_) => Occupant::Unidentified,
        },

        Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
            Occupant::Unidentified
        }

        Err(_) => Occupant::Nothing,
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
    ///
    /// **The owner is stated and not left to the token**, because it is what [`owner_of`] compares
    /// against on the other side. A descriptor that names none gets the creating token's default
    /// owner, and that is a machine policy rather than a fact: where "System objects: Default owner
    /// for objects created by members of the Administrators group" is set to the group, an
    /// administrator's pipe would be owned by `S-1-5-32-544` and every client would refuse it.
    fn granting_full_control(owner: &str) -> Result<Self> {
        // `O:` the owner, then `D:` a DACL, `P` protected from inheritance, and one allow-ACE
        // granting GENERIC_ALL.
        let sddl: Vec<u16> = format!("O:{owner}D:P(A;;GA;;;{owner})")
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{Descriptor, Listener, Occupant, PSID, dial, occupant, owner_of, sid};
    use crate::Error;
    use crate::ipc::Endpoint;

    /// A pipe of this test's own, named after a home nothing else uses.
    fn endpoint() -> (TempDir, Endpoint) {
        let run = TempDir::new().expect("the system temporary directory is writable");
        let endpoint = Endpoint::in_run_dir(run.path()).expect("this account has a SID");

        (run, endpoint)
    }

    // One listener per probe, and not two probes against one: a listener that is not inside
    // `accept` has exactly one free instance, so the first `occupant` takes it and the second meets
    // `ERROR_PIPE_BUSY` — which is `Unidentified` and is the right answer to a different question.

    #[tokio::test]
    async fn a_name_this_account_holds_is_recognised() {
        let (_run, endpoint) = endpoint();
        let _listener = Listener::bind(&endpoint).unwrap();

        assert!(matches!(
            occupant(endpoint.as_os_str(), &sid::current_user().unwrap()),
            Occupant::Ours
        ));
    }

    #[tokio::test]
    async fn a_name_somebody_else_holds_is_not_reported_as_a_daemon_of_ours() {
        // What the daemon says when it cannot bind. Both cases fail the start, so this is about the
        // message and not about the outcome — but "another process is already listening" sends the
        // user looking for a daemon of their own to stop, which is the wrong thing to be doing
        // while somebody else holds the name they are about to dial.
        let (_run, endpoint) = endpoint();
        let _listener = Listener::bind(&endpoint).unwrap();

        let held = occupant(endpoint.as_os_str(), "S-1-5-21-0-0-0-500");

        assert!(
            matches!(&held, Occupant::Theirs(account) if account == &sid::current_user().unwrap()),
            "the squatter was not named"
        );
    }

    #[test]
    fn a_name_nothing_answers_at_is_held_by_nobody() {
        let (_run, endpoint) = endpoint();

        assert!(matches!(
            occupant(endpoint.as_os_str(), &sid::current_user().unwrap()),
            Occupant::Nothing
        ));
    }

    #[test]
    fn the_descriptor_names_an_owner_rather_than_leaving_one_to_be_assigned() {
        // What the test below cannot show on this machine: a descriptor with no `O:` is
        // completed from the creating token's default owner, which is the user SID here and is
        // `Administrators` where the "Default owner for objects created by members of the
        // Administrators group" policy says so. Read structurally, off the descriptor itself, so
        // the claim does not depend on which of the two this machine is set to.
        let account = sid::current_user().unwrap();
        let descriptor = Descriptor::granting_full_control(&account).unwrap();

        let mut owner: PSID = std::ptr::null_mut();
        let mut defaulted = 0;

        #[expect(
            unsafe_code,
            reason = "the descriptor is alive for the call and the SID read out of it is rendered \
                      before it goes out of scope"
        )]
        let read = unsafe {
            windows_sys::Win32::Security::GetSecurityDescriptorOwner(
                descriptor.0,
                &raw mut owner,
                &raw mut defaulted,
            )
        };

        assert_ne!(read, 0, "the descriptor could not be read back");
        assert!(!owner.is_null(), "the descriptor names no owner at all");
        assert_eq!(sid::render(owner).unwrap(), account);
    }

    #[tokio::test]
    async fn the_pipe_states_this_account_as_its_owner() {
        // Stated in the descriptor rather than left to the token's default owner, which is a
        // machine policy and not a fact: on a machine set to "Administrators group" the pipe of an
        // administrator would be owned by a *group*, and every client would refuse it.
        let (_run, endpoint) = endpoint();
        let _listener = Listener::bind(&endpoint).unwrap();

        let client = super::ClientOptions::new()
            .open(endpoint.as_os_str())
            .unwrap();

        assert_eq!(
            owner_of(&client).unwrap(),
            sid::current_user().unwrap(),
            "the pipe does not name this account as its owner"
        );
    }

    #[tokio::test]
    async fn a_pipe_this_account_serves_is_dialled() {
        let (_run, endpoint) = endpoint();
        let _listener = Listener::bind(&endpoint).unwrap();

        dial(endpoint.as_os_str(), &sid::current_user().unwrap())
            .await
            .expect("this account's own daemon should be reachable");
    }

    #[tokio::test]
    async fn a_pipe_somebody_else_serves_is_refused() {
        // The attack: the pipe namespace is flat and the name is derivable, so another account can
        // create `\\.\pipe\mixengine.<our sid>.<fingerprint>` before the daemon comes up and be
        // handed every request `mix` makes.
        //
        // A second account cannot be created from a unit test, and Windows will not let this
        // process claim another account's SID as an owner — so what is varied is the *expectation*.
        // Everything else is real: a real pipe, its owner read off the handle by the OS, and the
        // refusal path that follows. The cross-account case belongs to the `system` job, which is
        // the one leg allowed to create an account to be refused from.
        let (_run, endpoint) = endpoint();
        let _listener = Listener::bind(&endpoint).unwrap();

        let error = dial(endpoint.as_os_str(), "S-1-5-21-0-0-0-500")
            .await
            .expect_err("a pipe served by another account should not be dialled");

        assert!(
            matches!(error, Error::EndpointNotOurs { .. }),
            "the wrong error came back: {error}"
        );
        assert!(
            error.to_string().contains(&sid::current_user().unwrap()),
            "the message does not say who is serving the pipe: {error}"
        );
    }
}
