//! Putting one site on the local network, and taking it back off — roadmap task **T74**.
//!
//! Five things move, and the order they move in is the whole of this module.
//!
//! **Sharing**: write the row, re-render and reload the front end, reissue the certificate, then ask
//! for the firewall rule. The rule is last because a rule open on a port nothing is listening on is
//! the one state a user cannot diagnose — the phone says "connection refused" either way, and the
//! machine has been changed in a way nobody can see.
//!
//! **Unsharing**: ask for the rule to go first, then the row, the rendering and the certificate.
//! Reversed for the same reason from the other side: the window where the machine is more open than
//! the configuration says is the window that must not exist.
//!
//! One `PrivilegedOp` either way, and only one — the feature spec's *"one elevation prompt, the only
//! one in normal day-to-day use"*. It is whole-state
//! ([`mixengine_proto::privileged::FirewallPlan`]), so it carries every shared site's
//! ports rather than this one's, and an unshare of the last shared site carries none at all.

use mixengine_core::sites::{self, Sharing};
use mixengine_proto::privileged::{FIREWALL_LABEL, FirewallPlan, PrivilegedOp};
use mixengine_proto::{Error, ErrorCode, SiteRef, SiteSharing, Timestamp};

use crate::error::ToWire as _;

impl super::Sites {
    /// `site.share` — let the local network reach one site.
    ///
    /// **Idempotent on the state that matters.** Sharing a site that is already shared on the same
    /// interface writes the same row, renders the same configuration and reissues nothing; what it
    /// does not do is skip the firewall plan, because the machine's rules are not something this
    /// daemon reads back and "already done" is the helper's answer to give.
    ///
    /// # Errors
    ///
    /// `not_found` for a site nothing answers to; whatever the platform says when this machine has
    /// no interface to share on, or more than one and none was named; and whatever writing the row,
    /// rendering the configuration or issuing the certificate reports.
    pub(crate) async fn share(
        &self,
        site: &SiteRef,
        interface: Option<&str>,
        now: Timestamp,
    ) -> Result<SiteSharing, Error> {
        let (record, _project) = self.expect(site).await?;

        let host = self.elevation.host();
        let found = host
            .network()
            .interfaces()
            .map_err(|error| refused(&error))?;

        // Never a guess — the T74 design, D5. The refusal carries the candidate list, which is what
        // a person reads before typing `--interface`.
        let chosen = mixengine_platform::choose_interface(&found, interface).map_err(|error| {
            refused(&error).with_hint("`mix site share <site> --interface <name>`")
        })?;

        let sharing = Sharing {
            interface: chosen.name.clone(),
            address: chosen.address,
            // The start of *this* share. Re-sharing an already-shared site on the same interface
            // keeps the original: T76 measures an expiry against it, and restarting the clock
            // because somebody typed the command twice would extend a share nobody extended.
            since: began(record.sharing.as_ref(), chosen.address, now),
        };

        let shared = sites::set_sharing(&self.store, record.id, Some(&sharing))
            .await
            .map_err(|error| error.to_wire())?;

        // Renders the second listener and reloads the running server; then the certificate gains
        // the address it will be asked for. Both before the rule.
        self.now_serves_what_it_declares().await?;
        self.certificates.issue(Some(shared.clone())).await?;
        self.wants_the_firewall().await?;

        self.answer(&sharing).await
    }

    /// `site.unshare` — take it back off the local network.
    ///
    /// **The rule goes first**, which is the opposite order to [`share`](Self::share) and is the
    /// same rule stated from the other end: the machine must never be more open than the
    /// configuration says it is.
    ///
    /// Unsharing a site that is not shared is not an error. It is the state the caller asked for,
    /// and the answer says what the site is now rather than what it was.
    ///
    /// # Errors
    ///
    /// `not_found` for a site nothing answers to, and whatever writing the row, rendering the
    /// configuration or reissuing the certificate reports.
    pub(crate) async fn unshare(&self, site: &SiteRef) -> Result<(), Error> {
        let (record, _project) = self.expect(site).await?;

        if record.sharing.is_none() {
            return Ok(());
        }

        let unshared = sites::set_sharing(&self.store, record.id, None)
            .await
            .map_err(|error| error.to_wire())?;

        // The rule first, then the listener it was protecting, then the certificate that named the
        // address. The row is already gone by now, so the plan this queues no longer carries this
        // site's ports.
        self.wants_the_firewall().await?;
        self.now_serves_what_it_declares().await?;
        self.certificates.issue(Some(unshared)).await?;

        Ok(())
    }

