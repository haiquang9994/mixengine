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
use mixengine_proto::{Error, ErrorCode, SharingChange, SiteRef, SiteSharing, Timestamp};

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
        for_seconds: Option<u64>,
        now: Timestamp,
    ) -> Result<SiteSharing, Error> {
        // **Held for the whole of the share, reconciliation included** — the T76 design, D9. What
        // this guards is not the row but the answer to *what should this machine have open?*, and
        // since T76 that question is asked by a clock as well as by a person.
        let _sharing = self.sharing.lock().await;

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

        // The start of *this* share. Re-sharing an already-shared site on the same interface keeps
        // the original: the expiry below is measured against it, and restarting the clock because
        // somebody typed the command twice would extend a share nobody extended.
        let since = began(record.sharing.as_ref(), chosen.address, now);

        // **Before the write, like the name collision above it and for the same reason.** A refusal
        // after the row is written would leave a site shared under a deadline this home has already
        // passed.
        let until = ends(
            record.sharing.as_ref(),
            since,
            for_seconds,
            chosen.address,
            now,
        )?;

        let sharing = Sharing {
            interface: chosen.name.clone(),
            address: chosen.address,
            since,
            until,
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

        let answer = self.answer(&record, &sharing).await?;

        self.announce(&record, Some(answer.clone()), SharingChange::Requested {});

        Ok(answer)
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
        // D9 again, from the other end. Nothing inside this method calls `share`, and the watcher
        // does not hold the lock while it calls this — a second acquisition here would deadlock a
        // background task rather than fail it, which is why both facts are stated rather than left
        // to a reader to verify.
        let _sharing = self.sharing.lock().await;

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

        self.announce(&record, None, SharingChange::Requested {});

        Ok(())
    }

    /// Say on the event stream what this home is now sharing — the T76 design, D7.
    ///
    /// **Best-effort and never a failure**, which is the whole contract of
    /// [`mixengine_proto::DaemonEvent`]: a share that happened and was not announced is still a
    /// share, and a client that missed the event finds out from `site.show`. That is why this
    /// returns nothing and why every caller places it after the work rather than inside it.
    ///
    /// A site with no domains announces nothing. It cannot be shared — the URL, the name and the
    /// certificate are all built from the primary domain — so there is no state for this to carry.
    pub(crate) fn announce(
        &self,
        record: &sites::SiteRecord,
        sharing: Option<SiteSharing>,
        because: SharingChange,
    ) {
        let Some(domain) = record.domains.first() else {
            return;
        };

        self.events
            .publish(mixengine_proto::DaemonEvent::SiteSharingChanged {
                domain: domain.clone(),
                sharing,
                because,
            });
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
            until: sharing.until,
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

/// When *this* share ends, or [`None`] for one that does not — the T76 design, D6.
///
/// **The deadline is measured from the start, and one already in the past is a refusal.** Both
/// halves come from [`began`] above: a repeated share keeps its start, so `--for` names an instant
/// rather than an interval from now — and the instant it names can therefore already have gone.
/// Answering that with a share means answering with a URL and a QR code for something the next pass
/// unshares, which is worse than a sentence saying so.
///
/// No `--for` at all keeps whatever this share already carried, on the same rule: a command typed
/// twice changes nothing, neither restarting the clock nor removing the alarm. A different address
/// is a different share and inherits neither.
///
/// # Errors
///
/// `InvalidArgument` for a deadline that has already passed.
fn ends(
    already: Option<&Sharing>,
    since: Timestamp,
    for_seconds: Option<u64>,
    address: std::net::Ipv4Addr,
    now: Timestamp,
) -> Result<Option<Timestamp>, Error> {
    let Some(seconds) = for_seconds else {
        return Ok(already
            .filter(|already| already.address == address)
            .and_then(|already| already.until));
    };

    // Saturating rather than checked: a number of seconds large enough to overflow a millisecond
    // timestamp is a share that never ends, which is what the caller asked for as nearly as this
    // column can say it.
    let millis = i64::try_from(seconds.saturating_mul(1_000)).unwrap_or(i64::MAX);
    let until = Timestamp(since.0.saturating_add(millis));

    if until.0 <= now.0 {
        return Err(Error::new(
            ErrorCode::InvalidArgument,
            format!(
                "this site has been shared for {}, which is longer than the {} asked for — a share                  measured from when it began has already ended",
                spelled(now.0.saturating_sub(since.0)),
                spelled(millis),
            ),
        )
        .with_hint("unshare it first, then share it again with `--for`"));
    }

    Ok(Some(until))
}

/// A number of milliseconds, in the words a person would use.
///
/// Coarse on purpose: it appears in one refusal, where the point is *longer than what you asked
/// for* rather than a duration somebody is going to do arithmetic on.
fn spelled(millis: i64) -> String {
    let seconds = millis / 1_000;

    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
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

    /// **A `--for` that has already run out is refused rather than honoured** — the T76 design, D6.
    ///
    /// The deadline is measured from `shared_since`, which a repeated share deliberately preserves,
    /// so `--for 1m` on a site shared an hour ago names an instant in the past. Honouring it would
    /// print a URL and a QR code for something the next pass unshares.
    #[test]
    fn a_deadline_that_has_already_passed_is_refused() {
        let already = sharing([192, 168, 1, 10], 1_000);

        let refusal = ends(
            Some(&already),
            Timestamp(1_000),
            Some(60),
            [192, 168, 1, 10].into(),
            Timestamp(3_600_000),
        )
        .expect_err("a deadline in the past");

        assert_eq!(refusal.code, ErrorCode::InvalidArgument);
        assert!(refusal.hint.is_some(), "{refusal:?}");
    }

    #[test]
    fn a_deadline_is_measured_from_the_start_of_the_share() {
        let already = sharing([192, 168, 1, 10], 1_000);

        assert_eq!(
            ends(
                Some(&already),
                Timestamp(1_000),
                Some(7_200),
                [192, 168, 1, 10].into(),
                Timestamp(2_000)
            )
            .expect("a deadline"),
            Some(Timestamp(7_201_000))
        );
    }

    /// **Sharing again without `--for` changes nothing**, which is `began`'s rule applied to the
    /// other column: a repeated command neither restarts the clock nor removes the alarm.
    #[test]
    fn re_sharing_without_a_for_keeps_the_deadline_it_had() {
        let already = Sharing {
            until: Some(Timestamp(9_000)),
            ..sharing([192, 168, 1, 10], 1_000)
        };

        assert_eq!(
            ends(
                Some(&already),
                Timestamp(1_000),
                None,
                [192, 168, 1, 10].into(),
                Timestamp(2_000)
            )
            .expect("the deadline it had"),
            Some(Timestamp(9_000))
        );
    }

    /// A different address is a different share, so it inherits neither the start nor the deadline.
    #[test]
    fn moving_to_another_address_drops_the_deadline_with_the_start() {
        let already = Sharing {
            until: Some(Timestamp(9_000)),
            ..sharing([192, 168, 1, 10], 1_000)
        };

        assert_eq!(
            ends(
                Some(&already),
                Timestamp(5_000),
                None,
                [10, 0, 0, 5].into(),
                Timestamp(5_000)
            )
            .expect("no deadline"),
            None
        );
    }

    #[test]
    fn a_share_with_no_for_has_no_deadline() {
        assert_eq!(
            ends(
                None,
                Timestamp(1_000),
                None,
                [192, 168, 1, 10].into(),
                Timestamp(1_000)
            )
            .expect("no deadline"),
            None
        );
    }

    #[test]
    fn a_length_of_time_is_spelled_in_the_largest_unit_that_fits() {
        assert_eq!(spelled(30_000), "30s");
        assert_eq!(spelled(90_000), "1m");
        assert_eq!(spelled(7_200_000), "2h");
        assert_eq!(spelled(172_800_000), "2d");
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
