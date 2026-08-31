//! Advertising a shared site under a name a phone can resolve — roadmap task **T75**.
//!
//! **Whole state, reconciled from the rows** — the T75 design, D4, which is T74's D6 applied to a
//! second mechanism. Every path that changes a share asks this module what the home should be
//! advertising and it makes that true; a daemon restarted with sites already shared advertises them
//! again with nobody asking, and *"is this right?"* is a comparison rather than a judgement.
//!
//! **A responder that will not start never fails a share.** Where UDP 5353 cannot be bound the site
//! is still shared by address, which is exactly what T74 shipped — the name is the improvement, not
//! the feature. The configuration and the certificate carry the name whatever this module is doing,
//! because deriving them from a responder's health would mean a responder dying triggers a
//! certificate reissue, on a timer, for every site.
//!
//! **The socket is bound when there is something to advertise, and let go when there is not** —
//! roadmap task **T76**, the design's D8. On Windows, binding UDP 5353 makes the operating system
//! raise its *own* firewall dialog, and the rule it writes when somebody clicks Allow is every
//! port, TCP and UDP, on the Private and Public profiles — wider than everything this feature
//! promises, created outside `mixengine-elevate`, and not removed by `site.unshare` because
//! MixEngine never made it. Nothing here can stop Windows asking. What it can do is make the
//! question arrive in the second after somebody typed `mix site share`, where it has an obvious
//! answer, instead of at the start of a daemon on a machine that is sharing nothing. `mix doctor`
//! reports the rule if it is there.
//!
//! **The name is one label under `.local`.** `blog-mixengine.local` and never
//! `blog.mixengine.local`: mDNS conventions single-label host names (RFC 6762 section 3) and
//! Windows' resolver enforces the convention. Measured, see the T75 design, D1.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

/// What one shared site is advertised as.
pub(crate) struct Advertisement {
    /// `<slug>-mixengine.local`.
    pub(crate) name: String,

    /// The address the name resolves to.
    pub(crate) address: Ipv4Addr,

    /// The interface it is announced on, by the name the OS gives it.
    pub(crate) interface: String,

    /// The site's primary domain, which is what a service browser shows.
    pub(crate) primary: String,

    /// The port this home's front end answers on.
    pub(crate) port: u16,
}

/// One bound responder, and the task watching for the daemon's shutdown on its behalf.
///
/// **The two are one value because they end together.** A responder is dropped whenever the last
/// share goes away, and the task waiting to shut it down has nothing left to do at that moment — so
/// `stop` cancels the child token and the task ends with the socket. Kept apart, each bind would
/// leave a parked task behind holding a handle to a responder that is already gone: within the
/// workspace rule about tasks outliving shutdown, since they all end at the daemon's own token, and
/// still one more of them for every time somebody shares and unshares.
struct Responder {
    /// The bound responder.
    daemon: mdns_sd::ServiceDaemon,

    /// Cancelled by [`Mdns::stop`], and a child of the daemon's own token so that a shutdown ends
    /// it too.
    watcher: CancellationToken,
}

/// The responder, and the names it is currently answering for.
pub(crate) struct Mdns {
    /// The responder, built the first time this home has something to advertise and dropped when it
    /// has nothing — roadmap task **T76**.
    ///
    /// **[`None`] is two states that behave alike**: a home that has not needed one yet, and one
    /// where the socket would not bind. Both advertise nothing, and both report
    /// `SiteSharing::advertised` as false — which is what T75 already says on the wire, so nothing
    /// above this module learns that the binding moved.
    daemon: Mutex<Option<Responder>>,

    /// What is registered: the mDNS name, against the service instance it was published as.
    ///
    /// **The instance name is kept rather than rebuilt.** A hostname is only ever published as part
    /// of a service (D7), and withdrawing one takes the service's *full* name — which `mdns-sd`
    /// escapes as it builds it, from a site domain that contains dots. Recomputing it here would be
    /// a second spelling of a string that has exactly one correct value, and the failure it buys is
    /// silent: an unshared site whose name goes on resolving.
    held: Mutex<BTreeMap<String, String>>,

    /// Cancelled when the daemon is going down, so a responder built later is still shut down.
    ///
    /// Held rather than captured once at start: the responder this token has to reach does not
    /// exist yet when [`Mdns::start`] runs, which is the whole of what T76 changed here.
    shutdown: CancellationToken,

