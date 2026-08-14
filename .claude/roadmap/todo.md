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
| [0 — Foundations](phase-0-foundations.md) | Daemon starts, CLI talks to it, state persists | T1–T11 | 16 / 16 | **M0** `mix status` prints a healthy daemon on all three OSes in CI |
| [1 — Process supervision](phase-1-process-supervision.md) | Run and babysit arbitrary programs correctly | T12–T19c | 13 / 14 | **M1** the daemon adopts what survived a kill and cleans what did not |
| [2 — Runtimes](phase-2-runtimes.md) | Multiple PHP/Node/Python/Ruby versions, selectable | T20–T29 | 7 / 12 | **M2** `php -v` differs between two directories, no shell hook |
| [3 — Services](phase-3-services.md) | Web server, databases and caches with generated config | T30–T38 | 0 / 9 | **M3** caddy + mariadb + redis healthy in under 10 s warm |
| [4 — Sites & elevation](phase-4-sites-and-elevation.md) | `http://blog.test` works, creating a site prompts for nothing | T39–T47 | 0 / 13 | **M4** a site opens with zero prompts after first-run setup |
| [5 — HTTPS](phase-5-https.md) | Green padlock, automatically, forever | T48–T54 | 0 / 7 | **M5** `https://blog.test` trusted in every browser |
| [6 — Desktop GUI](phase-6-desktop-gui.md) | The terminal becomes optional | T55–T67 | 0 / 13 | **M6** install → Laravel site with HTTPS, no terminal |
| [7 — Efficiency](phase-7-efficiency.md) | Deliver the promise that idle costs nothing | T68–T73 | 0 / 6 | **M7** 30 idle minutes leaves only the daemon and the web server |
| [8 — Differentiators](phase-8-differentiators.md) | LAN sharing, blueprints, extensions, MixDB | T74–T84 | 0 / 11 | **M8** capture, apply, open in MixDB, test from a phone |
| [9 — Ship](phase-9-ship.md) | Installers, updates, docs, beta | T85–T92 | 0 / 11 | **M9 — v0.1.0** |

[Parked](parked.md) — revisit deliberately, do not start early.

## Where we are

**Phase 0 is done**, and **M0 is reached**: `mix status` starts a daemon if there is none, talks to
it over the local endpoint and prints what it says, in both renderings, proved end to end by
`crates/mixengine-cli/tests/status.rs` — green on all three runners, not only the one it was written
on. The Windows third of that runs as an administrator (T2b), which changes nothing about what it
proves: `status.rs` asserts nothing a token decides. T9a closed it last, and late on purpose: a
daemon can now be *asked* to stop rather than found and killed, it stops its services in reverse
dependency order first, and the whole of that is bounded by one budget — `config.toml`'s over the
API, and whatever Windows's console clock allows when the OS is the one asking.

**Phase 1 is 13 of 14.** The vocabulary, the state machine, the supervision mechanisms, the log
capture, the dependency graph, the runner, the registry, the `service.*` surface, the CLI over it and
crash recovery are in: a declared service can be started, watched, restarted and stopped through a
real socket, every move is persisted and announced from one value, and a daemon that is killed no
longer takes the truth with it — the next one adopts what survived, stops what it cannot supervise
and clears the rest, before it serves a client. Every check a `ServiceSpec` can name is now one the
supervisor can make, and a service that needs a command of its own to shut down cleanly gets one
(T15a) — which is what Phase 3 was waiting for. A service's output now reaches a person as well: on
`GET /logs/{id}` and under `mix service logs`, on a stream of its own rather than as an event
([ADR 0009](../decisions/0009-logs-travel-on-their-own-stream.md), T16b). Each task's decisions — and
the four ADRs the work forced — are written up in
[phase-1-process-supervision.md](phase-1-process-supervision.md). **This page does not repeat them.**

