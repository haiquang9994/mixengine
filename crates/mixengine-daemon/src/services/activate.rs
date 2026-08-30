//! Starting a service from the connection that needed it — roadmap task **T70**.
//!
//! T69 taught the daemon to stop a service nothing was using and shipped it switched off, because
//! stopping a pool is only safe once something starts it again. This is that something.
//!
//! # It never reads what it carries
//!
//! The design's D1. A connection arrives, the service is made to run, the activator dials it and
//! copies bytes until one side closes — and nothing in that sentence mentions a protocol. FastCGI,
//! the MySQL protocol and RESP have nothing in common except that a client connects and something
//! is said, so an activator that parsed any of them would be a parser in `mixengined` written to
//! throw its own answer away.
//!
//! The consequence worth stating: a client that speaks first and one that waits to be greeted are
//! the same case here. The waiting client is the one a web-only design would never have met, and it
//! is most of what T70a will need.
//!
//! # Three answers, and each of them is bounded
//!
//! | The service is | This does |
//! | --- | --- |
//! | stopped, and the daemon idled it | start it, wait, proxy |
//! | stopped, and a person stopped it | close the connection |
//! | running | proxy straight through |
//!
//! The middle row is the design's D8 and it is not a detail: `mix service stop mariadb@main`
//! followed by the next connection undoing it is the tool overruling its user. What makes it
//! answerable after a restart is `services.idle_stopped`, written on every arrival at `stopped`.
//!
//! The third row is not a special case for its own sake — a service that is running and whose
//! primary address was refused anyway is a fault this is not the place to diagnose, and proxying is
//! both the honest answer and the harmless one.
//!
//! # What it deliberately does not do
//!
//! **It does not decide how long a start may take.** That budget is the service's own `ReadyCheck`
//! timeout, which is already per-recipe and already the number a person raises when their machine
//! is slow. A second timeout invented here would give one slow MariaDB two different opinions.
//!
//! **It does not start a process.** It calls the same `Services::start` a person's `mix service
//! start` calls, with the plan built from the graph — so a dependency is started in order, and a
//! service already mid-start for another reason is joined rather than started twice. Activation is
//! a second *caller*, never a second way.

use std::sync::Arc;

use mixengine_platform::activation::{Activation, Incoming, Listen, dial};
use mixengine_proto::{ServiceId, ServiceState};

use super::Registry;

/// Why a connection did not reach the service it asked for.
///
/// Every arm closes the connection, and the difference between them is only what is said in the
/// log — which is the whole of what a person has to go on when a site answers 502.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// A person stopped this service, so a request is not allowed to undo that — D8.
    StoppedOnPurpose,

    /// The service could not be started.
    WouldNotStart,

    /// It started, and then nothing answered at its own address.
    NotListening,
}

/// Hold `listen` for as long as the daemon runs, and start `service` for whoever dials it.
///
/// Returns once the listener is bound, so a caller knows the address is taken before it renders a
/// site file naming it; the accepting runs in a task of its own from there.
///
/// # Errors
///
/// Whatever binding costs — most often that something else already holds the address, which for an
/// activator means a home whose front end is pointed at a port a different program took first.
pub(crate) async fn spawn(
    services: Arc<Registry>,
    service: ServiceId,
    listen: Listen,
    target: Listen,
) -> mixengine_platform::Result<()> {
    let listener = Activation::bind(&listen).await?;

    tracing::debug!(
        service = service.as_str(),
        address = %listen,
        "holding an address so a request can start this service"
    );

    tokio::spawn(async move {
        loop {
            let accepted = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(
                        service = service.as_str(),
                        %error,
                        "the activator could not accept; it is giving up on this address"
                    );
                    return;
                }
            };

            let services = Arc::clone(&services);
            let service = service.clone();
            let target = target.clone();

            // One task per connection, or a service that takes five seconds to start would hold
            // every other request to every other pool behind it.
            tokio::spawn(async move {
                if let Err(refused) = carry(&services, &service, &target, accepted).await {
                    tracing::info!(
                        service = service.as_str(),
                        reason = ?refused,
                        "a connection asked for this service and was not served"
                    );
                }
            });
        }
    });

    Ok(())
}

/// Hold an address for every service in this home that a request may have to start.
///
/// **Every activatable service, whether or not it is running.** The address is held for as long as
/// the daemon lives, because a site file names it either way: one that changed when a pool stopped
/// would make every idle stop rewrite `etc/` and reload the front end — a reload storm driven by the
/// thing that exists to save work.
///
/// **One failure does not stop the rest.** An address something else already holds is one service
/// that cannot be woken, and a home where the other four still can is better than a daemon that
/// refused to finish starting.
///
/// # Errors
///
/// Whatever asking the generator costs — a home too deeply nested for a derived socket path, or a
/// row this build cannot read.
pub(crate) async fn hold_all(
    services: Arc<Registry>,
    paths: &mixengine_core::Paths,
    store: &mixengine_core::Store,
    host: &dyn mixengine_platform::Host,
) -> mixengine_core::Result<Vec<ServiceId>> {
    let generator = super::spec::generator(paths, store, host);
    let mut holding = HOLDING.lock().await;
    let mut held = Vec::new();

    for (service, (listen, target)) in generator.activators().await? {
        if holding.contains(&service) {
            continue;
        }

        let listen = to_listen(&listen);
        let target = to_listen(&target);

        match spawn(Arc::clone(&services), service.clone(), listen, target).await {
            Ok(()) => {
                holding.insert(service.clone());
                held.push(service);
            }
            Err(error) => tracing::warn!(
                service = service.as_str(),
                %error,
                "this service cannot be started by a request; something else holds its activator's                  address"
            ),
        }
    }

    Ok(held)
}

