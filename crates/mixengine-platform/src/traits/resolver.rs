//! Whether anything on this machine sends a managed TLD to MixEngine's own DNS server.

use mixengine_proto::domains::is_wired_tld;
use mixengine_proto::privileged::{ResolverPlan, ResolverTarget};

use crate::Result;

/// How this machine can be made to route one TLD to a nameserver of our choosing.
///
/// **Unlike [`PortAccessMethod`](crate::PortAccessMethod), this is a question asked at runtime
/// rather than a property of the operating system** — the T45 design, D2. macOS is always a
/// resolver directory and Windows is always NRPT, but Linux is [`SystemdLink`](Self::SystemdLink)
/// only where `systemd-resolved` is running and `systemd-networkd` manages links, and a machine
/// with neither has no scoped mechanism at all. That is why [`ResolverConfig::method`] returns a
/// `Result` where `PortAccess`' equivalent is a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverMethod {
    /// macOS: a file per TLD under `/etc/resolver`.
    ResolverDirectory,

    /// Linux: a `systemd-networkd` link of MixEngine's own.
    SystemdLink,

    /// Windows: one Name Resolution Policy rule.
    Nrpt,

    /// This machine offers no way to scope DNS to a TLD.
    ///
    /// **A valid answer, not an error.** This home stays on the hosts file,
    /// [`DnsStatus::because`](mixengine_proto::DnsStatus::because) says why in words, and nothing
    /// fails — which is `.claude/features/domains-and-dns.md`'s own instruction, with the
    /// correction that what is unsupported is this machine's configuration rather than the
    /// platform. Linux without systemd is the case it exists for.
    None,
}

/// What this machine routes to MixEngine today, and what it would take to route more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverState {
    /// Which mechanism this machine has.
    pub method: ResolverMethod,

    /// The TLDs it already routes here, on the port that was asked about.
    ///
    /// A TLD routed to *some* nameserver on this machine is not in this list unless that nameserver
    /// is this home's: two homes on one machine differ by port, and claiming the other one's wiring
    /// would report a home as working when none of its names resolve.
    pub wired: Vec<String>,

    /// Why the rest are not, in words, for `mix doctor` (T47).
    pub missing: Option<String>,
}

impl ResolverState {
    /// What would have to be applied for every TLD in `want` to arrive here, or [`None`] when
    /// nothing would.
    ///
    /// **Whole state** (D4): the plan names every TLD that should end up routed, not the difference,
    /// so a second call with the same intent is `AlreadyDone` rather than a second artifact.
    ///
    /// **And it never names one that may not be wired** (D9): `.local` is dropped here as well as
    /// refused by the helper, because a planner that proposes a refusal spends a prompt to be told
    /// no.
    #[must_use]
    pub fn plan(&self, want: &[&str], port: u16) -> Option<ResolverPlan> {
        let tlds: Vec<String> = want
            .iter()
            .filter(|tld| is_wired_tld(tld))
            .map(|tld| (*tld).to_owned())
            .collect();

        if tlds.is_empty() {
            return None;
        }

        // Already exactly right. Asking anyway would put a row on `mix status` whose only possible
        // outcome is `AlreadyDone` — T41's D11, one capability along.
        if self.wired.len() == tlds.len() && tlds.iter().all(|tld| self.wired.contains(tld)) {
            return None;
        }

        match self.method {
            ResolverMethod::None => None,
            ResolverMethod::ResolverDirectory => {
                Some(ResolverPlan::ResolverDirectory { tlds, port })
            }
            ResolverMethod::SystemdLink => Some(ResolverPlan::SystemdLink { tlds, port }),
            // NRPT has no field for a port, which is why T44 puts the server on 53 on this system.
            ResolverMethod::Nrpt => Some(ResolverPlan::Nrpt { tlds }),
        }
    }

    /// What would have to be removed to undo [`plan`](Self::plan), or [`None`] on a machine with no
    /// mechanism.
    ///
    /// **Nothing in T45 enqueues one** — D13, on T42's precedent. Uninstall (T87) is the producer.
    /// It exists now so that uninstall has a value to build rather than a reversal to invent
    /// against a wiring written five phases earlier.
    #[must_use]
    pub fn target(&self) -> Option<ResolverTarget> {
        match self.method {
            ResolverMethod::None => None,
            ResolverMethod::ResolverDirectory => Some(ResolverTarget::ResolverDirectory {}),
            ResolverMethod::SystemdLink => Some(ResolverTarget::SystemdLink {}),
            ResolverMethod::Nrpt => Some(ResolverTarget::Nrpt {}),
        }
    }
}

