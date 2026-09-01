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

use mixengine_proto::{BlueprintApply, BlueprintApplyResponse, Error, ErrorCode};

use super::Api;

impl Api {
    /// `blueprint.apply` — what applying one would do, and (from this task) doing it.
    ///
    /// # Errors
    ///
    /// `not_found` for a blueprint nothing is filed under; `invalid_argument` for a root that is not
    /// absolute; and the wire error of a table that could not be read.
    pub(crate) async fn blueprint_apply(
        &self,
        asked: &BlueprintApply,
    ) -> Result<BlueprintApplyResponse, Error> {
        let (_manifest, plan) = self.blueprints.planned(asked).await?;

        if asked.dry_run {
            return Ok(BlueprintApplyResponse::Planned { plan });
        }

        Err(Error::new(
            ErrorCode::PreconditionFailed,
            "this build plans an apply but does not carry one out",
        )
        .with_hint("`--dry-run` prints the plan; executing it arrives with roadmap task T78"))
    }
}
