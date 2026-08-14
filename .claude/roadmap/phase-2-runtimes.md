# Phase 2 — Runtimes

*Goal: multiple PHP/Node/Python/Ruby versions installed and selectable.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [x] **T20a** One real artifact and one real index, before a client is written against either. **(P)**
      **Ordered before T20 although it is lettered after it**, on T19c's precedent: T20–T24 are a
      client for a package index nobody had produced, and a client written against a sketch is a
      client written against a guess. What exists now is not a sketch — PHP 8.3.33 for four targets
      and a signed `index.json` describing exactly those, in
      [`mixengine-packages`](https://github.com/haiquang9994/mixengine-packages). The evaluations
      are written up in [../operations/runtime-packaging.md](../operations/runtime-packaging.md) and
      **this file does not repeat them**; what follows is only what that page cannot carry.
      **Its first output was a decision, and the decision was borrow.** PHP is repacked from
      windows.php.net on Windows and built with `static-php-cli` (MIT, 115 extensions, `cli` and
      `fpm`) on macOS and Linux. The macOS cell the plan feared most — `install_name_tool` over every
      bundled dylib followed by a re-sign that Ventura would reject — was never entered, because
      arm64-only puts the floor at 8.1, which is exactly where `static-php-cli` starts. Two limits
      that were chosen for unrelated reasons agreed, and a whole class of work disappeared.
      **One premise of the task was false, and finding that out was worth more than the artifacts.**
      It reasoned that an artifact its publisher already signs might run under Smart App Control
      where one we built is refused, and said to weigh that on the borrow side. Measured:
      `php.exe`, `nginx.exe` and `caddy.exe` are **unsigned**, `node.exe` is the only signed binary
      in the table. So the SAC risk is identical on both sides of every cell but one, borrowing wins
      on maintenance cost alone, and [T41a](phase-4-sites-and-elevation.md)'s answer now governs the
      whole table rather than the half we build.
      **The one thing this task was told to do and did not** is run that smoke test on a machine with
      SAC **enforced**. Nobody has one — SAC cannot be re-enabled once turned off, so it needs a
      fresh install — and the measurement above is why it is not worth blocking on: it would tell us
      about *every* artifact MixEngine ships rather than about a choice this task makes, which is
      T41a's question and not this one's. It moves there rather than staying open here.
      **Four bugs, and only one of them was found on a developer machine.** The interesting one is
      recorded as a rule in the packaging doc rather than here, because it outlives this task: PHP's
      ini parser rejects `~` in an unquoted value, Windows puts one in every 8.3 short path, and the
      failure is silent — every extension stops loading while `php -v` keeps answering. It passed
      locally and failed on the runner, which is the whole argument for the runners being the build
      machines. The other three: `php-win.exe` answers `-v` with nothing at all; `--build-shared`
      links what `download` already fetched and refuses at the *end* of a ten-minute build if it did
      not; and `static-php-cli` resolves two dozen libraries through an unauthenticated GitHub API
      that allows 60 requests an hour per IP, shared across everything Azure is running.
      **`SPC_LIBC=glibc` is forced, and it costs a floor.** Static musl has no `dlopen`, so the
      tool's default Linux output cannot load an extension at all — which would make T28 impossible
      on Linux. Choosing glibc means the binary needs a glibc at least as new as the machine that
      built it, so Linux builds on the oldest image GitHub still offers rather than the newest, and
      the requirement is *measured* off the finished binary and carried in the index. A floor read
      from the build machine would have been a guess that stayed conservative until it did not.
      Left for the tasks that need them: the index has **one runtime and one version in it**. The
      range the version policy promises — 7.0 upwards on Windows, 8.1 upwards elsewhere — is a
      matter of running the same workflow with a different argument, except for the Linux 7.x cell,
      which is **T27a** because it is the only part that costs a pipeline. Nothing here is a client:
      T20 fetches and verifies this index, T21 downloads from it.
- [ ] **T20** Package index client: fetch, Ed25519 signature verification, 6-hour cache, offline mode.
      Written against what T20a produced, not against the schema that preceded it.
- [ ] **T21** Download pipeline: resumable download, SHA-256 verification, staging dir, atomic rename,
      rollback on failure, post-install smoke test.
- [x] **T22** Job system: `jobs` table, `JobProgress`/`JobFinished` events, `job.wait`, cancellation.
      **Done before T20a and T21, out of the order above**, because it is the one task in this phase
      that needs nothing from outside the repo: T20a waits on a signing key and a place to publish
      artifacts, and this waits on nothing. What it costs is that the registry ships with **no
      producer** — the first is T21's download, and T23's `runtime.install` is the first method to
      return a job. That is T19's position exactly, which built the runner before anything could
      declare a service, and for the same reason: the alternative is writing the loop once inside the
      first producer and once properly afterwards.
      **`jobs.state` closes a promise `0001_initial.sql` made in its own header.** That file said the
      column was deliberately un-`CHECK`ed because the state machine "belongs to T22 and does not
      exist yet"; it exists now, four states, closed in Rust for the reason `ServiceState` is, with
      the column carrying the same list. `jobs.kind` stays free text on `packages.name`'s precedent —
      the set grows with every phase that has something long to do, and from T80 with every extension
      — and what keeps it honest is the rule that **a kind is the method that produced it**, so there
      is one vocabulary rather than a second one to keep in step with the method names.
      **T22 is the first task that had to write an `_at` column at runtime**, and it found that this
      workspace still has no date library: every other one is a literal in a fixture. So
      `jobs.started_at` and `finished_at` are epoch milliseconds, joining `services.last_started_at`
      — the same argument T15 made, reaching the same answer, and both are moments the daemon does
      arithmetic on rather than shows. The alternative was a civil-calendar dependency bought to
      parse back what we had just formatted.
      **A job does not survive the process running it, which makes recovery a different problem from
      T18's.** A service is a process of its own and can outlive the daemon that spawned it, so
      recovery there asks the OS what survived and adopts it. The work behind a job is a task
      *inside* the daemon: a row still saying `running` at boot means one thing only, and there is
      nothing to adopt and nothing to signal. `core::jobs::abandon` closes those before the first
      client is served, as **failed** rather than cancelled — nobody asked for the work to stop.
      **Cancellation is cooperative and there is no `Cancelling` state.** Nothing kills the work: a
      download half way through a file has a staging directory to remove, and a task dropped
      mid-`await` does not remove it. So `job.cancel` cancels a token the work watches and answers
      with the job *as it stands* — which may still say `running`, because claiming an outcome this
      daemon has not seen would be the same mistake T19a fixed in the service walk. A state between
      the asking and the ending would have to be written by every producer, and there is no producer
      yet to say whether it is wanted.
      **Work that finished while being cancelled has finished**, which is T15a's stop-command reading
      one layer up: the outcome is judged by what the work produced, and only work that gave up is
      recorded as cancelled — otherwise a download that completed in the instant somebody clicked
      cancel would be recorded as though it had not.
      **`job.wait` is the one method in this API that blocks on purpose**, and the timeout is what
      keeps that inside the rule rather than outside it. A wait that runs out is an **answer** and
      not an error, on `ServiceWalk::complete`'s precedent; the daemon caps what it grants, so a
      client asking for an hour does not hold a connection for one. The row is read *after* the wait
      on both paths, because a job that ended while the caller was being polled has ended.
      **The ordering `wait` rests on is written once, in `Jobs::ended`**: persist, announce, then let
      go of the entry. A waiter is released by a token the registry cancels only after the row is
      written, so it reads the ending rather than racing it — and "is there an entry" is never true
      for a job whose ending is not yet readable, which is what makes the no-entry path able to
      answer from the row alone.
      **Two smaller decisions, each where it is paid for.** `Api::new` reached eight arguments, which
      is the growth the `Shutdown` type's own note predicted, so the two registries became one
      `Supervision` argument rather than a lint being silenced. And jobs are waited for **beside**
      the services at shutdown rather than after them: they hold different things — a port and a data
      directory against a staging directory — and sequencing the two waits would add one budget to
      the other, which is the arithmetic T9a's single budget exists to prevent. A job that will not
      stop inside it is left, and the next daemon's `abandon` closes its row.
      Left for the task that needs it: **no CLI**. `mix job list|status|wait|cancel` is T23's to add
      beside `runtime.install`, on T19a/T19b's split — the wire surface and the client are separate
      tasks, and a client for a namespace nothing can produce a row in would be untestable end to
      end. `core::Error::NotFound { kind: "job" }` therefore names a namespace whose CLI does not
      exist yet, which is the one thing here that is true early rather than wrong.
- [ ] **T23** `runtime.install|uninstall|list_installed|list_available|set_default` — **PHP first**.
- [ ] **T24** Version resolution (`core::resolve`): flag → `mixengine.toml` → project record → default;
      exact/minor/caret constraints.
- [ ] **T25** Shim binary: name-based dispatch, in-process resolution without IPC, `exec` on Unix /
      Job-Object child on Windows, exit-code and signal passthrough. **(P)**
- [ ] **T26** PATH integration for `<root>/bin`, reversible. **(P)**
- [ ] **T27** Node.js, Python, Ruby support in the same pipeline.
- [ ] **T27a** PHP 7.0–8.0 on Linux — the one cell of the version policy nothing can be borrowed for.
      T20a settled the rest of it: Windows reaches 7.0 from the official archive for free, macOS is
      arm64-only and therefore starts at 8.1 where `static-php-cli` starts, and `static-php-cli`
      covers 8.1 upwards on Linux too. What is left is five EOL branches on Linux, and the reason
      they are their own task rather than part of T20a is that they are the only part of the range
      that costs a build pipeline — against sources that predate OpenSSL 3 and the libxml2 API
      removals, with no upstream security releases behind them.
      **Borrow the recipe, not the artifact.** `shivammathur/php-builder` is MIT and already builds
      5.6 through 8.6 with `redis` and `mongodb` on amd64 and arm64; what it produces installs under
      prefix `/usr`, which is exactly why T20a could not take its output. Re-prefixing a working
      recipe is a smaller job than reaching `./configure` for a 2016 tarball, and it is the only
      reason this is affordable at all.
      Deliberately **not** in scope: PHP 7.x on macOS. The policy says arm64 or nothing there, and
      upstream PHP had no Apple Silicon support until 8.0.
- [ ] **T28** PHP extensions: `conf.d` model, enable/disable, prebuilt extension artifacts, per-pool
      reload.
- [ ] **T29** Shim overhead benchmark in CI (< 15 ms budget).

**Milestone M2** — two PHP versions installed; `php -v` differs between two directories with no shell
hook installed.

---

Previous: [Phase 1 — Process supervision](phase-1-process-supervision.md) · Next: [Phase 3 — Services](phase-3-services.md)
