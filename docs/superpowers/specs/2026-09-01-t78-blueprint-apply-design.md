# T78 — executing a plan (design)

Roadmap task **T78**, phase 8. T77 decided what applying a blueprint would do and refused to do it;
this task carries the list out. The refusal it replaces is a typed `PreconditionFailed` in
[`daemon/blueprints.rs`](../../../crates/mixengine-daemon/src/blueprints.rs) that names this task by
number.

## Goal

`blueprint.apply { dry_run: false }` turns a `BlueprintPlan` into a project on this machine: a
registered project with its runtime pins, the services it names, the database and account it wants,
a site with its domains and certificate, and the PHP extensions it needs — as a job with progress,
resumable when it fails, and rolled back to what belonged to the project when it fails badly.

The two sentences with teeth are D2 (resumption is a re-plan, so idempotence is measured against the
world and not against a ledger) and D4 (a rollback removes what belongs to the project and keeps
what belongs to the machine).

## Scope

In: execution of every `PlanAction` T77 defined except `RunScaffold`; two actions T77 did not define
and cannot apply correctly without (D7, D8); the version-mismatch answer on the wire; the job, its
progress and its ending; rollback; and `mix blueprint apply` without `--dry-run`.

Out: running a `[scaffold]` command and everything about trusting one — T78a (D11). The built-in
gallery — T79. Importing a blueprint somebody else wrote: there is still no `blueprint.import` in
this build, so every blueprint an apply can reach was captured here.

## Decisions

**D1 — The plan is core's, the execution is the daemon's. T77a's division, one table across.**
`mixengine-core` reads this home's tables and decides the list; every action in that list is a
capability the daemon already has, and half of them (an install, a rendering, a supervisor reload, a
keyring write) are things `mixengine-core` deliberately cannot do. So the executor is
`daemon/src/api/apply.rs`, written as `impl Api` — the arrangement
[`api/create.rs`](../../../crates/mixengine-daemon/src/api/create.rs) already uses for
`service.create`, and for the same reason: `Api` is the one type holding `projects`, `runtimes`,
`packages`, `sites`, `domains`, `certificates`, `extensions` and `databases` at once. A `Blueprints`
struct given eight more fields would be a second assembly of the same handles.

`Blueprints` keeps planning, listing and capture. `blueprint.apply` enters through `Api`, which
answers a plan from `Blueprints` for a dry run and starts a job for a real one.

**D2 — Resumption is a re-plan. There is no ledger, no table and no migration.** Running
`blueprint.apply` a second time plans against the state the first one left: everything already done
comes back `Satisfied` and is skipped, and what remains is exactly what remains. This is T77a's
position one task on — the server is the record, the tables are the record — and it has one property
a ledger cannot have: it is also correct after a daemon restart, a manual `mix site create`, or a
partial cleanup somebody did by hand. A ledger would be a second source of truth that has to be
reconciled against the first one anyway.

What it costs is one honesty fix in [`plan.rs`](../../../crates/mixengine-core/src/blueprints/plan.rs),
which today cannot tell *already ours* from *somebody else's*:

- `register`: a project of this name at this root is `Satisfied`. The same name at a different root,
  or this root under a different name, stays `Blocked` — those are two projects colliding, which is
  what the block was written for.
- `domain_step`: a domain owned by a site of *this* project is `Satisfied`; owned by any other site
  it stays `Blocked`, naming the owner.

Both readings are strictly narrower than "the name is taken", and the tests say so in both
directions.

The consequence for `--dry-run` is a feature rather than a wrinkle: on a half-applied home the plan
prints what is left to do.

**D3 — Every action is an ensure, and one call may satisfy several steps.** The executor asks the
world before it acts, so a step that is already true costs a read. It walks `plan.steps` in the
order the plan gives, adds nothing, drops nothing, reorders nothing — the invariant T77 wrote down —
but a single daemon call may make more than one step true at once. `sites::create` writes the row,
queues the hosts entry, issues the certificate and reconfigures the front end in one call, so the
`AddDomain` and `IssueCertificate` steps that follow find themselves already satisfied when their
turn comes.

**A step is reported by what became true, not by how many calls it took.** That is the sentence
that keeps the acceptance criterion honest: `--dry-run` promises the same *actions*, and it delivers
them.

**D4 — A rollback removes what belongs to the project and keeps what belongs to the machine.** The
executor keeps a ledger in memory, and on failure undoes it in reverse: the site, then a service
instance dedicated to this project, then the project row.

Kept, and each one named in the failure:

- **the database**, which the feature doc already settled: by the time an apply has failed something
  may have migrated into it, and there is no `database.drop` in this product;
