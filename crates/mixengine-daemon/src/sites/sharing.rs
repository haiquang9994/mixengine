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

        // **The name before the row** — the T75 design, D2. A refusal after the write would leave a
        // site shared under a name this home cannot advertise, which is a worse state than the one
        // the caller asked to avoid.
        let records = sites::records(&self.store, None)
            .await
            .map_err(|error| error.to_wire())?;

        if let Some(refusal) = collides(&records, &record) {
            return Err(refusal);
        }

        let sharing = Sharing {
            interface: chosen.name.clone(),
            address: chosen.address,
            // The start of *this* share. Re-sharing an already-shared site on the same interface
            // keeps the original: T76 measures an expiry against it, and restarting the clock
            // because somebody typed the command twice would extend a share nobody extended.
            since: began(record.sharing.as_ref(), chosen.address, now),
            until: None,
        };

        let shared = sites::set_sharing(&self.store, record.id, Some(&sharing))
            .await
            .map_err(|error| error.to_wire())?;

        // Renders the second listener and reloads the running server; then the certificate gains
        // the address it will be asked for. Both before the rule.
        self.now_serves_what_it_declares().await?;
        self.certificates.issue(Some(shared.clone())).await?;
        self.advertises_what_it_declares().await?;
        self.wants_the_firewall().await?;

        self.answer(&record, &sharing).await
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
        self.advertises_what_it_declares().await?;
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

    /// Advertise every shared site's name, and no other — roadmap task **T75**, the design's D4.
    ///
    /// **Whole state, like [`wants_the_firewall`](Self::wants_the_firewall)**, and called from the
    /// same places for the same reason: the question is *what should this home be advertising?*,
    /// and the answer that supersedes the last one is simply the next reconciliation. A home with
    /// nothing shared reconciles to nothing, which is the withdrawal.
    ///
    /// # Errors
    ///
    /// Whatever reading the rows or the front end's port reports. A responder that will not answer
    /// is **not** an error: the site is still shared by address, and `SiteSharing::advertised` is
    /// how that is said rather than a failed command.
    pub(crate) async fn advertises_what_it_declares(&self) -> Result<(), Error> {
        let records = sites::records(&self.store, None)
            .await
            .map_err(|error| error.to_wire())?;

        let port = self.web_port().await?;

        let wanted: Vec<crate::mdns::Advertisement> = records
            .iter()
            .filter_map(|record| {
                let sharing = record.sharing.as_ref()?;
                let primary = record.domains.first()?;

                Some(crate::mdns::Advertisement {
                    name: sites::shared_name(primary)?,
                    address: sharing.address,
                    interface: sharing.interface.clone(),
                    primary: primary.clone(),
                    port,
                })
            })
            .collect();

        self.mdns.advertises(&wanted);

        Ok(())
    }

    /// What a share answers with.
    async fn answer(
        &self,
        record: &sites::SiteRecord,
        sharing: &Sharing,
    ) -> Result<SiteSharing, Error> {
        let port = self.web_port().await?;
        let url = sites::shared_url(sharing.address, port);

        let name = record
            .domains
            .first()
            .and_then(|primary| sites::shared_name(primary));

        Ok(SiteSharing {
            interface: sharing.interface.clone(),
            address: sharing.address.to_string(),
            // Where the authority is downloaded from, which is the shared site's own URL and the
            // one path its block answers outside the site — roadmap task T75.
            ca_url: format!("{url}/__mixengine/ca.crt"),
            advertised: name
                .as_deref()
                .is_some_and(|name| self.mdns.advertising(name)),
            name,
            url,
            since: sharing.since,
        })
    }
}

/// The refusal for a name another shared site already holds, or [`None`] — the T75 design, D2.
///
/// **A refusal and not a rename.** The alternative is a suffix, and a URL that changes because
/// somebody else shared a site is a URL nobody can write down. What comes back names the site to
/// unshare, because that is the action available to whoever reads it — the same shape T74's D5 uses
/// for an ambiguous interface.
///
/// [`None`] for a site whose primary domain yields no label: it is shared by address, and there is
/// no name for anything to collide with.
fn collides(records: &[sites::SiteRecord], record: &sites::SiteRecord) -> Option<Error> {
    let name = record
        .domains
        .first()
        .and_then(|primary| sites::shared_name(primary))?;

    let taken = sites::name_taken(records, record.id, &name)?;

    Some(
        Error::new(
            ErrorCode::AlreadyExists,
            format!("`{taken}` is already shared as `{name}`"),
        )
        .with_hint(format!("`mix site unshare {taken}`")),
    )
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
            until: None,
        }
    }

    /// A record as `collides` reads one.
    fn a_site(id: i64, primary: &str, shared: bool) -> sites::SiteRecord {
        sites::SiteRecord {
            id,
            project_id: 1,
            doc_root: String::new(),
            kind: mixengine_proto::SiteKind::Static,
            https_enabled: false,
            state: mixengine_proto::SiteState::Enabled,
            domains: vec![primary.to_owned()],
            services: Vec::new(),
            sharing: shared.then(|| sharing([192, 168, 1, 10], 1)),
        }
    }

    /// **Two shared sites cannot hold one name** — the T75 design, D2, and the refusal names the
    /// site somebody has to unshare rather than leaving them to work it out.
    #[test]
    fn a_second_shared_site_with_the_same_label_is_refused_by_name() {
        let records = vec![a_site(1, "blog.test", true), a_site(2, "blog.dev", false)];

        let refusal = collides(&records, &records[1]).expect("a collision");

        assert_eq!(refusal.code, ErrorCode::AlreadyExists);
        assert!(
            refusal.message.contains("blog-mixengine.local"),
            "{refusal:?}"
        );
        assert!(refusal.message.contains("blog.test"), "{refusal:?}");
    }

    /// **Re-sharing a site does not collide with itself**, which is what makes `site.share` twice
    /// the same answer rather than a refusal on the second try.
    #[test]
    fn re_sharing_the_same_site_is_not_a_collision() {
        let records = vec![a_site(1, "blog.test", true)];

        assert!(collides(&records, &records[0]).is_none());
    }

    /// An unshared namesake is not on the network, so there is nothing to collide with.
    #[test]
    fn an_unshared_namesake_is_not_a_collision() {
        let records = vec![a_site(1, "blog.test", false), a_site(2, "blog.dev", false)];

        assert!(collides(&records, &records[1]).is_none());
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
