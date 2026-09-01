# T78a — scaffold trust (design)

Roadmap task **T78a**, phase 8. T77 put a blueprint's own command in the plan and T78 applied
everything around it, ending that one step as `NotRun` with a sentence naming this task. This task
runs it — and decides, first, whether it may.

The sentence with teeth is D1: **trust is decided when a blueprint arrives and is never raised
afterwards**, which is what "a hand-imported one is marked untrusted for good" means when it is a
column rather than a promise.

## Goal

`[scaffold]` is arbitrary code from whoever wrote the blueprint. After this task it runs — in the new
project's directory, with the project's own runtime on PATH, output streamed while it runs — and it
runs only after somebody has agreed to the exact command, per apply, never on import. A blueprint
that arrived signed by the gallery key is trusted; one somebody handed over is untrusted for the rest
of its life, and `mix` asks for a different gesture before running its command.

## Scope

In: `blueprint.import`, which is the only thing that can produce a blueprint this machine did not
write; minisign verification against a gallery key of its own; the `trusted` column and its
migration; scaffold consent on the wire and its enforcement; executing the command as a supervised
one-shot with its output on the log surface; a second kind of log subject (`GET /logs/job/{id}`) and
`mix job logs`; `mix blueprint import`, the two scaffold flags on `mix blueprint apply`, and the
trust marking in `mix blueprint list`.

Out: the gallery itself — T79 is what ships signed blueprints and what generates a `.minisig` in the
packaging repository; this task only compiles in the public half and can verify. `blueprint.export`:
the manifest is already on disk at `blueprints/<slug>.toml` and copying a file is not a daemon
method. `blueprint.delete`, still.

## Decisions

**D1 — Trust is decided when a blueprint arrives, and nothing raises it.** A new
`blueprints.trusted` column, written once by whatever put the row there: `builtin` and `captured` are
trusted (the first is this build's own, the second is this machine's own), and `imported` is trusted
only when a detached minisign signature verified against the compiled-in gallery key. There is no
method that flips it and no flag that overrides it — an untrusted blueprint is untrusted for good,
which is the whole of what the roadmap line promises.

**It is not re-verified at apply time, and that is a departure from
[`index.rs`](../../../crates/mixengine-core/src/index.rs), which re-verifies a document it wrote a
minute ago.** The difference is what is stored: the index keeps the signed bytes and verifies those,
while a blueprint's truth is the `manifest_toml` row (the T77 design, D7) and the file beside it is a
rendering. Re-verifying would mean either keeping a second copy of the bytes as a shadow source of
truth, or verifying a signature against a rendering — and a check that can fail for a reason having
nothing to do with tampering is a check that gets turned off.

**D2 — The gallery has a signing key of its own.** `blueprints::trust::PUBLIC_KEY`, minisign, minted
and kept the way the index key is: the public half committed to the packaging repository beside
`minisign.pub`, the private half in that repository's Actions secrets and a backup outside every
working tree — never in a checkout, which is what its `.gitignore` says in as many words. Reusing
`index::PUBLIC_KEY` would make one compromise cost both the package index and the right to run
arbitrary code on every user's machine; those are different blast radii and they get different keys.
The price is one more secret and one more password to keep, paid once.
Rotating it needs an application release, exactly as the index key does and for the same reason: a
key the artifact itself could announce is a key an attacker serving the artifact could announce.

Verification is a free function taking the key, on
[`Catalog`](../../../crates/mixengine-core/src/index.rs)'s shape, so a test signs a fixture with
`minisign` (already a `mixengine-testkit` dev-dependency) and hands in its own public half. A
compiled-in constant no test can replace is a constant no test exercises.

