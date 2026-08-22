//! What may be written into the hosts file, decided by the binary that will write it.
//!
//! **The helper validates the request itself rather than trusting the daemon** —
//! `.claude/architecture/security-model.md`, and the T41 design, D3. If the daemon is compromised it
//! *is* the attacker, so nothing the request asserts can be believed: not the address, not the
//! domain, not the size of the list. This module is the whole of that decision and is meant to be
//! read in one sitting.
//!
//! The table it reads is a compile-time constant shared through `mixengine-proto` (D4). Being handed
//! a list of permitted TLDs *in the request* would be the trust this rules out; sharing a constant
//! is not. This binary is excluded from auto-update, so its table can be older than the daemon's —
//! and that is the correct failure: a TLD a future build manages is refused here, loudly, at its own
//! index.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use mixengine_platform::hosts::{self, Change};
use mixengine_proto::domains::{MANAGED_TLDS, is_domain_syntax};
use mixengine_proto::privileged::{HostEntry, OpOutcome};

/// The two addresses a managed name may point at — the T41 design, D5.
///
/// Not `IpAddr::is_loopback`, which would accept `127.0.0.2`: nothing MixEngine does needs one, and
/// an address this file did not name is precisely the hosts-file hijack Defender has a heuristic for.
const PERMITTED: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

/// The most names one request may point at loopback.
const LIMIT: usize = 512;

/// Validate, apply, and say what happened.
pub(crate) fn apply(entries: &[HostEntry]) -> OpOutcome {
    if let Some(reason) = refusal(entries) {
        return OpOutcome::Refused { reason };
    }

    let file = hosts::path();

    match hosts::apply(&file, entries) {
        Ok(Change::Unchanged) => OpOutcome::AlreadyDone,
        Ok(Change::Written { entries: 0 }) => OpOutcome::Applied {
            detail: format!("removed MixEngine's block from {}", file.display()),
        },
        Ok(Change::Written { entries }) => OpOutcome::Applied {
            detail: format!(
                "wrote {entries} name{} into {}",
                if entries == 1 { "" } else { "s" },
                file.display()
            ),
        },
        // What is wrong is on the machine and a person has to look at it, so the same request will
        // be refused again — which is exactly what `Refused` says.
        Err(error @ mixengine_platform::Error::MalformedBlock { .. }) => OpOutcome::Refused {
            reason: error.to_string(),
        },
        // A held lock, or an OS that said no. Nothing about the request is wrong.
        Err(error) => OpOutcome::Failed {
            message: error.to_string(),
        },
    }
}

/// Why this request will not be applied, or [`None`].
fn refusal(entries: &[HostEntry]) -> Option<String> {
    if entries.len() > LIMIT {
        return Some(format!(
            "{} entries is more than the {LIMIT} this helper will write",
            entries.len()
        ));
    }

    for entry in entries {
        if !PERMITTED.contains(&entry.address) {
            return Some(format!(
                "{} is not a loopback address; only 127.0.0.1 and ::1 may be written",
                entry.address
            ));
        }

        if !is_domain_syntax(&entry.domain) {
            return Some(format!("`{}` is not a domain name", entry.domain));
        }

        let tld = entry.domain.rsplit('.').next().unwrap_or_default();

        if !MANAGED_TLDS.contains(&tld) {
            return Some(format!(
                "`{}` is outside the TLDs this helper manages ({})",
                entry.domain,
                MANAGED_TLDS.join(", ")
            ));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(address: &str, domain: &str) -> HostEntry {
        HostEntry {
            address: address.parse().expect("a literal address"),
            domain: domain.to_owned(),
        }
    }

    /// The whole of the policy, row by row. Each line is something a compromised daemon would ask
    /// for and this binary is the last thing between it and `/etc/hosts`.
    #[test]
    fn what_this_helper_will_not_write() {
        for (bad, what) in [
            (entry("8.8.8.8", "blog.test"), "loopback"),
            (entry("127.0.0.2", "blog.test"), "loopback"),
            (entry("127.0.0.1", "evil.com"), "manages"),
            (entry("127.0.0.1", "BLOG.TEST"), "domain"),
            (entry("127.0.0.1", "*.blog.test"), "domain"),
            (entry("127.0.0.1", ""), "domain"),
        ] {
            let reason = refusal(std::slice::from_ref(&bad))
                .unwrap_or_else(|| panic!("{bad:?} was accepted"));

            assert!(reason.contains(what), "{reason}");
        }
    }

    /// An unbounded list from a compromised daemon is a denial of service against every name lookup
    /// the machine makes, and it costs one comparison to make unreachable.
    #[test]
    fn more_entries_than_anybody_needs_is_refused() {
        let many: Vec<HostEntry> = (0..=LIMIT)
            .map(|index| entry("127.0.0.1", &format!("s{index}.test")))
            .collect();

        assert!(refusal(&many).is_some());
        assert!(refusal(&many[..LIMIT]).is_none());
    }

    /// A refusal test that never says yes proves only that the code refuses.
    #[test]
    fn what_this_helper_will_write() {
        let good = [
            entry("127.0.0.1", "blog.test"),
            entry("127.0.0.1", "api.blog.test"),
            entry("127.0.0.1", "x.localhost"),
            entry("127.0.0.1", "printer.local"),
            // Permitted so T43 can start emitting it without touching this audited binary — D5.
            entry("::1", "blog.test"),
        ];

        assert_eq!(refusal(&good), None);
        assert_eq!(refusal(&[]), None, "an empty list removes the block");
    }
}
