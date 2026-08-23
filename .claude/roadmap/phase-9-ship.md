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
      [**T41a**](phase-4-sites-and-elevation.md), written five phases earlier because a bad answer
      there invalidates [ADR 0005](../decisions/0005-on-demand-elevation.md) and everything built on
      it, while a bad answer here only changes a release process. It was **not** run there: on
      2026-08-23 it was deferred to this release for want of a clean SAC-enforced VM and a bought
      certificate, so **the two halves now fall due together, and v0.1.0 does not ship before both
      are answered.** What is left that is this task's own is the part that only exists once there is
      something to install and something to update.
- [ ] **T87** Complete uninstall path + a clean-VM smoke test proving nothing is left behind.
- [ ] **T88** Auto-update, MixEngine's own: `mix self-update` against `latest.json` on GitHub
      Releases via the stable asset URL (not the API), signature verified before the JSON is parsed,
      daemon check at startup + 24 h interval, silent on failure, consent prompt with notes and size,
      stop → update → relaunch → restore running services, skip/later persisted. The Tauri updater
      this was written on left with [ADR 0011](../decisions/0011-no-gui-in-this-repository.md);
      the design did not.
- [ ] **T88a** `mixengine-elevate` update path: excluded from auto-update, own elevation prompt,
      minisign verified **inside** the elevated context, daemon↔elevate protocol negotiation.
- [ ] **T88c** `daemon.status` is not backwards compatible within one protocol version, and the
      sentence written for exactly that case no longer reaches anybody. Every field added to
      `DaemonStatus` since protocol 1 was fixed is **required** — `elevation` (T40b), `dns` (T44) —
      so a `mix` from a new build asking an older daemon that has not been restarted yet fails to
      *deserialise* the answer. `render::status` carries a note for that skew ("they speak the same
      protocol, so this is a daemon that has not been restarted since the upgrade"), with a test,
      and it is now unreachable: the parse fails before it renders. Found reviewing T44.
      Decide one rule for the whole struct rather than per field — `#[serde(default)]` with an
      `Option` for anything added after a version is frozen, or bumping the protocol whenever a
      required field appears — and apply it to both fields at once. Fixing one of them buys
      nothing while the other is still required, which is why T44 left it alone.
- [x] **T88b** ~~Post-update port-access re-probe~~ — **closed by T42**, which probes at every
      daemon start rather than after an update alone. That catches a capability lost to something
      that was not an update and needs no hook in the updater; two places describing one behaviour
      is what was avoided. See [phase 4](phase-4-sites-and-elevation.md) and
      [ADR 0012](../decisions/0012-a-boot-time-job-enables-the-packet-filter-on-macos.md).
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
