//! What may be routed where, decided by the binary that will route it.
//!
//! **The helper validates the request itself rather than trusting the daemon** —
//! `.claude/architecture/security-model.md`, and the T45 design, D3 and D5. If the daemon is
//! compromised it *is* the attacker, so nothing the request asserts can be believed: not the TLDs,
//! not the port, not the length of the list.
//!
//! **And the one thing that would matter most is not in the request at all.** The nameserver
//! address, the Linux link's name and address, and the Windows registry key are compiled into this
//! binary, in `mixengine_platform::resolver`. An operation that accepted an address would be one
//! that let whoever owns the daemon point this machine's name resolution anywhere — with a valid
//! signature, through this binary, under the user's own Allow click. The request carries the two
//! things this binary cannot know and nothing else: which of the managed TLDs, and which port the
//! server is listening on.
//!
//! The table it reads is the compile-time constant shared through `mixengine-proto`. This binary is
//! excluded from auto-update, so its table can be older than the daemon's — and that is the correct
//! failure: a TLD a future build wires is refused here, loudly, at its own index.

use mixengine_platform::resolver::{self, Change};
use mixengine_proto::domains::{WIRED_TLDS, is_wired_tld};
use mixengine_proto::privileged::{OpOutcome, ResolverPlan, ResolverTarget};

/// The most TLDs one plan may name.
///
/// There are only so many, so a longer list is a request that cannot be describing anything real.
const LIMIT: usize = mixengine_proto::domains::MANAGED_TLDS.len();

/// Validate, route, and say what happened.
pub(crate) fn wire(plan: &ResolverPlan) -> OpOutcome {
    if let Some(reason) = refusal(plan) {
        return OpOutcome::Refused { reason };
    }

    outcome(resolver::apply(plan))
}

/// Unwire, and say what happened.
///
/// Nothing to validate: a target names no TLD and no port, and every artifact each variant removes
/// is a constant in `mixengine_platform::resolver`.
pub(crate) fn unwire(target: &ResolverTarget) -> OpOutcome {
    outcome(resolver::revoke(target))
}

/// One platform answer as the outcome the daemon reads.
fn outcome(change: mixengine_platform::Result<Change>) -> OpOutcome {
    match change {
        Ok(Change::Unchanged) => OpOutcome::AlreadyDone,
        Ok(Change::Written { detail }) => OpOutcome::Applied { detail },

        // What is wrong is on the machine and a person has to look at it, so the same request will
        // be refused again — which is exactly what `Refused` says. A plan meant for another system
        // is the same kind of answer: retrying will not change it.
        Err(
            error @ (mixengine_platform::Error::MalformedBlock { .. }
            | mixengine_platform::Error::UnsupportedPlatform { .. }),
        ) => OpOutcome::Refused {
            reason: error.to_string(),
        },

        // A held lock, a reload that would not run, an OS that said no. Nothing about the request
        // is wrong.
        // **`flatten` and not `to_string`.** `Error::Os` renders as "cannot <action>" and keeps the
        // operating system's own words as its `#[source]`, so `to_string` alone hands back a
        // sentence with the cause cut off — which is the half a person needs. `mix` already
        // flattens the same errors at its own boundary; this is that boundary for the helper.
        Err(error) => OpOutcome::Failed {
            message: mixengine_proto::flatten(&error),
        },
    }
}

