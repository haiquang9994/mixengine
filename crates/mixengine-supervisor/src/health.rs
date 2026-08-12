//! *Is it still fine?* — asked over and over, once a service is ready.
//!
//! The counterpart to [`crate::ready`], and deliberately a different question with a different
//! answer. A ready check asks once and then stops; this one asks forever, and what it produces is
//! not "up" or "down" but a move between `Running` and `Degraded` — a process that is alive and
//! failing is the case the GUI shows in amber and `mix doctor` explains, and it is the case a
//! restart policy must *not* treat as a crash.
//!
//! # Why a probe is not a verdict
//!
//! One failed probe is not a sick service. A database checkpointing under load misses a ping; a
//! machine that has just woken from sleep misses several. A dashboard that flickers amber teaches
//! people to ignore it, which costs more than the flicker. So a verdict needs a *run* of agreeing
//! probes — `failures_before_degraded` of them to fall, `successes_before_running` to recover — and
//! this module keeps that count.
//!
//! # What it does not do
//!
//! It does not sleep, own a task, or decide anything about the process. The caller drives it —
//! `interval`, probe, fold the result in, act on the verdict — because the thing that owns the
//! timing also owns the service's state row, the events it publishes and the cancellation token
//! that stops it. Keeping the mechanism free of the loop is what makes every rule below testable
//! without a clock.

use std::time::Duration;

use mixengine_platform::ipc;
use mixengine_proto::{HealthCheck, HealthProbe};
use tokio::net::TcpStream;

use crate::{Error, Result};

/// A change of health worth acting on.
///
/// Absent — `None` from [`Health::observed`] — is the ordinary answer and means *nothing has
/// changed*, which is what most probes of most services produce forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Enough consecutive failures: `Running → Degraded`.
    Degraded,

    /// Enough consecutive successes after that: `Degraded → Running`.
    Recovered,
}

/// The health of one service, as a run of probes has judged it so far.
#[derive(Debug, Clone)]
pub struct Health {
    check: HealthCheck,

    /// Whether the last verdict was [`Verdict::Degraded`].
    degraded: bool,

    /// How many probes in a row have now argued for changing that.
    ///
    /// Reset by any probe that agrees with the current state, which is what makes the threshold
    /// mean *consecutive*: five failures spread over an afternoon are five services that recovered,
    /// not one that is sick.
    run: u32,
}

impl Health {
    /// Start watching a service that is [`Running`](mixengine_proto::ServiceState::Running).
    ///
    /// Healthy to begin with, because readiness has just been proved — a service that has to earn
    /// its first success would be `Degraded` for one interval on every start, and the GUI would
    /// flicker amber exactly when the user is watching.
    #[must_use]
    pub fn watching(check: &HealthCheck) -> Self {
        Self {
            check: check.clone(),
            degraded: false,
            run: 0,
        }
    }

