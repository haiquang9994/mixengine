//! Reading an [`IdleProbe`] — roadmap task **T69**.
//!
//! The third of the three questions a `ServiceSpec` can ask about a running service, and it lives
//! beside the other two for that reason. [`ready`](crate::ready) asks *can I route traffic to it
//! yet*, [`health`](crate::health) asks *is it still fine*, and this asks *is anybody using it*.
//!
//! **What is here is the reading and never the verdict.** Whether a service that has looked idle
//! for a while should be stopped depends on its policy, on what depends on it and on whether
//! somebody asked for it to stay warm — none of which this crate knows about. It supervises
//! processes; it does not decide which ones a home wants. So one call answers *one observation*,
//! and the daemon's sweeper is what counts them.
//!
//! # The third answer
//!
//! An observation is [`Busy`](Observation::Busy), [`Idle`](Observation::Idle) or
//! [`Unmeasurable`](Observation::Unmeasurable), and the third is the one this module exists to keep
//! separate. `lsof` missing, `/proc/net/tcp` unreadable, a status endpoint refusing the connection:
//! folding any of those into "nothing is connected" would stop a database somebody is using because
//! a tool was not installed. The caller's rule is written on [`Observation::Unmeasurable`] and the
//! sweeper keeps it.

use std::collections::BTreeMap;
use std::time::Duration;

use mixengine_platform::ConnectionCount;
use mixengine_proto::{IdleProbe, ServiceId};

use crate::http::Endpoint;

/// How long a status endpoint is given to answer before the reading is given up on.
///
/// **Short, because a slow answer is not worth waiting for here.** A ready check retries for a
/// budget the spec named and a health check has a timeout in its own probe; this is a sample among
/// many, taken every sweep for ever, and one that times out costs nothing — the next is thirty
/// seconds away. What a long patience would buy is a sweep that blocks on a wedged service.
const PATIENCE: Duration = Duration::from_secs(2);

/// What one reading of a probe saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// Somebody is using it.
    Busy,

    /// Nobody is using it, as far as this probe can tell.
    Idle,

    /// The question could not be asked.
    ///
    /// **Not a synonym for [`Idle`](Self::Idle), and no caller may treat it as one.** This is
    /// `PortOwner`'s documented rule with the stakes raised: there, a failed reading costs a
    /// diagnosis; here, reading *I could not measure* as *there is nothing to measure* stops a
    /// running service because a tool was missing. A sweeper resets its count on this rather than
    /// advancing it, so an unmeasurable service runs for ever instead of being stopped wrongly.
    Unmeasurable {
        /// In words, for the log line that is the only place this surfaces.
        because: String,
    },
}

/// The last counter value read from each service's status endpoint.
///
/// **State, because [`IdleProbe::HttpCounter`] is a difference rather than a level.** php-fpm's
/// `accepted conn` only rises; what says the pool did nothing is that it rose by zero since the
/// previous sweep. Held by the caller and handed back in, so this module keeps nothing between
/// calls and a daemon restart forgets — which is correct, since a service just adopted has been
/// observed zero times.
pub type Counters = BTreeMap<ServiceId, Reading>;

/// What a counting probe read the last time it was asked.
///
/// **A number was enough until T72a and is not now.** php-fpm's `accepted conn` resets when the pool
/// restarts, so a counter that fell is not a very quiet minute — it is a different pool, and telling
/// the two apart takes the `start time` the number was read against.
///
/// A remembered reading of the *other* variant is treated as no baseline at all: on the sweep after
/// a service's spec changed shape there is nothing honest to compare against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// [`IdleProbe::HttpCounter`]'s: one number that only rises.
    Counter(u64),

    /// [`IdleProbe::FastCgiStatus`]'s: php-fpm's `accepted conn`, and the `start time` it was read
    /// against.
    Pool {
        /// `accepted conn` at that sweep.
        accepted: u64,

        /// The pool's `start time`, which changes when it restarts and its counter resets.
        started: u64,
    },
}

