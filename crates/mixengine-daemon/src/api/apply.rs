//! `blueprint.apply`: carrying out the plan T77 decided — roadmap task **T78**.
//!
//! **The plan is core's, the execution is the daemon's** (the T78 design, D1).
//! `mixengine_core::blueprints::plan` reads this home's tables and decides the list; every action in
//! that list is a capability this daemon already has, and half of them — an install, a rendering, a
//! supervisor reload, a keyring write — are things `mixengine-core` deliberately cannot do. So the
//! executor is written as an `impl Api`, exactly as [`super::create`] is and for the same reason:
//! `Api` is the one type holding `projects`, `runtimes`, `packages`, `sites`, `domains`,
//! `certificates`, `extensions` and `databases` at once, and a `Blueprints` given eight more fields
//! would be a second assembly of the same handles.
//!
//! # Nothing here decides what the steps are
//!
//! The executor consumes `Vec<PlanStep>` and may **fail**, but may not add a step, drop one or
//! reorder them — the invariant T77 wrote down, and the only way `--dry-run` can promise to match
//! the real run.

use mixengine_proto::{
    AnswerSubject, BlueprintApply, BlueprintApplyResponse, BlueprintPlan, Disposition, Error,
    ErrorCode, PlanAction, ServiceId, VersionAnswer,
};

use super::Api;

impl Api {
    /// `blueprint.apply` — what applying one would do, and (from this task) doing it.
    ///
    /// # Errors
    ///
    /// `not_found` for a blueprint nothing is filed under; `invalid_argument` for a root that is not
    /// absolute, for a question nobody answered and for an answer nothing asked for;
    /// `precondition_failed` for a plan holding a step that cannot be done; and the wire error of a
    /// table that could not be read.
    pub(crate) async fn blueprint_apply(
        &self,
        asked: &BlueprintApply,
    ) -> Result<BlueprintApplyResponse, Error> {
        let (_manifest, plan) = self.blueprints.planned(asked).await?;

        if asked.dry_run {
            return Ok(BlueprintApplyResponse::Planned { plan });
        }

        // **Two plannings, both of them pure reads**, and it is what makes both refusals trivial:
        // one list says what was *asked*, the other says what was *decided*. Reading the questions
        // off a plan the answers have already altered would be reading one list twice.
        let asking = BlueprintApply {
            answers: Vec::new(),
            ..asked.clone()
        };
        let (_, unanswered) = self.blueprints.planned(&asking).await?;

        if let Some(refused) = refusal(&questions(&unanswered), &plan, &asked.answers) {
            return Err(refused);
        }

        Err(Error::new(
            ErrorCode::PreconditionFailed,
            "this build plans an apply but does not carry one out",
        )
        .with_hint("`--dry-run` prints the plan; executing it arrives with roadmap task T78"))
    }
}

/// Every question a plan asks, which is every `Choice` left in it.
///
/// Read from a plan built with **no** answers, so that "was this asked" and "was this answered" are
/// two readings rather than one reading the other has already altered.
fn questions(plan: &BlueprintPlan) -> Vec<AnswerSubject> {
    plan.steps
        .iter()
        .filter(|step| matches!(step.disposition, Disposition::Choice { .. }))
        .filter_map(|step| match &step.action {
            PlanAction::InstallRuntime { kind, .. } => Some(AnswerSubject::Runtime { kind: *kind }),

            // The pair travels apart in the action and together here, which is safe for exactly the
            // reason the answer type says: a service question only arises for an instance that
            // already exists, so the id was spellable before the question was asked.
            PlanAction::EnsureService {
                package, instance, ..
            } => ServiceId::parse(format!("{package}@{instance}"))
                .ok()
                .or_else(|| ServiceId::parse(package).ok())
                .map(|id| AnswerSubject::Service { id }),

            _ => None,
        })
        .collect()
}

