//! Which of this machine's network interfaces a site could be shared on.

use std::net::Ipv4Addr;

use crate::{Error, Result};

/// One interface this machine could answer on, as the OS names it.
///
/// **IPv4 only** — the T74 design, D4. An interface with a v6 address and no v4 one is not a
/// candidate here, and saying so is the whole of the filtering: a second family would double the
/// certificate SAN, the URL and the QR code for a case no acceptance criterion asks for, and the
/// column that stores the choice would stop holding one answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    /// The OS's own name for it — `Wi-Fi`, `en0`, `wlp3s0`. What a person types after
    /// `--interface`, so it is never normalised.
    pub name: String,

    /// The address it currently holds. This is what gets bound, certified and put in the URL, so
    /// the three cannot disagree by construction.
    pub address: Ipv4Addr,

    /// Whether this is the loopback interface.
    ///
    /// Kept rather than filtered at the source: [`NetworkInfo::interfaces`] answers what the machine
    /// has, and the decision about what may be *shared* on belongs to [`choose`], where it is
    /// visible and testable.
    pub loopback: bool,
}

/// What interfaces this machine has.
///
/// **Reads only.** Nothing here changes a machine's networking; sharing binds a listener the daemon
/// already owns and asks `mixengine-elevate` for a firewall rule. See
/// [`FirewallPlan`](mixengine_proto::privileged::FirewallPlan).
pub trait NetworkInfo: std::fmt::Debug + Send + Sync {
    /// Every interface that is up and carries an IPv4 address, loopback included.
    ///
    /// # Errors
    ///
    /// [`Error::Os`] where the OS would not enumerate them.
    fn interfaces(&self) -> Result<Vec<Interface>>;
}

/// The interface to share on, or a refusal that names the alternatives.
///
/// **It never guesses** — the T74 design, D5. One candidate is an answer; two are a question, and a
/// machine that picked the wrong one would put a site on a network the user did not mean to be on.
/// The refusal lists every candidate with its address, because that list *is* the remedy: it is
/// what the user reads before typing `--interface`.
///
/// # Errors
///
/// [`Error::NoInterface`] when nothing is shareable, when more than one is and `asked` is [`None`],
/// or when `asked` names one this machine does not have.
pub fn choose(found: &[Interface], asked: Option<&str>) -> Result<Interface> {
    let candidates: Vec<&Interface> = found.iter().filter(|found| !found.loopback).collect();

    if let Some(name) = asked {
        return candidates
            .iter()
            .find(|candidate| candidate.name == name)
            .map(|candidate| (*candidate).clone())
            .ok_or_else(|| Error::NoInterface {
                reason: format!(
                    "this machine has no shareable interface called {name} — it has {}",
                    listing(&candidates)
                ),
            });
    }

    match candidates.as_slice() {
        [] => Err(Error::NoInterface {
            reason: "no network interface on this machine is up with an IPv4 address, so there is \
                     nothing a phone could reach this site at"
                .to_owned(),
        }),
        [only] => Ok((*only).clone()),
        many => Err(Error::NoInterface {
            reason: format!(
                "this machine has more than one network to share on — name the one you mean with \
                 --interface: {}",
                listing(many)
            ),
        }),
    }
}

/// `Wi-Fi (192.168.1.10), Ethernet (10.0.0.5)`, or `none` for an empty list.
fn listing(candidates: &[&Interface]) -> String {
    if candidates.is_empty() {
        return "none".to_owned();
    }

    candidates
        .iter()
        .map(|candidate| format!("{} ({})", candidate.name, candidate.address))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, address: [u8; 4]) -> Interface {
        Interface {
            name: name.to_owned(),
            address: address.into(),
            loopback: false,
        }
    }

    #[test]
    fn one_candidate_needs_no_asking() {
        let found = vec![iface("Wi-Fi", [192, 168, 1, 10])];

        assert_eq!(choose(&found, None).expect("a choice").name, "Wi-Fi");
    }

    #[test]
    fn two_candidates_refuse_and_name_both() {
        let found = vec![
            iface("Wi-Fi", [192, 168, 1, 10]),
            iface("Ethernet", [10, 0, 0, 5]),
        ];

        let error = choose(&found, None).expect_err("a refusal");
        let rendered = error.to_string();

        assert!(rendered.contains("Wi-Fi (192.168.1.10)"), "{rendered}");
        assert!(rendered.contains("Ethernet (10.0.0.5)"), "{rendered}");
        assert!(rendered.contains("--interface"), "{rendered}");
    }

    #[test]
    fn a_named_interface_is_taken_from_the_two() {
        let found = vec![
            iface("Wi-Fi", [192, 168, 1, 10]),
            iface("Ethernet", [10, 0, 0, 5]),
        ];

        assert_eq!(
            choose(&found, Some("Ethernet")).expect("a choice").address,
            Ipv4Addr::new(10, 0, 0, 5)
        );
    }

    #[test]
    fn a_name_this_machine_does_not_have_is_refused() {
        let found = vec![iface("Wi-Fi", [192, 168, 1, 10])];

        let rendered = choose(&found, Some("eth7"))
            .expect_err("a refusal")
            .to_string();

        assert!(rendered.contains("eth7"), "{rendered}");
        assert!(rendered.contains("Wi-Fi"), "{rendered}");
    }

    #[test]
    fn no_candidate_at_all_is_refused_rather_than_defaulted() {
        assert!(choose(&[], None).is_err());
    }

    /// Loopback is enumerated and is never a candidate — including when it is the only interface
    /// there is, which is a laptop with the Wi-Fi switched off rather than a machine to share on.
    #[test]
    fn loopback_is_not_a_network_to_share_on() {
        let found = vec![Interface {
            name: "lo".to_owned(),
            address: Ipv4Addr::LOCALHOST,
            loopback: true,
        }];

        assert!(choose(&found, None).is_err());
        assert!(choose(&found, Some("lo")).is_err());
    }
}