/// Which services this daemon is already holding an address for.
///
/// **Because this is a repair and not a step**, exactly as `activation::ensure` is one: it runs at
/// boot *and* after a runtime install, since a pool created by that install has an activator nothing
/// has bound yet — and without a second call it would have none until the next daemon start, which
/// is a site that answers 502 for half an hour and then works after a restart nobody could have
/// known to make. That was T70's, found by T72a's cold path measuring it.
///
/// A set rather than nothing, because `Activation::bind` on an address this same daemon already
/// holds fails with `AddrInUse` — so a second pass would log every existing activator as an address
/// something else took, which is the opposite of true.
///
/// Process-wide, like `ports`' allocation lock and for the same reason: what it guards is a fact
/// about this operating system, not about any one caller.
static HOLDING: tokio::sync::Mutex<std::collections::BTreeSet<ServiceId>> =
    tokio::sync::Mutex::const_new(std::collections::BTreeSet::new());

/// One address, from the vocabulary a recipe renders in to the one the platform binds.
///
/// **Two types for one idea, deliberately.** `Upstream` is core's and belongs to the file it is
/// written into; `Listen` is the platform's and belongs to the call that binds it. Neither crate
/// depends on the other in the direction that would let them share one.
///
/// **Two callers**: this file's activator and T70a's [`hold`](super::hold), which is why it is not
/// inlined into either.
pub(super) fn to_listen(upstream: &mixengine_core::generate::Upstream) -> Listen {
    match upstream {
        mixengine_core::generate::Upstream::Socket(path) => Listen::Socket(path.clone()),
        mixengine_core::generate::Upstream::Tcp(address) => Listen::Tcp(*address),
    }
}

/// Make sure the service is running, then copy bytes between the client and it.
///
/// **Two callers, one splice.** [`spawn`] above holds an address of the activator's own; T70a's
/// [`hold`](super::hold) holds the service's *own* address and gives it back before it gets here.
/// What happens once the connection has arrived is identical, and a second copy of
/// `copy_bidirectional` would be a second place for D1 — *it never reads what it carries* — to be
/// true, or to quietly stop being.
pub(super) async fn carry(
    services: &Registry,
    service: &ServiceId,
    target: &Listen,
    mut client: Incoming,
) -> Result<(), Refused> {
    ensure_running(services, service).await?;

    let mut server = dial(target).await.map_err(|error| {
        tracing::warn!(
            service = service.as_str(),
            address = %target,
            %error,
            "the service is running and nothing answers at its own address"
        );

        Refused::NotListening
    })?;

    // **The client's first bytes have not been read yet**, which is what makes this protocol-blind:
    // whatever it said while the service was starting is still in the socket, and this is the moment
    // it is forwarded. A client that has said nothing yet is served by exactly the same call.
    if let Err(error) = tokio::io::copy_bidirectional(&mut client, &mut server).await {
        // Not a refusal: the service was reached and the two of them talked. A connection ending
        // untidily is ordinary — a browser navigating away closes one mid-response.
        tracing::debug!(
            service = service.as_str(),
            %error,
            "a proxied connection ended early"
        );
    }

    Ok(())
}

