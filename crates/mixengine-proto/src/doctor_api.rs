//! What `daemon.doctor` answers — roadmap task **T47a**.
//!
//! **A list of checks and not a list of problems.** A doctor that prints nothing on a healthy
//! machine leaves a person unsure it looked, so what was examined and what was found are one
//! structure: "eleven checks, all Ok" and "eleven checks, one Problem" are renderings of the same
//! (T47a design, D2).
//!
//! Nothing here says what to do about anything. A [`Problem`](Outcome::Problem) carries a
//! [`ProblemId`], which is a name for a condition rather than advice, and which
//! `daemon.doctor_repair` (T47b) matches on — so the two halves cannot drift, a repair for an id
//! nothing produces failing to compile against a closed enum (D3).

/// Everything `daemon.doctor` examined, and what each answered.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DoctorReport {
    /// Every check this build knows how to make, in a fixed order, whatever each answered.
    ///
    /// **Fixed order and never filtered**: a check that found nothing wrong is the evidence that it
    /// ran, and a shorter list on one operating system would read as a clean bill of health rather
    /// than as a question nobody asked.
    pub checks: Vec<Check>,
}

impl DoctorReport {
    /// Did anything examined turn out to be wrong?
    ///
    /// **[`Outcome::Note`] and [`Outcome::Skipped`] are not faults**, which is the whole of this
    /// function: it is what `mix doctor`'s exit code is, and a doctor whose exit code says "I ran"
    /// rather than "the machine is well" cannot be used in a script (T47a design, D8).
    #[must_use]
    pub fn has_a_problem(&self) -> bool {
        self.checks
            .iter()
            .any(|check| matches!(check.outcome, Outcome::Problem { .. }))
    }
}

/// One thing examined.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Check {
    /// What was examined, phrased for a person: "the managed hosts block".
    pub name: String,

    /// What was found.
    pub outcome: Outcome,
}

/// What a check found.
///
/// **Internally tagged**, so a client matches on a word rather than working out which fields
/// arrived.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// Examined, and what was expected is what is there.
    ///
    /// An empty struct variant rather than a unit one: `deny_unknown_fields` never fires on a unit
    /// variant of an internally tagged enum — it is read through `deserialize_any`, which T40
    /// established — and the rule holds for the variant carrying no fields as much as for the ones
    /// that do.
    Ok {},

    /// A fact worth stating that is not a fault.
    ///
    /// [ADR 0007](../../../.claude/decisions/0007-supervised-child-owns-a-process-group.md) is why
    /// this variant exists: what MixEngine can guarantee about killing a service's descendants
    /// differs by system, and reporting "macOS guarantees nothing" as a *problem* would be reporting
    /// the operating system as broken. Reporting it as nothing at all is the failure that ADR exists
    /// to prevent (T47a design, D4).
    Note {
        /// The fact, in a sentence.
        because: String,
    },

    /// Something is wrong.
    Problem {
        /// A stable name for the condition, which `daemon.doctor_repair` matches on.
        id: ProblemId,

        /// What is wrong, in a sentence, for a person. Never what to do about it.
        because: String,
    },

    /// Not examined, and why.
    ///
    /// **An outcome and not silence.** Windows' reserved port ranges do not exist on macOS or Linux,
    /// and the workspace rule is that an unsupported path answers with a type rather than a
    /// `todo!()`; here that answer is a check that ran and says why it had nothing to look at.
    Skipped {
        /// Why it could not be examined here.
        because: String,
    },
}

