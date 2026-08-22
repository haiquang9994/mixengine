# T40b — The elevation queue: one prompt for everything that is waiting

*Design, 2026-08-23. Roadmap task [T40b](../../../.claude/roadmap/phase-4-sites-and-elevation.md), Phase 4.*

## What this closes

[T40](2026-08-22-t40-elevate-design.md) built the helper and its file protocol.
[T40a](2026-08-22-t40a-elevation-design.md) built the capability that turns a request lying on disk
into an elevated process reading it. Between them there is a working mechanism that **nothing in this
workspace calls**: `mixengine-daemon` does not mention `mixengine-elevate`, `Elevation` has no caller
outside its own tests, and `ErrorCode::PrivilegedRequired` has carried a doc comment describing this
task since the error set was written.

T40b is the half that decides *when* a prompt is worth spending. It is the sentence
[ADR 0005](../../../.claude/decisions/0005-on-demand-elevation.md) writes as a rule and calls a
defect to break:

> Pending privileged operations are queued and flushed in a **single** elevated invocation.
> Elevating inside a loop is a defect. A declined prompt is a normal outcome, never an error.

So: a durable queue, one invocation over the whole of it, an `ElevationRequired` event, and a
degraded mode that survives being declined — and survives a daemon restart, which is the part a
queue in memory would quietly get wrong.

## What already exists, and is reused unchanged

- `mixengine_proto::privileged` — `PrivilegedRequest`, `PrivilegedOp`, `PrivilegedResponse`,
  `OpOutcome`, `ElevationOutcome`, `RESPONSE_FILE_NAME`. The wire is finished; this task adds one
  method to `PrivilegedOp` (D7) and no new document.
- `ErrorCode::PrivilegedRequired`, which has never been returned by anything. Its own doc comment
  names this task's two states — "either nobody has been asked yet or the user declined" — and D11
  is what finally returns it.
- `Elevation` on `Host`, with `probe()` and `run()`, and `mock::Host::with_home` /
  `declining_elevation` / `unable_to_elevate` / `prompts_raised()`. T40a built that mock explicitly
  as "the surface T40b's queue will be written against"; nothing about it changes.
- The job system (T22): rows, `JobKind`, `JobHandle`, cooperative cancellation, `job.wait`, and the
  boot pass that closes what a stopped daemon left running.
- `getrandom`, already in the workspace, for the nonce.

## Decisions

### D1 — The daemon never raises a prompt on its own initiative

`daemon-and-ipc.md` already writes the rule this task would otherwise have had to invent:

> **A method that writes outside `MIXENGINE_HOME` is never called on the daemon's own initiative**
> (T26).

Everything the helper will ever do — the hosts file, the trust store, resolver configuration, a
firewall rule — is outside `MIXENGINE_HOME` by definition; that is *why* it needs root. So enqueuing
and flushing are two different things with two different triggers. Producers enqueue. Only a client
flushes, by calling `elevation.grant`.

This is also what makes [T64](../../../.claude/roadmap/phase-4-sites-and-elevation.md) expressible at
all: a client cannot print "here is every operation and what each will literally change" *before* the
prompt if the daemon raises the prompt itself. The ordering T64 asks for is a property of this split,
not something the CLI arranges afterwards.

**The consequence, stated rather than discovered:** a fresh install where nobody ever calls
`elevation.grant` is a machine in degraded mode forever, and that is the correct behaviour. It is
also why the pending list is on `daemon.status` (D6) rather than only behind a method nobody thought
to call.

### D2 — The queue is a table, and its unique key is the operation itself

Migration `0007_pending_privileged_ops.sql`:

```sql
CREATE TABLE pending_privileged_ops (
    id           INTEGER PRIMARY KEY,
    op           TEXT    NOT NULL,          -- the serialised PrivilegedOp
    dedupe_key   TEXT    NOT NULL UNIQUE,   -- its canonical form
    requested_at INTEGER NOT NULL
);
```

Durable, because T64 says `mix status` keeps showing the pending list "until it is granted or
dropped" and a restart is neither of those. The daemon restarts on update, on a crash, and on every
`mix daemon shutdown`; a machine that still lacks its hosts entries after one of those is a machine
that should still say so. An in-memory queue would report "healthy" while the user's site stayed
unreachable — the specific failure a degraded mode exists to prevent.

**`dedupe_key` is the canonical JSON of the operation**, and it is where "no code path elevates in a
loop" stops being a matter of discipline. A producer that enqueues the same operation on every start,
on every `site.create`, or inside a retry writes one row. The property is enforced by the schema, so
no caller has to remember it and no reviewer has to check for it. The runtime half of the same rule
is D4.

