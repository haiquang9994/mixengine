# MixEngine build plan

Phases are ordered. Work top to bottom — each phase depends on the ones above it. Tick items as they
land; when new work appears, insert it **where it belongs in the order**, not at the end.

Each phase lives in its own file; this page is the index. Task numbers (`T1`…`T92`) are global and
never reused, so a task keeps its number wherever it is cited.

Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** = has a platform-layer component and
needs verification on Windows + macOS + Linux.

---

## Phases

| Phase | Goal | Tasks | Done | Milestone |
| --- | --- | --- | --- | --- |
| [0 — Foundations](phase-0-foundations.md) | Daemon starts, CLI talks to it, state persists | T1–T11 | 14 / 15 | **M0** `mix status` prints a healthy daemon on all three OSes in CI |
| [1 — Process supervision](phase-1-process-supervision.md) | Run and babysit arbitrary programs correctly | T12–T19b | 6 / 13 | **M1** the daemon adopts what survived a kill and cleans what did not |
| [2 — Runtimes](phase-2-runtimes.md) | Multiple PHP/Node/Python/Ruby versions, selectable | T20–T29 | 0 / 10 | **M2** `php -v` differs between two directories, no shell hook |
| [3 — Services](phase-3-services.md) | Web server, databases and caches with generated config | T30–T38 | 0 / 9 | **M3** caddy + mariadb + redis healthy in under 10 s warm |
| [4 — Sites & elevation](phase-4-sites-and-elevation.md) | `http://blog.test` works, creating a site prompts for nothing | T39–T47 | 0 / 12 | **M4** a site opens with zero prompts after first-run setup |
| [5 — HTTPS](phase-5-https.md) | Green padlock, automatically, forever | T48–T54 | 0 / 7 | **M5** `https://blog.test` trusted in every browser |
| [6 — Desktop GUI](phase-6-desktop-gui.md) | The terminal becomes optional | T55–T67 | 0 / 13 | **M6** install → Laravel site with HTTPS, no terminal |
| [7 — Efficiency](phase-7-efficiency.md) | Deliver the promise that idle costs nothing | T68–T73 | 0 / 6 | **M7** 30 idle minutes leaves only the daemon and the web server |
| [8 — Differentiators](phase-8-differentiators.md) | LAN sharing, blueprints, extensions, MixDB | T74–T84 | 0 / 11 | **M8** capture, apply, open in MixDB, test from a phone |
| [9 — Ship](phase-9-ship.md) | Installers, updates, docs, beta | T85–T92 | 0 / 11 | **M9 — v0.1.0** |

[Parked](parked.md) — revisit deliberately, do not start early.

## Where we are

Phase 0 is done apart from **T9a**, which is waiting on purpose: `daemon.shutdown`'s real shape is
"stop every supervised service in reverse dependency order, then stop", and there is no service to
stop before T13. **M0 is reached in substance** — `mix status` starts a daemon if there is none,
talks to it over the local endpoint and prints what it says, in both renderings, proved end to end
by `crates/mixengine-cli/tests/status.rs`. What is left of the milestone is CI having run that on
macOS and Linux as well as Windows.

T11 landed the fixtures the later phases are written against: `crates/mixengine-testkit`, with the
temporary `Home`, `FakeService` and the `fakeservice` binary. Two of the four things it named are
deliberately not in it — `mock::Host`'s recording arrived with T3a and needs nothing, and
`MockRegistry` has no caller until runtimes are installed in Phase 2.

**Phase 1 is under way.** T12 began with a decision rather than with code:
`process-supervision.md` had the supervisor consuming a spec "produced by `mixengine-core`" while
`overview.md` and `workspace_layering.rs` make those two crates siblings; both cannot be true while
the type lives in either. [ADR 0006](../decisions/0006-servicespec-in-proto-and-secret-free.md)
settles it — `proto` owns the vocabulary, and a spec names a keyring entry rather than carrying a
password — and sets the precedent Phase 4 reuses for `PrivilegedOp` (see T40).

T12 landed that vocabulary in `crates/mixengine-proto/src/service.rs`, with `Millis` joining
`Timestamp` and `Uptime` in `time.rs` as the third and last time type. `ServiceState` is not in it:
it arrives with T14, which is what persists and emits one.

**T13 ended the same way T12 began**, with a decision the code forced: `.claude/architecture/`
promised "no orphans, ever" as one sentence, and it is three. [ADR
0007](../decisions/0007-supervised-child-owns-a-process-group.md) writes the weakest platform down
honestly instead of averaging it — a kernel guarantee on Windows, the immediate child on Linux,
nothing on macOS — and gives every service a group of its own, owned by a `Supervised` whose `Drop`
stops it. What the weak cells rest on is T18, which has to exist anyway for the machine that lost
power, and the honest sentence to a user is owed by T47.

**T14 landed the state machine and the row it lives in**, and left the process untouched: nothing
spawns anything here. `ServiceState` is closed where the rest of the vocabulary is open — the
supervisor is meant to match it exhaustively — and `StateReason` is the open half beside it, because
the set of states is fixed by the machine while the set of explanations grows with every phase. The
same `ServiceTransition` is what `core::services::transition` persists and what
`DaemonEvent::ServiceStateChanged` carries, so the row and the event cannot describe different
events. Writing the transition table out showed the diagram in `process-supervision.md` was
compressing four real edges; the spec was corrected rather than the code bent to fit it.

