//! What `daemon.doctor_repair` answers — roadmap task **T47b**.
//!
//! **A record of what was done, and not a second report.** `daemon.doctor` lists every check
//! whatever it answered, because a check that found nothing is the evidence that it ran. This is the
//! other kind of document: it is read *after* an action, and an entry saying "this was already fine
//! so I did nothing" is noise in a list whose whole purpose is to say what changed. What was
//! examined is one call away (T47b design, D2).
//!
//! Nothing here decides anything. Which [`Action`] a condition gets is decided by the daemon's
//! exhaustive match on [`ProblemId`] — and being exhaustive is what stops a repair and this build's
//! findings drifting apart (D3).

use crate::{JobSummary, ProblemId};

/// What to repair, and whether to raise the prompt.
///
/// **`grant` exists because of T64**, not for symmetry: a person must be able to read what is about
/// to be allowed *before* it is allowed, and a call that enqueued and flushed in one step leaves no
/// moment for a client to show them. So the ordinary path is two calls — enqueue, show the batch,
/// then `elevation.grant` — and `grant: true` is for a caller that has already shown it and been
/// answered. `mix doctor --repair` takes the first path; `mix doctor --repair --yes` takes the
/// second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorRepair {
    /// Flush the queue in this same call, raising the one prompt.
    ///
    /// **Defaults to false**, which is the safe direction: a caller that forgot the field gets the
    /// path where somebody is shown the batch first.
    #[serde(default)]
    pub grant: bool,
}

/// What `daemon.doctor_repair` did.
///
/// No `Eq`: [`JobSummary`] carries a progress fraction, and a float has no total equality. `PartialEq`
/// is what the tests below need and all this type can honestly offer.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RepairReport {
    /// One entry per `Problem` the report found, in the report's own order.
    pub actions: Vec<Repair>,

    /// The single grant this call raised, when it was asked to and anything needed the helper.
    ///
    /// **[`None`] whenever no prompt was raised** — a healthy machine, a home whose only faults were
    /// inside its own directory, or the ordinary path where the caller means to show the batch and
    /// call `elevation.grant` itself ([`DoctorRepair::grant`]).
    ///
    /// A [`JobSummary`] rather than a bare id, so a caller can follow the job without asking again;
    /// it is the same value `elevation.grant` answers with.
    pub granting: Option<JobSummary>,
}

impl RepairReport {
    /// Was anything left that this build could not act on?
    ///
    /// **What `mix doctor --repair`'s exit code is**, and it deliberately does not mean "the machine
    /// is well": what was [`Action::Enqueued`] is not applied until the grant finishes. The question
    /// this answers is the narrower and more useful one — did everything found get acted on.
    #[must_use]
    pub fn left_something_undone(&self) -> bool {
        self.actions
            .iter()
            .any(|repair| matches!(repair.outcome, Action::Untouched { .. }))
    }
}

/// One condition, and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Repair {
    /// The condition this entry is about.
    pub id: ProblemId,

    /// What was examined, phrased for a person.
    ///
    /// `Check::name`, carried through rather than spelled a second time, so the report and the
    /// repair name one thing the same way.
    pub name: String,

    /// What happened to it.
    pub outcome: Action,
}

/// What a repair did.
///
/// **Internally tagged**, so a client matches on a word rather than working out which fields
/// arrived.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Done, inside the home, with no prompt and nothing pending.
    Repaired {
        /// What was done, in a sentence.
        what: String,
    },

    /// Needs the elevated helper. Applied by the grant this call raised, not by this call.
    ///
    /// **A separate word from [`Repaired`](Action::Repaired)**, because the difference is the one a
    /// person cares about: one is finished and the other is waiting on a prompt they have not
    /// answered yet. Collapsing them would report a machine as fixed while the operation that fixes
    /// it has not run.
    Enqueued {
        /// What is waiting, in a sentence.
        what: String,
    },

    /// Nothing this build can do, and why.
    ///
    /// **Not a failure of the call.** Three of the ten conditions have no repair here, and a report
    /// that hid them would claim a machine was seen to and left alone.
    Untouched {
        /// Why it was left, in a sentence. Never advice.
        because: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The action is tagged, so a client matches on a word rather than on which fields arrived —
    /// `Outcome`'s rule in `doctor_api`, and for the same reason.
    #[test]
    fn an_action_travels_tagged_and_names_the_condition_it_is_about() {
        let repair = Repair {
            id: ProblemId::GeneratedConfigStale,
            name: "the generated configuration".to_owned(),
            outcome: Action::Repaired {
                what: "two files were re-installed".to_owned(),
            },
        };

        let wire = serde_json::to_string(&repair).expect("a repair serialises");

        assert_eq!(
            wire,
            r#"{"id":"generated_config_stale","name":"the generated configuration","outcome":{"action":"repaired","what":"two files were re-installed"}}"#
        );
    }

    /// A report with nothing to grant says so with `null` rather than by leaving the field out: a
    /// client renders "an administrator's permission is needed" off its presence, and a missing
    /// field and a null one are the same absence only if every client agrees they are.
    #[test]
    fn a_report_that_raised_no_prompt_says_so() {
        let wire = r#"{"actions":[],"granting":null}"#;

        let report: RepairReport = serde_json::from_str(wire).expect("a report");

        assert!(report.actions.is_empty());
        assert_eq!(report.granting, None);
        assert_eq!(serde_json::to_string(&report).expect("back"), wire);
    }

    /// Untouched is not a failure of the call and is not hidden.
    #[test]
    fn an_untouched_condition_round_trips_with_its_reason() {
        let wire = r#"{"action":"untouched","because":"this system reserved the range"}"#;

        let action: Action = serde_json::from_str(wire).expect("an action");

        assert_eq!(
            action,
            Action::Untouched {
                because: "this system reserved the range".to_owned()
            }
        );
        assert_eq!(serde_json::to_string(&action).expect("back"), wire);
    }

    /// The exit code of `mix doctor --repair` is this function and nothing else — an `Enqueued` is
    /// not something left undone, it is something waiting on a person.
    #[test]
    fn only_an_untouched_condition_leaves_something_undone() {
        let mut report = RepairReport {
            actions: vec![
                Repair {
                    id: ProblemId::HomePermissionsLost,
                    name: "one".to_owned(),
                    outcome: Action::Repaired {
                        what: "restricted again".to_owned(),
                    },
                },
                Repair {
                    id: ProblemId::HostsBlockDiffers,
                    name: "two".to_owned(),
                    outcome: Action::Enqueued {
                        what: "two lines are waiting".to_owned(),
                    },
                },
            ],
            granting: None,
        };

        assert!(!report.left_something_undone());

        report.actions.push(Repair {
            id: ProblemId::PortRangeReserved,
            name: "three".to_owned(),
            outcome: Action::Untouched {
                because: "this system reserved the range".to_owned(),
            },
        });

        assert!(report.left_something_undone());
    }
}
