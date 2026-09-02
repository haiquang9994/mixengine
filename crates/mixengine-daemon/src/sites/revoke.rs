//! Ending a share nobody ended — roadmap task **T76**.
//!
//! Two things end a share on their own: the network it was shared on stopped being the network this
//! machine is on, and a `--for` deadline passed. Both take the road T74 built —
//! [`Sites::unshare`](super::Sites::unshare) — because the order that road moves in (the firewall
//! rule, then the listener, then the certificate) is a security property, and a background caller
//! with its own spelling of it is a second place for it to be wrong.
//!
//! **What is compared is the interface and its address, and nothing else** — the T76 design, D3.
//! Two networks that hand one adapter the same address are indistinguishable to this module; that
//! is written down as a limitation of this build in `.claude/features/lan-sharing.md` rather than
//! guessed at with an SSID this workspace would have to read three different ways.
//!
//! **A finding has to survive two passes before it is acted on** — D2, and the correction this
//! module is built around. A DHCP renewal, a wake from sleep or an adapter resetting can each make
//! one enumeration report an interface with no address; revoking on that single reading unshares
//! every site on the machine and reissues every certificate. **A false revoke costs more than a
//! late one**, which is also what makes a thirty-second period affordable.
//!
//! **The debounce covers the reading and not the decision.** An expiry is computed from a row and a
//! clock, with no flaky reading anywhere in it, so it acts on the first pass: waiting for a second
//! would delay something a person deliberately asked for and protect nothing.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use mixengine_core::sites;
use mixengine_platform::Interface;
use mixengine_proto::{SharingChange, Timestamp};

/// Why a share should end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ending {
    /// The length it was shared for has run out.
    Expired,

    /// This machine is not on the network the site was shared on.
    NetworkChanged {
        /// The address that was bound and written into the certificate.
        was: Ipv4Addr,

        /// What the interface holds now, or [`None`] where the interface is gone.
        now: Option<Ipv4Addr>,
    },
}

impl Ending {
    /// The same finding in the words the event stream speaks — the T76 design, D7.
    pub(crate) fn announced(&self) -> SharingChange {
        match self {
            Self::Expired => SharingChange::Expired {},
            Self::NetworkChanged { was, now } => SharingChange::NetworkChanged {
                was: was.to_string(),
                now: now.map(|now| now.to_string()),
            },
        }
    }

    /// The line for `daemon.log`, which is where somebody with only a CLI finds out why.
    pub(crate) fn because(&self) -> String {
        match self {
            Self::Expired => "the length it was shared for has run out".to_owned(),
            Self::NetworkChanged {
                was,
                now: Some(now),
            } => format!("the interface it was shared on holds {now} now, not {was}"),
            Self::NetworkChanged { was, now: None } => {
                format!("the interface it was shared on is gone, and it held {was}")
            }
        }
    }
}

/// Every shared site that looks wrong right now, by rowid.
///
/// **An answer about appearances and not a decision.** [`confirmed`] is what turns a repeated
/// appearance into an action, and keeping the two apart is what makes the debounce testable with no
/// clock, no database and no network.
pub(crate) fn looks_wrong(
    records: &[sites::SiteRecord],
    interfaces: &[Interface],
    now: Timestamp,
) -> BTreeMap<i64, Ending> {
    records
        .iter()
        .filter_map(|record| {
            let ending = ending(record.sharing.as_ref()?, interfaces, now)?;
            Some((record.id, ending))
        })
        .collect()
}

/// What one share looks like against this machine.
///
/// **The deadline is asked about first**, so a share that both expired and moved is reported as
/// expired: that is the reason its owner set, and it is the one whose sentence is worth reading.
fn ending(sharing: &sites::Sharing, interfaces: &[Interface], now: Timestamp) -> Option<Ending> {
    if sharing.until.is_some_and(|until| now.0 >= until.0) {
        return Some(Ending::Expired);
    }

    let held = interfaces
        .iter()
        .find(|interface| interface.name == sharing.interface)
        .map(|interface| interface.address);

    match held {
        Some(address) if address == sharing.address => None,
        now => Some(Ending::NetworkChanged {
            was: sharing.address,
            now,
        }),
    }
}

/// What has looked wrong twice running, and `previous` updated to this pass — the T76 design, D2.
///
/// The shape [`crate::certs::renewal`]'s `newly` established, turned around: there a set decides
/// what to *announce* once, here it decides what to act on only after a second reading agrees.
///
/// `previous` lives in the loop rather than in the database, because it is a property of this
/// process's last reading and not of the home — a daemon restarted mid-change simply takes one more
/// period to make up its mind.
///
/// [`Ending::Expired`] passes straight through: there is no reading in it to be wrong about.
pub(crate) fn confirmed(
    previous: &mut BTreeMap<i64, Ending>,
    seen: BTreeMap<i64, Ending>,
) -> BTreeMap<i64, Ending> {
    let acting = seen
        .iter()
        .filter(|(site, ending)| **ending == Ending::Expired || previous.get(site) == Some(*ending))
        .map(|(site, ending)| (*site, ending.clone()))
        .collect();

    *previous = seen;

    acting
}

