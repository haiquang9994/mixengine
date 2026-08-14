//! Being asked to stop, in whichever way this operating system asks.
//!
//! A daemon is stopped by a person pressing Ctrl-C, by a service manager (`systemctl --user stop`,
//! `launchctl bootout`, Task Scheduler's *End task*), and by the machine shutting down or the user
//! signing out. None of those are the same mechanism, and only the first one is the same on all
//! three systems — which is exactly the kind of difference that belongs here rather than behind a
//! `#[cfg]` in the daemon.
//!
//! **Registration is separate from waiting**, and the split is the point of the type. Handlers are
//! installed by [`Signals::listen`] once, at startup, where a failure can still stop the start:
//! a daemon that cannot be asked to stop is one somebody has to kill, and finding that out at
//! shutdown is finding out too late. [`Signals::stopped`] then only waits, which is what lets it sit
//! in a `select!` arm that is rebuilt on every iteration of the accept loop — registering there
//! instead would drop and reinstall the handlers continuously and could lose a signal that arrived
//! between two turns.

use std::fmt;
use std::time::Duration;

use crate::Result;
use crate::sys::signal as sys;

/// How long this system lets a process that has been asked to stop go on running.
///
/// [`None`] on Unix, where nothing is counting: `SIGTERM` is a request with no deadline attached,
/// and what bounds a shutdown there is the service manager's own patience — systemd's
/// `TimeoutStopSec` is ninety seconds by default, and `launchd`'s is twenty.
///
/// [`Some`] on Windows, where the three console control events that are not Ctrl-C run the handler
/// on a clock and terminate the process when it runs out. A shutdown that means to finish has to fit
/// inside this: what does not is not slower, it is a database killed mid-flush and a WAL left
/// uncheckpointed.
///
/// **A ceiling, not a budget.** It says what this OS will allow when it is the one asking; the
/// daemon's own `daemon.shutdown` arrives over a socket with no console event behind it and no clock
/// running, and is bounded by `config.toml` instead. Reading it is what keeps the difference out of
/// a `#[cfg]` in the daemon.
pub const STOP_CEILING: Option<Duration> = sys::STOP_CEILING;

/// Why the daemon is being asked to stop.
///
/// Two cases rather than one per mechanism, because that is as much as the daemon does anything
/// with: it is in the log line that explains a shutdown hours later, and nothing branches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Somebody at a terminal pressed Ctrl-C (`SIGINT`, `CTRL_C_EVENT`, `CTRL_BREAK_EVENT`).
    Interrupt,

    /// Something asked the process to end: a service manager (`SIGTERM`), the console closing, the
    /// user signing out, or the machine shutting down.
    Terminate,
}

/// The installed handlers, waiting.
#[derive(Debug)]
pub struct Signals(sys::Signals);

impl Signals {
    /// Install the handlers for every way this OS asks a process to stop.
    ///
    /// Must be called from inside a Tokio runtime, and once: the handlers are process-global, so a
    /// second set would be a second answer to the same question.
    ///
    /// # Errors
    ///
    /// [`Error::Os`](crate::Error::Os) when the OS refuses to install one of them. There is no
    /// partial success — a daemon that can be interrupted but not terminated would look healthy and
    /// then ignore its service manager.
    pub fn listen() -> Result<Self> {
        sys::Signals::listen().map(Self)
    }

    /// Wait until one of them arrives.
    ///
    /// Cancel safe, which is what the accept loop needs of it: every implementation is a `select!`
    /// over receivers that tokio documents as cancel safe, so a turn of the loop that ends up
    /// serving a client instead has not consumed a signal that arrived at the same moment.
    ///
    /// On Windows the answer can be the last thing this process does anything with: the console
    /// handlers behind `CTRL_CLOSE_EVENT`, `CTRL_LOGOFF_EVENT` and `CTRL_SHUTDOWN_EVENT` are given
    /// a few seconds by the OS and the process is then terminated whatever it was doing, so
    /// whatever the daemon does after this returns has to be quick rather than thorough.
    pub async fn stopped(&mut self) -> Stop {
        self.0.stopped().await
    }
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Interrupt => "an interrupt",
            Self::Terminate => "a request to stop",
        })
    }
}
