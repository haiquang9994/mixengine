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
- [x] **T85a** The second architecture: `aarch64-pc-windows-msvc` and `aarch64-unknown-linux-gnu`,
      and an old-glibc Linux build. **(P)**
      Split out of T85 rather than half-built inside it. Written as three cross-compilation questions
      and turned out to be one toolchain question and two free native runners: GitHub now hosts
      `windows-11-arm` and `ubuntu-24.04-arm` for public repositories, so both `aarch64` legs build
      natively, the same way macOS's two slices always have. What was left is the glibc floor, which
      both Linux legs now get from a pinned `manylinux_2_28` container rather than from the runner.
      Design: [2026-09-04-t85a-second-architecture-design.md](../../docs/superpowers/specs/2026-09-04-t85a-second-architecture-design.md).
- [x] **T85b** `ServiceInstaller`: register the daemon's autostart entry — Task Scheduler logon task,
      LaunchAgent, systemd **user** unit. **(P)**
      Design: [2026-09-04-t85b-autostart-design.md](../../docs/superpowers/specs/2026-09-04-t85b-autostart-design.md).
      Item 3 of *"What the installer does"* in
      [build-and-release.md](../operations/build-and-release.md), and the one item of that list that
      had never been built. Named here rather than left implied, because a product that installs
      cleanly and then does not come back after a reboot is one nobody would describe as installed.
      **Two things this task changed about its own sentence.** **No installer registers the entry** —
      the three formats that run as root are exactly the three that cannot know which account will
      use MixEngine, so it is `autostart.enable` and `mix autostart`, which is item 2's argument
      reversed ([ADR 0016](../decisions/0016-autostart-is-registered-by-mixengine.md)). And the
      Windows leg needed **a change inside `mixengined`**: a console program run by Task Scheduler is
      handed a *visible* console window in the user's session, measured, and `<Hidden>true</Hidden>`
      does not stop it — so the daemon now releases a console it is the only process attached to.
- [ ] **T85c** `mixengine-shim` is in none of the six artifacts. `packaging/stage.sh` builds three
      crates and `MIX_BINARIES` names three binaries; `core::shims::source` looks for a fourth beside
      the running `mixengined` and raises `Error::ShimMissing` when it is not there — which is an
      empty `bin/` and, with it, **every runtime command the product exists to provide**. So a
      release installed from any of the six artifacts starts, reports itself healthy, and cannot run
      `php`.
      Found by **T88**, which reads the same list. Left there rather than fixed there because it
      changes what every installer ships and needs each script's "open what was just made" check
      widened with it, which is T85's business and not an updater's — and because T88 is written so
      that adding the name is the whole of the fix: the swap set is the payload's own `provides`
      intersected with what is installed, so an installed 0.2.0 takes a 0.3.0 payload that has a shim
      with no further change.
- [x] **T86** Minisign updater keys: generation, CI signing of artifacts, pubkey pinned in the app.
      **No OS code signing** — see [ADR 0005](../decisions/0005-on-demand-elevation.md) and
      [updates.md](../features/updates.md).
      Design: [2026-09-04-t86-updater-signing-design.md](../../docs/superpowers/specs/2026-09-04-t86-updater-signing-design.md).
      **Two things this task settled that its own sentence left open.** The artifacts are signed
      **once, on one runner** and not in each of the five build legs — the secret would otherwise
      reach five jobs, and `minisign` has no official build for the arm64 Windows runner. And a tag
      does not publish a release: it assembles a **draft** somebody publishes, because T88's feed
      lives at a `releases/latest` URL that must not move on a tag push, and because T86a below has
      to watch a real download.
      `latest.json` stayed with **T88**, which produces the payload archives a feed would list; and
      the key T88's design proposed generating for itself arrived here instead, which is the roadmap
      order answering a question that design left open.