    /// Queue the one firewall operation this home now needs.
    ///
    /// **Whole state, computed from the rows** — the T74 design, D6. Every shared site's web ports
    /// go in one plan, so the queue holds one row for the question *what should this machine have
    /// open?* rather than one per share, and the answer that supersedes it is simply the next plan.
    /// A home with nothing shared queues the empty plan, which is the revoke.
    async fn wants_the_firewall(&self) -> Result<(), Error> {
        let records = sites::records(&self.store, None)
            .await
            .map_err(|error| error.to_wire())?;

        let shared: Vec<&mixengine_core::sites::SiteRecord> = records
            .iter()
            .filter(|record| record.sharing.is_some())
            .collect();

        // The TLS port only where a shared site is actually served over TLS. `front_end_tls_port`
        // answers what the front end's settings say, which is 443 on a home that has never issued a
        // certificate — and opening a port nothing listens on is exactly what "web ports only" is
        // supposed to rule out. Found by reading a real `netsh` rule that named 443 beside 8080 on a
        // machine where Caddy had bound one of them.
        let tls = shared
            .iter()
            .any(|record| self.certificates.serves_tls(record));

        let ports = ports(
            !shared.is_empty(),
            self.web_port().await?,
            match tls {
                true => self.services.front_end_tls_port().await,
                false => None,
            },
        );

        self.elevation
            .enqueue(&PrivilegedOp::FirewallApply {
                plan: FirewallPlan {
                    ports,
                    label: format!("{FIREWALL_LABEL}shared sites"),
                },
            })
            .await
    }

    /// What a share answers with.
    async fn answer(&self, sharing: &Sharing) -> Result<SiteSharing, Error> {
        Ok(SiteSharing {
            interface: sharing.interface.clone(),
            address: sharing.address.to_string(),
            url: sites::shared_url(sharing.address, self.web_port().await?),
            since: sharing.since,
        })
    }
}

/// A platform refusal, in the vocabulary the wire speaks.
///
/// `InvalidArgument` and not a failure: a machine with two networks up, or none, is a machine the
/// request has to say more about — and `flatten` keeps the operating system's own words, which
/// `Display` alone would cut off at the `#[source]`.
fn refused(error: &mixengine_platform::Error) -> Error {
    Error::new(ErrorCode::InvalidArgument, mixengine_proto::flatten(error))
}

/// When *this* share began.
///
/// Re-sharing an already-shared site on the same address keeps the original start: T76 measures an
/// expiry against it, and restarting the clock because somebody typed the command twice would extend
/// a share nobody extended. A different address is a different share.
fn began(already: Option<&Sharing>, address: std::net::Ipv4Addr, now: Timestamp) -> Timestamp {
    already
        .filter(|already| already.address == address)
        .map_or(now, |already| already.since)
}

/// Every port a home with `shared` sites should have open.
///
/// **The http port always, and the TLS port only where one is actually being served on** — the
/// caller decides the second by asking whether a shared site has a usable certificate, because a
/// home that declares HTTPS and has never issued one renders no TLS listener at all. Opening a port
/// nothing answers on is not dangerous, but it is wider than "web ports only" promises, and the
/// promise is the whole feature.
///
/// A home with nothing shared answers the empty list, which is the revoke.
fn ports(shared: bool, web: u16, tls: Option<u16>) -> Vec<u16> {
    if !shared {
        return Vec::new();
    }

    let mut ports = vec![web];
    ports.extend(tls);
    ports.sort_unstable();
    ports.dedup();

    ports
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sharing(address: [u8; 4], since: i64) -> Sharing {
        Sharing {
            interface: "Wi-Fi".to_owned(),
            address: address.into(),
            since: Timestamp(since),
        }
    }

    #[test]
    fn a_home_with_nothing_shared_asks_for_nothing_open() {
        assert!(ports(false, 80, Some(443)).is_empty());
    }

    #[test]
    fn a_shared_home_serving_tls_asks_for_both_web_ports() {
        assert_eq!(ports(true, 80, Some(443)), vec![80, 443]);
    }

    /// **A shared site with no certificate opens one port, not two.**
    ///
    /// The front end's settings name a TLS port whether or not anything is served on it, so a home
    /// that has never issued a certificate would otherwise have 443 opened for a listener that does
    /// not exist. Found by reading a real `netsh` rule: it named 443 beside 8080 on a machine where
    /// Caddy had bound only one of them.
    #[test]
    fn a_home_serving_no_tls_asks_for_one_port() {
        assert_eq!(ports(true, 80, None), vec![80]);
    }

    /// macOS binds 8080 and 8443 behind a redirect, and both numbers reach here unchanged.
    #[test]
    fn the_ports_are_whatever_this_home_answers_on() {
        assert_eq!(ports(true, 8080, Some(8443)), vec![8080, 8443]);
    }

    #[test]
    fn re_sharing_the_same_address_keeps_the_original_start() {
        let already = sharing([192, 168, 1, 10], 1_000);

        assert_eq!(
            began(Some(&already), [192, 168, 1, 10].into(), Timestamp(9_000)),
            Timestamp(1_000)
        );
    }

    #[test]
    fn moving_to_another_address_starts_a_new_share() {
        let already = sharing([192, 168, 1, 10], 1_000);

        assert_eq!(
            began(Some(&already), [10, 0, 0, 5].into(), Timestamp(9_000)),
            Timestamp(9_000)
        );
    }

    #[test]
    fn a_first_share_begins_now() {
        assert_eq!(
            began(None, [192, 168, 1, 10].into(), Timestamp(9_000)),
            Timestamp(9_000)
        );
    }
}