/// Take one reading of `probe`, and remember what a counting probe read.
///
/// `counters` is updated in place for the probes that need it, and left alone for the ones that do
/// not.
///
/// **The first reading of a counter is [`Busy`](Observation::Busy)**, never idle: with nothing to
/// compare against, "the number did not move" is a statement nothing supports. One sweep of
/// patience, once, at daemon start.
pub async fn observe(
    connections: &dyn ConnectionCount,
    service: &ServiceId,
    probe: &IdleProbe,
    counters: &mut Counters,
) -> Observation {
    match probe {
        IdleProbe::Connections { port } => match connections.established_on(*port) {
            Ok(0) => Observation::Idle,
            Ok(_) => Observation::Busy,
            Err(error) => Observation::Unmeasurable {
                because: format!("the connections to port {port} could not be counted: {error}"),
            },
        },

        IdleProbe::HttpCounter { url, field } => {
            let endpoint = match Endpoint::parse(url, "an idle probe") {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    return Observation::Unmeasurable {
                        because: error.to_string(),
                    };
                }
            };

            let Some(body) = endpoint.body(PATIENCE).await else {
                return Observation::Unmeasurable {
                    because: format!("{url} did not answer"),
                };
            };

            let Some(now) = counter_in(&body, field) else {
                return Observation::Unmeasurable {
                    because: format!("{url} answered without a numeric `{field}`"),
                };
            };

            let previous = counters.insert(service.clone(), Reading::Counter(now));

            match previous {
                Some(Reading::Counter(before)) if before == now => Observation::Idle,
                // Both the first reading and a counter that moved. They are one arm on purpose: a
                // service with no baseline is one this build cannot call idle, and saying so as
                // "busy" is the answer that keeps it running.
                _ => Observation::Busy,
            }
        }

        IdleProbe::FastCgiStatus { socket, path } => {
            let body =
                match crate::fastcgi::status(&crate::fastcgi::at(socket), path, PATIENCE).await {
                    Ok(body) => body,
                    Err(error) => {
                        return Observation::Unmeasurable {
                            because: format!(
                                "{} did not answer a status request: {error}",
                                socket.display()
                            ),
                        };
                    }
                };

            let (Some(accepted), Some(active), Some(started)) = (
                counter_in(&body, "accepted conn"),
                counter_in(&body, "active processes"),
                counter_in(&body, "start time"),
            ) else {
                return Observation::Unmeasurable {
                    because: format!(
                        "{} answered a status page without the three numbers this reads",
                        socket.display()
                    ),
                };
            };

            let previous = counters.insert(service.clone(), Reading::Pool { accepted, started });

            judge(previous.as_ref(), accepted, active, started)
        }

        // **The one wildcard arm in this task, and it is forced.** `IdleProbe` is `#[non_exhaustive]`
        // and lives in another crate, so this cannot be an exhaustive match however much T47b's rule
        // would prefer one. What it must not become is a silent fall-through: a probe this build
        // cannot read is a spec written for a newer MixEngine, and the honest answer is that nothing
        // was measured — which keeps the service running rather than stopping it on no evidence.
        other => Observation::Unmeasurable {
            because: format!("this build cannot read the idle probe {other:?}"),
        },
    }
}

/// What two readings of a pool's status page say about whether anybody is using it — **T72a**.
///
/// **Both halves, and each covers exactly what the other cannot.**
///
/// `accepted conn` is what sees traffic *between* two sweeps. A site under a steady stream of 50 ms
/// requests is almost certainly between two of them at the moment it is asked, and a rule reading
/// only the instant would call it unused.
///
/// `active processes` is what sees a request *spanning* sweeps. One that runs for several minutes
/// increments the counter once, in the first of them; every sweep after that sees the counter
/// advance by the probe alone, which to the counter alone is indistinguishable from a quiet pool.
/// Both halves were measured against php-fpm 8.3.6 before this rule was written.
///
/// **The `1` is the probe's own request.** Reading a status page is a request like any other, so
/// every reading costs the pool exactly one `accepted conn` and occupies one worker — itself. A
/// pool where nothing else happened advances by precisely that and no more.
///
/// **A `start time` that moved means the two readings are not comparable at all**: the pool
/// restarted and its counter began again, so a number that fell is a different pool rather than a
/// very quiet minute.
///
/// Everything that is not the one idle shape is [`Busy`](Observation::Busy) — the first reading
/// included, for the reason `observe`'s counter arm gives.
///
/// A pure function so that the rule can be tested on every system, including the one where a pool
/// never has this probe at all: what it judges is four numbers, not a connection.
fn judge(previous: Option<&Reading>, accepted: u64, active: u64, started: u64) -> Observation {
    match previous {
        Some(Reading::Pool {
            accepted: before,
            started: same,
        }) if *same == started && accepted.checked_sub(*before) == Some(1) && active <= 1 => {
            Observation::Idle
        }

        _ => Observation::Busy,
    }
}

