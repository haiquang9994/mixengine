//! Holding a database's own address while nothing is serving it — roadmap task **T70a**.
//!
//! T70's activator holds an address of its *own*, permanently, and a site file names it as a
//! fallback for the pool it belongs to. There is no front end in front of a database to name one
//! in — a client dials `127.0.0.1:3306` and nothing else will do — so here the daemon binds what
//! the service itself listens on, and gives it back when the service starts.
//!
//! # The window, stated rather than hidden
//!
//! Between the release and the server's own bind, nothing holds the address, and a connection
//! arriving in that interval is refused by the kernel. The interval is the service's start time.
//!
//! **The first client is always served**, because its connection is already accepted when the
//! release happens — it waits on the splice, not on the address. Only a *second* client, arriving
//! while the first one's start is still running, meets the window.
//!
//! That cost is accepted because the alternatives are worse, which is the design's D4 and not this
//! module's argument: always proxying would put every query's bytes through the daemon for the
//! connection's whole life and would make a *running* database unreachable when the daemon dies,
//! and not activating databases at all leaves M7 unreachable. `resource-isolation.md` says the same
//! thing where a user reads it.
//!
//! # D8 needs nothing here
//!
//! A service a person stopped is never held for — [`hold_if_wakeable`] binds on an idle stop and on
//! nothing else — so there is no address at which their stop could be undone. The web path needs
//! `services.idle_stopped` to answer the same question because its activator's address is bound
//! either way; this path answers it by not binding.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mixengine_platform::activation::{Activation, Listen};
use mixengine_proto::{ServiceId, ServiceState};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{Registry, lock};

/// One held address, and the two handles that give it back.
#[derive(Debug)]
struct Held {
    /// What stops the accept loop.
    cancel: CancellationToken,

    /// What proves it stopped.
    ///
    /// **Awaited by [`Holder::release`]**, which is the whole reason it is kept. A release that
    /// returned before the loop had dropped its listener would hand the service an address the
    /// daemon still holds, and the service would fail to start for a reason nothing in its own log
    /// explains — its log can only say that the address is taken, never by what.
    task: JoinHandle<()>,
}

/// Every address this daemon is holding on a stopped service's behalf.
#[derive(Debug, Default)]
pub(crate) struct Holder {
    /// A `std` mutex rather than tokio's, as [`Registry`]'s own map of runners is: nothing awaits
    /// while it is held, and the awaits in [`Holder::release`] are on entries this has already
    /// taken out of the map.
    held: Mutex<HashMap<ServiceId, Vec<Held>>>,
}

impl Holder {
    /// Take every address in `addresses`, so that a connection to any of them starts `service`.
    ///
    /// **Idempotent.** A service already held for is released first: holding one address twice is
    /// one bind that fails and one caller that cannot tell which of the two is live.
    ///
    /// **One failure does not stop the rest.** An address something else already holds is one way
    /// this service cannot be woken, and a database wakeable on its port but not on its socket is
    /// better than one wakeable on neither.
    pub(crate) async fn hold(
        &self,
        services: Arc<Registry>,
        service: &ServiceId,
        addresses: Vec<Listen>,
    ) {
        self.release(service).await;

        let mut held = Vec::new();

        for address in addresses {
            let activation = match Activation::bind(&address).await {
                Ok(activation) => activation,
                Err(error) => {
                    tracing::warn!(
                        service = service.as_str(),
                        address = %address,
                        %error,
                        "this service cannot be woken at this address; something else holds it"
                    );

                    continue;
                }
            };

            tracing::debug!(
                service = service.as_str(),
                address = %address,
                "holding a stopped service's own address until something needs it"
            );

            let cancel = CancellationToken::new();

            let task = tokio::spawn(wake(
                Arc::clone(&services),
                service.clone(),
                activation,
                address,
                cancel.clone(),
            ));

            held.push(Held { cancel, task });
        }

        if !held.is_empty() {
            lock(&self.held).insert(service.clone(), held);
        }
    }

    /// Give every address held for `service` back, and do not return until they are given back.
    ///
    /// **Called from [`Registry::begin`] before anything is spawned**, which is what makes the
    /// ordering sound: the process is about to bind exactly these addresses.
    ///
    /// A service nothing is held for is the ordinary case — every running service, and every
    /// service a person stopped — and is not worth a line in the log.
    ///
    /// [`Registry::begin`]: super::Registry
    pub(crate) async fn release(&self, service: &ServiceId) {
        let Some(held) = lock(&self.held).remove(service) else {
            return;
        };

        // Cancelled first, all of them, so that the waits below overlap rather than queue.
        for one in &held {
            one.cancel.cancel();
        }

        for one in held {
            if let Err(error) = one.task.await {
                tracing::warn!(
                    service = service.as_str(),
                    %error,
                    "the task holding this service's address did not finish cleanly"
                );
            }
        }
    }

