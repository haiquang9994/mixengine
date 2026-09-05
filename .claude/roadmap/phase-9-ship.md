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
- [x] **T85c** `mixengine-shim` is in none of the six artifacts. `packaging/stage.sh` builds three
      crates and `MIX_BINARIES` names three binaries; `core::shims::source` looks for a fourth beside
      the running `mixengined` and raises `Error::ShimMissing` when it is not there — which is an
      empty `bin/` and, with it, **every runtime command the product exists to provide**. So a
      release installed from any of the six artifacts starts, reports itself healthy, and cannot run
      `php`. Found by **T88**, which reads the same list.
      Design: [2026-09-05-t85c-the-shim-in-every-artifact-design.md](../../docs/superpowers/specs/2026-09-05-t85c-the-shim-in-every-artifact-design.md).
      **Adding the name turned out not to be the whole of the fix, in three places the task's own
      sentence could not see.** Two *other* hardcoded lists of the same three binaries existed:
      `packaging/linux/AppRun`, which fills the cache the AppImage actually executes from — and
      whose guard was `mix` alone, so a machine that had run one build of a version would never gain
      a binary a later one added — and `packaging/macos/probe.sh`, whose `cleanup` would have left
      the fourth file on the runner and whose "is this machine occupied" guard would then not have
      seen it. And **`packaging/feed.sh` keyed a Windows payload's `provides` by file name**, so
      `mix self-update` there refused its own release with `MissingFromArtifact` — which is the
      intersection this task was told it could rely on, empty. All three are fixed here, each with a
      check that reproduces the old behaviour: `packaging/linux/apprun-check.sh`,
      `packaging/feed-check.sh`, and `crates/mixengine-core/tests/packaging.rs`, which reads
      `MIX_BINARIES` at compile time and asserts it is the set of names the code looks for — so a
      fifth binary cannot arrive the way the fourth did not.
      **What it leaves.** An install predating a release that has the shim does not gain one by
      updating: `apply::swap` keeps, never adds, a binary the install does not have, on purpose —
      adding files is an install's business and not an update's. Nothing has been released from this
      repository, so that set is empty today; the moment it is not, the answer is a reinstall or a
      rule in `swap` that is somebody's design and not a line slipped into this one.
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
- [x] **T88a** `mixengine-elevate` update path: excluded from auto-update, own elevation prompt,
      minisign verified **inside** the elevated context, daemon↔elevate protocol negotiation.
      Design: [2026-09-05-t88a-the-helper-update-path-design.md](../../docs/superpowers/specs/2026-09-05-t88a-the-helper-update-path-design.md),
      and [ADR 0018](../decisions/0018-a-signed-candidate-is-what-lets-a-path-cross-the-boundary.md),
      which extends [ADR 0015](../decisions/0015-the-helper-installs-itself.md) rather than editing
      it: a path may cross into the elevated process when that process itself checks a signature
      over the bytes at it, and `HelperReplace {}` still carries no field.
      **Four things this task changed about its own sentence.** The first two clauses were already
      true, and the third could not be reached at all: **the upgrade path did not merely lack a
      check, it silently answered `AlreadyDone`**. `elevation::choose` prefers the installed copy, so
      the elevated process on any machine past its first prompt *is* that copy — it compared its own
      image with its own destination and did nothing, for ever, while `swap`'s `KEPT` rule meant
      nothing beside `mixengined` was ever newer either. A 0.1.0 shipped without this is a 0.1.0
      whose helper no later release could fix.
      **The candidate is fetched from the release rather than taken out of the payload**, because
      `UPDATE_SECRET_KEY` reaches exactly one step of one job, after all five `build` legs have
      uploaded: nothing signed can be inside an artifact a build leg produced. So each leg publishes
      its helper as its own asset, `sign.sh` signs it like everything else, and `latest.json` gains
      a `helpers` array — at the cost of an offline machine that cannot upgrade its helper, which is
      written down rather than discovered.
      **`require_helper` compares versions and not bytes, and enqueues nothing.** Bytes were wrong
      in two directions — stale after a `mix self-update`, and different on every rebuild in a
      development tree, which put a row on `mix status` whose only meaning was "you rebuilt". What
      replaces it is `mix elevation upgrade`, which is also the only place the network is touched.
      **And the negotiation's caller is `supported_ops`, not a future protocol.** Without it the only
      way to discover that an installed helper predates `helper-replace` is to enqueue one, spend a
      prompt, and be answered `Unsupported` — which deletes the row and leaves a person with a
      refusal and no sentence. The daemon reads it from an **unelevated** `probe`, so it costs no
      prompt, and marks every request at the lower of the two protocols.
      What this did **not** close is the first prompt on a machine with nothing installed: there the
      elevated binary is the copy beside the daemon and it installs its own image, unchecked. See
      [../architecture/security-model.md](../architecture/security-model.md).
