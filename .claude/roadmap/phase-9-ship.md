# Phase 9 — Ship

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T85** Installers: NSIS per-user + portable zip, `.dmg`, AppImage/`.deb`/`.rpm`; place
      `mixengine-elevate` in a root-owned directory. **(P)**
- [ ] **T86** Minisign updater keys: generation, CI signing of artifacts, pubkey pinned in the app.
      **No OS code signing** — see [ADR 0005](../decisions/0005-on-demand-elevation.md) and
      [updates.md](../features/updates.md).
- [ ] **T86a** Unsigned-distribution reality check: SmartScreen behaviour across two consecutive
      releases; Defender `HostsFileHijack` heuristic with full protection enabled; Gatekeeper flow on
      macOS 15+. Document the findings in `updates.md`. **(P)**
- [ ] **T87** Complete uninstall path + a clean-VM smoke test proving nothing is left behind.
- [ ] **T88** Auto-update: `latest.json` on GitHub Releases via the stable asset URL (not the API),
      launch check + 24 h interval, silent on failure, consent dialog with notes and size,
      stop → update → relaunch → restore running services, skip/later persisted.
- [ ] **T88a** `mixengine-elevate` update path: excluded from auto-update, own elevation prompt,
      minisign verified **inside** the elevated context, daemon↔elevate protocol negotiation.
- [ ] **T88b** Post-update port-access re-probe (`setcap` is lost when the binary is replaced) and
      re-request if needed. **(P)**
- [ ] **T89** Upgrade test: an old `mixengine.db` migrated by a new binary, in CI.
- [ ] **T90** User documentation site + in-app help; English and Vietnamese.
- [ ] **T91** Crash reporting that is opt-in and contains no project paths or credentials.
- [ ] **T92** Public beta: the packaging pipeline running for all runtimes across six OS/arch targets
      ([../operations/runtime-packaging.md](../operations/runtime-packaging.md)).

**Milestone M9 — v0.1.0.**

---

Previous: [Phase 8 — Differentiators](phase-8-differentiators.md) · Then: [Parked](parked.md)