/// Why this plan will not be applied, or [`None`].
fn refusal(plan: &ResolverPlan) -> Option<String> {
    let (tlds, port) = match plan {
        ResolverPlan::ResolverDirectory { tlds, port }
        | ResolverPlan::SystemdLink { tlds, port } => (tlds, Some(*port)),
        // Windows has nowhere to put a port, so its plan carries none and there is none to check.
        ResolverPlan::Nrpt { tlds } => (tlds, None),
    };

    if tlds.is_empty() {
        return Some(
            "a resolver plan that names no TLD would change nothing; removing a wiring is \
             resolver-revoke"
                .to_owned(),
        );
    }

    if tlds.len() > LIMIT {
        return Some(format!(
            "{} TLDs is more than the {LIMIT} that exist",
            tlds.len()
        ));
    }

    for (index, tld) in tlds.iter().enumerate() {
        if tlds[..index].contains(tld) {
            return Some(format!(
                "`{tld}` is named twice, and a plan that cannot say what it wants is not one to \
                 apply"
            ));
        }

        // Named before the general check so the refusal says *why*, which is the one a person is
        // most likely to meet and the one whose reason is least guessable — the T45 design, D9.
        if tld == "local" {
            return Some(
                "`local` is mDNS territory: routing it would send printer.local and every other \
                 Bonjour name on this machine's network to loopback. A site may use it; a resolver \
                 may not."
                    .to_owned(),
            );
        }

        if !is_wired_tld(tld) {
            return Some(format!(
                "`{tld}` is not one of the TLDs this helper may route ({})",
                WIRED_TLDS.join(", ")
            ));
        }
    }

    if port == Some(0) {
        return Some("port 0 is not a port a resolver can be sent to".to_owned());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(tlds: &[&str], port: u16) -> ResolverPlan {
        ResolverPlan::ResolverDirectory {
            tlds: tlds.iter().map(|tld| (*tld).to_owned()).collect(),
            port,
        }
    }

    /// D9, and the reason it lives here as well as in the planner: this is a rule about what may be
    /// done to a machine, and the binary that does it checks its own rules.
    #[test]
    fn local_is_refused_however_it_arrives() {
        let why = refusal(&directory(&["test", "local"], 53_535)).expect("a refusal");

        assert!(why.contains("local"), "{why}");
        assert!(why.contains("Bonjour") || why.contains("mDNS"), "{why}");
    }

    /// The table this binary compiles in, which may be older than the daemon's — the correct
    /// failure, loudly, rather than trusting a caller who says a TLD is fine.
    #[test]
    fn a_tld_outside_the_table_is_refused() {
        assert!(refusal(&directory(&["dev"], 53_535)).is_some());
        assert!(refusal(&directory(&["lc"], 53_535)).is_some());
        assert!(refusal(&directory(&["com"], 53_535)).is_some());
    }

    /// A port of zero would be a resolver pointed at nothing.
    #[test]
    fn a_port_of_zero_is_refused() {
        assert!(refusal(&directory(&["test"], 0)).is_some());
    }

    /// An empty plan is not "unwire everything" — that is a different operation, and confusing the
    /// two is how a machine gets silently unwired by a bug in a producer.
    #[test]
    fn an_empty_plan_is_refused() {
        assert!(refusal(&directory(&[], 53_535)).is_some());
        assert!(
            refusal(&ResolverPlan::Nrpt { tlds: Vec::new() })
                .is_some_and(|why| why.contains("resolver-revoke"))
        );
    }

    /// A list holding the same TLD twice would write one file twice and report two changes.
    #[test]
    fn a_duplicated_tld_is_refused() {
        assert!(refusal(&directory(&["test", "test"], 53_535)).is_some());
    }

    /// A plan longer than the table cannot be describing anything real.
    #[test]
    fn more_tlds_than_exist_is_refused() {
        let many = vec!["test"; LIMIT + 1];

        assert!(refusal(&directory(&many, 53_535)).is_some());
    }

    /// Without this the tests above all pass on a function that refuses everything.
    #[test]
    fn a_plan_naming_wired_tlds_and_a_real_port_is_accepted() {
        assert_eq!(
            refusal(&directory(&["test", "localhost", "internal"], 53_535)),
            None
        );
        assert_eq!(
            refusal(&ResolverPlan::SystemdLink {
                tlds: vec!["test".to_owned()],
                port: 53_535,
            }),
            None
        );
        assert_eq!(
            refusal(&ResolverPlan::Nrpt {
                tlds: vec!["test".to_owned()],
            }),
            None
        );
    }

    /// Every TLD the table says may be wired is one this helper accepts. Without it, a build could
    /// add a TLD to `WIRED_TLDS` and have it refused here by an omission nobody notices.
    #[test]
    fn every_wirable_tld_is_accepted() {
        for tld in WIRED_TLDS {
            assert_eq!(refusal(&directory(&[tld], 53_535)), None, "{tld}");
        }
    }
}
