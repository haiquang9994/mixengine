# T47b — `daemon.doctor_repair`, which acts on what T47a found

Roadmap task **T47b**, phase 4, the write half of the **T47** split. Design settled 2026-08-24,
before implementation. The read half is
[T47a](2026-08-24-t47a-doctor-design.md), and every decision below leans on something it built.

Everything under `.claude/` that this build touches is in force: no business logic in clients, no
client-only capability, no OS calls outside `mixengine-platform`, no persistent root process,
generated config is disposable, cross-platform or not merged.

## Scope

In: `daemon.doctor_repair`, the repair for each `ProblemId`, the single elevation prompt, the two
conditions the roadmap moved here — stale generated configuration and a service this daemon is not
supervising — and `mix doctor --repair`.

Out, each with an owner: rebinding a DNS server that could not bind is **not this task**, argued in
D9; the diagnostics archive is **T93**; the complete uninstall path is **T87**.

---

## D1 — A second method, and not a flag on the first

`daemon.doctor_repair` takes a `DoctorRepair` and answers a `RepairReport`.

**`daemon.doctor` does not grow a `repair: bool`.** T47a's module is documented as a method that
writes nothing — no row, no file, nothing enqueued, no prompt — and that is what makes it safe on a
timer, inside `mix status` and inside T93's bundle. A flag would make the guarantee conditional on
an argument, which is the same as not having it: every caller would have to read the call site to
know whether the machine was about to change.

Two methods also match what a client was already promised.
[client-surface.md](../../../.claude/features/client-surface.md) lists `daemon.doctor_repair` by
name, as one action under Settings — one button, not one button per finding. So the method repairs
**everything it can in one call** and takes no selection. A per-problem form is a thing to add when
a client asks for it, and nothing has.

## D2 — The report is one entry per problem, in three words

```rust
pub struct DoctorRepair {
    /// Flush the queue in this same call, raising the one prompt. Defaults to false — D4.
    pub grant: bool,
}

pub struct RepairReport {
    /// One entry per `Problem` the report found, in the report's own order.
    pub actions: Vec<Repair>,

    /// The grant this call raised, when it was asked to and anything needed the helper.
    ///
    /// A `JobSummary` rather than a bare id, so a caller follows the job without asking again; it is
    /// the same value `elevation.grant` answers with.
    pub granting: Option<JobSummary>,
}

pub struct Repair {
    /// The condition this entry is about.
    pub id: ProblemId,

    /// What was examined, phrased for a person — T47a's `Check::name`, carried through.
    pub name: String,

    /// What happened to it.
    pub outcome: Action,
}

#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Done, inside the home, with no prompt and nothing pending.
    Repaired { what: String },

    /// Needs the helper. Applied by the grant this call raised, not by this call.
    Enqueued { what: String },

    /// Nothing this build can do, and why.
    Untouched { because: String },
}
```

**Only problems appear.** T47a's report lists every check whatever it answered, because a check that
found nothing is the evidence that it ran. A repair report is the opposite kind of document: it is a
record of what was *done*, and an entry saying "this was already fine so I did nothing" is noise in
a list whose whole purpose is to be read after an action. What was examined is still available —
`daemon.doctor` is one call away, and the CLI makes that call itself (D10).

**Three words and not two.** `Repaired` and `Enqueued` are genuinely different outcomes to a person:
one is finished and one is waiting on a prompt they have not answered yet. Collapsing them would
mean a report that says a machine is fixed when the operation that fixes it has not run.

## D3 — Dispatch is an exhaustive `match` on `ProblemId`

The repair for each condition is selected by a `match` over the closed enum, with **no wildcard
arm**. This is the whole of what T47a bought by closing that enum, spent here: a `ProblemId` added
later stops this file compiling until somebody decides what repairing it means. A `_ => ` arm would
turn that compile error into a silent no-op and leave the two halves free to drift — which is the
defect the closed enum exists to prevent.

The same holds in the other direction. An id nothing produces cannot be repaired; if a check is ever
deleted, its id goes with it and this `match` fails to compile until the arm goes too.

## D4 — One prompt, raised only when the caller says so