/// The conditions this build can report, and the whole of what T47b can be asked to repair.
///
/// **Closed rather than a string.** A repair keyed off a spelling is a repair that silently stops
/// matching; keyed off this, a repair for a condition nothing produces does not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemId {
    /// The managed hosts block is not what this home's sites need.
    HostsBlockDiffers,

    /// Nothing on this machine sends a managed TLD to this daemon's DNS server.
    ResolverNotWired,

    /// This machine does not trust MixEngine's own certificate authority.
    ///
    /// **Repaired by asking again, which is why this exists where T48 declined to add one.** There
    /// the condition was a damaged authority and the repair would have been to regenerate — a
    /// destructive act, invalidating every leaf already issued and every store holding the old
    /// certificate, and T54's decision to make. Here the repair is the same one `ResolverNotWired`
    /// and `PortAccessMissing` already have: put the operation back in the queue.
    CaNotTrusted,

    /// Firefox or Chrome on this machine does not trust MixEngine's certificate authority.
    ///
    /// **A separate condition from [`Self::CaNotTrusted`], because they are repaired differently.**
    /// That one goes back into the elevation queue; this one needs no privilege at all, since these
    /// databases belong to the user. Collapsing the two would put a prompt in front of a repair
    /// that does not need one — and would report a machine as faulty for a store its browsers never
    /// read.
    BrowsersNotTrusted,

    /// A site that declares HTTPS has no usable certificate — roadmap task **T50**.
    ///
    /// **T48 declined to add a condition for its own subject and this adds one, and the difference
    /// is what the repair does.** There, the condition was a damaged authority and the repair would
    /// have been to regenerate — throwing away every leaf and every store holding the old
    /// certificate, in answer to a request nobody made. Here the repair is issuance: idempotent,
    /// destructive of nothing, and needing no privilege.
    SiteCertificateMissing,

    /// The DNS server could not bind, so this home has no wildcard names at all.
    DnsServerUnavailable,

    /// The front end may not answer on 80 and 443 here.
    PortAccessMissing,

    /// Something is waiting for permission that has not been granted or dropped.
    PermissionPending,

    /// A declared domain does not resolve on this machine.
    DomainUnreachable,

    /// The home is no longer restricted to its owner.
    HomePermissionsLost,

    /// This system has reserved a port range this home needs.
    PortRangeReserved,

    /// A service's installed configuration is not what its row renders to.
    GeneratedConfigStale,

    /// A `services` row claims to be supervised and this daemon has no supervisor for it.
    ServiceUnsupervised,

    /// This machine refuses to load an unsigned image, and every program MixEngine starts is one —
    /// roadmap task **T94**.
    ///
    /// **Reported even on a machine where everything currently runs**, and that is the cost this
    /// condition is worth paying: Smart App Control judges a *file*, so the next image MixEngine
    /// loads is a runtime archive whose hash has never existed anywhere — the first-seen case a
    /// refusal was measured for.
    ///
    /// **The one condition here with no repair that could ever exist.** Turning the policy off
    /// cannot be undone without reinstalling Windows, so `daemon.doctor_repair` declines it rather
    /// than asking somebody to lower their machine's defences.
    ApplicationControlEnforced,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The outcome is tagged, so a client matches on a word rather than on which fields arrived.
    #[test]
    fn an_outcome_travels_tagged_and_a_problem_carries_its_id() {
        let check = Check {
            name: "the managed hosts block".to_owned(),
            outcome: Outcome::Problem {
                id: ProblemId::HostsBlockDiffers,
                because: "two lines are missing".to_owned(),
            },
        };

        let wire = serde_json::to_string(&check).expect("a check serialises");

        assert_eq!(
            wire,
            r#"{"name":"the managed hosts block","outcome":{"outcome":"problem","id":"hosts_block_differs","because":"two lines are missing"}}"#
        );
    }

    /// `Ok` carries nothing and must still be spellable in both directions.
    #[test]
    fn a_healthy_check_round_trips() {
        let wire = r#"{"name":"the DNS server","outcome":{"outcome":"ok"}}"#;

        let check: Check = serde_json::from_str(wire).expect("a check");

        assert_eq!(check.outcome, Outcome::Ok {});
        assert_eq!(serde_json::to_string(&check).expect("back"), wire);
    }

    /// The exit code of `mix doctor` is this function and nothing else — a `Note` and a `Skipped`
    /// are not faults.
    #[test]
    fn only_a_problem_makes_the_report_unhealthy() {
        let mut report = DoctorReport {
            checks: vec![
                Check {
                    name: "one".to_owned(),
                    outcome: Outcome::Ok {},
                },
                Check {
                    name: "two".to_owned(),
                    outcome: Outcome::Note {
                        because: "macOS kills nothing when the daemon dies".to_owned(),
                    },
                },
                Check {
                    name: "three".to_owned(),
                    outcome: Outcome::Skipped {
                        because: "only Windows reserves port ranges".to_owned(),
                    },
                },
            ],
        };

        assert!(!report.has_a_problem());

        report.checks.push(Check {
            name: "four".to_owned(),
            outcome: Outcome::Problem {
                id: ProblemId::ResolverNotWired,
                because: "nothing sends .test here".to_owned(),
            },
        });

        assert!(report.has_a_problem());
    }

    /// The two conditions T47b repairs are ids like the rest, so a repair keys off them the same
    /// way — which is the whole of why they are here rather than inside the repair.
    #[test]
    fn the_conditions_t47b_repairs_are_spelled_on_the_wire() {
        for (id, spelling) in [
            (
                ProblemId::GeneratedConfigStale,
                r#""generated_config_stale""#,
            ),
            (ProblemId::ServiceUnsupervised, r#""service_unsupervised""#),
        ] {
            assert_eq!(serde_json::to_string(&id).expect("an id"), spelling);
        }
    }
}
