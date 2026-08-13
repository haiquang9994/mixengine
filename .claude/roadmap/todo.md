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
| [0 — Foundations](phase-0-foundations.md) | Daemon starts, CLI talks to it, state persists | T1–T11 | 15 / 16 | **M0** `mix status` prints a healthy daemon on all three OSes in CI |
| [1 — Process supervision](phase-1-process-supervision.md) | Run and babysit arbitrary programs correctly | T12–T19c | 12 / 14 | **M1** the daemon adopts what survived a kill and cleans what did not |
| [2 — Runtimes](phase-2-runtimes.md) | Multiple PHP/Node/Python/Ruby versions, selectable | T20–T29 | 0 / 11 | **M2** `php -v` differs between two directories, no shell hook |
| [3 — Services](phase-3-services.md) | Web server, databases and caches with generated config | T30–T38 | 0 / 9 | **M3** caddy + mariadb + redis healthy in under 10 s warm |
| [4 — Sites & elevation](phase-4-sites-and-elevation.md) | `http://blog.test` works, creating a site prompts for nothing | T39–T47 | 0 / 13 | **M4** a site opens with zero prompts after first-run setup |
| [5 — HTTPS](phase-5-https.md) | Green padlock, automatically, forever | T48–T54 | 0 / 7 | **M5** `https://blog.test` trusted in every browser |
| [6 — Desktop GUI](phase-6-desktop-gui.md) | The terminal becomes optional | T55–T67 | 0 / 13 | **M6** install → Laravel site with HTTPS, no terminal |
| [7 — Efficiency](phase-7-efficiency.md) | Deliver the promise that idle costs nothing | T68–T73 | 0 / 6 | **M7** 30 idle minutes leaves only the daemon and the web server |
| [8 — Differentiators](phase-8-differentiators.md) | LAN sharing, blueprints, extensions, MixDB | T74–T84 | 0 / 11 | **M8** capture, apply, open in MixDB, test from a phone |
| [9 — Ship](phase-9-ship.md) | Installers, updates, docs, beta | T85–T92 | 0 / 11 | **M9 — v0.1.0** |

[Parked](parked.md) — revisit deliberately, do not start early.

## Where we are

**Phase 0 is done apart from T9a**, which waits on purpose, and **M0 is reached**: `mix status`
starts a daemon if there is none, talks to it over the local endpoint and prints what it says, in
both renderings, proved end to end by `crates/mixengine-cli/tests/status.rs` — green on all three
runners, not only the one it was written on. The Windows third of that runs as an administrator
(T2b), which changes nothing about what it proves: `status.rs` asserts nothing a token decides.

**Phase 1 is 12 of 14.** The vocabulary, the state machine, the supervision mechanisms, the log
capture, the dependency graph, the runner, the registry, the `service.*` surface, the CLI over it and
crash recovery are in: a declared service can be started, watched, restarted and stopped through a
real socket, every move is persisted and announced from one value, and a daemon that is killed no
longer takes the truth with it — the next one adopts what survived, stops what it cannot supervise
and clears the rest, before it serves a client. Every check a `ServiceSpec` can name is now one the
supervisor can make, and a service that needs a command of its own to shut down cleanly gets one
(T15a) — which is what Phase 3 was waiting for. Each task's decisions — and the three ADRs the work
forced — are written up in [phase-1-process-supervision.md](phase-1-process-supervision.md).
**This page does not repeat them.**

**M1 is reached**: a daemon is killed mid-run, and the next one adopts the process that outlived it
and clears the row of the one that did not — `crates/mixengine-daemon/tests/lifecycle.rs`, with the
registry's own tests under it, green on ubuntu, windows and macos rather than on the machine it was
written on. That mattered more here than it did for M0: the reading the whole task rests on is per-OS
(`GetProcessTimes`, `proc_pidinfo`, `/proc/<pid>/stat`), and CI is what found the stop that reached
a process group nobody was leading — right on Windows, silently forgiven on both others.

Stated no louder than that. What the test proves is the *recovery*, on every system: it makes its own
survivors, because what a killed daemon leaves behind is a different thing on each of the three, and
that half is [ADR 0007](../decisions/0007-supervised-child-owns-a-process-group.md)'s own tests to
keep.

### What is open, and what each one blocks

| Debt | Blocks | Where |
| --- | --- | --- |
| **T16b** `LogLine` event, `GET /logs/{id}` | `mix service logs`; wants an ADR before it is built | [phase 1](phase-1-process-supervision.md) |
| **T9a** `daemon.shutdown` and the *total* shutdown budget | `mix daemon stop`, and it is now **overdue**: T15a's `StopBehaviour::Command` is what its note said would break the accidental fit inside Windows's five-second console ceiling | [phase 0](phase-0-foundations.md) |
| **T20a** one real artifact, one signed index | T20–T24 being written against something that exists | [phase 2](phase-2-runtimes.md) |
| **T41a** does an unsigned binary load under Smart App Control | whether [ADR 0005](../decisions/0005-on-demand-elevation.md) stands at all — its SAC half needs no code and should be run early | [phase 4](phase-4-sites-and-elevation.md) |
| **T15b** a Linux with no secret service | nothing; waits for somebody actually bitten | [phase 1](phase-1-process-supervision.md) |

**Two pieces of scaffolding carry an expiry date.** `MIXENGINE_DEV_SPECS` — a JSON file of specs read
only by a `debug_assertions` build — is deleted by **T30**, which is the real `SpecSource`; and
`mixengine_testkit::declare`, which writes the `packages` and `services` rows by hand, is replaced by
Phase 3's `service.create`.

## Working on this file

- Tick a task in **its phase file**, not here; update the `Done` column when a phase moves.
- New work goes into the phase file where it belongs in the order. Give it the next free suffix on
  the task it follows (`T40a`, `T40b`) rather than renumbering anything after it. A task may be
  lettered after the one it is ordered *before* — T19c and T20a both are — as long as it says so.
- A phase file carries its own goal, legend and milestone so it reads on its own.
- **One note, one place.** A decision that is *in* the code — why this type, why this order, why not
  the obvious alternative — belongs in the doc comment beside it. One that crosses crates belongs in
  an [ADR](../decisions/). What a phase file carries is only what neither can: what a task
  deliberately did **not** do, and which later task is expected to. A note that would still be true
  with the code deleted is a note the phase file should keep; one that merely describes the code is
  one the code should be carrying instead.
- **"Where we are" is the current phase and the open debts, and nothing else.** Not a changelog. A
  finished task is described by its phase file and by the code it landed, and a third telling is two
  more places for the story to go stale in. Keep this section under a screen; when a phase closes,
  its paragraphs go, they do not accumulate.
