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
/// **Only [`IdleProbe::HttpCounter`] keeps anything here**, and T72a is the task that found out why
/// a second counting probe should not: php-fpm's `accepted conn` counts the daemon's own health
/// checks, so a difference taken across two sweeps is mostly the daemon's own footprints — the
/// `FastCgiStatus` arm of [`observe`] says what it reads instead.
pub type Counters = BTreeMap<ServiceId, u64>;

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

            let previous = counters.insert(service.clone(), now);

            match previous {
                Some(before) if before == now => Observation::Idle,
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

            let Some(active) = counter_in(&body, "active processes") else {
                return Observation::Unmeasurable {
                    because: format!(
                        "{} answered a status page with no `active processes` in it",
                        socket.display()
                    ),
                };
            };

            judge(active)
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

/// What a pool's status page says about whether anybody is using it — **T72a**.
///
/// **A pool is busy when a worker is serving something, and idle when none is.** `active processes`
/// counts exactly that, and the `<= 1` is the probe's own request: reading a status page over the
/// pool's socket occupies one worker for the length of the read, so a pool with nothing else
/// happening reports one.
///
/// # Why not `accepted conn`, which is what a counter probe would reach for
///
/// The design this task was written from judged two numbers: the counter, to catch traffic
/// *between* two sweeps, and `active processes`, to catch a request *spanning* them. **Measured
/// against a running daemon, the counter is unusable, and the reason is ours rather than
/// php-fpm's**: a pool's health check is a connect-and-close on the same socket every ten seconds,
/// and php-fpm counts a bare connection as an accepted one — measured, three health checks between
/// two thirty-second sweeps make every delta four. The daemon would be reading its own footprints
/// and calling the pool busy for ever, which is exactly what the first run of `cold_path.rs` did.
///
/// Subtracting the daemon's own connections was rejected: it would mean every future caller that
/// dials a pool has to be counted somewhere, and a probe that is wrong when somebody forgets is
/// worse than one that never depended on it.
///
/// **What that costs, stated rather than hidden.** Traffic between two readings is invisible, so a
/// site being used in short bursts can look idle at every sample and be stopped. What it costs is
/// one cold path — the next request wakes the pool through its activator, inside the budget
/// `cold_path.rs` gates. What it never costs is a request: a pool serving something is never
/// stopped, because that is precisely what this number sees.
///
/// A pure function, so the rule is tested on every system including the one where a pool never has
/// this probe: what it judges is a number, not a connection.
fn judge(active: u64) -> Observation {
    if active <= 1 {
        Observation::Idle
    } else {
        Observation::Busy
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

    /// **A pool answering nothing but the probe itself is a quiet pool.**
    ///
    /// One active worker is the probe's own: reading the status page over the pool's socket is a
    /// request like any other, and it occupies a worker for the length of the read.
    #[test]
    fn a_pool_serving_nothing_but_the_probe_is_idle() {
        assert_eq!(judge(1), Observation::Idle);
        assert_eq!(
            judge(0),
            Observation::Idle,
            "a pool that answered without charging the read to a worker is emptier still, not busier"
        );
    }

    /// **A worker serving something is a busy pool, and this is the whole rule.**
    ///
    /// It is what makes the probe safe: whatever else the sweeper cannot see, it never stops a pool
    /// that is in the middle of a page.
    #[test]
    fn a_worker_serving_something_is_busy() {
        assert_eq!(judge(2), Observation::Busy);
        assert_eq!(judge(5), Observation::Busy);
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
