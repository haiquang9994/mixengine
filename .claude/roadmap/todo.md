# MixEngine build plan

Phases are ordered. Work top to bottom — each phase depends on the ones above it. Tick items as they
land; when new work appears, insert it **where it belongs in the order**, not at the end.

Each phase lives in its own file; this page is the index. Task numbers (`T1`…`T93`) are global and
never reused, so a task keeps its number wherever it is cited — which is why phase 6 is a gap rather
than a renumbering, and why T56 and T64 keep their numbers in the phases they moved to.

Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** = has a platform-layer component and
needs verification on Windows + macOS + Linux.

---

## Phases

| Phase | Goal | Tasks | Done | Milestone |
| --- | --- | --- | --- | --- |
| [0 — Foundations](phase-0-foundations.md) | Daemon starts, CLI talks to it, state persists | T1–T11 | 16 / 16 | **M0** `mix status` prints a healthy daemon on all three OSes in CI |
| [1 — Process supervision](phase-1-process-supervision.md) | Run and babysit arbitrary programs correctly | T12–T19c | 13 / 15 | **M1** the daemon adopts what survived a kill and cleans what did not |
| [2 — Runtimes](phase-2-runtimes.md) | Multiple PHP/Node/Python/Ruby versions, selectable | T20–T29 | 13 / 13 | **M2** `php -v` differs between two directories, no shell hook |
| [3 — Services](phase-3-services.md) | Web server, databases and caches with generated config | T30–T38 | 15 / 15 | **M3** caddy + mariadb + redis healthy in under 10 s warm |
| [4 — Sites & elevation](phase-4-sites-and-elevation.md) | `http://blog.test` works, creating a site prompts for nothing | T39–T47, T64, T93 | 6 / 16 | **M4** a site opens with zero prompts after first-run setup |
| [5 — HTTPS](phase-5-https.md) | Green padlock, automatically, forever | T48–T54 | 0 / 7 | **M5** `https://blog.test` trusted in every browser |
| ~~6 — Desktop GUI~~ | **Withdrawn** — a GUI is a client in its own repository, see [ADR 0011](../decisions/0011-no-gui-in-this-repository.md) | — | — | ~~M6~~ |
| [7 — Efficiency](phase-7-efficiency.md) | Deliver the promise that idle costs nothing | T68–T73 | 0 / 6 | **M7** 30 idle minutes leaves only the daemon and the web server |
| [8 — Differentiators](phase-8-differentiators.md) | LAN sharing, blueprints, extensions, MixDB | T74–T84 | 0 / 11 | **M8** capture, apply, open in MixDB, test from a phone |
| [9 — Ship](phase-9-ship.md) | Installers, updates, docs, beta | T56, T85–T92 | 0 / 12 | **M9 — v0.1.0** |

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

**Phase 1 is 13 of 15.** The vocabulary, the state machine, the supervision mechanisms, the log
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

**Phase 2 is done, and M2 is reached.** **T20a unblocked it**: PHP 8.3.33 exists
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
second process instead. **T28 closed the phase.** What it was waiting for had
arrived with [T32](phase-3-services.md) — a pool to reload per PHP version, and a measured
`PHP_INI_SCAN_DIR` on all three systems — and what it found was that "prebuilt extension artifacts"
were already inside the archive: the index publishes what each build ships loadable, so the task owed
a switch rather than a second download path. An installed PHP now carries a generated
`etc/php/<version>/conf.d/` that both its pool and the `php` on a terminal read, and
`mix runtime ext enable xdebug` moves one line in it and says what that did to the pool.

**Phase 3 is done — 15 of 15 — and M3 is not claimed.** Every task landed; what the milestone asks
for is a *number* — `mix service start caddy mariadb redis`, all healthy in under ten seconds warm —
and nobody has measured it. The measurement is T29's shape and belongs to whoever takes it, not to
the last task in the phase.

