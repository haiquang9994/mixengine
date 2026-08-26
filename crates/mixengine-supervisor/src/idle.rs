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