Repairs that need root do not elevate. They **enqueue**, through the same
`Elevation::require_hosts` / `require_resolver` / `require_port_access` that every other producer
uses. [ADR 0005](../../../.claude/decisions/0005-on-demand-elevation.md) settled that asking more
than once for one batch is the defect, and the daemon's own rule is that enqueuing and flushing have
different triggers: producers enqueue, and only a client's call flushes.

**Whether this call also flushes is `grant`, and T64 is why it is a parameter rather than always
true.** This decision was made the other way first and was wrong. T64's rule is that a person reads
what is about to be allowed *before* it is allowed — the exact hosts lines, the port, the store —
and a call that enqueued and flushed in one step leaves no moment for a client to show them. It
would have made `mix doctor --repair` the equivalent of `mix elevation grant --yes` without anybody
typing `--yes`, and the operating system's own prompt does not carry what T64 exists to show.

So the ordinary path is two calls: `daemon.doctor_repair` with `grant: false`, then the batch is read
through `elevation.status`, shown, answered, and `elevation.grant` raises the one prompt. `grant:
true` is for a caller that has already shown the batch and been answered, which on the command line
is `mix doctor --repair --yes`. Both are reachable from `mix`, so neither is a client-only path.

**The grant is raised only if something is waiting**, whatever was asked for. A home whose only
problem was stale configuration gets no prompt, because nothing was enqueued and nothing was already
waiting; `granting` is `None` there.

**A decline is not an error.** `Elevation::grant` already models it, and this method passes back the
job rather than the outcome — the caller waits on it exactly as it does for `elevation.grant`.

## D5 — What each condition's repair is

Ten conditions: T47a's eight, plus the two D6 adds.

| `ProblemId` | Action | What it does |
| --- | --- | --- |
| `hosts_block_differs` | `Enqueued` | `Elevation::require_hosts` — the same comparison T47a's check reported, now enqueued as operations |
| `resolver_not_wired` | `Enqueued` | `Elevation::require_resolver` |
| `port_access_missing` | `Enqueued` | `Elevation::require_port_access`, for the front end's own binary |
| `permission_pending` | `Enqueued` | Nothing new is enqueued. The queue was already the problem; the grant in D4 is the repair, and this entry says how many were waiting |
| `home_permissions_lost` | `Repaired` | Re-restricts the home to its owner. Inside `MIXENGINE_HOME`, so no prompt |
| `generated_config_stale` | `Repaired` | Installs the rendering — the same write an override change makes |
| `service_unsupervised` | `Repaired` | Reconciles the rows this registry does not hold: adopt what is provably the same process, stop what is not (D8) |
| `domain_unreachable` | `Untouched` | A name resolves once the hosts block and the resolver are what they should be. Both are repaired above when they were wrong; there is no third thing to do to a domain |
| `port_range_reserved` | `Untouched` | The operating system reserved the range. MixEngine cannot un-reserve it, and pretending otherwise is worse than saying so |
| `dns_server_unavailable` | `Untouched` | D9 |

`Untouched` is not a failure of the call. Three of ten conditions have no repair in this build, and a
`RepairReport` that hid them would be a report claiming a machine was seen to and left alone.

## D6 — Two new checks, and therefore two new ids

The roadmap moved two conditions into this task. Both become **reported checks in `daemon.doctor`**
with ids of their own, rather than repair-only actions:

- `generated_config_stale` — a service's installed configuration is not what its row renders to.
- `service_unsupervised` — a `services` row claims to be supervised, and this registry has no runner
  for it.

**Why they are reported and not merely repaired.** D3's rule is that a repair keys off a condition
this build can name, and a repair with no id is a repair outside that rule — it would run on
machines where nothing said it was needed, and leave nothing for a person to check afterwards. T47a
made the report the place a person looks; a fix for something the report never mentions is a fix
nobody can anticipate or verify.

The cost is real and is accepted: the report path now renders every declared service's
configuration in order to compare it. That is still a read — a rendering is built in memory and
nothing is installed — so T47a's guarantee survives intact, and `daemon.doctor` gets slower on a
home with many services.

## D7 — Detecting stale configuration means rendering it, which is why it lives here