- **a runtime or package this apply installed**, because those belong to the machine and are what a
  resumed apply would otherwise download again — throwing away eighty megabytes to tidy up is the
  expensive direction to be wrong in;
- **a PHP extension it turned on**, for the reason the plan renderer already says on that line: the
  choice reaches every project on the machine, and turning it back off would change somebody else's
  PHP;
- **the project directory**, on this house's standing rule — `project.delete` and `site.delete` keep
  the files and name them, because the files were never ours.

**The ledger records intent, not success.** `sites::create` deliberately keeps the row it wrote when
the rendering fails ("a declaration rolled back because the rendering failed would leave a person
with nothing to fix"), so a ledger written only after `Ok` would miss precisely the failures a
rollback exists for. The entry goes in before the call, and every undo is "remove it if it is there
and it is ours". A rollback that fails is logged; what the job reports is the original error.

**D5 — `job.cancel` stops, and does not roll back.** A cancellation is a person saying *stop*, not a
person asking for a cleanup, and D2 makes stopping safe: what has been done is done, and running the
apply again continues from there. Rollback is for a failure, where nobody asked for anything and the
half-made project is nobody's intent. The ending says which steps were done and that a second run
continues.

**D6 — The mismatch answer travels in the request, one per subject, and an unanswered question is a
refusal before anything happens.** `Disposition::Choice` is a question; a daemon has no keyboard, and
a job that stopped halfway to ask one would be a job holding a project directory hostage. So
`BlueprintApply` carries `answers`, the daemon re-plans, and:

- a `Choice` step with no answer refuses the whole apply, listing every question that is unanswered;
- an answer matching no question refuses too, naming it — somebody answering a question nobody asked
  is somebody holding a plan that has since changed.

The subject is a tagged value and not a bare string: `{ "runtime": "php" }` and
`{ "service": "mariadb@main" }` are two namespaces, and one string field holding both is one collision
away from applying an answer to the wrong thing. A service `Choice` only ever arises for an instance
that already exists, so its id is always spellable — which is why the answer may carry a `ServiceId`
where the action itself deliberately carries the package and the instance apart.

"Cancel", the third answer in the feature doc's sentence, needs nothing on the wire: it is not
sending the apply.

**D7 — The answer decides the pin, and `RegisterProject` carries the pins.** T77's action holds a
name and a root, which is not enough to produce the project the blueprint describes:
`ProjectCreate.pins` is what makes `[runtimes] php = "8.2.23"` true on the receiving machine, and
without it the new site resolves to whatever PHP this machine defaults to. Two things break at once
— the site runs the wrong runtime, and `blueprint.capture` on the applied project comes back empty,
because [`capture`](../../../crates/mixengine-core/src/blueprints/capture.rs) keeps only what
`resolve` reports as *not* the default.

It is also what makes D6 mean anything. Without a pin, "install 8.2.23" and "use the installed
8.2.29" produce identical machines and the question is theatre. With it, the answer is the pin:
`install` pins what the blueprint asked for, `use_installed` pins the version this machine has.

So `PlanAction::RegisterProject` gains `pins`, and the dry run prints them.

**D8 — The plan gains `InstallPackage`, because `EnsureService` cannot stand on a package that is not
there.** T77 planned `InstallRuntime` for languages and nothing at all for service packages, and
`service.create` refuses with `precondition_failed` when that version of MariaDB is not installed.
That is a plan getting four steps into a project directory before discovering the fifth was
impossible, which is exactly what D10 of the T77 design exists to prevent — and it is the ordinary
case for the feature's headline scenario, a blueprint from a teammate's machine.

`PlanAction::InstallPackage { package, version }` is decided from `packages::records` and sits
immediately before the `EnsureService` it serves, with the same three dispositions `InstallRuntime`
has: `Satisfied` when something installed matches, `Create` when nothing does, `Choice` when another
version is installed. Execution reuses `packages::perform`, the inner half of `package.install`.

**D9 — Nothing is written until every version has been resolved.** A plan holds a
`VersionConstraint` on purpose (T77 D9: no index, no network at plan time), and turning one into a
release is a question only the index can answer. So the job's first pass resolves every
`InstallRuntime` and `InstallPackage` against the index and writes nothing; a constraint the index
cannot satisfy fails the job while the ledger is still empty and there is nothing to roll back. It
is the same reasoning as `service_create`'s ordered checks: the cheapest refusal comes first.

**D10 — An apply queues elevation and never raises a prompt.** The standing rule is
[`elevation.rs`](../../../crates/mixengine-daemon/src/elevation.rs)'s: *this daemon never raises a
prompt on its own initiative — producers enqueue, and only a client calls `elevation.grant`.* An
apply is a producer. So the hosts entries and the trust-store work it causes are queued exactly as
`site.create` already queues them, the job never blocks on a dialog nobody is watching, and the
client is what spends the prompt: `mix blueprint apply` reports what is waiting and offers to grant
it, with `--grant` for a script and `confirm.rs` for a person.

`PlanStep.elevates` therefore means *this step will put something in the elevation queue*, and its
documentation is corrected to say so. A dry run still answers the question it was run to answer —
"will this ask for my password" — one sentence further along: yes, once, at the end, if you say so.

**D11 — The scaffold step is left, and named.** `[scaffold]` is arbitrary code from whoever wrote the
blueprint, and deciding whether it may run is T78a's whole subject. This build applies everything
else and ends the step as `NotRun`, carrying the exact command and the directory to run it in. The
alternative — refusing the apply outright, the way T77 refused execution — would make a blueprint
with a scaffold worthless in a build that can create nine tenths of what it describes. Capture never
writes a `[scaffold]`, so nothing this home produced is affected either way.

**D12 — One method, two answers, and a typed result.** `blueprint.apply` answers a tagged union:
`{ "outcome": "planned", "plan": … }` for a dry run and `{ "outcome": "started", "job": … }` for a
real one. T77 argued the single method — the plan a person reads and the plan the daemon carries out
have to be the same list — and a union is what lets that survive the second answer without a client
guessing from its own request.

The job's success value is `BlueprintApplied`: the blueprint, the project, the root, and one
`StepOutcome` per step (`Done`, `AlreadyTrue`, `NotRun { why }`). A failure is a failed job:
`Error` carries a code, a message and a hint and nothing else, so what a rollback left behind — the
database, the directory, an installed runtime — is said in the message, which is where a person
reads it anyway.

**D13 — Progress is a step's slice, and a nested install is scaled into it.** The apply reports
`done / total` steps. An install inside a step reports 0–100 of *itself* through
`install::Watcher`, and handing it the apply's own `JobHandle` would make the bar jump backwards
every time a download started. So the executor passes a wrapper that maps a child's percentage into
the slice belonging to that step. The wrapper is an `impl Watcher` of four lines, which is the same
answer T21 gave when it shaped `Watcher` after `JobHandle` rather than inventing an adapter.

**D14 — The site is where three earlier facts meet.** `site.create` takes its domains and its
service links in the call that creates it, and both matter beyond the moment: a site created without
its links has an empty `site_service_links`, and a capture of the applied project loses every
`[[services]]` entry. So the executor carries a context across the walk. The service ids come from
the `EnsureService` steps it has already visited; the domain names come from the plan's own
`AddDomain` steps, read off the list rather than recomputed, so that there stays exactly one place
where `{project}` was expanded.

Reading the following steps' domains is the one place the executor looks ahead, and it is worth
naming: a site cannot be created nameless, and the alternative — creating it with a default name and
renaming it a step later — would write a hosts entry for a domain nobody asked for.

## Data model

**No new tables and no migration.** D2 is what buys that: the record of what has been applied is the
`projects`, `sites`, `site_domains`, `site_service_links`, `services`, `runtime_installs` and
`packages` rows the apply wrote, which is what a re-plan reads. The only row this task adds is the
`jobs` row every job has.

## API

`mixengine-proto`:

- `BlueprintApply` gains `answers: Vec<VersionAnswer>` (defaulted, so an older client's request still
  decodes).
- `VersionAnswer { subject: AnswerSubject, answer: MismatchAnswer }`, with
  `AnswerSubject::{Runtime(RuntimeKind), Service(ServiceId)}` tagged, and
  `MismatchAnswer::{Install, UseInstalled}`.
- `BlueprintApplyResponse::{Planned { plan }, Started { job }}`, tagged `outcome`.
- `BlueprintApplied { blueprint, project, root, steps: Vec<StepOutcome> }` and
  `StepOutcome { action, result }` with `StepResult::{Done, AlreadyTrue, NotRun { why }}`.
- `PlanAction::RegisterProject` gains `pins: BTreeMap<RuntimeKind, VersionConstraint>` (D7).
- `PlanAction::InstallPackage { package, version }` is added (D8). The enum is `#[non_exhaustive]`
  and the discriminator travels in the object, so an older client renders it as an action it does not
  know rather than failing to decode the plan.

`rpc::method::BLUEPRINT_APPLY`'s documentation loses the sentence about `Unsupported`.

## CLI

```
mix blueprint apply <BLUEPRINT> --project <NAME> [--path <DIR>]
                    [--dry-run] [--install-missing | --use-installed] [--grant] [--no-wait] [--json]
```

Without `--dry-run`, `mix` plans first, prints the plan, and asks each version question as a
three-way — install / use the installed one / cancel — extending `confirm.rs`, whose `Unanswerable`
lesson applies unchanged: a closed standard input with questions outstanding is refused with the
names of the two flags, never treated as an answer. The two flags answer *every* question the same
way, which is what a script wants; a person answering one at a time is what the prompt is for, and
cancelling any single question cancels the apply before it is sent. Then it sends the apply with the answers it
collected and follows the job with the existing `follow()`.

When the job ends, `mix` prints the per-step outcome, the scaffold command if one was left, and — if
the elevation queue is not empty — either asks or, with `--grant`, calls `elevation.grant` and
follows that job too.

## Elevation

One prompt, at the end, spent by the client and never by the apply (D10). What reaches the queue is
what `site.create` already puts there: the hosts entries for the site's domains, and, on a machine
that has never issued a certificate, the trust-store install behind `cert.issue`.

## Testing

`mixengine-core`, against a temporary home: the two new plan readings in both directions (this
project at this root is `Satisfied`; the same name at another root is still `Blocked`; a domain owned
by this project's own site is `Satisfied`, by another site `Blocked`); `InstallPackage`'s three
dispositions; `RegisterProject` carrying the manifest's pins; and the step-order invariant extended
to cover the new action.

`mixengine-daemon`, in `tests/api.rs`: an apply of a captured blueprint under a new name; **a second
apply of the same blueprint that finds nothing left to do**, which is the proof of D2 and D3 at once;
a plan with a blocked step that never creates a job; an unanswered `Choice` refused with the question
in the message; an answer nobody asked for refused; and a failing apply — a service whose package is
absent and whose install is not planned — proving the rollback removes the project and keeps the
directory.

**The round-trip test, which is what catches D7 and D14 cheaply**: capture a fixture project, apply
it under a new name, capture *that*, and assert the two manifests differ only in the header. T77 made
the renderer byte-identical on purpose, and this is the assertion that spends it.

CLI: a render test over `BlueprintApplied`, and one over the three-way question.

## Dependencies

T77 for the plan and the manifest, T77a for `database.create`, T21/T22 for the job registry and the
install watcher, T31a for `service.create`, T40b for the elevation queue, T50 for certificate
issuance, T64 for `elevation.grant` and the confirmation the CLI reuses.

## Risks

**An apply is the first caller that composes half the daemon's API in one job.** A failure mode in
any of those calls now surfaces inside a job that has already written rows. Mitigated by D9 (nothing
is written until the resolvable things are resolved), D4 (the rollback records intent) and D2 (a
failed apply is resumable rather than ruined), and by the failure test being a real one rather than a
mocked error.

**Two plan actions are added to a wire type T77 shipped one release ago.** The enum is
`#[non_exhaustive]`, tagged in-object, and no client outside this repository exists yet; the cost is
one render arm in `mix` and one line in the plan's order test.

**The scaffold gap is visible to a user.** A blueprint with `[scaffold]` applies to nine tenths and
prints a command to run by hand. That is the honest state of a build without T78a, and the sentence
that says so names the task.

## Text that this task makes wrong

- [`features/blueprints.md`](../../../.claude/features/blueprints.md) — the Apply section says
  rollback is "limited to what this apply created"; D4 narrows that to what belongs to the *project*
  and lists what is kept. The version-mismatch sentence gains where the answer is given.
- [`daemon/blueprints.rs`](../../../crates/mixengine-daemon/src/blueprints.rs) — the module note and
  the `PreconditionFailed` refusal naming T78, both removed.
- [`proto/blueprint.rs`](../../../crates/mixengine-proto/src/blueprint.rs) — `PlanStep.elevates` says
  the step "asks the OS for an elevation prompt". It queues one (D10). `Disposition::Choice` says
  "T78 is what asks it"; the client asks, and the request carries the answer.
- [`proto/blueprint_api.rs`](../../../crates/mixengine-proto/src/blueprint_api.rs) — `dry_run`'s note
  that `false` is `Unsupported`.
- [`proto/rpc.rs`](../../../crates/mixengine-proto/src/rpc.rs) — the same, on `BLUEPRINT_APPLY`.
- [`daemon/tests/api.rs`](../../../crates/mixengine-daemon/tests/api.rs) — the test asserting the
  refusal mentions T78; it becomes a test of the apply.
- [`roadmap/phase-8-differentiators.md`](../../../.claude/roadmap/phase-8-differentiators.md) — T78
  ticked, with what it found in T77's plan (D7, D8) written where the next reader will look.
