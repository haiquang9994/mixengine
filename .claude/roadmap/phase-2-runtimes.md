# Phase 2 — Runtimes

*Goal: multiple PHP/Node/Python/Ruby versions installed and selectable.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T20a** One real artifact and one real index, before a client is written against either. **(P)**
      **Ordered before T20 although it is lettered after it**, on T19c's precedent. T20–T24 are a
      client for a package index that nobody has produced: the schema in
      [../operations/runtime-packaging.md](../operations/runtime-packaging.md) is a sketch, and a
      client written against a sketch is a client written against a guess — which is the same mistake
      T19 refused to make when it declined to invent a `spec_json` column before T30 existed.
      What this produces is the smallest thing that is not a guess: **one** runtime, end to end. A
      relocatable PHP artifact for each of the three OSes, a minisign-signed `index.json` holding
      exactly those, published where a URL reaches it, and `php -v` executed from a directory that is
      not the one it was built in.
      **On Windows that smoke test has a second half, and no code of ours can pass it.** Smart App
      Control judges *every* image load, which includes `php.exe`, `php-fpm.exe`, `caddy.exe`,
      `mariadbd.exe` — every runtime MixEngine downloads and starts, and all of it unsigned wherever
      we were the ones who built it. So `php -v` is run once on a machine with SAC **enforced** and
      not only on a developer machine where it is off, and the Authenticode status of every upstream
      artifact this project intends to redistribute is recorded while the table below is being filled
      in (`Get-AuthenticodeSignature`, which costs nothing and answers immediately).
      That changes what "borrow" is worth: an artifact whose publisher already signs it may run where
      one we built cannot, **independently of how much work the build would have been**. Whether a
      certificate we can buy fixes it for our own binaries is
      [T41a](phase-4-sites-and-elevation.md)'s question, not this one's — but if the answer there is
      no, this is where the consequence lands first. The pipeline may be a script, the CDN a static host and the key a
      developer key; what it may not be is absent, because T21's post-install smoke test and T23's
      `runtime.install` both need something real to install.
      **Its first output is a decision rather than a file.** For every "we build" cell, find out
      whether somebody has already solved relocatability for that runtime — the way
      `python-build-standalone` already does for Python, which is the one row of that table nobody
      has to maintain. A borrowed artifact costs one evaluation; an owned one costs a build pipeline
      kept current for every security release of that runtime, for as long as MixEngine offers the
      version. The candidates and the rule are in
      [../operations/runtime-packaging.md](../operations/runtime-packaging.md#borrow-before-you-build).
      **Attempt macOS first.** `install_name_tool` over every bundled dylib followed by a re-sign is
      where relocatability usually fails, and Ventura and later reject a signed binary that has been
      modified — so if a "we build" cell is going to be unaffordable, that is the one that says so.
      Finding it out here costs one task; finding it out at T92 costs the plan.
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
- [ ] **T28** PHP extensions: `conf.d` model, enable/disable, prebuilt extension artifacts, per-pool
      reload.
- [ ] **T29** Shim overhead benchmark in CI (< 15 ms budget).

**Milestone M2** — two PHP versions installed; `php -v` differs between two directories with no shell
hook installed.

---

Previous: [Phase 1 — Process supervision](phase-1-process-supervision.md) · Next: [Phase 3 — Services](phase-3-services.md)