The constraint is in the schema and not only in the insert. `ON CONFLICT DO NOTHING` is what the
insert says, but it is the `UNIQUE` index that makes a second writer unable to break the rule by
forgetting the clause — and what survives a conflict is the **original** row, with its original
`requested_at`. "Pending since" then reads honestly: the machine has been missing this since the
first time somebody needed it, not since the last time somebody noticed.

**A row this build cannot decode is deleted and logged**, not carried. The only way to produce one is
to downgrade the daemon underneath its own database, and a row no installed build can act on is a
degraded mode nobody can ever clear — the same argument D5 makes for `Unsupported`.

### D3 — Core owns the document, the daemon owns the prompt

The same cut [`crate::jobs`](../../../crates/mixengine-daemon/src/jobs.rs) documents: `mixengine-core`
owns the row and the state machine and has no loop, no clock and no task; the daemon owns the timing,
the cancellation and the events.

Applied here, `mixengine_core::elevation` owns:

- the table, its one reader (`pending`) and its three writers (`enqueue`, `settle`, `discard`),
- **writing the `PrivilegedRequest` into a fresh single-use directory**, and
- **reading `response.json` back and validating it** — version, nonce, and one outcome per operation
  at the same index.

The daemon owns raising the prompt, the `spawn_blocking`, the job, the event, and the
one-grant-at-a-time rule.

The line could have been drawn one step further towards the daemon, leaving core with only the table.
It is drawn here for a reason that is about testing and is worth stating: **`mixengine-daemon` is a
binary crate with no library target**, so nothing outside it can call its functions — as
`crates/mixengine-daemon/tests/api.rs` says in its own opening paragraph. With the document in core,
`crates/mixengine-core/tests/elevation.rs` can build a request with the shipped code, run the **real
`mixengine-elevate`** against it under an ordinary token, and read the report back with the shipped
code. That is a genuine round trip across the two crates, on every developer machine and in the
ordinary `test` job, with no token and no prompt — and it is possible only because T40/D5 made
`Probe` the operation that runs unelevated.

### D4 — A grant is a job, and there is at most one in flight

`Elevation::run` blocks with no deadline, because what it is waiting for is a person reading a dialog.
`daemon-and-ipc.md`: *long operations return a job; never block an RPC call for minutes*. The
exception that `service.start` earns does not transfer — that one is bounded by ready timeouts the
specs themselves declare, and this one is bounded by nothing at all.

So `elevation.grant` answers a `JobSummary` the moment the row exists, kind `elevation.grant`, and
the work runs `host.elevation().run(...)` on `spawn_blocking` exactly as the trait's own doc comment
anticipates.

**A second `grant` while one is in flight is refused with `Conflict`**, not queued behind the first.
That is the runtime half of D2: two concurrent grants are two prompts for one queue, which is the
defect ADR 0005 names, and refusing is the only answer that cannot become a loop. The refusal names
the job that is already running, so a client can wait on it instead.

Cancellation is the job system's, unchanged and cooperative — which here means the token is checked
before the prompt is raised and after it returns, and never during. A `CancellationToken` cannot
close a UAC dialog, and pretending otherwise would leave a job reported as cancelled while the person
at the machine was still looking at a prompt with MixEngine's name on it.

### D5 — What a report does to a row, outcome by outcome

| `OpOutcome` | The row | Why |
| --- | --- | --- |
| `Applied` | deleted | done |
| `AlreadyDone` | deleted | the machine is in the state that was asked for; that is the same outcome |
| `Refused` | deleted, reason kept in the job's result | proto: "the caller's fault, and the same request will be refused again". A row that cannot ever succeed and is never removed is a permanent degraded mode nobody can clear |
| `Unsupported` | deleted, reason kept in the job's result | the installed helper does not know this operation, and it is excluded from auto-update, so it will not learn |
| `Failed` | **kept** | proto: "the OS refused. Trying again may work; nothing about the request is wrong" |

Only one row in that table is kept, and it is the only one whose own type says retrying is
meaningful. The distinction is proto's already; this task's contribution is to not blur it.

**Three outcomes of the elevation itself, before any of that:**

- `Completed` — the helper *ran*. Read the report. **No report is a valid state**, not an
  impossibility: T40a is explicit that `Completed` does not promise a file, because a crash is not a
  per-OS event. The job ends failed with a message that says exactly that, and every row is kept.
- `Declined` — every row kept, the queue unchanged, the job ends **successfully** carrying the word
  `declined`. ADR 0005: a declined prompt is a normal outcome, never an error. A job that failed
  would put a red line in `mix job list` for a person exercising a choice the design offers them.
