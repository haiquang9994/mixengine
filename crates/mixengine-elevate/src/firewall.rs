//! What ports may be opened, decided by the binary that will open them.
//!
//! **The helper validates the request itself rather than trusting the daemon** —
//! `.claude/architecture/security-model.md`, and the T74 design, D7. If the daemon is compromised
//! it *is* the attacker, so nothing the request asserts can be believed: not the ports, not the
//! label, not the length of the list.
//!
//! **And this one cannot check the question it would most like to ask.** The resolver's helper can
//! compare a TLD against a table compiled into itself; "is this a web port" has no such table,
//! because a site's HTTP port is a column in a database this binary must never read. So it refuses
//! what is provably not a web port instead, and that is enough to hold the rule the feature exists
//! to keep: *databases, caches, Mailpit and the daemon API are never exposed*. A plan naming 3306
//! is refused here even if every layer above it agreed to send one.

use mixengine_platform::firewall::Applied;
use mixengine_proto::privileged::{FIREWALL_LABEL, FirewallPlan, OpOutcome};

/// The most ports one plan may name.
///
/// A site answers on two, and a plan carries every shared site's ports at once, so this bounds
/// sharing at four sites rather than bounding anything a person would notice. A longer list is a
/// request that cannot be describing anything real.
const LIMIT: usize = 8;

/// The ports MixEngine's own non-web services use, which may never be opened to a network.
///
/// MariaDB and MySQL, PostgreSQL, Redis, Memcached, and the two Mailpit answers on. Written here as
/// a constant rather than derived from anything: this list is a security rule, and a rule that is
/// computed can be computed differently.
const NEVER: &[u16] = &[3306, 5432, 6379, 11211, 1025, 8025];

/// Validate, apply, and say what happened.
pub(crate) fn apply(plan: &FirewallPlan) -> OpOutcome {
    if let Some(reason) = refusal(plan) {
        return OpOutcome::Refused { reason };
    }

    outcome(mixengine_platform::firewall::apply(plan))
}

/// What the machine did, in the vocabulary the response speaks.
///
/// [`Applied::Unmanaged`] survives the crossing as its own outcome rather than collapsing into
/// success — the T74 design, D8. It is the whole reason [`OpOutcome::Unmanaged`] exists: two of the
/// three systems reach it in their ordinary configuration, and a user told "applied" on a machine
/// where nothing was applied has been told the one thing that will waste their afternoon.
fn outcome(applied: mixengine_platform::Result<Applied>) -> OpOutcome {
    match applied {
        Ok(Applied::Unchanged) => OpOutcome::AlreadyDone,
        Ok(Applied::Written { detail }) => OpOutcome::Applied { detail },
        Ok(Applied::Unmanaged { reason, manual }) => OpOutcome::Unmanaged { reason, manual },

        // The tool ran and refused, or could not be run. Nothing about the request is wrong, so a
        // retry is not obviously pointless. `flatten` and not `to_string` for `resolver`'s reason:
        // `Error::Os` keeps the operating system's own words as its `#[source]`.
        Err(error) => OpOutcome::Failed {
            message: mixengine_proto::flatten(&error),
        },
    }
}

/// Which rule this plan breaks, or [`None`] where it breaks none.
///
/// Every rule here is one this binary can check alone, with no database and no network: that is the
/// whole design of it.
fn refusal(plan: &FirewallPlan) -> Option<String> {
    if !plan.label.starts_with(FIREWALL_LABEL) {
        return Some(format!(
            "a firewall rule MixEngine writes is named {FIREWALL_LABEL:?}…, and this plan asked for \
             {:?} — a rule under any other name would never be recognised as ours, and never \
             cleaned up",
            plan.label
        ));
    }

    if plan.ports.len() > LIMIT {
        return Some(format!(
            "a firewall plan may name at most {LIMIT} ports and this one names {}",
            plan.ports.len()
        ));
    }

    for &port in &plan.ports {
        if port == 0 {
            return Some("0 is not a port".to_owned());
        }

        if NEVER.contains(&port) {
            return Some(format!(
                "{port} is a port one of MixEngine's own databases or caches answers on, and \
                 sharing exposes web ports only"
            ));
        }

        if port < 1024 && port != 80 && port != 443 {
            return Some(format!(
                "{port} is a privileged port that is not 80 or 443, and sharing exposes web ports \
                 only"
            ));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(ports: &[u16]) -> FirewallPlan {
        FirewallPlan {
            ports: ports.to_vec(),
            label: format!("{FIREWALL_LABEL}blog"),
        }
    }

    #[test]
    fn a_database_port_is_refused_at_the_last_gate() {
        for port in [3306u16, 5432, 6379, 11211, 1025, 8025] {
            let refused = refusal(&plan(&[port]));
            assert!(refused.is_some(), "port {port} was not refused");
        }
    }

    #[test]
    fn a_privileged_port_other_than_the_two_web_ones_is_refused() {
        assert!(refusal(&plan(&[22])).is_some());
        assert!(refusal(&plan(&[80])).is_none());
        assert!(refusal(&plan(&[443])).is_none());
    }

    /// The ports a front end actually binds on macOS, where 80 and 443 are redirected — so the
    /// machine that needs this most must not be the one refused.
    #[test]
    fn the_high_ports_a_redirected_front_end_binds_are_allowed() {
        assert!(refusal(&plan(&[8080, 8443])).is_none());
    }

    #[test]
    fn a_label_without_the_fixed_prefix_is_refused() {
        let wrong = FirewallPlan {
            ports: vec![8080],
            label: "something else".to_owned(),
        };

        assert!(refusal(&wrong).is_some());
    }

    #[test]
    fn a_list_longer_than_the_limit_is_refused() {
        let many: Vec<u16> = (9000..9000 + u16::try_from(LIMIT).expect("small") + 1).collect();

        assert!(refusal(&plan(&many)).is_some());
    }

    #[test]
    fn an_empty_plan_is_allowed_because_that_is_how_sharing_is_revoked() {
        assert!(refusal(&plan(&[])).is_none());
    }

    #[test]
    fn port_zero_is_refused() {
        assert!(refusal(&plan(&[0])).is_some());
    }
}