    /// How long to wait before the next probe.
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.check.interval.as_duration()
    }

    /// Whether the service is currently judged degraded.
    #[must_use]
    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// Ask once, and fold the answer in. `Some` when the service crossed a threshold.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedCheck`] for a probe this build or this system cannot make — see
    /// [`probe`](Self::probe). A failing probe is not an error: it is the answer.
    pub async fn examine(&mut self) -> Result<Option<Verdict>> {
        let healthy = self.probe().await?;

        Ok(self.observed(healthy))
    }

    /// Ask once. `true` if the service answered the way it should.
    ///
    /// **Bounded by the check's own timeout**, which is the difference between a probe and a hang: a
    /// database that has stopped answering usually accepts the connection and then says nothing, so
    /// a probe with no deadline would simply stop returning and the service would look healthy for
    /// as long as it stayed broken.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedCheck`] for an `Http` or `Command` probe, which **this build** cannot
    /// make yet (roadmap task T15a), and [`Error::Platform`] for a `UnixSocket` probe on a system
    /// that has no such socket — the two are different variants because the refusal in the second
    /// case is `mixengine-platform`'s own and is passed through rather than re-described. A caller
    /// that treats "cannot be probed here" as one thing has to match both.
    pub async fn probe(&self) -> Result<bool> {
        let timeout = self.check.timeout.as_duration();

        match &self.check.probe {
            HealthProbe::Tcp { addr } => {
                Ok(tokio::time::timeout(timeout, TcpStream::connect(*addr))
                    .await
                    .is_ok_and(|connected| connected.is_ok()))
            }

            HealthProbe::UnixSocket { path } => {
                if !ipc::SERVICE_SOCKETS {
                    // The spec was written for another OS. Reported rather than answered `false`,
                    // which would degrade a service that is perfectly well for a reason nothing in
                    // the GUI could explain.
                    return ipc::reach_socket(path)
                        .await
                        .map(|()| true)
                        .map_err(Error::from);
                }

                Ok(tokio::time::timeout(timeout, ipc::reach_socket(path))
                    .await
                    .is_ok_and(|reached| reached.is_ok()))
            }

            HealthProbe::Http { url, .. } => Err(Error::UnsupportedCheck {
                check: "an HTTP health probe",
                reason: format!(
                    "nothing in this build can request {url} — an HTTP client arrives with the \
                     first service that needs one (roadmap task T15a)"
                ),
            }),

            // The honest probe for a database — a TCP accept only proves the listener is up, which
            // stays true while the server refuses every query — and the reason it is not here yet is
            // not laziness: running a command means a one-shot spawn in `mixengine-platform` that
            // suppresses a console window on Windows, and inventing one in this crate would be the
            // `#[cfg(windows)]` this crate is not allowed to contain.
            HealthProbe::Command { program, .. } => Err(Error::UnsupportedCheck {
                check: "a command health probe",
                reason: format!(
                    "nothing in this build can run {} — a one-shot spawn arrives with the first \
                     service that needs one (roadmap task T15a)",
                    program.display()
                ),
            }),

            other => Err(Error::UnsupportedCheck {
                check: "this health probe",
                reason: format!("the supervisor does not know how to make it: {other:?}"),
            }),
        }
    }

    /// Fold one probe's answer into the run, and say whether it changed anything.
    ///
    /// Separate from [`probe`](Self::probe) so the counting can be tested without a socket, and
    /// because the two really are different things: what a probe measures belongs to the machine,
    /// what a run of them *means* belongs to the policy in the spec.
    pub fn observed(&mut self, healthy: bool) -> Option<Verdict> {
        let argues_for_change = self.degraded == healthy;

        if !argues_for_change {
            self.run = 0;

            return None;
        }

        self.run += 1;

        let needed = if self.degraded {
            self.check.successes_before_running
        } else {
            self.check.failures_before_degraded
        };

        if self.run < needed {
            return None;
        }

        self.run = 0;
        self.degraded = !self.degraded;

        Some(if self.degraded {
            Verdict::Degraded
        } else {
            Verdict::Recovered
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use mixengine_proto::Millis;

    use super::*;

    fn check() -> HealthCheck {
        HealthCheck {
            probe: HealthProbe::Tcp {
                addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 3306)),
            },
            interval: Millis::from_secs(10),
            timeout: Millis::from_secs(2),
            failures_before_degraded: 3,
            successes_before_running: 2,
        }
    }

    #[test]
    fn a_service_that_answers_stays_running() {
        let mut health = Health::watching(&check());

        for _ in 0..10 {
            assert_eq!(health.observed(true), None);
        }

        assert!(!health.is_degraded());
    }

    #[test]
    fn it_takes_a_run_of_failures_to_degrade_a_service() {
        let mut health = Health::watching(&check());

        assert_eq!(health.observed(false), None);
        assert_eq!(health.observed(false), None);
        assert_eq!(health.observed(false), Some(Verdict::Degraded));
        assert!(health.is_degraded());
    }

    /// The rule the threshold exists for: failures have to be *consecutive*.
    #[test]
    fn a_single_success_clears_the_count_towards_degrading() {
        let mut health = Health::watching(&check());

        health.observed(false);
        health.observed(false);
        assert_eq!(
            health.observed(true),
            None,
            "a probe that answered is not a verdict"
        );
        assert_eq!(health.observed(false), None, "the count restarted");
        assert_eq!(health.observed(false), None);
        assert_eq!(health.observed(false), Some(Verdict::Degraded));
    }

    #[test]
    fn a_degraded_service_recovers_after_its_own_threshold() {
        let mut health = Health::watching(&check());

        for _ in 0..3 {
            health.observed(false);
        }
        assert!(health.is_degraded());

        assert_eq!(health.observed(true), None);
        assert_eq!(health.observed(true), Some(Verdict::Recovered));
        assert!(!health.is_degraded());
    }

    /// Recovery is judged by its own threshold, not by the one that degraded it.
    #[test]
    fn recovering_takes_fewer_probes_than_degrading_when_the_spec_says_so() {
        let mut health = Health::watching(&check());

        for _ in 0..3 {
            health.observed(false);
        }

        health.observed(true);
        assert_eq!(
            health.observed(true),
            Some(Verdict::Recovered),
            "two successes were asked for and two were given"
        );
    }

    /// A verdict is a *change*: the same answer repeated says nothing more.
    #[test]
    fn a_service_that_stays_degraded_produces_one_verdict_and_no_more() {
        let mut health = Health::watching(&check());

        for _ in 0..3 {
            health.observed(false);
        }

        for _ in 0..10 {
            assert_eq!(health.observed(false), None);
        }
    }

    /// A service that has just proved it is ready is not made to earn its first success.
    #[test]
    fn a_new_watch_starts_healthy() {
        assert!(!Health::watching(&check()).is_degraded());
    }

    #[tokio::test]
    async fn a_port_nothing_is_listening_on_is_a_failed_probe_and_not_an_error() {
        // Bound to be given a port, then dropped, so this is an address the OS has just confirmed
        // nothing is on.
        let scout = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .expect("a loopback port");
        let addr = scout.local_addr().expect("a bound listener has an address");
        drop(scout);

        let health = Health::watching(&HealthCheck {
            probe: HealthProbe::Tcp { addr },
            ..check()
        });

        assert!(
            !health
                .probe()
                .await
                .expect("a TCP probe can always be made"),
            "a refused connection is the answer, not a failure of the supervisor"
        );
    }

    #[tokio::test]
    async fn a_port_something_is_listening_on_is_a_passed_probe() {
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .expect("a loopback port");
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");

        let health = Health::watching(&HealthCheck {
            probe: HealthProbe::Tcp { addr },
            ..check()
        });

        assert!(
            health
                .probe()
                .await
                .expect("a TCP probe can always be made")
        );
    }

    #[tokio::test]
    async fn a_probe_this_build_cannot_make_says_so_rather_than_reporting_a_sick_service() {
        let health = Health::watching(&HealthCheck {
            probe: HealthProbe::Command {
                program: std::path::PathBuf::from("/opt/mixengine/bin/mariadb-admin"),
                args: vec!["ping".to_owned()],
            },
            ..check()
        });

        let error = health
            .probe()
            .await
            .expect_err("this build cannot run a command");

        assert!(
            matches!(&error, Error::UnsupportedCheck { check, .. } if check.contains("command")),
            "{error:?}"
        );
    }
}