**Phase 2 is 7 of 12, and the phase's spine is complete.** **T20a unblocked it**: PHP 8.3.33 exists
for Windows x86_64, macOS aarch64 and Linux on both architectures, each one run from a directory it
was moved to and made to load an extension there, described by a minisign-signed index at a permanent
URL. The pipeline that produced it is its own repository,
[`mixengine-packages`](https://github.com/haiquang9994/mixengine-packages), built on GitHub runners
because this project has no macOS or Linux of its own and an artifact nobody can reproduce is one
nobody can audit. **T20 reads that index** — signature checked before the JSON is parsed, cached for
six hours, served stale rather than not at all when the network is gone, and refused when a server
offers a document older than the one already held. **T21 installs what it names**, as one transaction
whose commit is a rename: resumable download, checksum, unpack into a staging directory beside the
destination, a run of the binary itself, and only then the move into place. **T22 is the job system**
— `jobs` rows, the two events, `job.list|status|wait|cancel`, cooperative cancellation, and a boot
that closes what a stopped daemon left running.

Each of those three shipped with nothing able to reach it, deliberately, and **T23 is the method in
front of each**: `runtime.install|uninstall|list_installed|list_available|set_default`, with
`mix runtime` and `mix job` over them. `runtime.install` is the job system's first and only producer
— the call answers a `JobSummary` the moment the row exists, the download reports through the
`Watcher` T21 shaped after `JobHandle`, and what the finished job carries is the same
`RuntimeSummary` a listing is made of. Proved end to end on a real socket against a signed index and
a real archive, in `crates/mixengine-daemon/tests/runtimes.rs` and
`crates/mixengine-cli/tests/runtime.rs`: a version is offered, installed, listed, chosen and removed,
and the directory on disk agrees at every step.

**T24 answers the question the rest of the phase was deferring to**: which version a directory uses.
`core::resolve` walks the four sources in order — a flag or `MIXENGINE_PHP`, the nearest
`mixengine.toml` *that names the language*, a registered project, the kind's default — and answers
with the installed runtime **and the source that decided it**, because "which PHP is this?" is asked
precisely when the answer is surprising. The grammar it needed went to `mixengine-proto` beside the
identifier it is about: `VersionConstraint` (a prefix or a caret) and `RuntimeVersion::cmp_precedence`,
which is a different order from the derived one and the one anything choosing a version wants.
`runtime.resolve` and `mix runtime resolve` are over it.

**T25 is that answer's first caller with no daemon to ask**, and the first binary here that is not a
client of one: `mixengine-shim` reads the name it was invoked by, resolves in its own process against
the database opened read-only, and then *becomes* the program — `exec` on Unix, a child in a Job
Object with the console interrupts swallowed on Windows, the program's own exit code either way.
`crates/mixengine-shim/tests/shim.rs` runs the real binary out of a real `bin/` from two directories
and gets two different runtimes, which is **M2's claim with its last piece missing**: nothing fills
`bin/` yet, so a person cannot type it until T26 does.

Next in order is **T26**, which puts `<root>/bin` on `PATH` reversibly and copies the shim into it
once per name in `core::shims::COMMANDS`.

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
| **T41a** does an unsigned binary load under Smart App Control | more than it used to. T20a measured that PHP, nginx and Caddy are unsigned *upstream*, so this governs every runtime MixEngine starts and not only the ones we build — and it needs a machine with SAC enforced, which nobody has and which cannot be created except by a fresh install | [phase 4](phase-4-sites-and-elevation.md) |
| **T15b** a Linux with no secret service | nothing; waits for somebody actually bitten | [phase 1](phase-1-process-supervision.md) |

**Two pieces of scaffolding carry an expiry date.** `MIXENGINE_DEV_SPECS` — a JSON file of specs read
only by a `debug_assertions` build — is deleted by **T30**, which is the real `SpecSource`; and
`mixengine_testkit::declare`, which writes the `packages` and `services` rows by hand, is replaced by
Phase 3's `service.create`.

**One promise is deferred rather than scaffolded.** `runtime.uninstall` refuses nothing: the checks
[runtime-versions.md](../features/runtime-versions.md) describes are a project pin (Phase 4) and a
php-fpm pool (**T28**), and a `--force` beside a refusal nothing can trigger would be a flag with
nothing to force past. The task that adds the first of those adds the refusal with it.

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
