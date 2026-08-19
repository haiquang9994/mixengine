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
| [2 — Runtimes](phase-2-runtimes.md) | Multiple PHP/Node/Python/Ruby versions, selectable | T20–T29 | 12 / 13 | **M2** `php -v` differs between two directories, no shell hook |
| [3 — Services](phase-3-services.md) | Web server, databases and caches with generated config | T30–T38 | 5 / 12 | **M3** caddy + mariadb + redis healthy in under 10 s warm |
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

**Phase 2 is 12 of 13, and M2 is reached.** **T20a unblocked it**: PHP 8.3.33 exists
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

**T26 gave that binary somewhere to be, and gave the directory it lives in a way onto the PATH**, and
with it **M2 is reached**: `crates/mixengine-shim/tests/shim.rs` runs the real shim out of a `bin/`
that `core::shims::refresh` filled, from two directories, and gets two different PHPs with no daemon
running and no shell hook installed. The two halves keep opposite policies about being done unasked —
`bin/` is a projection of a compiled-in table and is refreshed on every start, while the PATH is a
file in the user's home or a value in their registry hive and is written only by `path.install`. That
split, and the fact that `PathIntegrationApply` came *off* the privileged-operation list rather than
being implemented, are [phase 2](phase-2-runtimes.md)'s to keep.

**T27 is done, and all four languages are in the index**: twenty-five packages and one hundred and
eighteen artifacts, Node.js on five lines, Python on five, Ruby on four. What it cost *here* is three
tests and documentation — the kind enum, the command table, the smoke test and `resolve` were about
four languages rather than about PHP from the start, so every recipe lives in `mixengine-packages`.
Windows on ARM is a runtime target for three of the four now, where `windows.php.net` has never
published one at all. Ruby turned out to be two answers rather than one: RubyInstaller covers Windows
on both architectures, while macOS and Linux were the last cell in the whole table that nothing could
be borrowed for.

**[T27b](phase-2-runtimes.md) closed that cell and audited the packing code doing it.** Ruby is
compiled from ruby-lang.org's own source on all four Unix targets with `--enable-load-relative`, YJIT
on, and — the question the task was carved out to answer — **its own OpenSSL, taught to resolve its
default certificate paths against the loaded `libcrypto`'s location** rather than against the
distribution that built it, which is the same idea as the shim and as `--enable-load-relative`,
applied one library further down. Four rounds of CI and not one of them was Ruby: every failure was
in `relocate.py` or in what a check was asking, which is what a *second* build pipeline is for.

**T29 put a number on the promise the shim is built around, and a `bench` job in CI to keep it.** The
budget belongs to the *resolution* — that is where
[runtime-versions.md](../features/runtime-versions.md) puts it — and the resolution takes 0.58 ms on
macOS, 0.74 ms on Linux and 1.71 ms on Windows against a home with five runtimes in it, nine to
twenty-five times inside its 15 ms. What a person waits for is a different number and is reported
rather than gated, because it is process creation nearly all of it: the shim adds 2.19 ms on Linux
and 4.52 ms on macOS, where it `exec`s, and **15.03 ms on Windows**, where it cannot and starts a
second process instead. **T28 is what is left of the phase**, and half of it — the per-pool reload —
waits on [T32](phase-3-services.md), which is why Phase 3 was started ahead of it.

**Phase 3 is 5 of 12.** [T30](phase-3-services.md) is in, and with it the port T19 left open is
answered: a `services` row is rendered into `etc/<service-id>/` and into the `ServiceSpec` the
supervisor runs, on every `service.*` call, by `core::generate`. What a service *is* — the binary,
the template, the ready check — is a `Recipe` compiled into the daemon rather than anything the
package index publishes, which is what keeps a template on MixEngine's release schedule instead of
the packaging pipeline's. The catalogue this build ships is deliberately **empty**: each of
T31–T35 writes its own recipe against the real server, and what T30 proved instead is the machinery
around them — typed overrides that refuse a misspelling, a whole set staged and validated before any
of it is installed, a rendering identical to what is on disk written not at all. `MIXENGINE_DEV_SPECS`
went with it.

**[T30a](phase-3-services.md) gave the next four tasks a server to be judged against**, in
[`mixengine-packages`](https://github.com/haiquang9994/mixengine-packages) and with no change here:
Caddy is borrowed on all six targets, and each archive is *run as a web server* before it is
published — a rendered Caddyfile validated, the admin endpoint health-checked, a request served and
the server stopped through that endpoint, which are the four mechanisms T31 is built on.

**[T31](phase-3-services.md) is the first real recipe, and it is judged against that server.** A
`services` row becomes a Caddyfile that Caddy's own adapter accepts, a spec whose readiness, health
and stop are all the admin endpoint, and — the part that needed new vocabulary — a **reload**: an
override edited under a running Caddy is served by the same process a moment later, and a broken one
is refused with the last good configuration still live and the site still answering. `ReloadBehaviour`
is in `mixengine-proto` beside `StopBehaviour`, and the edge that uses it runs from `Registry::graph`
to the runner, which is where "what was rewritten" meets "what is running". CI fetches a real Caddy
on all three systems to prove it, because whether a Caddyfile with a Windows path in it parses is a
question only Caddy answers.

**[T31a](phase-3-services.md) closed the gap between "MixEngine can run Caddy" and "a user can ask
it to."** `package.install|uninstall|list|list_available` put a service package into
`paths.packages()` from the signed index — the job system's second producer, sharing one index client
and one installer with `runtime.*` — and `service.create|delete` are the two ends of a `services`
row's life. Only packages this build has a recipe for are offered or installed, a service's package
is read off its own id, a recipe declares whether it exists once or by name, and a delete keeps the
data directory and says which one. What that bought the suites is the point: every fixture service is
now created through the shipped method rather than by an insert, and `caddy.rs` installs its real
Caddy through `package.install`. What is still open in this phase is written in its own file, not
here.

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

**The scaffolding that carried an expiry date has half met it.** `mixengine_testkit::declare` no
longer writes a `services` row: **T31a**'s `service.create` does, over a real socket, so the row every
supervision suite runs against is the one the shipped method writes. What is left of it is the
`packages` row for `fakeservice`, which no index will ever publish and which therefore has no method
to replace it. Its sibling `MIXENGINE_DEV_SPECS` is gone: T30 made a row into a real declaration, and
what a test needs beyond that is a *recipe* for the fixture — one a debug build carries and a release
build does not, and that runs one program rather than whatever a variable named.

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