/// Whether a managed TLD arrives at MixEngine's DNS server — roadmap task **T45**.
///
/// **Reads only, and never prompts.** The write needs a token this process does not have; it is
/// [`ResolverApply`](mixengine_proto::privileged::PrivilegedOp::ResolverApply), applied by
/// `mixengine-elevate`. Reading needs no privilege on any of the three systems — measured:
/// `/etc/resolver/<tld>` is world-readable, `resolvectl status` answers an ordinary user, and the
/// rule's key is readable under `HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet`. That is what makes
/// it affordable to ask on every daemon start, which the producer depends on.
pub trait ResolverConfig: std::fmt::Debug + Send + Sync {
    /// Which mechanism this machine has, or [`ResolverMethod::None`].
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when something this reads cannot be read. **Every caller
    /// treats an error as "no answer" and carries on**: this is asked at start-up, and a probe that
    /// failed must not become the thing that stops a daemon.
    fn method(&self) -> Result<ResolverMethod>;

    /// Which of `tlds` this machine already routes to `port`, and why the rest do not.
    ///
    /// **This answers "is the configuration in place", not "does a name resolve right now"**, and
    /// the difference is measurable rather than pedantic: on Linux `systemd-networkd` brings the
    /// link up *after* the reload returns, and on Windows the DNS Client reads its policy when it is
    /// told to — so there is a window in which this says `wired` and a lookup still fails. The
    /// honest end-to-end check is a real lookup through the resolver the operating system gives
    /// programs, which is `domain.dns_status`' job (T46). `PortAccess`' macOS probe draws the same
    /// line for the same reason.
    ///
    /// # Errors
    ///
    /// As [`method`](Self::method).
    fn probe(&self, tlds: &[&str], port: u16) -> Result<ResolverState>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(method: ResolverMethod, wired: &[&str]) -> ResolverState {
        ResolverState {
            method,
            wired: wired.iter().map(|tld| (*tld).to_owned()).collect(),
            missing: None,
        }
    }

    /// A machine already wired asks for nothing, so no prompt is spent on a row whose only possible
    /// outcome is `AlreadyDone`.
    #[test]
    fn a_machine_already_wired_needs_no_plan() {
        let state = state(ResolverMethod::ResolverDirectory, &["test", "localhost"]);

        assert_eq!(state.plan(&["test", "localhost"], 53_535), None);
    }

    /// Whole state: the plan names every TLD that should end up routed, not only the missing one.
    #[test]
    fn a_partly_wired_machine_is_asked_for_the_whole_state() {
        let state = state(ResolverMethod::ResolverDirectory, &["test"]);

        assert_eq!(
            state.plan(&["test", "internal"], 53_535),
            Some(ResolverPlan::ResolverDirectory {
                tlds: vec!["test".to_owned(), "internal".to_owned()],
                port: 53_535,
            })
        );
    }

    /// D2: a machine with no scoped mechanism is a mode, not a failure, and it asks for nothing.
    #[test]
    fn a_machine_with_no_mechanism_asks_for_nothing() {
        let state = state(ResolverMethod::None, &[]);

        assert_eq!(state.plan(&["test"], 53_535), None);
        assert_eq!(state.target(), None);
    }

    /// Windows' plan carries no port, because NRPT has nowhere to put one.
    #[test]
    fn an_nrpt_plan_is_built_without_the_port_it_was_offered() {
        let state = state(ResolverMethod::Nrpt, &[]);

        assert_eq!(
            state.plan(&["test"], 53),
            Some(ResolverPlan::Nrpt {
                tlds: vec!["test".to_owned()],
            })
        );
    }

    /// D9, checked here as well as in the helper: a planner never proposes `.local`.
    #[test]
    fn a_plan_never_names_a_tld_that_may_not_be_wired() {
        let state = state(ResolverMethod::SystemdLink, &[]);

        let plan = state.plan(&["test", "local"], 53_535).expect("a plan");

        match plan {
            ResolverPlan::SystemdLink { tlds, .. } => {
                assert_eq!(tlds, vec!["test".to_owned()]);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A request for nothing but unwirable TLDs is nothing to ask for, not an empty plan to apply.
    #[test]
    fn a_request_for_only_unwirable_tlds_is_no_plan_at_all() {
        let state = state(ResolverMethod::SystemdLink, &[]);

        assert_eq!(state.plan(&["local"], 53_535), None);
        assert_eq!(state.plan(&[], 53_535), None);
    }

    /// Every mechanism can be undone, and the one that is no mechanism cannot.
    #[test]
    fn every_mechanism_has_something_to_remove() {
        assert_eq!(
            state(ResolverMethod::Nrpt, &[]).target(),
            Some(ResolverTarget::Nrpt {})
        );
        assert_eq!(
            state(ResolverMethod::SystemdLink, &[]).target(),
            Some(ResolverTarget::SystemdLink {})
        );
        assert_eq!(
            state(ResolverMethod::ResolverDirectory, &[]).target(),
            Some(ResolverTarget::ResolverDirectory {})
        );
    }
}