/// End the shares that should end, every `every`, until `shutdown` — roadmap task **T76**.
///
/// **The first tick is thrown away**, as [`crate::certs::renewal::start`] throws its away: a daemon
/// that has just started has already reconciled its shares, and keeping it would make every start do
/// the same work twice. **Nothing catches up**, for that module's reason too — a machine suspended
/// over a weekend counts none of it, so a tick can arrive late, and a pass that finds nothing due
/// does nothing.
///
/// The loop never holds [`Sites`](super::Sites)' sharing lock: it reads the rows without it and
/// takes it only inside `unshare`. Stated because the failure it prevents is a hang rather than an
/// error.
pub(crate) fn start(
    sites: std::sync::Arc<super::Sites>,
    host: std::sync::Arc<dyn mixengine_platform::Host>,
    every: std::time::Duration,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        let mut previous: BTreeMap<i64, Ending> = BTreeMap::new();
        let mut ticker = tokio::time::interval(every);
        ticker.tick().await;

        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = ticker.tick() => {}
            }

            if let Err(error) = pass(&sites, host.as_ref(), &mut previous).await {
                // Debug and not warn: a home whose sites cannot be read has larger problems, all of
                // which are reported elsewhere, and this line would otherwise repeat every period.
                tracing::debug!(?error, "this home's shares could not be checked");
            }
        }
    });
}