**D3 — `blueprint.import` is the only producer of `imported`.** It takes an absolute path, an
optional signature path (defaulting to `<path>.minisig` when that file exists), an optional slug and
the same `overwrite` flag `blueprint.capture` has. It reads the bytes, verifies them if a signature
is there, parses the manifest with the parser T77 wrote, validates the slug with
[`store::validated_slug`](../../../crates/mixengine-core/src/blueprints/store.rs) — the security
boundary that keeps a name from escaping the blueprints directory — and writes the row plus its
rendering. A manifest that does not parse is refused naming what was wrong with it; a signature that
does not verify is **not** a refusal but an import that lands untrusted, because a file whose
signature is stale is still a file its owner may want to use, and saying so is more useful than
throwing it away.

The slug is the `--name` when one was given and the manifest's `[blueprint] name` otherwise.

**D4 — Consent travels in the request and names the exact command.** `BlueprintApply` gains
`scaffold: Option<ScaffoldConsent>`, and `ScaffoldConsent` carries the command the person read and
whether they were told it was untrusted. This is T78's own answer to a daemon with no keyboard, one
task on: a job that stopped halfway to ask would be a job holding a project directory hostage.

Three readings, in this order:

- **No consent, and the plan has a scaffold**: the step ends `NotRun { why }` and everything else is
  applied. T78's D11 position, kept — a blueprint with a scaffold must not become worthless because
  nobody answered one question.
- **Consent whose `command` is not the plan's**: the whole apply is refused before anything happens,
  the way an answer to a question the plan does not ask is refused. A blueprint can be re-imported
  between the plan a person read and the apply they sent; a consent that names the old command is
  consent to something else.
- **Consent whose `untrusted` disagrees with the row**: refused the same way, and this is the case
  that matters — a blueprint re-imported without its signature between the reading and the sending
  would otherwise run under a consent given for a signed one.

**D5 — The plan says which blueprint it is and whether it is trusted.** `BlueprintPlan` gains
`source: BlueprintSource` and `trusted: bool`. Two facts rather than one because they are
independent: a gallery file imported by hand is `imported` and trusted, and a build that derived one
from the other would have to lie about that case. Carrying them on the plan is what lets a graphical
client show the right warning from the answer it already has, rather than making a second call the
CLI does not need — the client-surface rule, applied before there is a client to apply it to.

**D6 — `{project}` is expanded into the scaffold command, and T77 did not do it.**
[`plan.rs`](../../../crates/mixengine-core/src/blueprints/plan.rs) clones `scaffold.command`
verbatim while every other value it emits goes through `expand()`, so
`composer create-project laravel/laravel {project}` reaches the plan, the confirmation and the shell
with the token still in it. The expansion moves to where the others are — once, in core, so no later
branch can expand it a second time — and the command a person agrees to is therefore the command that
runs. The substitution is safe to put in front of a shell because the project name has already been
through the slug charset, which has no shell metacharacter in it.

**D7 — A step that ran and failed is a fourth outcome.** `StepResult::Failed { why }`, beside `Done`,
`AlreadyTrue` and `NotRun`. The job **succeeds** and carries a complete `BlueprintApplied`; `mix`
reads the outcomes and exits non-zero when one of them is `Failed`.

The alternative — failing the job — throws away the report: a failed job carries an `Error` and no
result, so a scaffold that exited 1 would take the record of the nine steps that worked with it. And
the job did do what it was asked: it applied the blueprint and ran the command, and the command's own
exit code is the command's news, not the apply's.

**D8 — A failed scaffold rolls back nothing, and the ledger is not spent.** T78's D4 unwinds when a
failure leaves a project that does not work; a scaffold that exited non-zero leaves a project that
does — the site serves, the database is there, the pins are set. Destroying that because a
`composer` post-install script failed is the more expensive direction to be wrong in, and it is the
position [`site.create`](../../../crates/mixengine-daemon/src/api/create.rs) already takes for a
certificate that would not issue. Running the apply again re-offers the command.

**D9 — Through the OS shell, spawned as a supervised group.** `cmd.exe /C <command>` on Windows and
`/bin/sh -c <command>` elsewhere, behind a new `mixengine-platform::process` entry point. The string
is what was confirmed and the shell is what makes `composer install && npm ci` mean what whoever
wrote the blueprint meant; a hand-rolled argv split would be a quoting rule of MixEngine's own,
documented nowhere the blueprint's author is looking.