- [~] **T86a** Unsigned-distribution reality check for the **installer and the updater**: SmartScreen
      behaviour across two consecutive releases; Gatekeeper flow on macOS 15+. Document the findings
      in `updates.md`. **(P)**
      Design: [2026-09-04-t86a-unsigned-distribution-design.md](../../docs/superpowers/specs/2026-09-04-t86a-unsigned-distribution-design.md).
      **What this task found was that its own sentence asks two questions of different kinds, and
      only one of them needs a person.** Both readings are dialogs as written — but under each dialog
      is a mechanism with an input a machine can read. SmartScreen's gate is reached through
      **Mark-of-the-Web**, Gatekeeper's through **`com.apple.quarantine`**, and both marks are written
      by the application that downloaded the file. So *"how often does a user see the warning"*
      reduces to *"which files in a MixEngine install ever carry a mark"*, which is a property of our
      own artifacts and is now measured on every run of the `build` job by
      `packaging/windows/probe.sh` and `packaging/macos/probe.sh` — against the real installer, the
      real portable zip and the real `.pkg`, with a reading that comes back wrong failing the leg and
      anything the machine could not answer printed as a **void reading** rather than passing
      silently.
      **What stays open is two dialogs**, and they are now release-checklist item 4's rather than
      nobody's: SmartScreen's own verdict on a browser download of a published release, and macOS
      15's System Settings → "Open Anyway" flow in Finder. That also resolves a contradiction this
      entry used to carry — it said v0.1.0 ships after this is answered, while the SmartScreen half
      asks about *two consecutive* releases, which cannot both be true. **The first-release dialog
      gates v0.1.0; the reset across releases gates v0.1.1**, and the reset is not a surprise waiting
      to happen: with no publisher identity, reputation accrues to a file hash and the hash changes
      every build, which is what the probe's W1 establishes.
      The elevation and hosts half of this question is
      [**T41a**](phase-4-sites-and-elevation.md), written five phases earlier because a bad answer
      there invalidates [ADR 0005](../decisions/0005-on-demand-elevation.md) and everything built on
      it, while a bad answer here only changes a release process. It was **not** run there: on
      2026-08-23 it was deferred to this release for want of a clean SAC-enforced VM, and on
      2026-08-24 its certificate question was split off as **T94** below — so three readings fell due
      together, and v0.1.0 does not ship before all of them are answered. **T94 answered its own on
      2026-09-04 and needed no VM to do it**, so what is left is T41a's two, both of which still do.
      What is left that is this task's own is the part that only exists once there is something to
      install and something to update.