    /// How many services are held for, for a test and for a log line at boot.
    pub(crate) fn holding(&self) -> usize {
        lock(&self.held).len()
    }
}

/// Accept until cancelled, then give the address back.
///
/// **Every connection is carried in a task of its own, and that is not an optimisation.** The start
/// this asks for calls back into [`Holder::release`], which awaits *this* task — so carrying the
/// connection inline would be this task waiting for itself, and the service would never start.
async fn wake(
    services: Arc<Registry>,
    service: ServiceId,
    activation: Activation,
    address: Listen,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            () = cancel.cancelled() => break,
            accepted = activation.accept() => accepted,
        };

        let accepted = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(
                    service = service.as_str(),
                    address = %address,
                    %error,
                    "the daemon could not accept on this address, so it is giving it up"
                );

                break;
            }
        };

        let services = Arc::clone(&services);
        let service = service.clone();

        // The service's own address, which is what it will be listening on by the time the splice
        // dials — this task has released it by then, and the start has finished.
        let target = address.clone();

        tokio::spawn(async move {
            if let Err(refused) =
                super::activate::carry(&services, &service, &target, accepted).await
            {
                tracing::info!(
                    service = service.as_str(),
                    reason = ?refused,
                    "a connection asked for this service and was not served"
                );
            }
        });
    }

    if let Err(error) = activation.release() {
        tracing::warn!(
            service = service.as_str(),
            %error,
            "this service's own address was not given back cleanly, so it may fail to start"
        );
    }
}