Spawned with [`spawn_supervised`](../../../crates/mixengine-platform/src/process.rs) rather than
`run_once`, for the process group: `composer` starts children, and `job.cancel` has to stop the tree
rather than orphan it. A cancellation kills the group and reports the step as
`NotRun { why: "it was cancelled" }` — T78's rule that a cancellation leaves what was made and is not
a request to delete anything.

**D10 — No timeout.** Every number that could be picked here kills a legitimate
`composer install` on a slow line, and a scaffold is by definition somebody else's program doing an
unknown amount of work. The bound is the job: it is visible in `job.list`, its output is streaming,
and `job.cancel` stops it. This is stated rather than left implicit because every other process this
daemon starts one-shot has a deadline.

**D11 — What the command sees, and what it does not.** The working directory is the project root.
`PATH` is `<home>/bin` — the shim directory, whose shims resolve the version from the project they
are run in, which is how the blueprint's `[runtimes]` reaches the command without anything computing
a runtime path here — prepended to the daemon's own `PATH`. Nothing else is invented.

**It never touches the elevation queue.** A scaffold runs under the user's own account, with no path
to `mixengine-elevate` and nothing queued on its behalf. T78's "one prompt, at the end" is about the
hosts file and the trust store; a blueprint's command is not admitted to it.

**D12 — The Windows command line takes the command as a raw tail.** This spawn does not go through
`std::process::Command` at all:
[`windows/restricted.rs`](../../../crates/mixengine-platform/src/windows/restricted.rs) builds the
`CreateProcess` line itself, quoting each argument the way `CommandLineToArgvW` parses it back —
which is the right rule for a program and the wrong one for `cmd.exe`, whose own parser does not
honour a backslash-escaped quote. A scaffold command with a quote in it would arrive mangled.

So the shell spawn appends the command **verbatim** after `cmd.exe /C `, which is what
`Command::raw_arg` does on the ordinary path: one entry point of its own in `restricted`, rather than
a flag threaded through the argument quoting that every service start depends on. The existing
comment there — that `cmd /c` strips the outer quotes only when the whole line carries exactly two —
is the reason the tail is appended rather than wrapped.

The alternative considered and dropped: writing the command to a temporary `.cmd` and running the
file — no quoting problem, at the cost of a temporary artifact and batch's own `%` expansion, which
is a second distortion in place of the first.

**D13 — The log surface grows a second kind of subject.** The registry in
[`services/logs.rs`](../../../crates/mixengine-daemon/src/services/logs.rs) is keyed by a
`LogSubject { Service(ServiceId), Job(JobId) }`, and the route becomes `GET /logs/service/{id}` and
`GET /logs/job/{id}` — two segments always, so nothing has to decide whether a first segment is a
package name or a word. The ring, the frames, the `Gap` a slow reader is told about and the
per-connection back-pressure are the ones that are already there.

This is [ADR 0009](../../../.claude/decisions/0009-logs-travel-on-their-own-stream.md) applied rather
than amended: the volume is decided by somebody else's program, which is exactly what that decision
keeps off the event stream. Putting scaffold output on `JobProgress` would spend every connected
client's 1024-message allowance on one chatty `npm install`.

**A job's log is the ring and no file.** A service's `current.log` is written by the supervisor and
recovered when the daemon's ring is empty; a job has no such file, and giving it one would be a
directory per job on a machine that never prunes the `jobs` table. What survives the ring is the last
of the output quoted into `Failed { why }`, which is the part somebody needs after the terminal has
scrolled.

**D14 — `mix job logs <id> [-f] [--tail N]` exists because the endpoint does.** The rule is the
repository's: a client-only capability is a gap in the product. It answers nothing for a job that
prints nothing, which today is every job but this one, and it says so rather than looking broken.

