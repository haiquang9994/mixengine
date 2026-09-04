//! Registering the daemon so this machine starts it again at the next login.
//!
//! **"Service" here is the operating system's word, not MixEngine's.** The trait is
//! [`ServiceInstaller`] because
//! [ADR 0002](../../../../.claude/decisions/0002-cross-platform-from-day-one.md) named it that on
//! the first day and an accepted decision record is not edited; what it installs is one autostart
//! entry for `mixengined`, and nothing in it is about MariaDB or php-fpm. Everything a reader meets
//! more often — this module, the values below, the API, the command — is spelled `autostart`.
//!
//! **Nothing here is elevated, on any of the three systems.** A Task Scheduler logon task under this
//! account's own SID, a plist in this user's `~/Library/LaunchAgents`, a systemd *user* unit in this
//! user's `~/.config` — all three belong to the account MixEngine runs as, so this capability does
//! what `.claude/architecture/overview.md` says every other change outside the root needs
//! `mixengine-elevate` for, and needs it for none of it. Which is also why it stays out of the
//! privileged-operation list, exactly as [`PathIntegration`](crate::PathIntegration) does.
//!
//! **One entry per user, and it names one home.** The mechanisms are per-user and MixEngine is
//! per-home; a label keyed by the home would produce entries nobody could find by looking, and ones
//! nothing could enumerate at uninstall. So enabling from a second home replaces the entry, and
//! [`AutostartState::command`] is what lets an answer say *"registered — for another home"* rather
//! than *"registered"*.
//!
//! **`enable` registers and does not start; `disable` removes and does not stop.** Somebody who
//! asked for "start at login" did not ask for "start it", and somebody who turned that off must not
//! lose the daemon they are using.

use std::path::PathBuf;

use crate::Result;

/// How this machine starts something at login, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartMechanism {
    /// Windows: a Task Scheduler logon task.
    LogonTask,

    /// macOS: a LaunchAgent in this user's `~/Library/LaunchAgents`.
    LaunchAgent,

    /// Linux: a systemd **user** unit.
    SystemdUser,

    /// This machine offers no way to start something at login.
    ///
    /// **A valid answer, not an error** — [`ResolverMethod::None`](crate::ResolverMethod)'s
    /// sentence, and for its reason. A Linux machine with no systemd user manager is the case it
    /// exists for; Windows and macOS never answer it, because Task Scheduler and launchd are part
    /// of the operating system.
    None,
}

/// What an entry would be asked to start.
///
/// Fields and not a rendered command line: the program and the home go into the entry as separate
/// elements — `<Command>` and `<Arguments>`, a `ProgramArguments` array, an `ExecStart=` line — so
/// nothing here is ever quoted into a string something else would have to take apart again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartPlan {
    /// The `mixengined` to start: the daemon's own image, so an entry names the binary that
    /// registered it rather than one found by searching.
    pub program: PathBuf,

    /// The home it owns, passed as `--home` rather than left to the child's environment — a daemon
    /// that re-resolved `MIXENGINE_HOME` at login could end up owning a different home.
    pub home: PathBuf,
}

/// What this machine says about the entry, after being asked or after being changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartState {
    /// Which mechanism this machine has.
    pub mechanism: AutostartMechanism,

    /// Where a person would go and look: the task name, the plist path, the unit path. On a machine
    /// with no mechanism, what was looked for and did not answer.
    pub location: String,

    /// Whether the entry is registered as things stand.
    pub enabled: bool,

    /// Whether *this call* wrote.
    ///
    /// Always `false` from [`state`](ServiceInstaller::state). An implementation skips the write
    /// when the entry already says exactly what would be written; where a system cannot be asked
    /// that faithfully — a Windows console codepage that will not round-trip a non-ASCII path — it
    /// writes and says so, which is a needless rewrite and never a wrong answer.
    pub changed: bool,

    /// What the registered entry will run, read back off the machine. Empty when nothing is.
    ///
    /// Read back rather than composed from what would have been written, which is the whole point of
    /// carrying it: an entry naming a `mixengined` that has moved, or a home that is not this one,
    /// is the failure this field exists to be able to report.
    pub command: Vec<String>,
}

/// Where this OS keeps what it starts at login, and how to put one entry there reversibly.
///
/// Every implementation follows `.claude/architecture/platform-abstraction.md`'s rules the way
/// [`PathIntegration`](crate::PathIntegration) does: a mutation is reversible, a read-modify-write
/// of a file goes through a temporary in the same directory and a rename, and a machine that cannot
/// do this at all is *detected* rather than failed against.
pub trait ServiceInstaller: std::fmt::Debug + Send + Sync {
    /// Register the entry, replacing whatever was there.
    ///
    /// Idempotent: an entry that already says exactly this is left alone and reported as
    /// `changed: false`. `plan.program` need not exist yet — an entry naming a missing file is
    /// something the OS complains about at login, and refusing here would make the order in which a
    /// machine is set up matter.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) on a machine with no
    /// mechanism, with the manual command in the reason.
    /// [`Error::Io`](crate::Error::Io) naming the file that could not be written, and
    /// [`Error::Command`](crate::Error::Command) naming the tool that refused.
    fn enable(&self, plan: &AutostartPlan) -> Result<AutostartState>;

    /// Take it away again, leaving everything else exactly as it was.
    ///
    /// Idempotent in the same way, and the half that makes this capability worth having: a home that
    /// is deleted must not leave something starting a daemon for it at every login.
    ///
    /// # Errors
    ///
    /// As [`enable`](Self::enable), minus the unsupported case — a machine with no mechanism has
    /// nothing registered, which is what `disable` was asking for.
    fn disable(&self) -> Result<AutostartState>;

    /// What is registered now, changing nothing.
    ///
    /// **Never fails for want of a mechanism**: a machine that cannot do this at all answers
    /// [`AutostartMechanism::None`], because a status reports rather than refuses.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when the entry is there and cannot be read, and
    /// [`Error::Command`](crate::Error::Command) when the tool that holds it refuses to say.
    fn state(&self) -> Result<AutostartState>;
}
