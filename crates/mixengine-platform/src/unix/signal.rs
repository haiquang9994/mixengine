//! `SIGINT` and `SIGTERM`, through tokio's signal handling.
//!
//! The same two on Linux and macOS, which is why this is in `unix/` — systemd sends `SIGTERM`,
//! `launchctl bootout` sends `SIGTERM`, and a terminal sends `SIGINT`.
//!
//! `SIGHUP` is deliberately absent. Its conventional meaning to a daemon is "reload your
//! configuration", which MixEngine does through the API rather than through a signal, and a detached
//! daemon has no controlling terminal to be hung up on in the first place. Leaving it unhandled
//! keeps the default action — which terminates the process — rather than quietly redefining it.

use std::time::Duration;

use tokio::signal::unix::{Signal, SignalKind, signal};

use crate::signal::Stop;
use crate::{Error, Result};

/// Nothing here counts. `SIGTERM` is a request, and a process that ignores it is left running until
/// whoever sent it decides otherwise — see [`crate::signal::STOP_CEILING`].
pub(crate) const STOP_CEILING: Option<Duration> = None;

/// One handler per signal, registered.
#[derive(Debug)]
pub(crate) struct Signals {
    interrupt: Signal,
    terminate: Signal,
}

impl Signals {
    pub(crate) fn listen() -> Result<Self> {
        Ok(Self {
            interrupt: install(SignalKind::interrupt(), "listen for SIGINT")?,
            terminate: install(SignalKind::terminate(), "listen for SIGTERM")?,
        })
    }

    pub(crate) async fn stopped(&mut self) -> Stop {
        // `Signal::recv` yields `None` only once the handler is deregistered, which cannot happen
        // while this struct owns it — the branch is unreachable rather than ignored, and answering
        // it with the same `Stop` the signal itself means keeps the loop from spinning if it ever
        // becomes reachable.
        tokio::select! {
            _ = self.interrupt.recv() => Stop::Interrupt,
            _ = self.terminate.recv() => Stop::Terminate,
        }
    }
}

/// Register one handler, naming the signal if the OS will not have it.
fn install(kind: SignalKind, action: &'static str) -> Result<Signal> {
    signal(kind).map_err(|source| Error::Os { action, source })
}