- [x] **T94** Does a certificate this project can buy repair Smart App Control, and what is left if
      it cannot? **(P)**
      Design: [2026-09-04-t94-application-control-design.md](../../docs/superpowers/specs/2026-09-04-t94-application-control-design.md).
      Decision: [ADR 0017](../decisions/0017-smart-app-control-is-an-unsupported-configuration.md).
      Findings beside T86a's in [../features/updates.md](../features/updates.md).
      **Three things this task changed about its own sentence.** **The answer needed no purchase.**
      The entry says to settle it "by buying the cheapest usable certificate and trying it"; a
      certificate covers the four images this project builds, Smart App Control judges each image
      load on its own file, and T20a's table says every runtime but Node is unsigned upstream — so it
      repairs the *first* load and the product dies at the second, whatever an EV certificate turns
      out to do. **The population's precondition dissolved.** The count was to decide between the
      remedies; the other two are refused at every size — rebuilding the runtimes re-argues a
      maintenance decision that has only got more expensive, and asking somebody to turn SAC off is a
      one-way door on their own machine — so 1% and 90% lead to the same move, and there is nothing
      here to measure it with anyway. And **it does not supersede
      [ADR 0005](../decisions/0005-on-demand-elevation.md)**, against what this entry predicted: "no
      OS code signing" never stopped being that trade, because the certificate was never the thing
      standing between this product and this policy.
      What it built is the third remedy done honestly: an `AppControl` platform capability reading
      the policy value, a seventeenth `mix doctor` check whose repair declines out loud, and a
      sentence in front of `os error 4551` where MixEngine loads a program it did not build. **The
      check names Smart App Control and the sentence does not** — an enterprise WDAC policy refuses
      the same loads while that value reads `0`, and sending somebody to the wrong setting is worse
      than sending them nowhere.
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
- [x] **T87** Complete uninstall path + a clean-VM smoke test proving nothing is left behind.
      Design: [2026-09-04-t87-uninstall-design.md](../../docs/superpowers/specs/2026-09-04-t87-uninstall-design.md).
      **`--dry-run` is this task's**, and was M4's until 2026-08-24: a milestone three phases earlier
      cannot require a run of something that does not exist yet, and a dry run belongs beside the
      thing it is a run of. What it must list is everything the elevated helper has ever written —
      the hosts block, the resolver wiring, the port grant, the macOS anchor and its boot-time job,
      the CA in every store, and **the audit log**, which is root-owned and outside `MIXENGINE_HOME`
      and therefore needs a privileged operation of its own to remove.
      T47's `mix doctor` already enumerates most of that to reconcile it; this reads the same
      inventory rather than building a second one.
      **Two things this task changed about its own sentence.** The dry run is **a method and not a
      flag** — `daemon.uninstall_plan` beside `daemon.uninstall`, on `daemon.doctor`/`doctor_repair`'s
      split, which is what makes the read half provably a read: no row written, nothing enqueued, no
      prompt possible. And *"nothing is left behind"* **cannot literally hold on Windows**: a file
      whose image is mapped cannot be unlinked and the helper is the running program when it removes
      itself, so there one file leaves at the next restart, the report says so with its own word, and
      the smoke test asserts the operating system accepted the removal rather than that the file is
      gone. What is shared with `mix doctor` turned out to be the **readers** rather than its report:
      `Outcome::Ok` means "installed" on the trust row and "matches" on the hosts row, and an
      uninstall driven off that would remove the wrong one on each machine.
      The clean VM is a fresh CI runner, in the `system` job on all three systems — which is also
      what the two unignored tests that remove anything check for, and skip when the machine running
      them is a workstation with a helper of its own.
- [x] **T88** Auto-update, MixEngine's own: `mix self-update` against `latest.json` on GitHub
      Releases via the stable asset URL (not the API), signature verified before the JSON is parsed,
      daemon check at startup + 24 h interval, silent on failure, consent prompt with notes and size,
      stop → update → relaunch → restore running services, skip/later persisted. The Tauri updater
      this was written on left with [ADR 0011](../decisions/0011-no-gui-in-this-repository.md);
      the design did not.
      Design: [2026-09-04-t88-self-update-design.md](../../docs/superpowers/specs/2026-09-04-t88-self-update-design.md).
      **Three things this task changed about its own sentence.** The order is **download → verify →
      unpack → smoke → stop → swap**, not *stop → update*: taken the other way a developer's database
      is down for the length of a download on a connection nobody promised anything about, and a
      download that fails after the stop has cost an outage for nothing. The signature check on the
      *artifact* is a **SHA-256 inside the minisign-signed feed** rather than a second detached
      signature — one key-handling path establishing the property, which is what `core::index`
      already does for every runtime this product installs. And the whole sequence runs **inside
      `mixengined`** rather than in `mix`, because `mix` may not link `mixengine-core`; what has to
      outlive the daemon is the client, and what it does afterwards is one thing — start the new one.
      **A fourth thing the implementation changed about the design**: *remind me later* is not
      clamped on read but **disbelieved** past seven days. A clamp re-evaluated against `now` moves
      its own deadline forward on every read and never comes due, which the test written from that
      sentence caught.
      What this task did **not** do is replace `mixengine-elevate` — that is **T88a**, and the swap
      excludes it by name and reports it as kept.
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
