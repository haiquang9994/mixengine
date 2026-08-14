//! Putting `<root>/bin` on the PATH that every shell this user starts inherits.
//!
//! **One entry, never one per version.** What goes on the PATH is the directory of shims
//! ([runtime-versions.md](../../../../.claude/features/runtime-versions.md)), and the shim is what
//! decides which PHP a directory uses — so the PATH is written once, at the moment somebody asks
//! for it, and never touched again when a version is installed or removed.
//!
//! **Nothing here is elevated, on any of the three systems.** The user's own environment is a
//! user-writable registry value on Windows and a file in the user's home on both others, so this
//! capability does what `.claude/architecture/overview.md` says every other change outside the root
//! needs `mixengine-elevate` for — and needs it for none of it. Which is also why it stays out of
//! the privileged-operation list: an operation that would prompt for a password to edit
//! `~/.zprofile` would be teaching people to type one for no reason.
//!
//! **The one thing a user-level change cannot promise is precedence.** On Windows the effective
//! PATH is the machine's value followed by this user's, so a PHP installed for the whole machine is
//! ahead of `<root>/bin` whatever this writes; on Unix a profile file is read before whatever a
//! later-sourced script does to `PATH`. Prepending inside the value we own is the most either
//! system allows without touching something that is not ours, and `mix doctor` (T47) is where
//! "something else is winning" belongs.

use std::path::Path;

use crate::Result;

/// One place this operating system keeps a `PATH` that survives a reboot.
///
/// A file on Unix and a registry value on Windows, named as a person would look for it rather than
/// as the OS addresses it — the point of listing them at all is that somebody who wants to undo
/// this by hand, or who wonders why a new terminal still cannot find `php`, can go and look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathLocation {
    /// What to show: a file path, or the registry value's full name.
    pub name: String,

    /// Whether the directory is in it as things stand.
    pub present: bool,

    /// Whether *this call* is what put it there or took it away.
    ///
    /// Always `false` from [`PathIntegration::state`], which changes nothing. It is what lets a
    /// caller say "already done" rather than claiming to have done it again — an install that
    /// reports a write it did not perform is one nobody can tell from a real one.
    pub changed: bool,
}

/// Every such place, and what each of them says.
///
/// A list rather than a boolean because Unix has more than one: a home with a `.bash_profile` and a
/// `.zprofile` needs the line in both, and a person whose login shell is the one that was missed
/// would otherwise be told the PATH was set up while their terminal disagreed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathState {
    /// Each place, in the order this OS reads them.
    pub locations: Vec<PathLocation>,
}

impl PathState {
    /// Is the directory on the PATH of every shell this user can start?
    ///
    /// **Every location and not any of them.** One of two profile files carrying it means exactly
    /// one of two shells finds `php`, which is the confusing half-state this answer exists to name.
    #[must_use]
    pub fn complete(&self) -> bool {
        !self.locations.is_empty() && self.locations.iter().all(|location| location.present)
    }

    /// Did this call write anything at all?
    #[must_use]
    pub fn changed(&self) -> bool {
        self.locations.iter().any(|location| location.changed)
    }
}

/// Where this OS keeps the PATH, and how to add one directory to it reversibly.
///
/// Every implementation follows `.claude/architecture/platform-abstraction.md`'s first two rules
/// literally, because this is the capability they were written for: a mutation is **tagged** — the
/// Unix block sits between `# BEGIN MixEngine` and `# END MixEngine` and nothing outside it is ever
/// read or written — and a read-modify-write is **atomic**, through a temporary file in the same
/// directory and a rename.
pub trait PathIntegration: std::fmt::Debug + Send + Sync {
    /// Put `dir` on the persisted PATH, ahead of what is already there.
    ///
    /// Idempotent: a location that already carries it is left byte for byte alone and reported as
    /// `changed: false`. `dir` need not exist yet — a PATH entry naming a missing directory is
    /// ignored by every shell, and refusing here would make the order in which a home is set up
    /// matter.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) naming the file or the key that could not be written,
    /// [`Error::Os`](crate::Error::Os) when the registry refuses, and
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) where the machine has no
    /// home directory to write a profile into.
    fn add(&self, dir: &Path) -> Result<PathState>;

    /// Take it off again, leaving everything else exactly as it was.
    ///
    /// Idempotent in the same way, and the half that makes this capability worth having: a home
    /// that is deleted must not leave a line behind in somebody's `.zprofile` naming a directory
    /// that is gone.
    ///
    /// # Errors
    ///
    /// As [`add`](Self::add).
    fn remove(&self, dir: &Path) -> Result<PathState>;

    /// What is in force now, changing nothing.
    ///
    /// Reads the *persisted* PATH and not this process's own environment: what a person is asking
    /// is whether the next terminal they open will find `php`, and the daemon's environment is
    /// whatever started it.
    ///
    /// # Errors
    ///
    /// As [`add`](Self::add), minus everything that can only go wrong while writing.
    fn state(&self, dir: &Path) -> Result<PathState>;
}