What the phase established, in one sentence each. **A `services` row is a rendered configuration and
a runnable spec** (T30), from a `Recipe` compiled into the daemon rather than published by the
package index — so a template travels on MixEngine's release schedule and never on the packaging
pipeline's. **Eight of them exist now**: two front ends (T31, T37), a pool that comes out of a
runtime rather than a package (T32), three databases (T33, T34, T34c), two caches (T35). **A user
reaches all of it through shipped methods** — `package.*` and `service.create|delete` (T31a) — which
is why every supervision fixture is now a row the real method wrote. **A port is allocated when a
row is written** (T34c), free means free on the machine, and a port lost to somebody's XAMPP is
reported with that program's name (T38). **Two instances of one server run side by side** (T36),
because every earlier decision keyed itself by service id rather than by package.

**The three refusals the phase added are what it is really made of**, because each one exists only
because the task before it made the mistake possible: a data directory two rows both name (T36), a
runtime uninstalled under a running pool (T32), and a second front end (T37). Each is refused where
it is written down rather than discovered where the files are opened.

**[T37](phase-3-services.md) closed the phase, and its own finding is that "exactly one front end"
had never been a rule anything could break.** With two front-end recipes it is: `Instancing` is
about a package, both of them answer `Single`, and a home obeying both still gets a Caddy and an
nginx rendered against the same 80 and 443. `Recipe::role` is the one distinction that closes it —
`FrontEnd` or `Other`, defaulted to the second — and the refusal reads `core::services::front_end`,
which answers by role so that neither program is the one the code happens to know about. The recipe
itself is Caddy's shape answered by a server with none of Caddy's mechanisms: **nginx has no admin
endpoint, so the template renders one**, because a TCP accept cannot tell a serving nginx from one
whose workers have all died. And the parity the task owed is literal — the arc both front ends walk
is one file, `crates/mixengine-cli/tests/harness/frontend.rs`, driven twice, with each suite reduced
to four constants over it.

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
| **M3** is a number nobody has taken — three services healthy in under ten seconds warm | nothing, and that is why it is written down: a phase closed with its own claim unmeasured. T29's `bench` job is the shape the measurement would take | [phase 3](phase-3-services.md) |

**The scaffolding that carried an expiry date has half met it.** `mixengine_testkit::declare` no
longer writes a `services` row: **T31a**'s `service.create` does, over a real socket, so the row every
supervision suite runs against is the one the shipped method writes. What is left of it is the
`packages` row for `fakeservice`, which no index will ever publish and which therefore has no method
to replace it. Its sibling `MIXENGINE_DEV_SPECS` is gone: T30 made a row into a real declaration, and
what a test needs beyond that is a *recipe* for the fixture — one a debug build carries and a release
build does not, and that runs one program rather than whatever a variable named.

**Phase 4 has started, and its elevation half is now built from the bottom to the surface.** The
site model and its four kinds are in (**T39a**), the one-shot helper and its file protocol are in
(**T40**), and **T40a** gave the daemon the one thing that turns a request lying on disk into an
elevated process reading it: an `Elevation` capability on `Host`, with UAC, osascript and pkexec
behind it. **T40b** is the half that decides *when* a prompt is worth spending — a durable queue
whose unique key is the operation itself, one grant slot, `ElevationRequired`, and the degraded mode
a decline leaves behind — and **T64** is the surface over it: `mix elevation grant` prints every
operation and what each will literally change, and only then asks. It refuses to raise anything it
cannot be answered about, which is what a cron job and a CI step look like.

**What that stack still has no producer for is the point.** Nothing in this build enqueues anything
but a `Probe`, deliberately, and **T41's `HostsApply` is the first — the next task in this phase**.
Until it lands, the one row every elevation suite runs against is written by a fixture
(`mixengine_testkit::privileged`), which is scaffolding with T41's name on it.

**Both promises are kept.** `runtime.uninstall` refuses over a running php-fpm pool (**T32**) and
over a registered project whose pin the removal would leave with no answer (**T39**), and `--force`
crosses the second and never the first — a broken pin is a statement about the next `cd`, a running
pool is a process serving requests now.

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