    /// Whether this home may open a socket at all.
    ///
    /// **False only in tests, and it earns its place there.** Until T76 a test could hold an `Mdns`
    /// whose responder was [`None`] and know it would stay that way, because the socket was opened
    /// once at start. Now that the socket arrives with the first advertisement, a test that shares
    /// a site would bind UDP 5353 and announce a name on the Wi-Fi of whoever is running
    /// `cargo test` — which is exactly the side effect
    /// `silent_for_tests` was written to prevent. (Not a link: that constructor is `cfg(test)`,
    /// so it does not exist in a documentation build.)
    binds: bool,
}

impl Mdns {
    /// Take the shutdown signal, and bind nothing.
    ///
    /// **No socket is opened here** — roadmap task **T76**, and this is the whole of the change:
    /// until T76 this call bound UDP 5353 at every daemon start, which on Windows put a firewall
    /// dialog in front of somebody who had asked for nothing. The socket now arrives with the first
    /// share, in [`responder`](Self::responder).
    pub(crate) fn start(shutdown: CancellationToken) -> Self {
        Self {
            daemon: Mutex::new(None),
            held: Mutex::new(BTreeMap::new()),
            shutdown,
            binds: true,
        }
    }

    /// A responder that answers for nothing, for the API tests — roadmap task **T75**.
    ///
    /// **The same object a home with no responder holds**, and not a mock: an API test that shared
    /// a site would otherwise announce a name on whatever network the machine running the test is
    /// on, which is a test with a side effect on somebody's Wi-Fi.
    #[cfg(test)]
    pub(crate) fn silent_for_tests() -> Self {
        Self {
            daemon: Mutex::new(None),
            held: Mutex::new(BTreeMap::new()),
            shutdown: CancellationToken::new(),
            binds: false,
        }
    }

    /// Make the network agree with `wanted` — the whole of this module's contract.
    ///
    /// Registering what is missing and dropping what is no longer there, so that calling this after
    /// every change to a share is both correct and cheap: reconciling a set that already matches
    /// touches no socket.
    pub(crate) fn advertises(&self, wanted: &[Advertisement]) {
        // **A home advertising nothing holds no socket** — roadmap task T76. Every unshare of the
        // last shared site arrives here, and so does every daemon start, so this is also what keeps
        // a machine that has never shared anything from ever binding UDP 5353.
        if wanted.is_empty() {
            self.withdraw();
            self.stop();
            return;
        }

        let Some(daemon) = self.responder() else {
            return;
        };

        let names: Vec<String> = wanted.iter().map(|one| one.name.clone()).collect();
        let held: Vec<String> = self.held().keys().cloned().collect();

        let (register, unregister) = difference(&held, &names);

        for name in unregister {
            // The instance's full name as it was registered — a hostname is only ever published as
            // part of a service (D7), so it is a service that is withdrawn.
            let Some(instance) = self.held().remove(&name) else {
                continue;
            };

            let _ = daemon.unregister(&instance);
        }

        for one in wanted.iter().filter(|one| register.contains(&one.name)) {
            match self.register(&daemon, one) {
                Ok(instance) => {
                    self.held().insert(one.name.clone(), instance);
                }
                Err(error) => tracing::warn!(
                    name = %one.name,
                    %error,
                    "this site is shared, and reachable by address only"
                ),
            }
        }
    }

    /// The responder, built on first use — roadmap task **T76**, the design's D8.
    ///
    /// **A responder that will not start is still not a failure** — T75's D5, unchanged: the site
    /// is shared by address either way, and `SiteSharing::advertised` is how that is said. What T76
    /// changed is only *when* the attempt is made, and therefore when Windows asks its question.
    ///
    /// A home where the socket will not bind retries on the next reconciliation rather than
    /// remembering the refusal. That is the cheaper direction to be wrong in: the alternative is a
    /// daemon that gave up at some point nobody saw and never advertises again.
    fn responder(&self) -> Option<mdns_sd::ServiceDaemon> {
        let mut held = self.daemon.lock().expect("the responder is never poisoned");

        if held.is_none() && self.binds {
            match mdns_sd::ServiceDaemon::new() {
                Ok(daemon) => {
                    tracing::info!("shared sites are advertised by name on the local network");

                    // A child of the daemon's token, so a shutdown reaches it — and so [`stop`] can
                    // end this task without waiting for one.
                    let watcher = self.shutdown.child_token();
                    let cancelled = watcher.clone();
                    let going_down = daemon.clone();

                    tokio::spawn(async move {
                        cancelled.cancelled().await;
                        let _ = going_down.shutdown();
                    });

                    *held = Some(Responder { daemon, watcher });
                }
                Err(error) => {
                    tracing::warn!(%error, "shared sites are reachable by address only");
                }
            }
        }

        held.as_ref().map(|held| held.daemon.clone())
    }

