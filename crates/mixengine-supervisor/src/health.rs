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

use crate::command::Surroundings;
use crate::http::Endpoint;
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
    /// `where` is the service's own directory and environment, and it is used by exactly one probe —
    /// [`HealthProbe::Command`], which runs a program the service shipped. See [`Surroundings`] for
    /// why a probe that ran anywhere else would be asking about a different server.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedCheck`] for a probe this build or this system cannot make — see
    /// [`probe`](Self::probe). A failing probe is not an error: it is the answer.
    pub async fn examine(&mut self, place: &Surroundings) -> Result<Option<Verdict>> {
        let healthy = self.probe(place).await?;

        Ok(self.observed(healthy))
    }

    /// Ask once. `true` if the service answered the way it should.
    ///
    /// **Bounded by the check's own timeout**, which is the difference between a probe and a hang: a
    /// database that has stopped answering usually accepts the connection and then says nothing, so
    /// a probe with no deadline would simply stop returning and the service would look healthy for
    /// as long as it stayed broken. Every variant here is bounded by it, including the command one —
    /// a `mariadb-admin ping` that hangs is the same fault as a socket that never replies, and is
    /// killed at the deadline rather than left behind.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedCheck`] for a probe **this build** cannot make — an `https://` URL, or a
    /// variant added to `HealthProbe` since this was last read — and [`Error::Url`] for one whose
    /// URL is not a URL. [`Error::Platform`] is the refusal of the machine rather than of the build:
    /// a `UnixSocket` probe on a system that has no such socket, or a command a spec names and this
    /// machine does not have. They are different variants because the second is
    /// `mixengine-platform`'s own sentence and is passed through rather than re-described; a caller
    /// that treats "cannot be probed here" as one thing has to match all three.
    pub async fn probe(&self, place: &Surroundings) -> Result<bool> {
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

            HealthProbe::Http { url, expect_status } => {
                let endpoint = Endpoint::parse(url, "an HTTP health probe")?;

                Ok(endpoint.answered(timeout).await == Some(*expect_status))
            }

            // The honest probe for a database: a TCP accept only proves the listener is up, which
            // stays true while the server refuses every query.
            //
            // **A program that ran and failed is the answer, not an error.** `mariadb-admin ping`
            // exits non-zero against a server that is refusing connections, which is exactly the
            // service this check is meant to catch; what would be an error is the *binary* not being
            // there, and that is the spec's fault and travels as one.
            HealthProbe::Command { program, args } => {
                Ok(place.run(program, args, timeout).await?.succeeded())
            }

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
    use mixengine_testkit::FakeService;

    use super::*;
    use crate::http::fake;

    /// Where the command probes below are run. Nothing of the service's is needed by `fakeservice`,
    /// which is the point of it — what these tests are about is the running, not the environment.
    fn anywhere() -> Surroundings {
        Surroundings::new(std::env::temp_dir(), std::collections::BTreeMap::new())
    }

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
                .probe(&anywhere())
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
                .probe(&anywhere())
                .await
                .expect("a TCP probe can always be made")
        );
    }

    /// The program a spec named is not on this machine. **Not** an unhealthy service: nothing was
    /// measured, and degrading a service on the strength of it would report a fault in the spec as a
    /// fault in the database.
    #[tokio::test]
    async fn a_probe_command_that_cannot_be_started_is_reported_rather_than_counted_as_a_failure() {
        let health = Health::watching(&HealthCheck {
            probe: HealthProbe::Command {
                program: std::path::PathBuf::from("/opt/mixengine/bin/mariadb-admin"),
                args: vec!["ping".to_owned()],
            },
            ..check()
        });

        let error = health
            .probe(&anywhere())
            .await
            .expect_err("no such program is installed");

        assert!(matches!(&error, Error::Platform(_)), "{error:?}");
    }

    /// Zero is healthy, anything else is not — the whole of what a command probe reads.
    #[tokio::test]
    async fn a_probe_command_is_judged_by_its_exit_status() {
        for (code, healthy) in [(0, true), (3, false)] {
            let fixture = FakeService::new().exit_after(0).exit_code(code);
            let health = Health::watching(&HealthCheck {
                probe: HealthProbe::Command {
                    program: FakeService::program(),
                    args: fixture
                        .args()
                        .iter()
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect(),
                },
                ..check()
            });

            assert_eq!(
                health
                    .probe(&anywhere())
                    .await
                    .expect("the fixture is installed"),
                healthy,
                "exit code {code}"
            );
        }
    }

    /// A probe that hangs is a failed probe, not a supervisor that stops asking.
    ///
    /// The case a deadline exists for, and the reason it is the platform's kill rather than a bare
    /// `timeout`: what must not survive this is the *process*. `fakeservice` with nothing told to it
    /// runs for as long as it is left alone.
    #[tokio::test]
    async fn a_probe_command_that_never_finishes_fails_at_the_deadline() {
        let health = Health::watching(&HealthCheck {
            probe: HealthProbe::Command {
                program: FakeService::program(),
                args: Vec::new(),
            },
            timeout: Millis(200),
            ..check()
        });

        assert!(
            !health
                .probe(&anywhere())
                .await
                .expect("the fixture is installed"),
            "a probe that ran out of time is a failed probe"
        );
    }

    #[tokio::test]
    async fn an_http_probe_passes_only_on_the_status_the_spec_expects() {
        let server = fake::Server::answering(&[503]).await;

        for (expect_status, healthy) in [(503, true), (200, false)] {
            let health = Health::watching(&HealthCheck {
                probe: HealthProbe::Http {
                    url: server.url("/health"),
                    expect_status,
                },
                ..check()
            });

            assert_eq!(
                health
                    .probe(&anywhere())
                    .await
                    .expect("the server is there"),
                healthy,
                "expecting {expect_status}"
            );
        }
    }

    /// The spec's fault, reported as one — a URL that cannot be requested is not a sick service.
    #[tokio::test]
    async fn an_http_probe_with_a_url_that_is_not_one_says_so() {
        let health = Health::watching(&HealthCheck {
            probe: HealthProbe::Http {
                url: "127.0.0.1:2019/health".to_owned(),
                expect_status: 200,
            },
            ..check()
        });

        let error = health
            .probe(&anywhere())
            .await
            .expect_err("that is not a URL");

        assert!(matches!(&error, Error::Url { .. }), "{error:?}");
    }
}