There is no cheap test for "does this file still match the state". Generated configuration is
disposable and is **never parsed back** — the workspace rule — so the only way to compare is to
render the whole thing again and diff the result against what is on disk. That is exactly what the
install path does before it writes.

Building it in T47a would have meant building it twice, or building the repair early and calling it
a diagnostic. Both were rejected there and the condition was moved here, where the machinery exists
for its own reasons.

**Why it is worth repairing at all**, given a drifted file is corrected by the next write that
touches it: the fault is not the file, it is that *the front end is serving a stale rendering right
now*. That is a thing a repair fixes in the moment and a report can only name.

## D8 — `Registry::recover` cannot be re-run on a live daemon

`Registry::recover` is the reconciliation this repair wants, and calling it is the wrong way to get
it. It walks **every** `services` row and decides, for each, to adopt the process or stop it. On a
boot that is correct, because nothing is supervised yet. On a running daemon it would walk rows this
registry already holds runners for and stop services that are working.

So `service_unsupervised` is defined narrowly enough to be safe: **a row that claims to be
supervised — its state says so, or it names a pid — and for which this registry has no runner**.
Those are the only rows a live daemon may reconcile, and the decision for each is `recover`'s own:
adopt when the pid *and* the recorded start time both match, stop otherwise, and never signal on a
pid alone. A pid is reused within minutes and signalling somebody else's program is the one accident
this product cannot have.

## D9 — `dns_server_unavailable` is `Untouched`, and what would change that

`Dns` has `start` and `reprobe` and no way to bind again. A repair would have to re-register both
transports and re-probe, which is a task rather than an arm of a `match`. In this build the entry
carries the bind error the server itself recorded.

**What would reopen it** is evidence that the condition is common and transient — something taking
53 during boot and releasing it — which would make a rebind the difference between a working machine
and a restart. Nothing measured says that yet.

## D10 — `mix doctor --repair`, and what its exit code means

`mix doctor --repair` calls `daemon.doctor_repair` with `grant: false`, renders the actions, and
then — if anything is waiting — reads `elevation.status`, shows the batch, asks, and raises the one
prompt through `elevation.grant`, following the job as `mix elevation grant` does. `--yes` sends
`grant: true` instead, so the daemon raises it in the first call; `--no-wait` returns as soon as the
grant has started. Both flags mean what they mean in `mix elevation grant`, because it is the same
grant.

**Exit code**: zero when every problem was `Repaired` or `Enqueued`, non-zero when any was
`Untouched`. That is the honest reading — "everything I found, I did something about" — and it is
what a script can act on. It deliberately does **not** mean "the machine is well": the enqueued half
is not applied until the grant finishes, and `mix doctor` on its own is still the question "is
anything wrong".

## D11 — Testing

**The elevated half cannot be tested without raising a prompt, so it is not.** Those tests assert
that the operation is **enqueued** — visible through `elevation.status` — and stop there. A test
that asserted the hosts file changed would be a test that cannot run unattended anywhere, and CI's
elevated legs are a separate job with a separate rule.

**The unprivileged half is tested end to end**, because it can be: corrupt a generated file, run
`mix doctor --repair`, and see the condition gone from a fresh `mix doctor`.

**Every negative assertion carries a control taken with the same instrument at the same moment** —
T46's D9 and T47a's D10, and the rule that keeps a test from passing because the instrument was
broken. A test that says "`generated_config_stale` is absent after the repair" must show the same
call reporting it as present before, or it is asserting that the check works rather than that the
repair does.

**A suite may assert that its own condition is absent and then present. It may never assert that the
machine running it is well.** That is T47a's finding, paid for by a Windows runner whose port 80 is
inside a reserved range, and it applies with more force here: several conditions in D5's table are
properties of the machine.

## What this task does not settle

- **Rebinding the DNS server** (D9).
- **A fresh report after a repair.** `mix doctor` is one command away and the enqueued half is not
  applied until the grant finishes, so a report printed immediately after would be misleading about
  exactly the operations a person just approved.
- **Repairing one problem rather than all of them.** No client has asked; `client-surface.md`
  describes one action.
- **A repair that runs on a schedule.** Everything here is on a client's call, which is the daemon's
  standing rule for anything that writes outside the home.