The workspace's first `sqlx::query!` came with it, and with the machinery that makes it free for
everyone else: `.sqlx/` is committed, a build needs no `DATABASE_URL`, and `lint` runs
`cargo sqlx prepare --check` because the failure mode — a query edited without regenerating — is
invisible on the machine that caused it.

**T15 landed the mechanisms of supervision and, like the two before it, a decision the code forced.**
Log capture came first in the shape T16 will build on — reader threads rather than tasks, because an
anonymous pipe on Windows cannot be read asynchronously and a pipe nobody drains stops the service
that is writing to it — because two things needed it: `ReadyCheck::LogPattern`, and a crash-loop
cutoff, which is why `StateReason::CrashLoop` now carries a `tail`. "It kept crashing" explains
nothing without the line that says `Address already in use`.

Waiting for readiness races **three** outcomes; the third — the process exiting while the probe
waits — is the most common way a service fails to start, and the one a naive implementation reports
as a timeout thirty seconds late. Health is a run of probes rather than a probe, and a restart
decision has three answers because a service has three states to be left in.

[ADR 0008](../decisions/0008-no-signal-stop-on-windows.md) is what T13 sent here: Windows has no
signal a daemon can send to a process it gave no console to, so `CAN_ASK_TO_STOP` says so and a
grace period is not spent on a request nobody sent. On the way through, T15 also closed T14's open
question (`last_started_at` is epoch milliseconds), gave a supervised child the environment its spec
states rather than the daemon's, and landed the `Keyring` capability ADR 0006 implies.

**Two things are deliberately not in it.** The probes that need a dependency this crate should not
invent — HTTP, and a command — are **T15a**, and answer `Error::UnsupportedCheck` until Phase 3 has
a service that wants them. And the *runner* that ties spawn, ready, health and restart into one
task belongs to **T19**: every piece here is free of a loop, a clock and a state row, because the
thing that owns the timing is the daemon's registry of running services, which does not exist until
something can ask it to start one.

**T16 was split by that same registry** and gave the log its third reader: `current.log` under
`logs/services/<service-id>/`, written from the reader threads T15 already had, so the supervisor
still has no loop of its own and a line reaches the disk before it reaches a subscriber. The event
and `GET /logs/{id}?follow=1` are **T16b**, because both begin by looking a `ServiceId` up in a
registry that arrives with T19 — and because putting every line of every service on the one bounded
stream the GUI watches for state changes is a decision, not a detail. T16b states it and asks for an
ADR rather than discovering it as a `Resync` storm.

Two things moved rather than being written twice. `LogLine` and `Stream` are `mixengine-proto`'s
now, on T14's precedent that the value which is kept and the value which is published are one; and
`RotatingFile` moved *down* from the daemon into the supervisor, since the process holding a
service's handle is the one that must enforce its size. That move forced its one behavioural change:
it reports a failed rotation instead of writing it, because `daemon.log` wants that note in
`log.format`'s shape while a service's file — which is the upstream program's output and nothing
else — must not be given a sentence of ours at all.

**T17 corrected the task it was written as.** "Cycle detection at spec-build time" is the one place
it cannot happen: a cycle is a property of a *set* of specs, and `ServiceSpecBuilder::build` sees
one — which is why it rejects only the case a spec can see about itself. `ServiceGraph` in
`mixengine_core::services::graph` checks all three set-level invariants where they are decidable, at
assembly, and afterwards answers questions without being able to fail; the roadmap's wording was
corrected rather than the check moved somewhere it cannot work. It sits in `core` on ADR 0006's own
line — `proto` owns the vocabulary, the supervisor owns no registry, and a topological sort over
declared services is domain logic.

A plan is **tiers rather than a flat list**, which is what leaves T19 free to walk them one at a time
now and concurrently for M3 without recomputing anything. Start and stop are opposite walks and not
one walk reversed: over the whole set they coincide, over a subset they name different services. The
failure path is fail-fast, and it brought the `StateReason` the architecture had reserved for this
task — `DependencyFailed { dependency }` — which needed no new edge in the state machine, since
`SpawnFailed` has reached `Failed` from `Starting` without a process ever existing since T14.

**T19 moved ahead of T18, and split into three.** The order T18 → T19 assumed adoption could be
built before there was anything to adopt, and it cannot: a survivor is a `services` row carrying
`state = 'running'` with a `pid` and a `pid_start_time`, and those three columns have never been
written by anybody, because nothing in the workspace can start a service yet. That is the same
shape as T9a waiting for T13, and it is settled the same way — the thing that produces the state
goes first. T18 keeps its number and its milestone; it is now the last task in the phase, which is
also where M1 belongs.

The split follows what each piece can be finished and reviewed on its own: **T19** is the runner and
the registry inside the daemon — the loop, the clock and the `CancellationToken` T15 deliberately
does not contain — **T19a** is the `service.*` RPC surface over it, and **T19b** is the CLI that
renders what that returns. `mix service logs` left T19b for **T16b**, which is where the endpoint it
would call is built; a CLI reading `current.log` off the disk itself would be exactly the
business-logic-in-a-client bug `CLAUDE.md` forbids. T16b moved down with it, since it was already
waiting on T19's registry and now reads in the order it will be built.

## Working on this file

- Tick a task in **its phase file**, not here; update the `Done` column when a phase moves.
- New work goes into the phase file where it belongs in the order. Give it the next free suffix on
  the task it follows (`T40a`, `T40b`) rather than renumbering anything after it.
- A phase file carries its own goal, legend and milestone so it reads on its own.