**D15 — Two flags, and neither one is `--yes`.** `mix blueprint apply --run-scaffold` runs a trusted
blueprint's command without asking; `--run-untrusted-scaffold` is what an unsigned one needs. Two
names rather than one flag and a modifier, so that a script that runs somebody's unsigned command
says so in the line that does it, and so no blanket agreement can grow to cover it later. Neither is
implied by any other flag.

Interactively, `mix` prints the command and asks, through
[`confirm.rs`](../../../crates/mixengine-cli/src/confirm.rs).

**Where there is nobody to ask, the command is left rather than the apply refused, and that is a
departure from `answered`'s `Unanswerable` rule — found by building it.** A version question has no
safe default: the two answers leave different machines, so a `--json` run with one outstanding is
refused. This question does have one, and there is no flag for *no*: refusing a closed standard input
would make "apply this blueprint without its command" impossible from a script, which is the ordinary
case in CI. So `--json`, and an end of file, leave the step unrun with a line on stderr naming the
flag that would have agreed to it. Nothing is read as agreement either way, which is the half of the
rule that mattered.

**D16 — The rendering is not the signed artifact.** After an import, `blueprints/<slug>.toml` is
rendered from the row, so a person checking the `.minisig` against *that* file may find it does not
verify — comments and key order are the author's, and the renderer's are the renderer's. Nothing
depends on it (D1), and a test asserts that a gallery-shaped manifest survives a round trip byte for
byte, which is what T77's byte-identical renderer was for.

## Data model

Migration `0014_blueprint_trust.sql`:

```sql
ALTER TABLE blueprints ADD COLUMN trusted INTEGER NOT NULL DEFAULT 0;
UPDATE blueprints SET trusted = 1 WHERE source IN ('builtin', 'captured');
```

Every row that exists on any machine today is `captured` — nothing else can write one until this task
ships `blueprint.import` — so the `UPDATE` is what keeps this build's own blueprints where they
were. The `DEFAULT 0` is for the direction a mistake should fall in.

## API

`mixengine-proto`:

- `BlueprintImport { path, signature: Option<String>, name: Option<String>, overwrite: bool }`,
  answered with the `BlueprintSummary` of the row that was written.
- `rpc::method::BLUEPRINT_IMPORT = "blueprint.import"`.
- `BlueprintSummary` gains `trusted: bool` (`#[serde(default)]`, so a missing field reads as
  untrusted — the safe direction).
- `BlueprintPlan` gains `source: BlueprintSource` and `trusted: bool` (D5).
- `BlueprintApply` gains `scaffold: Option<ScaffoldConsent>`;
  `ScaffoldConsent { command: String, untrusted: bool }` (D4).
- `StepResult` gains `Failed { why }` (D7). The enum is `#[non_exhaustive]` and tagged in-object.
- `LogSubject { Service(ServiceId), Job(JobId) }`, and the two log routes it names (D13).

## CLI

```
mix blueprint import <FILE> [--name <NAME>] [--signature <FILE>] [--overwrite] [--json]
mix blueprint apply  <BLUEPRINT> --project <NAME> [--path <DIR>] [--dry-run]
                     [--install-missing | --use-installed]
                     [--run-scaffold | --run-untrusted-scaffold] [--grant] [--no-wait] [--json]
mix job logs <JOB> [-f] [--tail <N>]
```

`mix blueprint list` marks an untrusted blueprint on its row; the marking is a word, not a colour,
because the JSON output carries the same fact.

`mix blueprint apply` plans, prints the plan, asks the version questions it already asks, and then —
if the plan holds a scaffold — prints the command exactly as it will run, says whether the blueprint
was signed, and asks. While the job runs it opens that job's log stream and prints the lines beside
the progress it already follows. At the end the per-step outcomes print as they do now, with the
scaffold's exit code where its line is, and the process exits non-zero if any step is `Failed`.

## Testing

