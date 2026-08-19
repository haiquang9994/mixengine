//! Everything the operating system does differently.
//!
//! Core, supervisor, daemon and the clients contain **zero** `#[cfg(target_os = …)]`; all of it
//! lives here behind traits, with `windows/`, `macos/`, `linux/` implementations and an in-memory
//! `mock/` one that is always compiled and used by tests and `--dry-run`.
//!
//! See `.claude/architecture/platform-abstraction.md` for the trait list and the rules every
//! implementation follows (reversible and tagged mutations, atomic read-modify-write, `probe()`
//! before acting, [`Error::UnsupportedPlatform`] instead of `unimplemented!()`).

#![warn(missing_docs)]

use std::sync::Arc;

pub mod ipc;
pub mod lock;
pub mod mock;
pub mod process;
// The one capability whose implementation is not per-OS, because the crate behind it already is —
// see the module's own documentation.
mod secrets;
pub mod signal;
mod traits;

// Shared by `linux/` and `macos/`, which both name what they take from it.
#[cfg(unix)]
mod unix;

// The one thing `secrets` publishes outside this crate: everything else in it is reached through
// the `Keyring` trait a `Host` hands out, and a random string has no host to belong to.
pub use secrets::generate_secret;
pub use traits::{
    DirectoryAccess, HomeDirs, Host, KEYRING_SERVICE, Keyring, PathIntegration, PathLocation,
    PathState,
};

// The three supported operating systems keep their own directory, exactly as the architecture
// document describes them; `#[path]` maps whichever one applies onto a single `sys` name so the
// rest of the crate never spells out a target.
#[cfg(target_os = "linux")]
#[path = "linux/mod.rs"]
mod sys;
#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod sys;
#[cfg(windows)]
#[path = "windows/mod.rs"]
mod sys;

// A new OS gets a directory of its own and an entry above. Failing at compile time is the point:
// silently falling back to "Linux, probably" would put a user's data somewhere no uninstaller
// knows about.
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
compile_error!(
    "MixEngine supports Windows, macOS and Linux. Porting means adding a directory next to \
     src/linux/ and an implementation of every trait in src/traits/."
);

/// The machine this process is running on.
///
/// Constructed once at startup and passed down as `Arc<dyn Host>`; tests inject
/// [`mock::Host`] instead and assert on what it recorded.
#[must_use]
pub fn host() -> Arc<dyn Host> {
    Arc::new(sys::Host::new())
}

/// Failure of a platform operation.
///
/// Library-local on purpose: the conversion into the wire error happens at the daemon boundary, so
/// this enum can describe OS specifics without the API having to know about them.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The capability genuinely does not exist on this OS, or the machine is not configured for it.
    ///
    /// This is a normal answer, not a bug — `reason` is shown to the user and should describe the
    /// manual workaround where one exists.
    #[error("{capability} is not available on this platform: {reason}")]
    UnsupportedPlatform {
        /// The capability that was asked for, e.g. `"PortAccess"`.
        capability: &'static str,
        /// Why it is unavailable, phrased for a user rather than a developer.
        reason: String,
    },

    /// The OS would not say where the current user's data belongs.
    ///
    /// In practice this means the environment is missing what the platform considers mandatory
    /// (`%LOCALAPPDATA%`, `$HOME`), which happens to service accounts and to stripped-down
    /// containers. Setting `MIXENGINE_HOME` is the way out, so the message says so.
    #[error(
        "cannot determine the user's data directory ({reason}) — set MIXENGINE_HOME to choose one \
         explicitly"
    )]
    NoHomeDirectory {
        /// What was missing, phrased for a user rather than a developer.
        reason: &'static str,
    },

    /// A file or directory the OS was asked about could not be touched.
    ///
    /// Shaped like `mixengine_core::Error::Io` on purpose: the path belongs in the message because
    /// "access denied" on its own names nothing, and the OS error stays the `#[source]` so a
    /// message never prints its own cause twice.
    #[error("cannot {action} {}", path.display())]
    Io {
        /// What was being attempted, e.g. `"restrict"`.
        action: &'static str,
        /// The path it was attempted on.
        path: std::path::PathBuf,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// The OS credential store was there and would not do it.
    ///
    /// Names the entry and never the value — a credential must not reach a log through an error
    /// message, which is the accident this whole capability exists to prevent. The store's own
    /// complaint is the `#[source]`, boxed rather than typed: what the backend crate calls its
    /// failures is not vocabulary the daemon should be matching on, and the one distinction that
    /// *is* actionable — no store on this machine at all — is already
    /// [`Error::UnsupportedPlatform`] by the time it gets here.
    #[error("cannot {action} the credential {service}/{key} in the OS keyring")]
    Secret {
        /// What was being attempted: `"read"`, `"store"`, `"forget"`, `"address"`.
        action: &'static str,
        /// The namespace the credential is filed under.
        service: String,
        /// The account within it.
        key: String,
        /// The store's own complaint.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// An operating-system call failed, and there is no path to name in the message.
    ///
    /// [`Error::Io`]'s sibling for the calls that are about something other than a file: reading
    /// this account's SID out of the process token, impersonating whoever is at the other end of a
    /// pipe. `action` completes the sentence "cannot …" and the OS's own message is appended as the
    /// cause, the same way it is there.
    #[error("cannot {action}")]
    Os {
        /// What was being attempted, e.g. `"identify this account"`.
        action: &'static str,
        /// The underlying OS error, generally built from `GetLastError`.
        #[source]
        source: std::io::Error,
    },

    /// The address of the local endpoint is one this OS will not accept.
    ///
    /// Not an I/O failure — nothing was attempted. The address is computed from `MIXENGINE_HOME`,
    /// so `reason` has to name the constraint the home broke, which is the only thing the user can
    /// act on.
    #[error("{address} cannot be used as a local endpoint: {reason}")]
    Address {
        /// The address that was rejected, rendered the way the OS names one.
        address: String,
        /// Which rule it broke, phrased for a user rather than a developer.
        reason: String,
    },

    /// Something is already listening at the local endpoint.
    ///
    /// Its own variant rather than an [`Error::Io`] carrying `AddrInUse`, because it is the normal
    /// answer to "is a daemon already running for this home?" — the question the single-instance
    /// check asks (roadmap task T9), and one whose answer is not a failure at all.
    #[error("another process is already listening on {address}")]
    EndpointInUse {
        /// The endpoint that is taken.
        address: String,
    },

    /// A command the platform layer shells out to failed.
    ///
    /// Kept distinct from [`Error::Io`]: the binary ran and said no, which is a different problem
    /// from not being able to run it, and its own diagnostics are the only ones worth showing.
    #[error("{command} failed{} ({status}){}", about(path.as_deref()), said(output))]
    Command {
        /// The program that was run, e.g. `"icacls"`.
        command: &'static str,
        /// The path it was run against, when it was run against one. `None` for a tool that was
        /// asked about the machine rather than about a file.
        path: Option<std::path::PathBuf>,
        /// How it exited, rendered for a human.
        status: String,
        /// Whatever it wrote to stderr, trimmed. Empty when it said nothing.
        output: String,
    },
}

/// ` for <path>`, when there is one.
fn about(path: Option<&std::path::Path>) -> String {
    path.map_or_else(String::new, |path| format!(" for {}", path.display()))
}

/// The tool's own complaint, when it made one. A tool that fails silently should not leave a
/// dangling colon behind in the message.
fn said(output: &str) -> String {
    if output.is_empty() {
        String::new()
    } else {
        format!(": {output}")
    }
}

/// Result of a platform operation.
pub type Result<T> = std::result::Result<T, Error>;
