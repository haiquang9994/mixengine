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

/// The responder, and the names it is currently answering for.
pub(crate) struct Mdns {
    /// [`None`] on a home where the responder would not start. Every method is then a no-op, and
    /// `advertised` on the wire is what says so.
    daemon: Option<mdns_sd::ServiceDaemon>,

    /// What is registered: the mDNS name, against the service instance it was published as.
    ///
    /// **The instance name is kept rather than rebuilt.** A hostname is only ever published as part
    /// of a service (D7), and withdrawing one takes the service's *full* name — which `mdns-sd`
    /// escapes as it builds it, from a site domain that contains dots. Recomputing it here would be
    /// a second spelling of a string that has exactly one correct value, and the failure it buys is
    /// silent: an unshared site whose name goes on resolving.
    held: Mutex<BTreeMap<String, String>>,
}

impl Mdns {
    /// Start the responder, or record that this home has none.
    ///
    /// **Nothing here fails the daemon's start**, on the rule [`crate::dns::Dns::start`] follows: a
    /// port somebody else is holding is a state to report, not a machine with no daemon.
    pub(crate) fn start(shutdown: CancellationToken) -> Self {
        let daemon = match mdns_sd::ServiceDaemon::new() {
            Ok(daemon) => {
                tracing::info!("shared sites will be advertised by name on the local network");
                Some(daemon)
            }
            Err(error) => {
                tracing::warn!(%error, "shared sites will be reachable by address only");
                None
            }
        };

        if let Some(daemon) = daemon.clone() {
            tokio::spawn(async move {
                shutdown.cancelled().await;
                let _ = daemon.shutdown();
            });
        }

        Self {
            daemon,
            held: Mutex::new(BTreeMap::new()),
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
            daemon: None,
            held: Mutex::new(BTreeMap::new()),
        }
    }

    /// Make the network agree with `wanted` — the whole of this module's contract.
    ///
    /// Registering what is missing and dropping what is no longer there, so that calling this after
    /// every change to a share is both correct and cheap: reconciling a set that already matches
    /// touches no socket.
    pub(crate) fn advertises(&self, wanted: &[Advertisement]) {
        let Some(daemon) = self.daemon.as_ref() else {
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
            match self.register(daemon, one) {
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

    /// Whether a name is being answered for right now.
    ///
    /// What `SiteSharing::advertised` is filled from, so that a name nothing answers for is said
    /// rather than printed as though it resolved.
    pub(crate) fn advertising(&self, name: &str) -> bool {
        self.held().contains_key(name)
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
            .field("responding", &self.daemon.is_some())
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
    #[test]
    fn a_home_with_no_responder_advertises_nothing() {
        let mdns = Mdns {
            daemon: None,
            held: Mutex::new(BTreeMap::new()),
        };

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