- `Unavailable { reason }` — every row kept, the job ends failed, and `reason` is what
  `elevation.status` reports from then on. On Linux that string is the whole `pkexec` command to run
  by hand, which T40a built it to be and which is worthless if the daemon drops it here.

The request directory is removed when the job ends, on every one of those branches. It is
single-use by construction — `response.json`'s existence is T40/D10's whole anti-replay check — so
leaving one behind would make the next grant's fresh directory the only thing keeping the property
true, and that is a property worth having in two places rather than one.

### D6 — Degraded mode is derived, never stored

There is no `degraded` column and no flag. *Degraded* means `pending_privileged_ops` is not empty,
computed where it is asked for. A second representation of one fact is a second thing that can be
wrong, and this one would be wrong in the worst direction: stale-clear, reporting a healthy machine
that is missing its hosts entries.

`DaemonStatus` grows one field:

```rust
pub elevation: ElevationSummary,   // { elevated: bool, can_prompt: bool, pending: usize }
```

Three booleans-worth of fact, in the one call every client already makes, so `mix status` can say
"3 operations are waiting for permission" without a second round trip and without a client deciding
what "degraded" means. The list itself — what each operation is and what it will change — is
`elevation.status`, because that is a screen and not a status line.

`can_prompt` is `Elevation::probe()`, which spawns nothing and raises nothing. T40a wrote that method
for exactly two callers and this is the first of them.

### D7 — `describe()` lives on the operation, beside `name()`

T64 must print "what each will literally change — the exact hosts lines, the port, the store". The
alternative shape is a `summary: String` written by the producer at enqueue time and stored in the
row. It is rejected: a description composed beside the operation is a description that can disagree
with the operation, and the row would preserve that disagreement across a restart, through the one
screen whose entire job is to tell the truth before somebody clicks Allow.

So `PrivilegedOp::describe(&self) -> String`, next to `name()` and `requires_elevation()` — derived
from the operation every time it is rendered, and impossible to desynchronise from what will actually
be applied. `Probe {}` describes itself as reporting the installed helper, its token and its audit
log.

It returns `String` and not `&'static str` because the operations that matter carry data —
`HostsApply`'s description is its domains — and a constant would be a shape T41 has to break
immediately.

### D8 — The event carries the batch, read back inside the transaction that wrote it

```rust
DaemonEvent::ElevationRequired { pending: Vec<PendingOp> }
```

The task line says "batch pending ops into one invocation, `ElevationRequired` event", and T64 says
`mix` prints "every operation an `ElevationRequired` batches" — so the batch is what the event is
about, and carrying only the newly-written row would make every client fetch the rest.

The list is read back **inside the transaction that inserted**, which is this repository's existing
rule applied to a value that happens to be plural: `ServiceStateChanged` carries the
`ServiceTransition` the write handed back, so that the row and the event cannot disagree. Here the
persisted thing *is* the list, so the list is what is read and published.

An enqueue that changes nothing — the `ON CONFLICT DO NOTHING` case in D2 — publishes no event. The
machine's needs did not change, and an event per attempt would put a producer's retry loop on a
client's screen.

### D9 — The helper is the file beside `mixengined`, and there is no override

