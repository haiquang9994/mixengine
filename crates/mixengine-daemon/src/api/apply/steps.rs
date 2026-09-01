//! One step of a plan, carried out — roadmap task **T78**.
//!
//! **Every action is an ensure** (the T78 design, D3): a doer asks the world before it acts, so a
//! step that is already true costs a read. That is what makes a failed apply resumable rather than
//! restartable, and it is why one daemon call may make several steps true at once — `site.create`
//! writes the row, queues the hosts entries and issues the certificate, and the steps that follow it
//! find themselves already so.
//!
//! A step is reported by **what became true**, not by how many calls it took.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mixengine_proto::{
    BlueprintPlan, Disposition, Error, ErrorCode, PackageVersion, PlanAction, PlanStep,
    RuntimeKind, ScaffoldConsent, ServiceId, StepResult,
};

/// What a step needs from this apply that is not in the step itself.
///
/// Carried across the walk rather than recomputed, because the site is where three earlier facts
/// meet: the project that owns it, the instances the `EnsureService` steps made sure of, and the
/// names the `AddDomain` steps carry (D14).
pub(crate) struct Context {
    /// The project's name, which is also what `{project}` was expanded to.
    pub(crate) project: String,

    /// Where it lives.
    pub(crate) root: PathBuf,

    /// Every instance the `EnsureService` steps so far have made sure of, in the order they were
    /// named — which is what the site is linked to when its turn comes (D14).
    pub(crate) ensured: Vec<ServiceId>,

    /// What this apply has made, for the rollback (D4).
    pub(crate) ledger: super::ledger::Ledger,

    /// Every version this apply will install, decided before it wrote anything (D9).
    pub(crate) resolved: BTreeMap<String, PackageVersion>,

    /// The agreement to run the blueprint's own command, if the request carried one — roadmap task
    /// **T78a**. Already checked against this plan before the job began.
    pub(crate) consent: Option<ScaffoldConsent>,
}

impl Context {
    /// The release the resolution pass settled on for one key.
    ///
    /// # Errors
    ///
    /// `internal` for a key the pass did not visit, which would mean the walk and the pass disagree
    /// about which steps are installs — a bug here rather than anything a person did.
    pub(crate) fn resolution(&self, key: &str) -> Result<PackageVersion, Error> {
        self.resolved.get(key).cloned().ok_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                format!("nothing was resolved for {key} before this apply began"),
            )
        })
    }
}

/// How a language is spelled in the resolution map.
pub(crate) fn runtime_key(kind: RuntimeKind) -> String {
    format!("runtime:{}", kind.as_str())
}

/// How a service package is spelled in the resolution map.
///
/// Prefixed, so that a package called `php` and the language `php` are two keys rather than one.
pub(crate) fn package_key(package: &str) -> String {
    format!("package:{package}")
}

