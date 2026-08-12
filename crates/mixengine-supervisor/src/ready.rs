//! *Can traffic be routed to it yet?*
//!
//! A started process is not a started service. `mariadbd` accepts on its port seconds after it is
//! spawned and answers queries seconds after that; php-fpm creates its socket before it has forked a
//! single worker. Everything a site depends on hangs off the moment a `ReadyCheck` passes, which is
//! why the spec has five of them rather than one.
//!
//! # Three answers, not two
//!
//! Waiting for readiness races three outcomes, and the third is the one a naive implementation
//! misses: the process **exits** while we are waiting for it. The port was taken, the config did not
//! parse, the data directory belongs to another version. Treating that as "not ready yet" means
//! waiting out the whole timeout — thirty seconds of a service that has been dead for one — and then
//! reporting the wrong thing. `Starting → Restarting` is an edge in the state machine for exactly
//! this reason (see `.claude/architecture/process-supervision.md`), and [`Ready::Exited`] is what
//! feeds it.
//!
//! # Two probes are not here yet
//!
//! `ReadyCheck::Http` and `HealthProbe::Command` answer with [`Error::UnsupportedCheck`] rather than
//! with a `todo!()`, per the rule in `CLAUDE.md`. Neither is a gap in this module so much as a
//! dependency it should not invent on its own: HTTP needs a client in a crate that has none, and
//! running a command needs a one-shot spawn in `mixengine-platform` that suppresses a console window
//! on Windows — the daemon learned that lesson once already, in eight terminal windows appearing on
//! a user's desktop. Both arrive with the first spec that needs them, in Phase 3 (roadmap task
//! T15a).

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use mixengine_platform::ipc;
use mixengine_platform::process::{Exit, Supervised};
use mixengine_proto::ReadyCheck;
use regex::Regex;
use tokio::net::TcpStream;
use tokio::sync::broadcast::error::RecvError;

use crate::logs::Capture;
use crate::{Error, Result};

/// How often a probe that can only be retried is retried.
///
/// A compromise measured against what it costs: a service that becomes ready 5 ms after the previous
/// attempt waits this long to be noticed, and a service that takes a minute to start is asked twelve
/// hundred times. Small enough that no user perceives it, large enough that a slow start is not a
/// busy loop.
const RETRY: Duration = Duration::from_millis(50);

/// How often the process itself is checked while a probe is waiting.
///
/// Separate from [`RETRY`] because it is a different question — *is it still there* rather than *is
/// it ready* — and because `LogPattern` and `PidAlive` do not poll at all and still need this.
const HEARTBEAT: Duration = Duration::from_millis(50);

/// How a wait for readiness ended.
#[derive(Debug)]
pub enum Ready {
    /// The check passed. The service can be routed to.
    Ready,

    /// The process ended before it ever became ready, and this is how.
    ///
    /// Not a failure of this module: it is the most common way a service fails to start, and the
    /// caller turns it into `Starting → Restarting` or `Starting → Failed` according to the restart
    /// policy — with the reason taken from the last lines the service printed.
    Exited(Exit),

    /// The check's own timeout ran out while the process was still running.
    TimedOut,
}

/// Wait until `check` passes, the process exits, or the check's timeout runs out.
///
/// Takes `&mut Supervised` because watching for the exit means reaping it: a supervisor that only
/// polled the probe would leave a zombie behind on Unix and would learn about the exit from a
/// timeout instead of from the OS.
///
/// # Errors
///
/// Three, and none of them is a state of the service — a spec that cannot be checked was never going
/// to become ready, and reporting one as a timeout would send the reader looking at the service
/// instead of at the spec. Which variant carries which is worth knowing, because a caller that
/// matches on one of them will not see the others:
///
/// - [`Error::UnsupportedCheck`] for a probe **this build** cannot make: an `Http` check, or a
///   variant added to `ReadyCheck` since this module was last read.
/// - [`Error::Platform`] for one **this system** cannot make: a `UnixSocket` check on Windows, where
///   the refusal is `mixengine-platform`'s own and is passed through rather than re-described.
/// - [`Error::Pattern`] for a `LogPattern` whose regex does not compile.
pub async fn wait(check: &ReadyCheck, service: &mut Supervised, logs: &Capture) -> Result<Ready> {
    tokio::select! {
        // Biased, so that a process which exited and a probe which passed in the same moment are
        // reported as the exit. A service that printed its ready line and then died is not ready,
        // and the opposite order would route traffic to a process that is gone.
        biased;

        exit = ended(service) => exit.map(Ready::Exited),

        outcome = settled(check, logs) => outcome,
    }
}