- [x] **T88c** `daemon.status` is not backwards compatible within one protocol version, and the
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
      **Both were made optional** — [ADR 0019](../decisions/0019-an-added-response-member-is-optional.md)
      settles the rule for every response type, a protocol-1 floor fixture in `mixengine-proto` keeps
      it, and `render::status` prints no line for a member nobody reported and names it in the note
      that is now reachable.
- [x] **T88b** ~~Post-update port-access re-probe~~ — **closed by T42**, which probes at every
      daemon start rather than after an update alone. That catches a capability lost to something
      that was not an update and needs no hook in the updater; two places describing one behaviour
      is what was avoided. See [phase 4](phase-4-sites-and-elevation.md) and
      [ADR 0012](../decisions/0012-a-boot-time-job-enables-the-packet-filter-on-macos.md).
- [x] **T89** Upgrade test: an old `mixengine.db` migrated by a new binary, in CI.
      Design: [2026-09-05-t89-the-upgrade-test-design.md](../../docs/superpowers/specs/2026-09-05-t89-the-upgrade-test-design.md).
      **Three things this task changed about its own sentence.** *"An old `mixengine.db`"* had to be
      decided rather than assumed: three suites already exercised migrations and **all three built
      the old database out of today's migration files** — `store.rs`' unit tests write two
      migrations at run time, `migration_extensions.rs` replays a prefix of the real ones — which is
      a reconstruction and not an artifact. What only a committed blob can carry is
      `_sqlx_migrations`' **checksums**, and they are the one thing in this repository that can
      catch an edit to a migration that has already shipped — the first rule in
      [data-model.md](../architecture/data-model.md)'s compatibility list, and until now enforced
      only against migrations a unit test wrote seconds earlier. So the fixtures are frozen files
      committed as bytes, captured by `cargo run -p mixengine-core --example
      capture-upgrade-fixture`, which refuses a destination that exists.
      **"In CI" needed no job.** The suite is `cargo test` with nothing to download and no privilege
      to acquire, so it runs in `test` on all three runners with no edit to the workflow — the rule
      `ci.yml` states about a job arriving only with something to run. All three legs earn it: the
      path under test copies a file, opens it, runs `VACUUM INTO` and renames across a directory,
      and every one of those is where Windows differs.
      **And it found two migrations that empty a table.** `0006` drops `sites`, `site_domains` and
      `site_service_links` outright and `0016` drops `extensions` — no `INSERT … SELECT` in either,
      while the `services` rebuild in that same file does carry its rows over. Recorded and **not
      repaired**: nothing has ever been released from this repository, so the set of databases below
      schema 17 is empty and no user will perform either upgrade, while rewriting a shipped
      migration would break the rule above and invalidate every developer's local database in
      exchange for nothing. The suite names the four tables keyed by version and asserts the loss is
      *total* — an exception that quietly covered a partial one would be worse than none.
      Release-checklist item 2 in
      [build-and-release.md](../operations/build-and-release.md) changes with it: verifying the path
      is CI's, and what stays a person's is **capturing a fixture at the schema being released**,
      because the tree only knows which schema is current and never which one shipped.
      **What it leaves.** `Store::open_read_only` does not migrate, so between a binary upgrade and
      the next daemon start a shim reads a schema its queries were not compiled against. Measured by
      the suite and written down in [../architecture/data-model.md](../architecture/data-model.md);
      closing it is a question about start-up ordering and about what a shim should say, which is
      somebody's design and not a line slipped into a test.
- [x] **T56** Publish the API contract: `ts-rs` bindings generated from `mixengine-proto`,
      committed, checked current by CI, and released as an artifact beside the binaries.
      Moved here from the withdrawn Phase 6
      ([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)). It waits until shipping because nothing in this workspace consumes them: maintaining a published artifact
      against a still-moving API is the same speculative work that ADR withdrew. A client wanting
      them sooner generates them from `mixengine-proto` itself — what this task adds is the
      committed, versioned, checked copy.
      Design: [2026-09-05-t56-the-published-api-contract-design.md](../../docs/superpowers/specs/2026-09-05-t56-the-published-api-contract-design.md).
      Decision: [ADR 0020](../decisions/0020-the-published-contract-is-the-shape-the-daemon-writes.md).
      **Three things this task changed about its own sentence.** *"Generated from
      `mixengine-proto`"* was one type short of possible: **two of its types were called
      `Outcome`**, and `ts-rs` names a file after each — so they were one file, and because the
      exporter *merges* rather than truncates, that file would have carried both declarations in
      whatever order the harness ran them. `rpc::Outcome` is now `ResponseOutcome`, and *type names
      are unique across the crate* is an invariant a test holds rather than a coincidence, because
      `#[ts(rename)]` would only have moved the next collision somewhere a client cannot grep back
      into this repository. **The default mapping of `u64` is a false statement about this wire**:
      `ts-rs` writes `bigint`, `JSON.parse` produces `number`, and a binding that says the first
      cannot type-check against the value it describes — so `TS_RS_LARGE_INT` is set in a new
      `.cargo/config.toml` rather than in the script, because a generator whose answer depends on
      how it was invoked is not one a CI job can check. And *"committed"* turned out to be a claim
      about a **directory** and not a file: `bindings/` is generated to its last byte, barrel and
      README included, which is what lets the check be `diff -r` with nothing to exclude and what
      makes a deleted type take its file with it. `package.json` is the one thing deliberately
      *not* in it — it carries the version, and a committed one would make "cutting a release is a
      version bump and nothing else" false — so `--pack` stamps it into the archive instead.
      **What it leaves.** **Nothing in this repository type-checks the generated TypeScript.**
      There is no frontend toolchain here and [ADR 0011](../decisions/0011-no-gui-in-this-repository.md)
      is why; installing one to run `tsc --noEmit` over somebody else's code generator is a standing
      maintenance cost for a check with no consumer in this tree. What stands in its place is that
      this crate uses none of the shapes `ts-rs` is known to need help with — measured, and it has
      no generic types at all. The first client repository to compile these is the real check, and
      the first bug it finds belongs back in the design. And **`PROTOCOL_VERSION` is not exported**:
      it is a constant, a type-only package cannot carry a runtime value without becoming something
      that has to be built, and a number frozen into a binding would be the version the bindings
      were *generated* from — which is the wrong end of the connection to trust.
