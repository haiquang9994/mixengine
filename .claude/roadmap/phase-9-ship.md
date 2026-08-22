# Phase 9 — Ship

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T85** Installers: NSIS per-user + portable zip, `.dmg`, AppImage/`.deb`/`.rpm`; place
      `mixengine-elevate` in a root-owned directory. **(P)**
- [ ] **T86** Minisign updater keys: generation, CI signing of artifacts, pubkey pinned in the app.
      **No OS code signing** — see [ADR 0005](../decisions/0005-on-demand-elevation.md) and
      [updates.md](../features/updates.md).
- [ ] **T86a** Unsigned-distribution reality check for the **installer and the updater**: SmartScreen
      behaviour across two consecutive releases; Gatekeeper flow on macOS 15+. Document the findings
      in `updates.md`. **(P)**
      The elevation and hosts half of this question is
      [**T41a**](phase-4-sites-and-elevation.md), run five phases earlier on purpose: a bad answer
      there invalidates [ADR 0005](../decisions/0005-on-demand-elevation.md) and everything built on
      it, while a bad answer here changes a release process. What is left for this task is the part
      that only exists once there is something to install and something to update.
- [ ] **T87** Complete uninstall path + a clean-VM smoke test proving nothing is left behind.
- [ ] **T88** Auto-update, MixEngine's own: `mix self-update` against `latest.json` on GitHub
      Releases via the stable asset URL (not the API), signature verified before the JSON is parsed,
      daemon check at startup + 24 h interval, silent on failure, consent prompt with notes and size,
      stop → update → relaunch → restore running services, skip/later persisted. The Tauri updater
      this was written on left with [ADR 0011](../decisions/0011-no-gui-in-this-repository.md);
      the design did not.
- [ ] **T88a** `mixengine-elevate` update path: excluded from auto-update, own elevation prompt,
      minisign verified **inside** the elevated context, daemon↔elevate protocol negotiation.
- [ ] **T88b** Post-update port-access re-probe (`setcap` is lost when the binary is replaced) and
      re-request if needed. **(P)**
- [ ] **T89** Upgrade test: an old `mixengine.db` migrated by a new binary, in CI.
- [ ] **T56** Publish the API contract: `ts-rs` bindings generated from `mixengine-proto`,
      committed, checked current by CI, and released as an artifact beside the binaries.
      Moved here from the withdrawn Phase 6
      ([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)). It waits until shipping because nothing in this workspace consumes them: maintaining a published artifact
      against a still-moving API is the same speculative work that ADR withdrew. A client wanting
      them sooner generates them from `mixengine-proto` itself — what this task adds is the
      committed, versioned, checked copy.
- [ ] **T90** User documentation site + in-app help; English and Vietnamese.
- [ ] **T91** Crash reporting that is opt-in and contains no project paths or credentials.
- [ ] **T92** Public beta: the packaging pipeline running for all runtimes across six OS/arch targets
      ([../operations/runtime-packaging.md](../operations/runtime-packaging.md)).

**Milestone M9 — v0.1.0.**

---

Previous: [Phase 8 — Differentiators](phase-8-differentiators.md) · Then: [Parked](parked.md)