/// Run the probe under whatever deadline the check implies, which for one of them is none.
async fn settled(check: &ReadyCheck, logs: &Capture) -> Result<Ready> {
    let Some(deadline) = gives_up_after(check) else {
        return probe(check, logs).await.map(|()| Ready::Ready);
    };

    match tokio::time::timeout(deadline, probe(check, logs)).await {
        Ok(passed) => passed.map(|()| Ready::Ready),
        Err(_elapsed) => Ok(Ready::TimedOut),
    }
}

/// How long this check may run before it is a [`Ready::TimedOut`], or `None` for one that cannot
/// run out of time.
///
/// **`PidAlive` is the `None`, and giving it a deadline was a race it lost.** Its `settle` is not a
/// deadline the check may miss — it is the check: the service is ready *because* it survived that
/// long, so there is no outcome for a timeout to report. Handing that same duration to a
/// `tokio::time::timeout` around a sleep of the same length put two timers on the same instant, and
/// which one fired first came down to whether they rounded into the same tick. They did not always:
/// the outer timer starts when `wait` builds the future and the inner one when the probe is first
/// polled, which is after the biased arm above has asked the OS whether the process is still there.
/// Measured at roughly one wait in six hundred on an idle machine and one in ten with a few hundred
/// microseconds of load in between — reported as a healthy service that never started, which the
/// caller then restarts.
///
/// The process exiting inside the settling window is still caught, by the arm racing this one. That
/// is the only way a `PidAlive` check fails, and it always was.
fn gives_up_after(check: &ReadyCheck) -> Option<Duration> {
    match check {
        ReadyCheck::PidAlive { .. } => None,
        other => Some(other.timeout().as_duration()),
    }
}

/// Resolve when the process ends, however it ends.
///
/// Polled rather than awaited, because the handle is the standard library's: a `Child` cannot be
/// awaited, and the alternative — a thread per service blocked in `wait` — buys nothing here, where
/// something is already watching the clock.
async fn ended(service: &mut Supervised) -> Result<Exit> {
    loop {
        if let Some(exit) = service.exited().map_err(Error::from)? {
            return Ok(exit);
        }

        tokio::time::sleep(HEARTBEAT).await;
    }
}

/// Resolve when the check passes. Never resolves on its own if it does not.
async fn probe(check: &ReadyCheck, logs: &Capture) -> Result<()> {
    match check {
        ReadyCheck::Tcp { addr, .. } => accepting(*addr).await,
        ReadyCheck::UnixSocket { path, .. } => listening(path).await,
        ReadyCheck::LogPattern { regex, .. } => announced(regex, logs).await,

        // The one check that measures nothing: it is the last resort the spec calls it, for a
        // program that opens no port and says nothing. Surviving the settling period is the whole
        // of it, and the process exiting inside that window is caught by the arm racing this one.
        // Deliberately **not** wrapped in a timeout by the caller — see `gives_up_after`.
        ReadyCheck::PidAlive { settle } => {
            tokio::time::sleep(settle.as_duration()).await;

            Ok(())
        }

        ReadyCheck::Http { url, .. } => Err(Error::UnsupportedCheck {
            check: "an HTTP ready check",
            reason: format!(
                "nothing in this build can request {url} — an HTTP client arrives with the first \
                 service that needs one (roadmap task T15a)"
            ),
        }),

        // `ReadyCheck` is `#[non_exhaustive]`, so a variant added in `mixengine-proto` reaches here
        // before it reaches the match above. Saying which one is unhandled beats a compile error
        // nobody sees until they rebuild this crate — and beats a `todo!()` in a daemon.
        other => Err(Error::UnsupportedCheck {
            check: "this ready check",
            reason: format!("the supervisor does not know how to make it: {other:?}"),
        }),
    }
}