- [x] **T90** User documentation site + in-app help; English and Vietnamese. Hosted at
      `mixnz.github.io/mixengine`. Content must be structured so an AI agent can easily fetch and
      understand it (e.g. plain Markdown pages, predictable URLs/paths, no JS-only rendering of the
      actual text) — not just human-readable HTML.
      Design: [2026-09-05-t90-the-documentation-site-design.md](../../docs/superpowers/specs/2026-09-05-t90-the-documentation-site-design.md).
      Decision: [ADR 0021](../decisions/0021-the-handbook-is-one-corpus-published-three-ways.md).
      **Three things this task changed about its own sentence.** *"In-app help"* is **not an API
      method**, and this repository had already decided why: `client-surface.md`'s *Left to the
      client* puts localisation with the client rather than the API, so a `help.get` carrying
      Vietnamese prose would be `mixengined` deciding a client's localisation policy. The second
      reason is stronger and is about failure — the page somebody reaches for is usually the one
      explaining why nothing starts, so `mix docs` is answered **before a home is resolved and
      before a socket is opened**. What a graphical client uses instead is the published Markdown,
      which is what those URLs are stable for.
      **And *"plain Markdown pages"* turned out to decide what `mix` prints.** `render.rs` forbids
      colour and forbids a dependency for one, which removed a terminal renderer from the options —
      and what was left is better than what it replaced: `mix docs sites` emits the same bytes as
      `…/en/sites.md`, so there is one document rather than a document and a rendering of it. The
      corpus is written to read as plain text because of it, and a test holds every page to 100
      columns.
      **The corpus needed no CI job**, which is T89's rule again: parity between the two languages,
      resolvable links, a translation revisited after its source changed and the wrapping are all
      `cargo test`, so `test` runs them on all three operating systems with no edit to the workflow.
      The `docs` job exists for the two things that are not tests — building the site, which compiles
      a Markdown renderer, and diffing `docs/guide/en/cli.md` against what `mix docs --reference`
      prints. Publishing is `pages.yml`, a workflow of its own because deploying needs `pages: write`
      and `ci.yml` is `contents: read`.
      **What it leaves.** **Nothing detects the operating system's language.** Windows sets no
      `LANG`, so doing it properly means a `mixengine-platform` trait method with three
      implementations and a verification round on three operating systems — for a default. What
      ships instead is `--lang`, `MIXENGINE_LANG`, the POSIX variables, and one line of Vietnamese at
      the foot of every English page naming the command that shows the Vietnamese one; the trait
      method belongs beside the other platform work rather than smuggled into a documentation task.
      **Nothing verifies that a translation is correct** — the SHA-256 in each Vietnamese page's
      front matter knows only that somebody looked. **No external link is checked**, because it needs
      the network in a job that otherwise has none. There is no search, which needs JavaScript, and
      the site documents one version, because there is one. And **`mix service logs`' help text lost
      a link**: it cited an ADR by repository path, which is a dead link for every reader of
      `--help` and of the handbook alike — the citation is now a `//` comment beside it.
- [ ] **T91** Crash reporting that is opt-in and contains no project paths or credentials.
- [ ] **T92** Public beta: the packaging pipeline running for all runtimes across six OS/arch targets
      ([../operations/runtime-packaging.md](../operations/runtime-packaging.md)).

**Milestone M9 — v0.1.0.**

---

Previous: [Phase 8 — Differentiators](phase-8-differentiators.md) · Then: [Parked](parked.md)
