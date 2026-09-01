//! One step of a plan, carried out — roadmap task **T78**.
//!
//! **Every action is an ensure** (the T78 design, D3): a doer asks the world before it acts, so a
//! step that is already true costs a read. That is what makes a failed apply resumable rather than
//! restartable, and it is why one daemon call may make several steps true at once — `site.create`
//! writes the row, queues the hosts entries and issues the certificate, and the steps that follow it
//! find themselves already so.
//!
//! A step is reported by **what became true**, not by how many calls it took.

use std::path::PathBuf;

use mixengine_proto::{Disposition, PlanAction, PlanStep, ServiceId, StepResult};

/// What a step needs from this apply that is not in the step itself.
///
/// Carried across the walk rather than recomputed, because the site is where three earlier facts
/// meet: the project that owns it, the instances the `EnsureService` steps made sure of, and the
/// names the `AddDomain` steps carry (D14).
pub(crate) struct Context {
    /// The project's name, which is also what `{project}` was expanded to.
    #[expect(
        dead_code,
        reason = "read by the doers, which arrive with the next task in this series"
    )]
    pub(crate) project: String,

    /// Where it lives.
    #[expect(
        dead_code,
        reason = "read by the doers, which arrive with the next task in this series"
    )]
    pub(crate) root: PathBuf,

    /// Every instance the `EnsureService` steps so far have made sure of, in the order they were
    /// named — which is what the site is linked to when its turn comes (D14).
    #[expect(
        dead_code,
        reason = "written and read by the doers, which arrive with the next task in this series"
    )]
    pub(crate) ensured: Vec<ServiceId>,
}

/// The outcome a disposition decides on its own, without touching anything.
///
/// [`None`] means *this one is work*, and the caller is what does it.
pub(crate) fn untouched(step: &PlanStep) -> Option<StepResult> {
    match &step.disposition {
        Disposition::Satisfied => Some(StepResult::AlreadyTrue),

        // **T78a's, and named as such** (D11). A blueprint's own command is arbitrary code from
        // whoever wrote it, and this build applies everything else rather than refusing a whole
        // blueprint over the one step it may not run — leaving a person a project they can use and
        // one line to run themselves.
        Disposition::Confirm { what } => Some(StepResult::NotRun {
            why: format!(
                "`{what}` was not run: a blueprint's own command needs the trust marking roadmap \
                 task T78a brings — run it yourself in the project directory"
            ),
        }),

        // Every one of these was refused before the job existed. Reaching one here means the plan
        // changed underneath this apply, which is a failure and not a step outcome — so it is left
        // to the caller, which turns [`None`] into work and finds there is none to do.
        Disposition::Blocked { .. }
        | Disposition::Unsupported { .. }
        | Disposition::Choice { .. }
        | Disposition::Create => None,

        // A disposition a later build added, met by an executor that cannot know what it means.
        // Refusing to guess is the only safe reading.
        _ => Some(StepResult::NotRun {
            why: "this build does not know what to make of that step".to_owned(),
        }),
    }
}

/// The one line a job's progress says while a step is being carried out.
///
/// Its own rendering rather than `mix`'s: what a client prints is a client's, and a daemon reaching
/// into one would be the daemon holding a client's vocabulary.
pub(crate) fn describe(action: &PlanAction) -> String {
    match action {
        PlanAction::RegisterProject { name, .. } => format!("registering the project {name}"),
        PlanAction::InstallRuntime { kind, wanted } => {
            format!("installing {} {}", kind.as_str(), wanted.as_str())
        }
        PlanAction::InstallPackage { package, wanted } => match wanted {
            Some(wanted) => format!("installing {package} {}", wanted.as_str()),
            None => format!("installing {package}"),
        },
        PlanAction::EnsureService {
            package, instance, ..
        } => format!("making sure of {package}@{instance}"),
        PlanAction::CreateDatabase {
            database, package, ..
        } => format!("creating the database {database} on {package}"),
        PlanAction::CreateSite { .. } => "creating the site".to_owned(),
        PlanAction::AddDomain { domain, .. } => format!("adding the name {domain}"),
        PlanAction::IssueCertificate { .. } => "issuing the certificate".to_owned(),
        PlanAction::SetPhpExtension { name, .. } => format!("turning on the PHP extension {name}"),
        PlanAction::RunScaffold { .. } => "the blueprint's own command".to_owned(),
        _ => "a step this build does not know".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mixengine_proto::{RuntimeKind, VersionConstraint};

    fn step(disposition: Disposition) -> PlanStep {
        PlanStep {
            action: PlanAction::InstallRuntime {
                kind: RuntimeKind::Php,
                wanted: VersionConstraint::parse("8.2.23").expect("a constraint"),
            },
            disposition,
            elevates: false,
        }
    }

    /// Every step is reported, including the ones that needed nothing: a second apply whose every
    /// line says *already true* is the proof that the first one finished.
    #[test]
    fn a_step_that_needs_nothing_is_reported_rather_than_left_out() {
        assert_eq!(
            untouched(&step(Disposition::Satisfied)),
            Some(StepResult::AlreadyTrue)
        );
    }

    /// **D11.** The scaffold is left, and the sentence that says so carries the command, because
    /// that is the one line a person has to act on.
    #[test]
    fn a_scaffold_is_left_with_the_command_that_was_not_run() {
        let left = untouched(&step(Disposition::Confirm {
            what: "composer install".to_owned(),
        }));

        let Some(StepResult::NotRun { why }) = left else {
            panic!("a scaffold is left rather than done");
        };
        assert!(why.contains("composer install"), "{why}");
        assert!(why.contains("T78a"), "{why}");
    }

    /// And a step that is work is left to the caller, which is what does work.
    #[test]
    fn a_step_that_is_work_is_not_decided_here() {
        assert_eq!(untouched(&step(Disposition::Create)), None);
    }
}