/// Hold every address `service` can be woken at, if it is a service a connection may wake.
///
/// **The row decides, never the caller.** A service is held for only when it is `stopped` *and* the
/// column says the daemon is what stopped it — so a person's `mix service stop` leaves nothing
/// bound and a service that failed leaves nothing bound either. Anything else would be the tool
/// overruling its user, or a failure quietly re-armed.
pub(crate) async fn hold_if_wakeable(services: &Arc<Registry>, service: &ServiceId) {
    let record = match mixengine_core::services::record(services.store(), service).await {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(
                service = service.as_str(),
                %error,
                "cannot read the row of a service that may need its address held"
            );

            return;
        }
    };

    if record.state != ServiceState::Stopped || !record.idle_stopped {
        return;
    }

    let addresses = match services.wakeable_at(service).await {
        Ok(addresses) => addresses,
        Err(error) => {
            tracing::warn!(
                service = service.as_str(),
                %error,
                "cannot work out where this service would be woken, so it will not be"
            );

            return;
        }
    };

    if addresses.is_empty() {
        return;
    }

    services
        .holder()
        .hold(Arc::clone(services), service, addresses)
        .await;
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::atomic::{AtomicU16, Ordering};

    use mixengine_platform::activation::dial;

    use super::super::fixture::{Declared, home, registry, service, spec};
    use super::*;

    /// See the note on the counter in [`super::super::activate`]'s tests: two addresses chosen
    /// before either is bound are the same address, so each search starts where the last stopped.
    static NEXT: AtomicU16 = AtomicU16::new(0);

    fn somewhere(home: &mixengine_testkit::Home, name: &str) -> Listen {
        if cfg!(windows) {
            let base = 26_100 + NEXT.fetch_add(32, Ordering::Relaxed);

            let port = (base..base + 32)
                .find(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok())
                .expect("a free port in the window");

            Listen::Tcp((Ipv4Addr::LOCALHOST, port).into())
        } else {
            Listen::Socket(home.path().join(name))
        }
    }

    /// Take the address the way the *service* takes it, and not the way [`Activation::bind`] does —
    /// which clears a stale socket file first and would answer that the address is free whether or
    /// not the release gave it back. It is a `mariadbd` that has to succeed here.
    #[cfg(unix)]
    fn as_a_server_would(listen: &Listen) {
        match listen {
            Listen::Socket(path) => {
                std::os::unix::net::UnixListener::bind(path)
                    .expect("the address was still held after the release returned");
            }

            Listen::Tcp(address) => {
                TcpListener::bind(address).expect("the address was still held");
            }
        }
    }

    /// The same, on the system whose services all listen on ports.
    #[cfg(windows)]
    fn as_a_server_would(listen: &Listen) {
        let Listen::Tcp(address) = listen else {
            unreachable!("`somewhere` chooses a port on this system");
        };

        TcpListener::bind(address).expect("the address was still held after the release returned");
    }

    /// **The address is held while the service is stopped, and given back when it is asked for** —
    /// the design's D4, and the ordering the whole database path rests on.
    ///
    /// A database told to bind a port the daemon is still holding does not start, and its own log
    /// can only say that the address is taken — never by what. So a `release` that returned before
    /// the listener was really closed would be a failure that reads as somebody else's, which is
    /// why this asserts a *server's* bind rather than merely that `release` returned.
    #[tokio::test]
    async fn an_address_held_for_a_stopped_service_is_given_back_on_request() {
        let (fixture_home, paths, store) = home(&["db"]).await;

        let registry = Arc::new(registry(
            &paths,
            &store,
            Arc::new(Declared(vec![spec("db").build().expect("a usable spec")])),
        ));

        let own = somewhere(&fixture_home, "db.sock");
        let holder = Holder::default();

        holder
            .hold(Arc::clone(&registry), &service("db"), vec![own.clone()])
            .await;

        // Held: something answers there while the service is stopped.
        dial(&own).await.expect("nothing was holding the address");

        holder.release(&service("db")).await;

        as_a_server_would(&own);
    }

    /// **The address is given back before the process is spawned** — roadmap task **T70a**, and
    /// the ordering the whole database path rests on.
    ///
    /// A database asked to bind an address the daemon is still holding does not start. This drives
    /// a real start through [`Registry::begin`], which is the single funnel every start goes
    /// through — a person's, a dependency's, and an activation's alike — and asserts that the
    /// holder let go on the way. A release wired anywhere later would leave this address held for
    /// the whole of the service's life.
    #[tokio::test]
    async fn starting_a_held_service_gives_its_address_back_first() {
        let (fixture_home, paths, store) = home(&["db"]).await;

        let registry = Arc::new(registry(
            &paths,
            &store,
            Arc::new(Declared(vec![spec("db").build().expect("a usable spec")])),
        ));

        let own = somewhere(&fixture_home, "started.sock");

        registry
            .holder()
            .hold(Arc::clone(&registry), &service("db"), vec![own.clone()])
            .await;

        assert_eq!(registry.holder().holding(), 1, "the address was never held");

        let graph = registry.graph().await.expect("one declared service");
        let plan = graph.start_plan([&service("db")]).expect("a plan");
        let walk = registry.start(&graph, &plan).await;

        assert!(walk.failed.is_none(), "{walk:?}");

        assert_eq!(
            registry.holder().holding(),
            0,
            "the daemon went on holding an address the service had just been started to bind"
        );

        as_a_server_would(&own);
    }

    /// **A person's stop leaves nothing bound** — the design's D8, answered here by construction
    /// rather than by reading a column at the moment of the connection.
    ///
    /// `mix service stop mariadb@main` followed by the next connection starting it again is the
    /// tool overruling its user. The web path needs `services.idle_stopped` to tell the two stops
    /// apart because its activator's address is bound either way; this path never binds for a stop
    /// it did not make, so `mariadb` says "connection refused", which is the truth.
    #[tokio::test]
    async fn a_service_a_person_stopped_is_not_held_for() {
        let (fixture_home, paths, store) = home(&["db"]).await;

        // What a person's stop leaves behind: stopped, and not by the daemon.
        sqlx::query("UPDATE services SET state = 'stopped', idle_stopped = 0 WHERE id = 'db'")
            .execute(store.pool())
            .await
            .expect("the row");

        let registry = Arc::new(registry(
            &paths,
            &store,
            Arc::new(Declared(vec![spec("db").build().expect("a usable spec")])),
        ));

        hold_if_wakeable(&registry, &service("db")).await;

        assert_eq!(
            registry.holder().holding(),
            0,
            "a stop nobody asked the daemon for was armed to be undone"
        );

        // **And the same holder does hold things**, so the assertion above is about the row's
        // answer and not about a fixture with nothing to hold. The addresses are handed over
        // directly because this fixture declares specs rather than recipes, and `wakeable_at` asks
        // a recipe.
        registry
            .holder()
            .hold(
                Arc::clone(&registry),
                &service("db"),
                vec![somewhere(&fixture_home, "person.sock")],
            )
            .await;

        assert_eq!(
            registry.holder().holding(),
            1,
            "the holder never holds anything, so the assertion above was vacuous"
        );
    }
}