/// The number `field` holds in a JSON document, if it holds one.
///
/// Top-level fields only. Every status endpoint this is pointed at — php-fpm's `?json`, Caddy's
/// admin — publishes its counters at the top, and a path syntax would be a small query language to
/// specify, document and get wrong in an `extension.toml`.
fn counter_in(body: &[u8], field: &str) -> Option<u64> {
    let document: serde_json::Value = serde_json::from_slice(body).ok()?;

    document.get(field)?.as_u64()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mixengine_platform::{Host as _, mock};

    use super::*;
    use crate::http::fake::Server;

    fn service() -> ServiceId {
        ServiceId::parse("fakeservice@main").expect("a valid id")
    }

    /// What the pool read last sweep, for the four rules below to be judged against.
    fn last(accepted: u64, started: u64) -> Reading {
        Reading::Pool { accepted, started }
    }

    /// **A counter that moved by the probe's own request and nothing else is a quiet pool.**
    #[test]
    fn a_counter_that_moved_by_the_probe_alone_is_idle() {
        assert_eq!(judge(Some(&last(10, 100)), 11, 1, 100), Observation::Idle);
        assert_eq!(
            judge(Some(&last(10, 100)), 11, 0, 100),
            Observation::Idle,
            "a pool with a status listener of its own would report no active worker; the rule must \
             not insist on exactly one"
        );
    }

    /// Somebody else's request moved it too.
    #[test]
    fn a_counter_that_moved_by_more_than_the_probe_is_busy() {
        assert_eq!(judge(Some(&last(10, 100)), 12, 1, 100), Observation::Busy);
    }

    /// **The long-request case, and the whole reason the counter alone is not enough.**
    ///
    /// A request spanning several sweeps increments `accepted conn` once, in the first of them.
    /// Every sweep after that sees the counter advance by the probe alone while a worker is still
    /// serving — so a rule reading only the counter would stop a pool in the middle of a page.
    #[test]
    fn a_worker_still_serving_is_busy_however_the_counter_moved() {
        assert_eq!(judge(Some(&last(10, 100)), 11, 2, 100), Observation::Busy);
    }

    /// A pool that restarted reset its counter, so the two readings are about two different pools.
    #[test]
    fn a_pool_that_restarted_is_not_compared_against_the_pool_it_was() {
        assert_eq!(
            judge(Some(&last(800, 100)), 1, 1, 900),
            Observation::Busy,
            "a counter from a different start says nothing about this one"
        );
    }

    /// The first sweep after a pool starts has nothing to compare against, and a counter this build
    /// remembered for a *different* probe is no baseline either.
    #[test]
    fn a_reading_with_no_comparable_baseline_is_busy() {
        assert_eq!(judge(None, 11, 1, 100), Observation::Busy);
        assert_eq!(
            judge(Some(&Reading::Counter(10)), 11, 1, 100),
            Observation::Busy,
            "a service whose spec changed shape has no honest baseline"
        );
    }

    /// **A pool nothing is listening on is unmeasurable, and never idle.**
    ///
    /// The dial fails before any framing happens — with no such file on Unix and with
    /// `UnsupportedPlatform` on Windows, which has no Unix sockets — and both are the same answer:
    /// nothing was measured, so nothing is stopped.
    #[tokio::test]
    async fn a_pool_that_cannot_be_dialled_is_unmeasurable() {
        let host = mock::Host::with_home("/home");
        let mut counters = Counters::new();

        let observation = observe(
            host.connections(),
            &service(),
            &IdleProbe::FastCgiStatus {
                socket: std::path::PathBuf::from("nothing-is-listening-on-this.sock"),
                path: "/mixengine-status".to_owned(),
            },
            &mut counters,
        )
        .await;

        assert!(
            matches!(observation, Observation::Unmeasurable { .. }),
            "a pool that could not be asked was reported as {observation:?}"
        );
        assert!(
            counters.is_empty(),
            "a reading that never happened must not be remembered as one"
        );
    }

    /// A port with connections is busy and one without them is idle.
    #[tokio::test]
    async fn connections_decide_a_port_probe() {
        let busy = mock::Host::with_connections("/home", BTreeMap::from([(3306, 2)]));
        let mut counters = Counters::new();

        assert_eq!(
            observe(
                busy.connections(),
                &service(),
                &IdleProbe::Connections { port: 3306 },
                &mut counters
            )
            .await,
            Observation::Busy
        );

        assert_eq!(
            observe(
                busy.connections(),
                &service(),
                &IdleProbe::Connections { port: 6379 },
                &mut counters
            )
            .await,
            Observation::Idle,
            "a port this machine has nothing connected to is idle"
        );

        assert!(
            counters.is_empty(),
            "a probe that counts nothing remembers nothing"
        );
    }

    /// A machine that cannot count is neither busy nor idle.
    ///
    /// The whole reason there are three answers: this must not reach a sweeper as a zero.
    #[tokio::test]
    async fn a_machine_that_cannot_count_is_unmeasurable() {
        let broken = mock::Host::unable_to_count_connections("/home", "no lsof on this machine");

        assert!(matches!(
            observe(
                broken.connections(),
                &service(),
                &IdleProbe::Connections { port: 3306 },
                &mut Counters::new()
            )
            .await,
            Observation::Unmeasurable { .. }
        ));
    }

    /// A counter that did not move is idle; one that moved is busy; the first reading is busy.
    #[tokio::test]
    async fn a_counter_is_idle_only_when_it_stops_moving() {
        let server = Server::counting("accepted", &[7, 9, 9]).await;
        let host = mock::Host::with_home("/home");
        let mut counters = Counters::new();

        let probe = IdleProbe::HttpCounter {
            url: server.url("/status"),
            field: "accepted".to_owned(),
        };

        assert_eq!(
            observe(host.connections(), &service(), &probe, &mut counters).await,
            Observation::Busy,
            "the first reading has nothing to compare against and may not be called idle"
        );

        assert_eq!(
            observe(host.connections(), &service(), &probe, &mut counters).await,
            Observation::Busy,
            "7 to 9 is two requests served"
        );

        assert_eq!(
            observe(host.connections(), &service(), &probe, &mut counters).await,
            Observation::Idle,
            "9 to 9 is a pool that did nothing between two sweeps"
        );
    }

    /// A status endpoint that is not there, and one that answers without the field.
    #[tokio::test]
    async fn a_counter_that_cannot_be_read_is_unmeasurable() {
        let host = mock::Host::with_home("/home");

        let absent = IdleProbe::HttpCounter {
            // Port 0 connects to nothing on every system, which is what a stopped status endpoint
            // looks like from here.
            url: "http://127.0.0.1:0/status".to_owned(),
            field: "accepted".to_owned(),
        };

        assert!(matches!(
            observe(
                host.connections(),
                &service(),
                &absent,
                &mut Counters::new()
            )
            .await,
            Observation::Unmeasurable { .. }
        ));

        let server = Server::counting("accepted", &[7]).await;

        let misnamed = IdleProbe::HttpCounter {
            url: server.url("/status"),
            field: "requests".to_owned(),
        };

        assert!(
            matches!(
                observe(
                    host.connections(),
                    &service(),
                    &misnamed,
                    &mut Counters::new()
                )
                .await,
                Observation::Unmeasurable { .. }
            ),
            "a document with no such field is a probe pointed at the wrong thing, not an idle pool"
        );
    }

    /// An HTTPS probe is refused the way every other check in this crate refuses one.
    #[tokio::test]
    async fn an_https_probe_is_refused_rather_than_attempted() {
        let host = mock::Host::with_home("/home");

        let probe = IdleProbe::HttpCounter {
            url: "https://127.0.0.1:9000/status".to_owned(),
            field: "accepted".to_owned(),
        };

        assert!(matches!(
            observe(host.connections(), &service(), &probe, &mut Counters::new()).await,
            Observation::Unmeasurable { .. }
        ));
    }
}