`mixengine-core`: a signature that verifies and one that does not, both against a key the test
supplies; an import landing `trusted` and one landing untrusted; the absence of any path that raises
trust, asserted as the store writing the column once; the slug rules an import inherits; `{project}`
expanded into the scaffold command exactly once (D6); and a gallery-shaped manifest rendering back
byte for byte (D16).

`mixengine-daemon`, in the executor's own `#[cfg(test)]` module: the consent decision as a table over
(plan, consent, trusted) — no consent is `NotRun`, a mismatched command is a refusal before anything
happens, a mismatched trust flag is a refusal, and a matching consent is work; a failed scaffold
leaving the ledger unspent (D8); and the log registry answering a `Job` subject.

`mixengine-platform`, in `tests/process.rs` beside the one-shot tests: a shell command that prints
and exits 0, one that exits non-zero, and a command with quotes in it arriving intact — the last is
D12's assertion and the one that only fails on Windows.

`mixengine-cli`, in `tests/blueprint.rs` against a real daemon: import a hand-written blueprint with a
scaffold, apply it with `--run-untrusted-scaffold`, and assert the file the command wrote is in the
project directory and the step says `Done`; the same apply without the flag leaving the step `NotRun`
and the project otherwise complete. The command is written per OS by the test, which is the honest
consequence of D9.

Render tests for the confirmation, the `Failed` outcome, and the untrusted marking in the listing.

## Dependencies

T77 for the manifest, the store and the plan; T78 for the executor, the ledger and the job; T16b for
the log ring and its endpoint; T21 for the job registry and cancellation; T64 for `confirm.rs`. The
packaging repository's Actions secrets take the private half of the new key (D2), which is a step
outside this repository and blocks nothing here — verification is testable with a key a test makes,
and what this build compiles in is only the public half.

## Risks

**This is the first thing MixEngine runs that MixEngine did not write.** Every mitigation is a
decision above: consent per apply naming the exact command (D4), a trust mark that cannot be raised
(D1), no elevation (D11), no PATH invention (D11), and a process group that a cancellation can
actually stop (D9). What remains is the honest residue: somebody who agrees to a command has agreed
to it.

**The Windows shell path is where this will break.** Quoting (D12) is the known one; the unknown one
is a `cmd.exe` that inherits a console the daemon does not have. The platform test with quotes in the
command is what catches the first, and the CLI test running a real command on a real daemon is what
catches the second — and per the working agreements, the per-OS files are compiled in WSL before this
is pushed.

**The log key change touches a shipped endpoint.** `GET /logs/{service_id}` becomes
`GET /logs/service/{id}`. `mix` is the only client in this repository, so the cost is one line there;
it is named here because it is the kind of change that is cheap now and expensive after T79 ships
anything that reads logs.

## Text that this task makes wrong

- [`features/blueprints.md`](../../../.claude/features/blueprints.md) — the scaffold section becomes
  what was built: where consent lives, what untrusted costs, and that capture still never writes one.
- [`features/client-surface.md`](../../../.claude/features/client-surface.md) — a graphical client
  gains the scaffold confirmation and the untrusted marking as obligations, and `blueprint.import`
  as a method it must reach.
- [`proto/blueprint.rs`](../../../crates/mixengine-proto/src/blueprint.rs) — `RunScaffold` and
  `BlueprintSource` both name T78a as the task that will decide this; it has.
- [`core/blueprints/plan.rs`](../../../crates/mixengine-core/src/blueprints/plan.rs) — the comment
  saying T78a is what gates the step, and the missing expansion (D6).
- [`daemon/api/apply/steps.rs`](../../../crates/mixengine-daemon/src/api/apply/steps.rs) — the
  `Confirm` arm's sentence about running it yourself.
- [`cli/main.rs`](../../../crates/mixengine-cli/src/main.rs) — the `BlueprintCommand` note that
  `import` is deliberately absent because importing is where T78a's marking lives.
- [`roadmap/phase-8-differentiators.md`](../../../.claude/roadmap/phase-8-differentiators.md) — T78a
  ticked, with what it found in T77's plan (D6) written where the next reader will look.
