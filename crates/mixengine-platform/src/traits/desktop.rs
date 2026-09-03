//! Finding and starting a desktop application somebody installed — roadmap task **T83**.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::Result;

/// An application this machine has, as it would be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    /// The executable — never a bundle directory, never a bare name.
    pub program: PathBuf,

    /// Arguments its launcher fixes before any of ours: what a desktop entry's `Exec=` carried
    /// besides the program and the field codes. Empty on Windows and macOS.
    pub args: Vec<OsString>,
}

/// What looking for an application found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// It is here.
    Installed(InstalledApp),

    /// It is not, and this is where the system looked — phrased for a person, in this system's own
    /// currency: "App Paths and the uninstall table", "Spotlight, by bundle identifier",
    /// "~/.local/share/applications and /usr/share/applications".
    NotInstalled {
        /// Where.
        searched: String,
    },
}

/// What became of a started application after one second — the T83 design's D8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// Still up.
    Running {
        /// Its process id.
        pid: u32,
    },

    /// Exited successfully inside the judgement: a single-instance application forwarding to the
    /// copy already running, which is the common case and not a failure.
    HandedOn,

    /// Exited otherwise inside the judgement. `status` is how, rendered for a person.
    Failed {
        /// How it exited.
        status: String,
    },
}

/// Locating an installed desktop application, and starting it.
///
/// **The one capability that starts a process the daemon does not supervise**, and the reason it is
/// a capability rather than a free function in [`crate::process`]: what a test of the handoff has
/// to see is *which program, which arguments, which variable names* — a recorder — while the OS
/// mechanism underneath (`spawn_detached`) is proved once in `tests/desktop.rs` against a shell.
///
/// `hint` is the manifest's per-OS name for the application: an executable's file name on
/// Windows, a bundle identifier on macOS, a desktop entry's file name on Linux.
///
/// # Blocking
///
/// Both methods block — a registry walk, a Spotlight query, a one-second judgement — and are
/// called through `spawn_blocking`.
pub trait DesktopApps: std::fmt::Debug + Send + Sync {
    /// Find the application `hint` names, or say where this system looked.
    ///
    /// # Errors
    ///
    /// [`Error::Command`](crate::Error::Command) where a tool this system needs to look could not
    /// run, [`Error::Os`](crate::Error::Os) where the registry would not answer. Neither is
    /// "not installed", which is a [`Located`] and not an error.
    fn locate(&self, hint: &str) -> Result<Located>;

    /// Start `app` with `args` after its own, `env` added to this process's environment, detached,
    /// and judged for one second.
    ///
    /// **`env` is where a credential goes** and it goes nowhere else: not into `args`, not into a
    /// log, and — for the mock — not into what is recorded.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) naming the program when it could not be started at all. An
    /// application that started and died is [`Started::Failed`], not an error.
    fn launch(
        &self,
        app: &InstalledApp,
        args: &[OsString],
        env: &BTreeMap<String, String>,
    ) -> Result<Started>;
}