/// Why this apply may not start, or [`None`].
///
/// **Three refusals, in the order they are cheapest to explain**: something that cannot be done at
/// all, a question nobody answered, and an answer to a question nobody asked. All of them are
/// decided here rather than five actions into a project directory, which is the whole point of
/// having planned first (the T77 design, D10).
fn refusal(
    questions: &[AnswerSubject],
    plan: &BlueprintPlan,
    answers: &[VersionAnswer],
) -> Option<Error> {
    let blocked: Vec<String> = plan
        .steps
        .iter()
        .filter_map(|step| match &step.disposition {
            Disposition::Blocked { reason } | Disposition::Unsupported { reason } => {
                Some(reason.clone())
            }
            _ => None,
        })
        .collect();

    if !blocked.is_empty() {
        return Some(
            Error::new(
                ErrorCode::PreconditionFailed,
                format!(
                    "this blueprint cannot be applied here: {}",
                    blocked.join("; ")
                ),
            )
            .with_hint(
                "`mix blueprint apply --dry-run` prints every step and what stands in its way",
            ),
        );
    }

    let unanswered: Vec<String> = questions
        .iter()
        .filter(|subject| !answers.iter().any(|given| &&given.subject == subject))
        .map(ToString::to_string)
        .collect();

    if !unanswered.is_empty() {
        return Some(
            Error::new(
                ErrorCode::InvalidArgument,
                format!(
                    "this blueprint asks for a version this machine does not have, and nobody \
                     answered for: {}",
                    unanswered.join(", ")
                ),
            )
            .with_hint(
                "`mix blueprint apply` asks each question at the terminal; `--install-missing` and \
                 `--use-installed` answer them all",
            ),
        );
    }

    let unasked: Vec<String> = answers
        .iter()
        .filter(|given| !questions.contains(&given.subject))
        .map(|given| given.subject.to_string())
        .collect();

    match unasked.is_empty() {
        true => None,
        false => Some(
            Error::new(
                ErrorCode::InvalidArgument,
                format!(
                    "nothing in this plan asks about: {} — this answers a plan made against \
                     another machine, or another moment",
                    unasked.join(", ")
                ),
            )
            .with_hint("`mix blueprint apply --dry-run` prints the questions this plan does ask"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mixengine_proto::{
        MismatchAnswer, PackageVersion, PlanStep, RuntimeKind, VersionConstraint,
    };

    fn step(action: PlanAction, disposition: Disposition) -> PlanStep {
        PlanStep {
            action,
            disposition,
            elevates: false,
        }
    }

    fn a_plan(steps: Vec<PlanStep>) -> BlueprintPlan {
        BlueprintPlan {
            blueprint: "blog-stack".to_owned(),
            project: "shop".to_owned(),
            root: "/tmp/shop".to_owned(),
            steps,
        }
    }

    fn php() -> PlanAction {
        PlanAction::InstallRuntime {
            kind: RuntimeKind::Php,
            wanted: VersionConstraint::parse("8.2.23").expect("a constraint"),
        }
    }

    fn a_php_question() -> Disposition {
        Disposition::Choice {
            installed: PackageVersion::parse("8.2.29").expect("a version"),
            wanted: VersionConstraint::parse("8.2.23").expect("a constraint"),
        }
    }

    /// A plan with nothing in its way and nothing to ask is one an apply may start.
    #[test]
    fn a_plan_that_is_all_work_is_not_refused() {
        let plan = a_plan(vec![step(php(), Disposition::Create)]);

        assert!(refusal(&[], &plan, &[]).is_none());
    }

    /// **The whole point of having planned first**: a blocked step is refused here, naming what
    /// stands in the way, rather than five actions into a project directory.
    #[test]
    fn a_blocked_step_refuses_the_apply_and_says_what_is_in_the_way() {
        let plan = a_plan(vec![step(
            PlanAction::AddDomain {
                domain: "shop.test".to_owned(),
                primary: true,
            },
            Disposition::Blocked {
                reason: "shop.test is already answered by blog.test".to_owned(),
            },
        )]);

        let refused = refusal(&[], &plan, &[]).expect("a refusal");

        assert_eq!(refused.code, ErrorCode::PreconditionFailed);
        assert!(refused.message.contains("shop.test"), "{refused:?}");
    }

    /// A question nobody answered is a question, and a daemon has no keyboard.
    #[test]
    fn an_unanswered_question_refuses_the_apply_and_names_the_question() {
        let plan = a_plan(vec![step(php(), a_php_question())]);

        let refused = refusal(
            &[AnswerSubject::Runtime {
                kind: RuntimeKind::Php,
            }],
            &plan,
            &[],
        )
        .expect("a refusal");

        assert!(refused.message.contains("php"), "{refused:?}");
        assert!(
            refused
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("--use-installed")),
            "{refused:?}"
        );
    }

    /// The one that was asked and answered stops being a refusal, and it is matched by subject —
    /// so an answer about PHP does not settle a question about MariaDB.
    #[test]
    fn an_answer_settles_its_own_question_and_no_other() {
        let plan = a_plan(vec![step(php(), a_php_question())]);
        let asked = [
            AnswerSubject::Runtime {
                kind: RuntimeKind::Php,
            },
            AnswerSubject::Service {
                id: ServiceId::parse("mariadb@main").expect("an id"),
            },
        ];

        let refused = refusal(
            &asked,
            &plan,
            &[VersionAnswer {
                subject: AnswerSubject::Runtime {
                    kind: RuntimeKind::Php,
                },
                answer: MismatchAnswer::UseInstalled,
            }],
        )
        .expect("the other question is still open");

        assert!(refused.message.contains("mariadb@main"), "{refused:?}");
        assert!(!refused.message.contains("php"), "{refused:?}");
    }

    /// **Somebody answering a question nobody asked is somebody holding a plan that has since
    /// changed**, and saying so is cheaper than applying their answer to nothing.
    #[test]
    fn an_answer_to_a_question_this_plan_does_not_ask_is_refused_by_name() {
        let plan = a_plan(vec![step(php(), Disposition::Satisfied)]);

        let refused = refusal(
            &[],
            &plan,
            &[VersionAnswer {
                subject: AnswerSubject::Runtime {
                    kind: RuntimeKind::Php,
                },
                answer: MismatchAnswer::UseInstalled,
            }],
        )
        .expect("a refusal");

        assert_eq!(refused.code, ErrorCode::InvalidArgument);
        assert!(refused.message.contains("php"), "{refused:?}");
    }

    /// The questions are read off the actions, and only where the disposition is one.
    #[test]
    fn only_a_step_that_asks_becomes_a_question() {
        let plan = a_plan(vec![
            step(php(), a_php_question()),
            step(
                PlanAction::EnsureService {
                    package: "mariadb".to_owned(),
                    instance: "main".to_owned(),
                    version: None,
                    dedicated: false,
                },
                Disposition::Satisfied,
            ),
        ]);

        assert_eq!(
            questions(&plan),
            vec![AnswerSubject::Runtime {
                kind: RuntimeKind::Php
            }]
        );
    }
}