    /// Withdraw every name this home has registered.
    ///
    /// Factored out of [`advertises`](Self::advertises) rather than written twice: the unregister
    /// half of a reconciliation and the withdrawal of everything are the same operation over
    /// different sets, and the escaping bug T75 found came from exactly this string being spelled
    /// in two places.
    fn withdraw(&self) {
        let Some(daemon) = self
            .daemon
            .lock()
            .expect("the responder is never poisoned")
            .as_ref()
            .map(|held| held.daemon.clone())
        else {
            return;
        };

        for (_, instance) in std::mem::take(&mut *self.held()) {
            let _ = daemon.unregister(&instance);
        }
    }

    /// Let the socket go, if there is one, and end the task that was watching it.
    fn stop(&self) {
        if let Some(held) = self
            .daemon
            .lock()
            .expect("the responder is never poisoned")
            .take()
        {
            // Cancel first: the watcher's whole job is to shut this responder down, and it has just
            // been done here. Without this the task would wait for the daemon's own token holding a
            // handle to a responder that is already gone.
            held.watcher.cancel();
            let _ = held.daemon.shutdown();
        }
    }

    /// Whether a name is being answered for right now.
    ///
    /// What `SiteSharing::advertised` is filled from, so that a name nothing answers for is said
    /// rather than printed as though it resolved.
    pub(crate) fn advertising(&self, name: &str) -> bool {
        self.held().contains_key(name)
    }

    /// Whether a responder is held at all — roadmap task **T76**.
    ///
    /// [`advertising`](Self::advertising) answers about a *name*; this answers about the socket,
    /// which is the thing D8 moved and the only thing a test of D8 can look at.
    #[cfg(test)]
    pub(crate) fn responding(&self) -> bool {
        self.daemon
            .lock()
            .expect("the responder is never poisoned")
            .is_some()
    }

    /// One site, announced on one interface.
    ///
    /// **Pinned to the shared interface** — the T75 design, D6. `mdns-sd` announces on every
    /// interface by default; the machine this was written on has eight addresses across seven
    /// interfaces, and a name announced on a network the site is not bound to and the firewall does
    /// not cover is a URL that fails for whoever is handed it.
    ///
    /// **No TXT properties** — D7. There is no hostname-only registration in `mdns-sd`, so a name
    /// is always published as part of a service, and a shared site therefore appears in every
    /// service browser on the Wi-Fi. What it says is chosen rather than defaulted: the site's own
    /// domain, the port a browser would use, and nothing about the project, its root or its
    /// runtime. A document root is an absolute path on somebody's laptop.
    /// Answers the service's full name, which is the only string `unregister` accepts.
    fn register(
        &self,
        daemon: &mdns_sd::ServiceDaemon,
        one: &Advertisement,
    ) -> Result<String, mdns_sd::Error> {
        daemon.disable_interface(mdns_sd::IfKind::All)?;
        daemon.enable_interface(one.interface.as_str())?;

        let info = mdns_sd::ServiceInfo::new(
            "_http._tcp.local.",
            &one.primary,
            // Trailing dot: `mdns-sd` refuses a hostname that does not end in `.local.`, and the
            // name this home computes is written the way a URL is.
            &format!("{}.", one.name),
            std::net::IpAddr::V4(one.address),
            one.port,
            &[] as &[(&str, &str)],
        )?;

        // Read back rather than rebuilt: `ServiceInfo::new` escapes the instance name, and the
        // site domain it is built from contains dots.
        let instance = info.get_fullname().to_owned();

        daemon.register(info)?;

        Ok(instance)
    }

    /// The registered set, with the lock's poisoning treated as unreachable.
    ///
    /// Nothing under this lock can panic — it is a `BTreeSet<String>` and two set operations — so a
    /// poisoned lock would mean something impossible has already happened.
    fn held(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.held.lock().expect("the name map is never poisoned")
    }
}

impl std::fmt::Debug for Mdns {
    /// The names, and whether there is a responder at all.
    ///
    /// Hand-written because `mdns_sd::ServiceDaemon` is not [`Debug`], and what a reader of a log
    /// wants from this object is what it is answering for rather than what is inside it.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Mdns")
            .field(
                "responding",
                &self
                    .daemon
                    .lock()
                    .expect("the responder is never poisoned")
                    .is_some(),
            )
            .field("names", &self.held().keys().collect::<Vec<_>>())
            .finish()
    }
}

