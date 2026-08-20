//! Who is listening on a local TCP port.

use crate::Result;

/// The process holding a port, as far as this machine will say.
///
/// **Both fields are optional, and how much of it can be answered is per-OS.** Windows publishes
/// the owning pid of every listener to anybody who asks and refuses the *name* of a process
/// belonging to another account; Linux publishes the listening socket but maps it to a pid only
/// through `/proc/<pid>/fd`, which the same refusal applies to — so a listener owned by another
/// user is a holder with nothing filled in at all.
///
/// That is deliberately not the same answer as nobody listening. "3306 is held by mysqld.exe" is
/// the best sentence, "held by pid 4242" a worse one, "held by another program on this machine"
/// worse again — and all three of them send a user somewhere useful, which "not ready within 30s"
/// does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortHolder {
    /// The process id, where this account may learn it.
    pub pid: Option<u32>,

    /// The file name of its program — `mysqld.exe`, `httpd` — or `None` where the OS refused.
    ///
    /// The file name rather than the full path, because this is shown to somebody who has to
    /// recognise the program they installed, and because the path is not always readable when the
    /// name is.
    pub name: Option<String>,
}

/// Who is listening on a local TCP port.
///
/// The question a failed start asks. Every service in this product binds something, and the most
/// common reason one of them will not start on a developer's machine is that a program MixEngine
/// does not manage is already on its port — an XAMPP, a Homebrew MariaDB, Windows' own `MySQL80`
/// service. None of those has a `services` row, so the daemon cannot look this up in its own state
/// and has to ask the OS.
///
/// **Deliberately not [`crate::PathIntegration`]'s kind of capability: nothing here mutates.** It
/// reads a table the OS already keeps, and is safe to call from an error path — which is where it
/// is called from, so a diagnosis that fails must never become the failure being diagnosed.
///
/// Not to be confused with the `PortAccess` capability of roadmap task T42, which is about being
/// *allowed* to bind 80 and 443. This one is about who got there first.
pub trait PortOwner: std::fmt::Debug + Send + Sync {
    /// Which process is listening on `port`, if any is.
    ///
    /// Loopback and every-interface listeners both count, and a socket that is merely *connected*
    /// to that port does not: what a start collides with is a listener.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) where this OS offers no
    /// way to ask, [`Error::Command`](crate::Error::Command) where the tool it asks through failed,
    /// and [`Error::Io`](crate::Error::Io) for the read itself. **Every caller of this is expected
    /// to treat an error as "no diagnosis" and carry on** — the failure being explained is the one
    /// the user has to hear about.
    fn listening_on(&self, port: u16) -> Result<Option<PortHolder>>;
}
