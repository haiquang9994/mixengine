//! The five console control events, through tokio's signal handling.
//!
//! Windows has no signals to speak of; what it has is `SetConsoleCtrlHandler`, and tokio wraps each
//! event it delivers as a receiver of its own. All five are taken because they are five genuinely
//! different ways of being asked to stop and none of them is covered by another: Ctrl-C and
//! Ctrl-Break come from somebody at a console, closing the window is `CTRL_CLOSE_EVENT`, signing out
//! is `CTRL_LOGOFF_EVENT` and shutting the machine down is `CTRL_SHUTDOWN_EVENT`.
//!
//! **The last three are on a clock.** Windows gives a handler a few seconds — five by default, and
//! rather less during shutdown — and then terminates the process regardless. That is
//! [`STOP_CEILING`], which the daemon reads to size a shutdown the OS asked for rather than
//! discovering it as a process that vanished mid-flush.
//!
//! A daemon started with `--detach` has no console at all, so none of these can reach it. That is
//! not a gap this module can close: without a console there is nothing to send it an event, and what
//! stops such a daemon is `mix daemon stop` or the task that started it. Task Scheduler's *End task*
//! terminates outright, which is why nothing here is allowed to be the only path that leaves the
//! home in a consistent state.

use std::time::Duration;

use tokio::signal::windows::{
    CtrlBreak, CtrlC, CtrlClose, CtrlLogoff, CtrlShutdown, ctrl_break, ctrl_c, ctrl_close,
    ctrl_logoff, ctrl_shutdown,
};

use crate::signal::Stop;
use crate::{Error, Result};

/// The five seconds `CTRL_CLOSE_EVENT` and its two siblings allow a handler before the process is
/// terminated regardless — see [`crate::signal::STOP_CEILING`].
///
/// The documented default, and deliberately not a value read from the registry: `WaitToKillTimeout`
/// and `HungAppTimeout` can each shorten it, and a daemon that trusted a longer number it found
/// there would spend a grace period it does not have. Five is what this build plans against, and the
/// margin the daemon subtracts from it is what covers a machine that allows less.
///
/// Ctrl-C and Ctrl-Break are *not* on this clock, and one constant covers both anyway: it is the
/// case the daemon cannot tell apart at the moment it has to decide, and being quick when there was
/// no hurry costs a service its polite stop only where its spec asked for more than this.
pub(crate) const STOP_CEILING: Option<Duration> = Some(Duration::from_secs(5));

/// One receiver per console control event, registered.
#[derive(Debug)]
pub(crate) struct Signals {
    interrupt: CtrlC,
    broken: CtrlBreak,
    closed: CtrlClose,
    logoff: CtrlLogoff,
    shutdown: CtrlShutdown,
}

impl Signals {
    pub(crate) fn listen() -> Result<Self> {
        Ok(Self {
            interrupt: register(ctrl_c(), "listen for Ctrl-C")?,
            broken: register(ctrl_break(), "listen for Ctrl-Break")?,
            closed: register(ctrl_close(), "listen for the console closing")?,
            logoff: register(ctrl_logoff(), "listen for the user signing out")?,
            shutdown: register(ctrl_shutdown(), "listen for the machine shutting down")?,
        })
    }

    pub(crate) async fn stopped(&mut self) -> Stop {
        tokio::select! {
            _ = self.interrupt.recv() => Stop::Interrupt,
            _ = self.broken.recv() => Stop::Interrupt,
            _ = self.closed.recv() => Stop::Terminate,
            _ = self.logoff.recv() => Stop::Terminate,
            _ = self.shutdown.recv() => Stop::Terminate,
        }
    }
}

/// Register one receiver, naming what it was for if Windows will not have it.
fn register<T>(registered: std::io::Result<T>, action: &'static str) -> Result<T> {
    registered.map_err(|source| Error::Os { action, source })
}