/// What to register and what to drop, so that the network says `wanted`.
fn difference(held: &[String], wanted: &[String]) -> (Vec<String>, Vec<String>) {
    let held: BTreeSet<&String> = held.iter().collect();
    let wanted: BTreeSet<&String> = wanted.iter().collect();

    (
        wanted
            .difference(&held)
            .map(|name| (*name).clone())
            .collect(),
        held.difference(&wanted)
            .map(|name| (*name).clone())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Nothing binds UDP 5353 until a site is shared** — the T76 design, D8.
    ///
    /// On Windows, binding that port makes the operating system raise its own firewall dialog, and
    /// the rule it writes when somebody clicks Allow is every port, TCP and UDP, on two profiles.
    /// MixEngine cannot stop it being offered and does not create it — what it can do is make sure
    /// the question arrives in the second after somebody typed `mix site share`, where it has an
    /// obvious answer, rather than at the start of a daemon on a machine sharing nothing.
    #[test]
    fn a_home_sharing_nothing_holds_no_responder() {
        let mdns = Mdns::start(CancellationToken::new());

        assert!(!mdns.responding(), "nothing shared, nothing bound");

        // Reconciling to nothing is what every daemon start and every last unshare does, and it is
        // the call that must not bind.
        mdns.advertises(&[]);

        assert!(!mdns.responding());
        assert!(!mdns.advertising("blog-mixengine.local"));
    }

    /// **Whole state, not a delta** — the T75 design, D4. A daemon restarted while a site is shared
    /// advertises it again without anybody asking, and a share that ended stops being announced by
    /// the same call rather than by a second one somebody has to remember.
    #[test]
    fn reconciling_registers_what_is_missing_and_drops_what_is_gone() {
        let held = vec![
            "blog-mixengine.local".to_owned(),
            "old-mixengine.local".to_owned(),
        ];
        let wanted = [
            "blog-mixengine.local".to_owned(),
            "shop-mixengine.local".to_owned(),
        ];

        let (register, unregister) = difference(&held, &wanted);

        assert_eq!(register, vec!["shop-mixengine.local".to_owned()]);
        assert_eq!(unregister, vec!["old-mixengine.local".to_owned()]);
    }

    /// Reconciling what is already advertised touches nothing, which is what makes it safe to call
    /// from every path that changes a share.
    #[test]
    fn reconciling_what_is_already_advertised_changes_nothing() {
        let held = vec!["blog-mixengine.local".to_owned()];
        let wanted = ["blog-mixengine.local".to_owned()];

        let (register, unregister) = difference(&held, &wanted);

        assert!(register.is_empty(), "{register:?}");
        assert!(unregister.is_empty(), "{unregister:?}");
    }

    /// A home that shares nothing withdraws everything it was announcing.
    #[test]
    fn a_home_that_shares_nothing_withdraws_every_name() {
        let held = vec!["blog-mixengine.local".to_owned()];

        let (register, unregister) = difference(&held, &[]);

        assert!(register.is_empty(), "{register:?}");
        assert_eq!(unregister, vec!["blog-mixengine.local".to_owned()]);
    }

    /// **The instance name is not the site's domain with a suffix**, which is why what is
    /// registered is read back rather than rebuilt.
    ///
    /// `ServiceInfo::new` escapes the instance label, and a site domain is full of dots. This test
    /// exists because the first version of this module withdrew a service by rebuilding the string,
    /// which silently left an unshared site's name resolving — the one thing the feature spec says
    /// unsharing must stop.
    #[test]
    fn the_registered_instance_name_is_not_what_a_caller_would_have_guessed() {
        let info = mdns_sd::ServiceInfo::new(
            "_http._tcp.local.",
            "blog.test",
            "blog-mixengine.local.",
            std::net::IpAddr::V4([192, 168, 1, 10].into()),
            80,
            &[] as &[(&str, &str)],
        )
        .expect("a service");

        assert_ne!(info.get_fullname(), "blog.test._http._tcp.local.");
        assert_ne!(
            info.get_fullname(),
            "blog-mixengine.local._http._tcp.local."
        );
    }

    /// **A home with no responder answers every question without one**, rather than holding a set
    /// that says it is advertising names nothing is answering for.
    ///
    /// Since T76 this is also the test that a silent home stays silent when it is asked to
    /// advertise something: the socket now arrives with the first advertisement, so without
    /// `binds` this call would open UDP 5353 and put a name on the Wi-Fi of whoever ran the suite.
    #[test]
    fn a_home_with_no_responder_advertises_nothing() {
        let mdns = Mdns::silent_for_tests();

        mdns.advertises(&[Advertisement {
            name: "blog-mixengine.local".to_owned(),
            address: [192, 168, 1, 10].into(),
            interface: "Wi-Fi".to_owned(),
            primary: "blog.test".to_owned(),
            port: 80,
        }]);

        assert!(!mdns.advertising("blog-mixengine.local"));
    }
}