The daemon resolves `mixengine-elevate` next to `std::env::current_exe()`. That is correct in a
workspace build (`target/<profile>/`, where T40a's own tests already look for it) and correct in a
shipped install, where the binaries travel together.

**There is no environment variable and no config key.** A setting that chooses which file is run as
root is a setting that chooses which file is run as root; the directory beside `mixengined` is
already exactly as trustworthy as `mixengined` itself, which is the trust boundary
[security-model.md](../../../.claude/architecture/security-model.md) and ADR 0005 both already
accept, and an override would widen it for nothing. D3's split is what removes the reason anyone
would want one: the round trip is testable in core without a prompt, so no test needs to redirect
what the daemon spawns.

Installing the helper into a root-owned directory is ADR 0005's mitigation and remains T85/T92's;
until an installer puts it somewhere else, "beside the daemon" is both the honest answer and the only
true one.

A helper that is not there is not an error at startup. `can_prompt` is false with that as its reason,
and `elevation.grant` answers `DependencyMissing` — which is D11's other half.

### D10 — An elevated daemon is reported, not refused

The question [T40 recorded and did not answer](../../../.claude/roadmap/phase-4-sites-and-elevation.md).
`is_elevated()` is read once at startup: a `tracing::warn!` line, and `DaemonStatus.elevation.elevated`
so `mix status` and later `mix doctor` (T47) both say it out loud.

**Refusing to start is the answer ADR 0005's first sentence seems to demand, and it is wrong here for
a reason that is measured rather than argued:** CI's whole Windows third runs the daemon suites under
a full administrator token (T2b). A hard refusal would turn one of three platforms red for a reason
that has nothing to do with the code under test, and the usual remedy — an escape-hatch environment
variable — would be set permanently in CI, which is to say the rule would never be checked in the one
place it is checked at all.

What is worth saying about an elevated daemon is not that it cannot elevate. It is that every
supervised service inherits that token: an nginx and a MariaDB running with administrative rights,
writing files into a user's home as an administrator. That is a supervision concern and a `mix doctor`
finding, and reporting it plainly is what makes it one.

### D11 — `PrivilegedRequired` finds its first user, and `DependencyMissing` is a different sentence

Four states a caller must not confuse, and they are not all the same code:

| State | Code | Sentence |
| --- | --- | --- |
| Operations are pending; nobody has been asked, or the user declined | `PrivilegedRequired` | "this needs permission — `mix elevation grant`" |
| `mixengine-elevate` is not beside the daemon | `DependencyMissing` | "the helper is not installed; nothing can be granted" |
| A machine that cannot raise a prompt at all | `PrivilegedRequired`, carrying `probe()`'s reason | on Linux, the `pkexec` command to run by hand |
| A grant while one is in flight | `Conflict` | names the running job |

`PrivilegedRequired`'s doc comment has described the first row since the error set was written and
nothing has ever returned it. The third row is deliberately the same code as the first, not
`UnsupportedPlatform`: the operation is not unsupported on this platform, it is unperformed on this
machine, and the difference is the reason string that tells the user what to type.

## The interface

**`mixengine-proto`**, new module `elevation`:

```rust
pub struct PendingOpId(pub i64);

pub struct PendingOp {
    pub id: PendingOpId,
    pub op: PrivilegedOp,       // decoded; a row this build cannot decode is deleted, not carried
    pub description: String,    // PrivilegedOp::describe(), rendered here so a client needs no logic
    pub requested_at: Timestamp,
}

pub struct ElevationSummary {   // on DaemonStatus — D6
    pub elevated: bool,
    pub can_prompt: bool,
    pub pending: usize,
}

pub struct ElevationStatus {    // elevation.status
    pub elevated: bool,
    pub can_prompt: bool,
    pub reason: Option<String>, // why not, when can_prompt is false
    pub helper: Option<String>, // where the helper was found
    pub pending: Vec<PendingOp>,
    pub last: Option<GrantOutcome>,
}

pub struct GrantOutcome {       // what the most recent grant did
    pub job: JobId,
    pub at: Timestamp,
    pub outcome: ElevationOutcome,
    pub applied: usize,
    pub still_pending: usize,
}

pub struct ElevationDrop {      // elevation.drop
    pub op: Option<PendingOpId>, // None drops all of them
}
```

and `DaemonEvent::ElevationRequired { pending: Vec<PendingOp> }`, internally tagged and flat like its
neighbours.

**`last` is the only thing here that does not survive a restart, and deliberately.** It is what *this
daemon* did the last time it was granted, held in memory beside the in-flight slot; the durable fact
is the queue, and a persisted "you declined once" would outlive the reason it was true. A daemon that
has just started answers `None` and the pending list says everything a client needs.

`PrivilegedOp::describe(&self) -> String` — D7.

**`mixengine-core`**, new module `elevation`:

```rust
pub async fn enqueue(store: &Store, op: &PrivilegedOp, at: Timestamp) -> Result<Option<Vec<PendingOp>>>;
pub async fn pending(store: &Store) -> Result<Vec<PendingOp>>;
pub async fn discard(store: &Store, which: Option<PendingOpId>) -> Result<usize>;
pub async fn settle(store: &Store, results: &[(PendingOpId, OpOutcome)]) -> Result<Settled>;

pub struct Settled { pub applied: usize, pub refused: Vec<(PendingOpId, String)>, pub kept: usize }

pub fn write_request(directory: &Path, home: &Path, ops: &[PendingOp]) -> Result<Request>;
pub fn read_report(request: &Request) -> Result<PrivilegedResponse>;
```

`enqueue` answers `None` when the row was already there (D8: no event). `Request` holds the
directory, the path and the nonce, so `read_report` checks the nonce against the request it belongs
to rather than against something a caller remembered.

`discard` and not `drop`: the wire verb stays `elevation.drop`, but a free function called `drop` in
a module every caller imports is a function shadowing the one in the prelude, and the confusion is
not worth the symmetry.

**`mixengine-daemon`**, new module `elevation`, in the shape of `jobs.rs`: an `Elevation` registry
holding the `Store`, the `Events`, the `Arc<dyn Host>`, the `Jobs`, and the single in-flight grant.

**Methods** — a thirteenth namespace in `daemon-and-ipc.md`'s table, on the precedent `job.*` and
`path.*` set:

```
elevation.*  status, grant, drop
```

**CLI** — `mix elevation status|grant|drop`, enough that no capability is client-only. `grant` follows
its job the way `mix runtime install` already follows one. T64 is what turns `status` into the screen
that explains every operation before `grant` raises anything.

## Crate changes

**`mixengine-proto`** — the `elevation` module, one method on `PrivilegedOp`, one `DaemonEvent`
variant, one field on `DaemonStatus`. No new dependency.

**`mixengine-core`** — the `elevation` module, migration `0007`, and `sqlx::query!` call sites, so
`cargo sqlx prepare` runs. No new dependency; `getrandom` is already in the workspace.

**`mixengine-daemon`** — `src/elevation.rs`, three RPC methods, one field on the API bundle, the
`is_elevated()` reading at startup. `mixengine-platform` is already a dependency with `default`
features, which is what carries `Elevation`.

**`mixengine-cli`** — one subcommand tree, both renderings.

**No change** to `mixengine-elevate`, `mixengine-supervisor` or `mixengine-shim`. The helper is
finished; this task is its first caller.

## Testing

**Unit, in `mixengine-core`.** `enqueue` twice writes one row and keeps the first `requested_at`;
`pending` decodes and describes; `drop` by id and drop-all; `settle` applies D5's table outcome by
outcome, including the one row that survives.

**`crates/mixengine-core/tests/elevation.rs` — the real round trip, no token, no prompt.** Build a
request with `write_request`, run the real `mixengine-elevate` binary against it directly (found
beside the test binary, the way `crates/mixengine-platform/tests/elevation.rs` finds it), read it
back with `read_report`. What this proves is the seam nothing else touches: that the document the
daemon will write is one the shipped helper accepts, and that the report the shipped helper writes is
one the daemon can read. Possible only because T40/D5 lets `Probe` run unelevated.

**Unit, in `crates/mixengine-daemon/src/elevation.rs`, against `mock::Host`.** This is where T40a
pointed the mock:

- **enqueue three operations, one `grant`, `prompts_raised().len() == 1`** — and the one prompt names
  the request the daemon had just written. That is the task line's "no code path elevates in a loop",
  asserted rather than asserted-about.
- a second `grant` while the first is in flight is `Conflict`.
- `declining_elevation` — the queue is unchanged, the job succeeded, the word is `declined`.
- `unable_to_elevate("no polkit agent")` — the queue is unchanged, and the reason survives into
  `elevation.status` intact, because on Linux it is a command a person is meant to type.
- `Completed` with no report beside the request — the mock writes none, which makes this the default
  rather than a case somebody had to think to write. Job failed, queue kept.

**`crates/mixengine-daemon/tests/elevation.rs`, over a real socket.** `elevation.status` reports what
was enqueued, `daemon.status` carries the summary, and `elevation.drop` empties it.

The `DependencyMissing` leg needs a daemon with **no helper beside it**, which a workspace build never
produces — `target/<profile>/` holds both binaries. So that test copies `mixengined` alone into a
temporary directory and starts it from there. D9's rule is what makes this expressible: the helper is
found relative to `current_exe()` and nothing else, so moving the binary is the whole of the fixture.

**What this suite must never do is call `elevation.grant` successfully.** Those tests spawn a real
`mixengined`, which uses the real `Host` — a successful grant there is a real UAC dialog on the
machine running `cargo test`. The prompt-raising assertions are the unit tests' precisely because
those can inject a mock and this suite cannot. Stated here so that a later reader does not "fix" the
gap.

**What T40b does not prove**, so nobody reads the green as covering it: no operation with an effect is
ever applied, because none exists yet — `Probe` changes nothing by design. The first time this queue
carries something that edits a system file is T41, and the "unrelated lines survive" regression test
belongs there.

## Out of scope, and where each goes

| Not here | Where |
| --- | --- |
| The CLI that prints every operation and what it changes *before* the prompt, and keeps the pending list in `mix status` after a decline | T64 |
| `HostsApply` — the first operation with an effect, and this queue's first real producer | T41 |
| Whether an unsigned build survives Smart App Control | T41a |
| Reconciling the queue against what the machine actually has | T47 (`mix doctor`) |
| Installing the helper into a root-owned directory | T85 / T92 |
| Removing the audit log at uninstall | T47 / T92 |