/// The three answers in the table above.
async fn ensure_running(services: &Registry, service: &ServiceId) -> Result<(), Refused> {
    let record = match mixengine_core::services::record(services.store(), service).await {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(
                service = service.as_str(),
                %error,
                "cannot read the row of a service a connection asked for"
            );

            return Err(Refused::WouldNotStart);
        }
    };

    if record.state != ServiceState::Stopped {
        // Running, starting, or on its way down. Proxy and let the dial say whether anything is
        // there — a service in trouble is not this function's to diagnose.
        return Ok(());
    }

    if !record.idle_stopped {
        return Err(Refused::StoppedOnPurpose);
    }

    let Ok(graph) = services.graph().await else {
        return Err(Refused::WouldNotStart);
    };

    let Ok(plan) = graph.start_plan(std::slice::from_ref(service)) else {
        return Err(Refused::WouldNotStart);
    };

    let walk = services.start(&graph, &plan).await;

    if walk.failed.is_some() {
        return Err(Refused::WouldNotStart);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::Arc;

    use mixengine_platform::activation::dial;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::super::fixture::{Declared, home, registry, service};
    use super::*;

    /// An address of the shape this system's services use, chosen by binding rather than written
    /// down — a number this file merely hoped for is one another program is entitled to hold.
    /// **The counter is not decoration.** Two addresses chosen before either is bound are the same
    /// address on Windows: the first search has not taken its port yet when the second one looks, so
    /// both are handed the lowest free number and the second bind fails with `AddrInUse`. Each call
    /// therefore starts its search where the last one stopped.
    static NEXT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

    fn somewhere(home: &mixengine_testkit::Home, name: &str) -> Listen {
        if cfg!(windows) {
            let base = 25_500 + NEXT.fetch_add(32, std::sync::atomic::Ordering::Relaxed);

            let port = (base..base + 32)
                .find(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok())
                .expect("a free port in the window");

            Listen::Tcp((Ipv4Addr::LOCALHOST, port).into())
        } else {
            Listen::Socket(home.path().join(name))
        }
    }

    /// Mark a stopped service as one the daemon idled, which is what `transition` writes for real.
    async fn idled(store: &mixengine_core::Store, id: &str, idle: bool) {
        sqlx::query("UPDATE services SET idle_stopped = ? WHERE id = ?")
            .bind(i64::from(idle))
            .bind(id)
            .execute(store.pool())
            .await
            .expect("the row");
    }

    /// **A person's stop is not undone by a connection** — the design's D8.
    ///
    /// `mix service stop` followed by the next request restarting the service is the tool overruling
    /// its user, and after a daemon restart the only thing that can still tell the two stops apart
    /// is the column this reads.
    #[tokio::test]
    async fn a_service_a_person_stopped_is_not_started_by_a_connection() {
        let (home, paths, store) = home(&["waker"]).await;
        idled(&store, "waker", false).await;

        let registry = Arc::new(registry(
            &paths,
            &store,
            Arc::new(Declared(vec![
                super::super::fixture::spec("waker")
                    .build()
                    .expect("a usable spec"),
            ])),
        ));

        let listen = somewhere(&home, "activate-person.sock");
        let target = somewhere(&home, "target-person.sock");

        spawn(
            Arc::clone(&registry),
            service("waker"),
            listen.clone(),
            target,
        )
        .await
        .expect("a bound address");

        let mut client = dial(&listen).await.expect("a connection");

        // The activator closes it without starting anything, so the read ends rather than hanging.
        let mut anything = [0_u8; 1];
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.read(&mut anything),
        )
        .await
        .expect("the activator closed the connection rather than holding it");

        assert_eq!(read.ok(), Some(0), "a refusal is a close, not a hang");

        assert_eq!(
            mixengine_core::services::record(&store, &service("waker"))
                .await
                .expect("the row")
                .state,
            mixengine_proto::ServiceState::Stopped,
            "the connection started a service its owner had stopped"
        );
    }

    /// **A running service is proxied straight through, in both conversational orders** — D1.
    ///
    /// The activator never reads what it carries, so a client that speaks first (FastCGI) and one
    /// that waits to be greeted (MySQL) are the same case. The second is the one a web-only design
    /// would never meet, and it is most of what T70a needs.
    #[tokio::test]
    async fn a_client_that_speaks_first_and_one_that_waits_are_both_carried() {
        let (home, paths, store) = home(&["waker"]).await;

        // Running, so `ensure_running` proxies without starting anything: what is under test here is
        // the splice, and a real pool would make it a test of php-fpm.
        sqlx::query("UPDATE services SET state = 'running' WHERE id = 'waker'")
            .execute(store.pool())
            .await
            .expect("the row");

        let registry = Arc::new(registry(&paths, &store, Arc::new(Declared(Vec::new()))));

        let target = somewhere(&home, "target-splice.sock");
        let listen = somewhere(&home, "activate-splice.sock");

        // A stand-in for the service: greet, then echo whatever is said.
        let served = Activation::bind(&target).await.expect("a stand-in service");
        tokio::spawn(async move {
            while let Ok(mut connection) = served.accept().await {
                tokio::spawn(async move {
                    connection.write_all(b"hello").await.expect("the greeting");

                    let mut said = [0_u8; 4];
                    connection.read_exact(&mut said).await.expect("a read");
                    connection.write_all(&said).await.expect("the echo");
                });
            }
        });

        spawn(
            Arc::clone(&registry),
            service("waker"),
            listen.clone(),
            target,
        )
        .await
        .expect("a bound address");

        let mut client = dial(&listen).await.expect("a connection");

        // Speaks first: the bytes were written before the service was ever dialled.
        client.write_all(b"ping").await.expect("a write");

        let mut greeting = [0_u8; 5];
        client.read_exact(&mut greeting).await.expect("a read");
        assert_eq!(
            &greeting, b"hello",
            "the greeting did not survive the splice"
        );

        let mut echoed = [0_u8; 4];
        client.read_exact(&mut echoed).await.expect("a read");
        assert_eq!(&echoed, b"ping", "what the client said first was lost");
    }
}