/// The outcome a disposition decides on its own, without touching anything.
///
/// [`None`] means *this one is work*, and the caller is what does it.
///
/// `consent` is the agreement the request carried, if it carried one — roadmap task **T78a**, its
/// design's D4. By the time it reaches here it has already been checked against this plan.
pub(crate) fn untouched_with_consent(
    step: &PlanStep,
    consent: Option<&ScaffoldConsent>,
) -> Option<StepResult> {
    match &step.disposition {
        Disposition::Satisfied => Some(StepResult::AlreadyTrue),

        // **Agreed to, or left** (T78a, D4). A blueprint's own command is arbitrary code from
        // whoever wrote it: with a consent naming it this is work, and without one the step is left
        // as a sentence while everything else is applied — because a blueprint must not become
        // worthless over the one step nobody answered for.
        Disposition::Confirm { what } => match consent {
            Some(_) => None,
            None => Some(StepResult::NotRun {
                why: format!(
                    "`{what}` was not run: nobody agreed to it — `mix blueprint apply \
                     --run-scaffold` shows it, asks, and runs it in the project directory"
                ),
            }),
        },

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

/// The names the `AddDomain` steps immediately after `position` carry.
///
/// **The one place the walk looks ahead** (D14), and it is worth naming: a site cannot be created
/// nameless, and the alternative — creating it under a default name and renaming it a step later —
/// would write a hosts entry for a domain nobody asked for. They are read off the plan rather than
/// expanded a second time, so there stays exactly one place where `{project}` became `shop`.
///
/// The reading stops at the first step that is not a name, which is the certificate or whatever
/// comes next: a plan's domains are contiguous, and a `Blocked` one never reaches an apply at all.
pub(crate) fn names_after(plan: &BlueprintPlan, position: usize) -> Vec<String> {
    plan.steps
        .iter()
        .skip(position + 1)
        .map_while(|step| match &step.action {
            PlanAction::AddDomain { domain, .. } => Some(domain.clone()),
            _ => None,
        })
        .collect()
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
            untouched_with_consent(&step(Disposition::Satisfied), None),
            Some(StepResult::AlreadyTrue)
        );
    }

    /// **Nobody agreed to it, so it was left** — roadmap task **T78a**, its design's D4. The
    /// sentence carries the command, because that is the one line a person has to act on.
    #[test]
    fn a_scaffold_nobody_agreed_to_is_left_with_the_command() {
        let left = untouched_with_consent(
            &step(Disposition::Confirm {
                what: "composer install".to_owned(),
            }),
            None,
        );

        let Some(StepResult::NotRun { why }) = left else {
            panic!("a scaffold is left rather than done");
        };
        assert!(why.contains("composer install"), "{why}");
        assert!(why.contains("--run-scaffold"), "{why}");
    }

    /// And with a consent it is work, which is the executor's to do.
    #[test]
    fn a_scaffold_somebody_agreed_to_is_work() {
        let consent = ScaffoldConsent {
            command: "composer install".to_owned(),
            untrusted: false,
        };

        assert_eq!(
            untouched_with_consent(
                &step(Disposition::Confirm {
                    what: "composer install".to_owned(),
                }),
                Some(&consent),
            ),
            None
        );
    }

    /// And a step that is work is left to the caller, which is what does work.
    #[test]
    fn a_step_that_is_work_is_not_decided_here() {
        assert_eq!(
            untouched_with_consent(&step(Disposition::Create), None),
            None
        );
    }

    fn a_plan(steps: Vec<PlanStep>) -> BlueprintPlan {
        BlueprintPlan {
            blueprint: "blog-stack".to_owned(),
            project: "shop".to_owned(),
            root: "/tmp/shop".to_owned(),
            steps,
            source: mixengine_proto::BlueprintSource::Captured,
            trusted: true,
        }
    }

    fn named(domain: &str, primary: bool) -> PlanStep {
        PlanStep {
            action: PlanAction::AddDomain {
                domain: domain.to_owned(),
                primary,
            },
            disposition: Disposition::Create,
            elevates: true,
        }
    }

    fn a_site() -> PlanStep {
        PlanStep {
            action: PlanAction::CreateSite {
                kind: mixengine_proto::SiteKind::PhpFpm { pool: None },
                doc_root: "public".to_owned(),
                https: true,
            },
            disposition: Disposition::Create,
            elevates: false,
        }
    }

    /// **D14.** A site's names are the domains the plan adds after it, in the plan's own order —
    /// read off the list rather than expanded a second time.
    #[test]
    fn a_sites_names_are_the_domains_the_plan_adds_after_it() {
        let plan = a_plan(vec![
            step(Disposition::Satisfied),
            a_site(),
            named("shop.test", true),
            named("www.shop.test", false),
            PlanStep {
                action: PlanAction::IssueCertificate {
                    domains: vec!["shop.test".to_owned()],
                },
                disposition: Disposition::Create,
                elevates: true,
            },
        ]);

        assert_eq!(
            names_after(&plan, 1),
            vec!["shop.test".to_owned(), "www.shop.test".to_owned()]
        );
    }

    /// And the reading stops at the first step that is not a name, rather than sweeping up every
    /// domain in the plan.
    #[test]
    fn the_reading_stops_at_the_first_step_that_is_not_a_name() {
        let plan = a_plan(vec![a_site(), step(Disposition::Create)]);

        assert!(names_after(&plan, 0).is_empty());
    }
}