/// Retry a TCP connection until one succeeds.
///
/// A refused connection is the ordinary answer for a service that is still binding, so it is
/// retried rather than reported. What ends this is the caller's timeout — including for an address
/// that is filtered rather than refused, where a single connect can hang for the whole of it.
async fn accepting(addr: SocketAddr) -> Result<()> {
    loop {
        if TcpStream::connect(addr).await.is_ok() {
            return Ok(());
        }

        tokio::time::sleep(RETRY).await;
    }
}

/// Retry a Unix socket connection until one succeeds.
///
/// **Connecting, not looking.** The socket file appears when the service binds and stays there after
/// it crashes, so its existence answers neither question a ready check is asking; only a connection
/// distinguishes bound from accepting, and a live socket from a stale one left by a kill.
async fn listening(path: &Path) -> Result<()> {
    if !ipc::SERVICE_SOCKETS {
        // Asked once and answered, rather than retried: on a system with no such socket, waiting
        // would spend the whole timeout on something that cannot happen.
        return ipc::reach_socket(path).await.map_err(Error::from);
    }

    loop {
        if ipc::reach_socket(path).await.is_ok() {
            return Ok(());
        }

        tokio::time::sleep(RETRY).await;
    }
}

/// Wait for a line matching `pattern` on either stream.
///
/// **Subscribes before it looks at what has already arrived**, which is the whole of the race: a
/// service fast enough to print its ready line before this function runs — a Caddy, on a warm cache
/// — would otherwise be waited for until its timeout, and one that printed it between the scan and
/// the subscription would be missed by both halves.
///
/// A subscriber that falls behind keeps waiting rather than failing: the line may have been in what
/// it missed, the ring is not a substitute for it, and the timeout ends the wait either way.
async fn announced(pattern: &str, logs: &Capture) -> Result<()> {
    let regex = Regex::new(pattern).map_err(|source| Error::Pattern {
        pattern: pattern.to_owned(),
        source: Box::new(source),
    })?;

    let mut lines = logs.subscribe();

    if logs
        .recent(usize::MAX)
        .iter()
        .any(|line| regex.is_match(&line.text))
    {
        return Ok(());
    }

    loop {
        match lines.recv().await {
            Ok(line) if regex.is_match(&line.text) => return Ok(()),
            Ok(_) | Err(RecvError::Lagged(_)) => {}

            // Both reader threads have ended, so the streams are closed and no further line can
            // arrive. Pending rather than an error: the process is the thing that has gone, and the
            // arm racing this one is what reports it — as the exit it is, with a status, rather than
            // as a log stream that ended.
            Err(RecvError::Closed) => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use mixengine_proto::Millis;

    use super::*;

    /// A `PidAlive` check must not be given a deadline, because its deadline would be its own
    /// settling period and the two would race.
    ///
    /// The structural half of a bug whose behavioural half cannot be tested without running the
    /// wait a few thousand times: whichever of two timers on the same instant fires first is a
    /// question of timer ticks, and the wrong answer reported a living service as one that never
    /// started. There is nothing to round here — either the check has a deadline or it does not.
    #[test]
    fn a_pid_alive_check_has_no_deadline_to_race_its_own_settle() {
        assert_eq!(
            gives_up_after(&ReadyCheck::PidAlive {
                settle: Millis(200)
            }),
            None,
            "surviving the settling period is the check, not a deadline it can miss"
        );
    }

    /// The other half: every check that *is* a probe still gives up on time.
    #[test]
    fn a_check_that_polls_for_something_keeps_its_timeout() {
        for check in [
            ReadyCheck::Tcp {
                addr: "127.0.0.1:3306".parse().expect("a literal address"),
                timeout: Millis::from_secs(20),
            },
            ReadyCheck::UnixSocket {
                path: std::path::PathBuf::from("/tmp/php-fpm.sock"),
                timeout: Millis::from_secs(20),
            },
            ReadyCheck::LogPattern {
                regex: "ready".to_owned(),
                timeout: Millis::from_secs(20),
            },
        ] {
            assert_eq!(
                gives_up_after(&check),
                Some(Duration::from_secs(20)),
                "a probe that waits for something else has to be able to give up: {check:?}"
            );
        }
    }

    /// The syntax a spec author may write, against the `regex` features the workspace selected.
    ///
    /// **This is a test about `Cargo.toml`, not about this module.** A Unicode feature the crate is
    /// built without is not a pattern that behaves differently — it is a pattern the engine
    /// *refuses*, so trimming one turns a spec somebody wrote into a service that waits out its
    /// whole timeout and then blames itself. `(?i)` is the case that matters: it is the first thing
    /// anybody reaches for against a log line, and it needs `unicode-case`.
    #[test]
    fn the_syntax_a_spec_may_use_compiles_with_the_features_this_workspace_selected() {
        for pattern in [
            "(?i)ready for connections",
            r"\d+ workers ready",
            r"\w+: ready",
            r"\s*listening on",
            r"\p{L}+ started",
            "^caddy: serving$",
        ] {
            assert!(
                Regex::new(pattern).is_ok(),
                "`{pattern}` is syntax a spec may reasonably contain, and this build cannot \
                 compile it: {:?}",
                Regex::new(pattern).err()
            );
        }
    }

    /// A pattern that does not compile is the spec's fault, and is reported before anything waits.
    #[tokio::test]
    async fn a_pattern_that_does_not_compile_is_refused_rather_than_waited_out() {
        let logs = Capture::detached();

        let error = announced("what(", &logs)
            .await
            .expect_err("an unclosed group is not a regex");

        assert!(
            matches!(&error, Error::Pattern { pattern, .. } if pattern == "what("),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn an_http_check_says_what_is_missing_rather_than_panicking() {
        let logs = Capture::detached();
        let check = ReadyCheck::Http {
            url: "http://127.0.0.1:2019/config/".to_owned(),
            expect_status: 200,
            timeout: Millis::from_secs(10),
        };

        let error = probe(&check, &logs)
            .await
            .expect_err("this build has no HTTP client");

        assert!(
            matches!(&error, Error::UnsupportedCheck { reason, .. } if reason.contains("2019")),
            "{error:?}"
        );
    }

    /// A line already in the ring counts: a service can be ready before anything asks.
    #[tokio::test]
    async fn a_pattern_already_printed_is_matched_without_waiting_for_another_line() {
        let logs = Capture::detached();
        logs.record("mariadbd: ready for connections.");

        tokio::time::timeout(
            Duration::from_secs(1),
            announced("ready for connections", &logs),
        )
        .await
        .expect("a line already in the ring needs no further one")
        .expect("the pattern compiles");
    }

    #[tokio::test]
    async fn a_pattern_printed_later_is_matched_too() {
        // Shared through an `Arc` rather than cloned: a capture owns reader threads, so it is one
        // value with one lifetime and not a handle to be copied around.
        let logs = std::sync::Arc::new(Capture::detached());
        let waiting = tokio::spawn({
            let logs = std::sync::Arc::clone(&logs);

            async move { announced("ready for connections", &logs).await }
        });

        // Given a moment to subscribe first, so this test exercises the subscription rather than
        // the scan of the ring the test above covers.
        tokio::time::sleep(Duration::from_millis(50)).await;
        logs.record("mariadbd: ready for connections.");

        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("the line arrives while the wait is running")
            .expect("the waiting task did not panic")
            .expect("the pattern compiles");
    }
}