/// One pass: look, confirm, and end what two readings agreed about.
///
/// **An enumeration that fails is not a network change** — the T76 design, D2. A machine that cannot
/// be asked about its own interfaces has not said that its network moved, so the pass keeps what it
/// believed and does nothing. That is a decision rather than a failure to make one, which is why it
/// is `Ok` here and not an error.
///
/// # Errors
///
/// Whatever reading the site rows reports.
async fn pass(
    sites: &super::Sites,
    host: &dyn mixengine_platform::Host,
    previous: &mut BTreeMap<i64, Ending>,
) -> Result<(), mixengine_proto::Error> {
    let records = sites.records().await?;

    let interfaces = match host.network().interfaces() {
        Ok(interfaces) => interfaces,
        Err(error) => {
            tracing::debug!(%error, "this machine's interfaces could not be read, so no share was ended");
            return Ok(());
        }
    };

    let now = Timestamp::from_system_time(std::time::SystemTime::now());
    let acting = confirmed(previous, looks_wrong(&records, &interfaces, now));

    for (id, ending) in acting {
        let Some(record) = records.iter().find(|record| record.id == id) else {
            continue;
        };
        let Some(domain) = record.domains.first().cloned() else {
            continue;
        };

        // **The road a person's `mix site unshare` takes** — D4 — so the ordering that keeps this
        // machine from ever being more open than its configuration says has one spelling.
        match sites
            .unshare(&mixengine_proto::SiteRef::Domain(domain.clone()))
            .await
        {
            Ok(()) => {
                tracing::info!(
                    %domain,
                    because = %ending.because(),
                    "this site is no longer shared on the local network"
                );
                sites.announce(record, None, ending.announced());
            }

            // Left in `previous`, so the next pass finds the same thing and tries again — a
            // certificate that could not be written is a reason to retry rather than to forget.
            Err(error) => tracing::warn!(
                %domain,
                ?error,
                because = %ending.because(),
                "this site should no longer be shared and could not be unshared"
            ),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(id: i64, interface: &str, address: [u8; 4], until: Option<i64>) -> sites::SiteRecord {
        sites::SiteRecord {
            id,
            owner: mixengine_core::sites::SiteOwner::Project(1),
            doc_root: String::new(),
            kind: mixengine_proto::SiteKind::Static,
            https_enabled: false,
            state: mixengine_proto::SiteState::Enabled,
            domains: vec![format!("site{id}.test")],
            services: Vec::new(),
            sharing: Some(sites::Sharing {
                interface: interface.to_owned(),
                address: address.into(),
                since: Timestamp(1_000),
                until: until.map(Timestamp),
            }),
        }
    }

    fn up(name: &str, address: [u8; 4]) -> Interface {
        Interface {
            name: name.to_owned(),
            address: address.into(),
            loopback: false,
        }
    }

    fn moved() -> BTreeMap<i64, Ending> {
        BTreeMap::from([(
            1,
            Ending::NetworkChanged {
                was: [192, 168, 1, 10].into(),
                now: None,
            },
        )])
    }

    #[test]
    fn a_share_on_an_interface_that_still_holds_its_address_is_left_alone() {
        let records = vec![shared(1, "Wi-Fi", [192, 168, 1, 10], None)];
        let interfaces = vec![up("Wi-Fi", [192, 168, 1, 10])];

        assert!(looks_wrong(&records, &interfaces, Timestamp(2_000)).is_empty());
    }

    /// The ordinary roaming case: the same adapter, a new network's DHCP lease.
    #[test]
    fn an_address_that_has_changed_ends_the_share_and_names_both() {
        let records = vec![shared(1, "Wi-Fi", [192, 168, 1, 10], None)];
        let interfaces = vec![up("Wi-Fi", [10, 0, 0, 4])];

        assert_eq!(
            looks_wrong(&records, &interfaces, Timestamp(2_000)).get(&1),
            Some(&Ending::NetworkChanged {
                was: [192, 168, 1, 10].into(),
                now: Some([10, 0, 0, 4].into()),
            })
        );
    }

    /// Wi-Fi switched off: the adapter carries no address, so `if-addrs` does not report it at all.
    #[test]
    fn an_interface_that_is_gone_ends_the_share_with_no_second_address() {
        let records = vec![shared(1, "Wi-Fi", [192, 168, 1, 10], None)];

        assert_eq!(
            looks_wrong(&records, &[], Timestamp(2_000)).get(&1),
            Some(&Ending::NetworkChanged {
                was: [192, 168, 1, 10].into(),
                now: None,
            })
        );
    }

    #[test]
    fn a_deadline_that_has_passed_ends_the_share() {
        let records = vec![shared(1, "Wi-Fi", [192, 168, 1, 10], Some(5_000))];
        let interfaces = vec![up("Wi-Fi", [192, 168, 1, 10])];

        assert_eq!(
            looks_wrong(&records, &interfaces, Timestamp(5_000)).get(&1),
            Some(&Ending::Expired),
            "the deadline is the instant it ends at, not the instant after"
        );
        assert!(looks_wrong(&records, &interfaces, Timestamp(4_999)).is_empty());
    }

    /// **An expiry outranks a network change**, because it is the one somebody asked for and the one
    /// whose sentence is worth reading: a share with thirty seconds left whose laptop moved has
    /// ended for the reason its owner set.
    #[test]
    fn a_deadline_is_reported_ahead_of_a_network_change() {
        let records = vec![shared(1, "Wi-Fi", [192, 168, 1, 10], Some(5_000))];

        assert_eq!(
            looks_wrong(&records, &[], Timestamp(6_000)).get(&1),
            Some(&Ending::Expired)
        );
    }

    #[test]
    fn a_site_that_is_not_shared_is_not_examined() {
        let mut record = shared(1, "Wi-Fi", [192, 168, 1, 10], None);
        record.sharing = None;

        assert!(looks_wrong(&[record], &[], Timestamp(9_000)).is_empty());
    }

    /// **D2, the whole of it.** One reading revokes nothing; the same reading twice revokes.
    #[test]
    fn a_network_change_has_to_survive_two_passes() {
        let mut previous = BTreeMap::new();

        assert!(
            confirmed(&mut previous, moved()).is_empty(),
            "a single reading is a DHCP renewal until a second one agrees with it"
        );
        assert_eq!(confirmed(&mut previous, moved()).len(), 1);
    }

    /// A reading that recovers takes the suspicion with it — the wake-from-sleep case.
    #[test]
    fn a_reading_that_recovers_confirms_nothing() {
        let mut previous = BTreeMap::new();

        assert!(confirmed(&mut previous, moved()).is_empty());
        assert!(confirmed(&mut previous, BTreeMap::new()).is_empty());
        assert!(
            confirmed(&mut previous, moved()).is_empty(),
            "the count starts again, so one blip either side of a good reading is not two"
        );
    }

    /// **A change of reason is not a confirmation.** An interface that vanished and then came back
    /// on another address is two different findings, and acting on the pair would be acting on a
    /// reading nothing has agreed with.
    #[test]
    fn two_different_findings_do_not_confirm_each_other() {
        let mut previous = BTreeMap::new();

        confirmed(&mut previous, moved());

        assert!(
            confirmed(
                &mut previous,
                BTreeMap::from([(
                    1,
                    Ending::NetworkChanged {
                        was: [192, 168, 1, 10].into(),
                        now: Some([10, 0, 0, 4].into()),
                    },
                )]),
            )
            .is_empty()
        );
    }

    /// **An expiry is not debounced**, and this pins the distinction: a deadline is computed from a
    /// row and a clock, with no flaky reading in it, so a second pass would double the latency of
    /// something a person deliberately asked for and protect nothing.
    #[test]
    fn a_deadline_acts_on_the_first_pass() {
        let mut previous = BTreeMap::new();

        assert_eq!(
            confirmed(&mut previous, BTreeMap::from([(1, Ending::Expired)])).len(),
            1
        );
    }

    /// What a client is told, and what the log says. Both are built from one value, so a reason
    /// rendered two ways cannot become two reasons.
    #[test]
    fn a_finding_says_the_same_thing_to_a_client_and_to_the_log() {
        let gone = Ending::NetworkChanged {
            was: [192, 168, 1, 10].into(),
            now: None,
        };

        assert_eq!(
            gone.announced(),
            SharingChange::NetworkChanged {
                was: "192.168.1.10".to_owned(),
                now: None,
            }
        );
        assert!(
            gone.because().contains("192.168.1.10"),
            "{}",
            gone.because()
        );

        assert_eq!(Ending::Expired.announced(), SharingChange::Expired {});
    }
}
