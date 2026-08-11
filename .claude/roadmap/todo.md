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
| [1 — Process supervision](phase-1-process-supervision.md) | Run and babysit arbitrary programs correctly | T12–T19 | 0 / 8 | **M1** the daemon adopts what survived a kill and cleans what did not |
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

Next is **Phase 1**, starting at T12 — which begins with a decision rather than with code.
`process-supervision.md` had the supervisor consuming a spec "produced by `mixengine-core`" while
`overview.md` and `workspace_layering.rs` make those two crates siblings; both cannot be true while
the type lives in either. [ADR 0006](../decisions/0006-servicespec-in-proto-and-secret-free.md)
settles it — `proto` owns the vocabulary, and a spec names a keyring entry rather than carrying a
password — and sets the precedent Phase 4 reuses for `PrivilegedOp` (see T40).

## Working on this file

- Tick a task in **its phase file**, not here; update the `Done` column when a phase moves.
- New work goes into the phase file where it belongs in the order. Give it the next free suffix on
  the task it follows (`T40a`, `T40b`) rather than renumbering anything after it.
- A phase file carries its own goal, legend and milestone so it reads on its own.
