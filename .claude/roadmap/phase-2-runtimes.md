# Phase 2 — Runtimes

*Goal: multiple PHP/Node/Python/Ruby versions installed and selectable.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T20** Package index client: fetch, Ed25519 signature verification, 6-hour cache, offline mode.
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
