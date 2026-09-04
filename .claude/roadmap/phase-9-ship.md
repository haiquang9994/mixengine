# Phase 9 — Ship

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [x] **T85** Installers: NSIS per-user + portable zip, ~~`.dmg`~~ **`.pkg`**, AppImage/`.deb`/`.rpm`;
      place `mixengine-elevate` in a root-owned directory. **(P)**
      Design: [2026-09-04-t85-installers-design.md](../../docs/superpowers/specs/2026-09-04-t85-installers-design.md).
      **Two things this task changed about its own sentence.** macOS ships a **`.pkg`**: a `.dmg` is a
      carrier for something you drag out of it, and the application bundle that used to be dragged
      left with [ADR 0011](../decisions/0011-no-gui-in-this-repository.md) — what is there to ship is
      three command-line binaries, and a `.pkg` additionally runs as root. And **no installer places
      the helper**: four of the six formats install entirely as the user, so the placement is a
      privileged operation of MixEngine's own, applied inside the prompt first-run setup already
      costs — [ADR 0015](../decisions/0015-the-helper-installs-itself.md). A `.deb`, `.rpm` or `.pkg`
      does it at install time anyway and the operation then answers `AlreadyDone`.
- [ ] **T85a** The second architecture: `aarch64-pc-windows-msvc` and `aarch64-unknown-linux-gnu`,
      and an old-glibc Linux build. **(P)**
      Split out of T85 rather than half-built inside it. Both are cross-compilations of a workspace
      that builds SQLite, AWS-LC and libdbus from C, on runners that carry no cross toolchain, and
      the Linux row additionally wants a manylinux-style container so binaries run on LTS distros —
      three toolchain questions that have nothing to do with what an installer is. macOS was not
      split off and is universal already, because Apple's own toolchain builds the other slice with
      no extra sysroot.
- [ ] **T85b** `ServiceInstaller`: register the daemon's autostart entry — Task Scheduler logon task,
      LaunchAgent, systemd **user** unit. **(P)**
      Item 3 of *"What the installer does"* in
      [build-and-release.md](../operations/build-and-release.md), and the one item of that list that
      has never been built: the trait is in
      [platform-abstraction.md](../architecture/platform-abstraction.md)'s table and has no
      implementation on any of the three systems. Named here rather than left implied, because a
      product that installs cleanly and then does not come back after a reboot is one nobody would
      describe as installed.
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
      2026-08-23 it was deferred to this release for want of a clean SAC-enforced VM, and on
      2026-08-24 its certificate question was split off as **T94** below — so **three readings now
      fall due together, and v0.1.0 does not ship before all of them are answered.** What is left
      that is this task's own is the part that only exists once there is something to install and
      something to update.
- [ ] **T94** Does a certificate this project can buy repair Smart App Control, and what is left if
      it cannot? **(P)**
      Split out of [**T41a**](phase-4-sites-and-elevation.md) on 2026-08-24, and **here rather than
      there because of what the answer changes**: T41a's half can invalidate ADR 0005 and five phases
      resting on it, which is why it was written early; this half changes how the product is
      distributed, which is this phase's business and nobody else's.
      **The question is narrower than it was when T41a asked it, and that narrowing is the reason it
      moved.** SAC admits a file on its signature *or* on ISG reputation; a freshly issued OV
      certificate has no reputation, and whether an EV one is honoured immediately the way SmartScreen
      honours it is a thing to settle by buying the cheapest usable certificate and trying it, not by
      reading about it. All of that still holds — **for the binaries this project builds**. What T20a
      and T27 measured is that PHP, nginx and Caddy are unsigned *upstream*, so those were never the
      binaries the question was really about.
      So the task is three readings and not one: what a certificate covers, what it leaves uncovered,
      and what the cheapest thing that covers the rest is. The candidates are rebuilding and signing
      the runtimes — which "borrow before you build" refused on maintenance cost and which would have
      to be re-argued rather than assumed — asking a user to turn SAC off, which is "a product that
      does not start" in another phrasing, and accepting the loss while naming what it costs.
      **Only for that last one is the population worth counting** — SAC enabled on a clean Windows 11
      install, off after an in-place upgrade, switching itself out of evaluation when it observes a
      developer at work. It was the first thing T41a asked for and it was the wrong first question
      there, because a number nobody can act on is not a measurement; it becomes actionable exactly
      when the remedies above are closed.
      A bad answer here **supersedes** [ADR 0005](../decisions/0005-on-demand-elevation.md) rather
      than amending it: "no OS code signing" would have stopped being a trade of first-launch
      friendliness against a few hundred dollars a year.
      Findings go in [../features/updates.md](../features/updates.md), beside T41a's and T86a's.
- [ ] **T87** Complete uninstall path + a clean-VM smoke test proving nothing is left behind.
      **`--dry-run` is this task's**, and was M4's until 2026-08-24: a milestone three phases earlier
      cannot require a run of something that does not exist yet, and a dry run belongs beside the
      thing it is a run of. What it must list is everything the elevated helper has ever written —
      the hosts block, the resolver wiring, the port grant, the macOS anchor and its boot-time job,
      the CA in every store, and **the audit log**, which is root-owned and outside `MIXENGINE_HOME`
      and therefore needs a privileged operation of its own to remove.
      T47's `mix doctor` already enumerates most of that to reconcile it; this reads the same
      inventory rather than building a second one.
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
