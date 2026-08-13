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
- [ ] **T22** Job system: `jobs` table, `JobProgress`/`JobFinished` events, `job.wait`, cancellation.
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
