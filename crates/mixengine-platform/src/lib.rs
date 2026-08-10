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

pub mod mock;
mod traits;

pub use traits::{HomeDirs, Host};

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
}

/// Result of a platform operation.
pub type Result<T> = std::result::Result<T, Error>;
