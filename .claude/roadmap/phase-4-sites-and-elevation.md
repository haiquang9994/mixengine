# Phase 4 — Sites, domains and on-demand elevation

*Goal: `http://blog.test` works, and creating a site prompts for nothing.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

Design: [ADR 0005](../decisions/0005-on-demand-elevation.md). Nothing here installs a persistent
root process.

---

- [x] **T39** Project model: `project.create|list|show|update|delete|export`, `mixengine.toml`
      read and write, and the `runtime.uninstall` refusal a project pin earns.
      Design: [T39 spec](../../docs/superpowers/specs/2026-08-22-t39-project-model-design.md).
      **`create` is also the import**: with no `--name` and no `--pin`, both come from the manifest
      lying at the root, so a second method would have been a second code path for one outcome.
- [x] **T39a** Site model: `sites`, `site_domains`, `site_service_links`, the four site kinds
      (`php-fpm`, `static`, `reverse-proxy`, `node-app`), doc roots, and the `[site]` and
      `[[services]]` halves of `mixengine.toml`.
      Design: [T39a spec](../../docs/superpowers/specs/2026-08-22-t39a-site-model-design.md).
      T39 left those sections opaque: `core::manifest` reads the file whole and its writer preserves
      them byte for byte, so this task gives them types rather than teaching a second reader about
      them. T43 renders what this declares.
      Three things the task grew that this line did not say. **No new table** — the three have stood
      since the initial migration; `0006_site_state.sql` only closes `sites.state` as
      `enabled`/`disabled`, which is the CHECK `0001_initial.sql` deferred to "a later phase". **The
      fourth refusal is T39a's**, because T39a creates the debt: `service.delete` refuses a service a
      site declares, with a `--force` that crosses the declaration and never the running process
      (T39/D8's line). And **`core::domains`** arrives here rather than with T46, because a site
      cannot be created without deciding what a domain may be.
      **A known gap, recorded rather than left to be discovered:** nothing in this roadmap supervises
      a node process. `node-app` is a declaration; if T43 renders it identically to `reverse-proxy`,
      that is the honest outcome and belongs written down there.
- [x] **T40** **`mixengine-elevate`**: one-shot binary, typed request/response over files, self
      validation, atomic writes under lock, root-owned audit log, distinct "user declined" exit code. **(P)**
      Design: [T40 spec](../../docs/superpowers/specs/2026-08-22-t40-elevate-design.md).
      **The frame plus exactly one operation, `Probe`**, which applies nothing: an empty frame cannot
      be run, and the first time the request/response lifecycle ran for real would otherwise be inside
      a task simultaneously learning what a hosts-file marker block is. `Probe` is also the version
      negotiation the auto-update exclusion makes necessary, and it hands T41a a real binary to put in
      front of Smart App Control.
      Four things this line did not say. **The exit code is the fallback and the response file is the
      protocol** — exit 0 means "there is a report", not "it worked", which inverts what the stub's
      own comment asked for and answers the same danger better. **Elevation is per operation, not a
      gate on the process**, because the operation that reports whether the token is elevated must be
      able to report `false`. **The audit log lives outside `MIXENGINE_HOME`**, since a root-owned
      file in a user-owned directory can be unlinked by that user whatever its mode says. And
      **`mixengine-platform` grew features**, so a binary that runs as root does not carry tokio, the
      keyring backend and its vendored libdbus; CI diffs its whole dependency closure against
      `.github/elevate-dependencies.txt`.
      **This task created the `system` CI job**, which `build-and-release.md` said would arrive with
      the first `#[ignore]`d system test.
      **Three things the code said that the design had not.** `serde`'s `deny_unknown_fields` never
      fires on a *unit* variant of an internally tagged enum — it is read through `deserialize_any`,
      which drops every key but the tag — so `Probe` is `Probe {}`, an empty struct variant, and the
      rule holds for the operation carrying no fields as well as the ones that will. Cargo refuses a
      member's `default-features = false` on an inherited dependency whose workspace entry leaves the
      defaults on, so the default is off at the root and the six crates that want the whole platform
      crate say `features = ["default"]`. And `mixengine-platform` had never been built on its own:
      `tokio/rt` and `tokio/time` were reaching it through the daemon's feature unification.
      **A debt it created:** the audit log is the first thing MixEngine leaves outside
      `MIXENGINE_HOME`, and removing it is itself a privileged operation — `mix uninstall` owes it
      one (T47, T92).
      **A question it recorded rather than answered:** whether `mixengined` should refuse to start
      under an elevated token. That is a change to the daemon, not to the helper, and belongs with
      T40b.
- [x] **T40a** `Elevation` trait: `ShellExecuteEx`/`runas`, osascript `with administrator privileges`,
      `pkexec` — **including polkit-agent detection and the manual-command fallback on Linux**. **(P)**
      Design: [T40a spec](../../docs/superpowers/specs/2026-08-22-t40a-elevation-design.md).
      **The capability stops at the prompt.** It raises one, waits, and answers `Completed`,
      `Declined` or `Unavailable`; it never opens `response.json`, which is `serde_json` over
      types with no operating system in them and is T40b's. `Completed` therefore means the
      helper *ran*, not that it left a report — a crash is not a per-OS event and every
      caller handles it anyway.
      **The half of each launcher that is a decision is compiled on all three systems.**
      `src/prompt.rs` holds the tables — which exit code means the person said no, which
      means there was nobody to ask, how a path is quoted — so each system's table is tested
      on every one of them; only the call that can be made nowhere else stays in `sys::prompt`.
      That is what a `#[path]`-mapped OS directory otherwise costs: a test beside
      `linux/prompt.rs` runs on Linux alone.
      Measured, not reasoned about: on a macOS runner already running as root,
      `do shell script — with administrator privileges` **runs straight through without
      authenticating** — the whole round trip took 0.19 s, so the row stayed a round trip
      rather than being reduced to `probe()`. The Linux runner asserts the opposite branch for
      real: no graphical session, so `Unavailable` carrying the `pkexec` command to run by hand,
      and nothing written beside a request no elevated process ever opened.
      Not proved by any CI run: nobody clicks Cancel — 1223, `-128` and 126 are held by unit
      tests and confirmed only by a person at a machine. T41a is the natural place for the
      Windows leg.
- [x] **T40b** Elevation queue in the daemon: batch pending ops into one invocation,
      `ElevationRequired` event, decline → degraded mode with a pending list. Test: no code path
      elevates in a loop.
      Design: [T40b spec](../../docs/superpowers/specs/2026-08-23-t40b-elevation-queue-design.md).
      The queue is a table whose unique key is the operation itself, so "no code path elevates in a
      loop" is a property of the schema rather than of anybody's discipline; the runtime half is one
      grant slot, and a second is `conflict`. Answered the question T40 recorded and left open: an
      elevated daemon is warned about and reported in `daemon.status`, **not** refused — CI's whole
      Windows third runs the daemon suites under a full token (T2b), and a hard refusal would turn
      one platform red for a reason unrelated to the code under test.
      **No producer ships with it.** T41's `HostsApply` is the first, on T22's and T19's precedent:
      the alternative is writing the queue twice, once inside the first producer and once properly.
- [ ] **T64** The CLI half of elevation UX: `mix` prints every operation an `ElevationRequired`
      batches and what each will literally change — the exact hosts lines, the port, the store —
      *before* raising the prompt, and after a decline `mix status` keeps showing the pending list
      until it is granted or dropped. Moved here from the withdrawn Phase 6
      ([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)); the CLI is the only client now,
      so this is the whole of the elevation UX rather than half of it.
- [ ] **T41** `PrivilegedOp::HostsApply` — marker-block editing with atomic write, locking, and the
      "unrelated lines survive" regression test. **(P)**
- [ ] **T41a** Does an unsigned build run at all, and does this edit survive a machine that has never
      heard of us? **(P)**
      Two questions, and **the first one is already half answered — badly.** Smart App Control refuses
      to *load* an unsigned binary that has no reputation: no warning, no "Run anyway", no path
      exclusion, and Defender's own exclusion list does not apply to it. That is measured rather than
      feared — two of this workspace's own test binaries were refused on a developer machine on
      2026-08-13, inside a directory Defender had been told to ignore, with the Code Integrity events
      recorded in [../features/updates.md](../features/updates.md). Every binary in this product is
      unsigned by design ([ADR 0005](../decisions/0005-on-demand-elevation.md)), so under an enforcing
      SAC there is nothing to elevate, nothing to supervise and nothing to prompt with.
      **Measure the remedy before the population.** "How many users have SAC enforced" is a number
      nobody can act on — 30% and 60% lead to the same next move — and it was the first thing this
      task asked for, wrongly. The question with an action attached is whether **a certificate this
      project can actually buy** makes SAC accept the binary: if it does, the whole thing is a line
      item in T86 and a few hundred dollars a year, ADR 0005 survives with one clause struck, and the
      population stops mattering. SAC admits a file on its signature *or* on ISG reputation, and a
      freshly issued OV certificate has no reputation; whether an EV one is honoured immediately the
      way SmartScreen honours it is precisely the thing to settle by buying the cheapest usable
      certificate and trying it on the VM, not by reading about it.
      Only if that answer is **no** is the population worth counting — SAC enabled on a clean Windows
      11 install, off after an in-place upgrade, switching itself out of evaluation when it sees a
      developer at work — and then it decides between accepting the loss and changing how this is
      distributed.
      The second question is the one this task was originally written for. Defender ships a
      `HostsFileHijack` heuristic aimed at writes to `%SystemRoot%\System32\drivers\etc\hosts`, and an
      unsigned binary doing it is far likelier to trip it. So: an unsigned build, a clean Windows VM
      with full protection on, elevation through the real prompt, the marker block written — and a
      record of what actually happened, including SmartScreen on the first run of the elevated binary
      and the Gatekeeper equivalent on macOS.
      **The SAC half does not need T41 and should not wait for it.** It needs a binary and a clean
      machine, both of which exist today; only the hosts half needs the code this phase builds. Run
      it as soon as there is a VM to run it on.
      **Here rather than with T86a because of what a bad answer costs.** T86a asks the same question
      of the *installer and the updater*, where a bad answer changes a release process. This one can
      invalidate ADR 0005 itself — and T42, T43, T44, T45 and the whole of Phase 5 are built on top
      of it, so learning at phase 9 that the elevated write is quarantined means five phases resting
      on a design that never reaches a user's machine. It is a day's work against a VM, which is the
      entire argument for doing it now: cheap to run, and cheap to be wrong about only while it is
      early.
      Findings go in [../features/updates.md](../features/updates.md) beside T86a's, not into this
      file.
- [ ] **T42** `PortAccess`: no-op on Windows, pf anchor redirect on macOS, `setcap`/nftables on Linux,
      plus **re-probe after every app update** (setcap is lost when the binary is replaced). **(P)**
- [ ] **T43** Site → config → reload end-to-end; `site.start|stop`, idempotent re-runs.
- [ ] **T44** Built-in DNS server (`hickory`): bind **5353** on macOS/Linux and **53** on Windows,
      wildcard answers for managed TLDs, upstream forwarding, loopback-only recursion, port-in-use
      detection with the owning process reported.
- [ ] **T45** Resolver wiring per OS with a custom port: `/etc/resolver` + `port`,
      `resolvectl dns …:5353` / dnsmasq `#5353`, NRPT (port 53) — TLD-scoped only, never global. **(P)**
- [ ] **T46** `domain.*` RPC + `domain.dns_status` real-lookup diagnostics.
- [ ] **T46a** Hosts-only fallback mode: wildcards disabled, batched hosts prompts, reported as a
      distinct mode on the API so any client can say so plainly.
- [ ] **T47** `mix doctor` / `doctor_repair`: reconcile hosts, DNS, resolver, port grant, orphans,
      stale config; flush deferred privileged ops; **say which orphan guarantee this OS actually
      gives** — total on Windows, the immediate child only on Linux, none on macOS ([ADR
      0007](../decisions/0007-supervised-child-owns-a-process-group.md), settled by T13), because
      repeating Windows's promise on macOS is the specific failure that ADR exists to prevent;
      **detect Windows excluded port ranges**
      (`netsh int ipv4 show excludedportrange`) which look like permission errors but are not.
      Also re-check home permissions via `DirectoryAccess::is_restricted_to_owner` (T3a). **Decide
      there whether to keep `icacls`**: the answer it gives on Windows is narrow — inheritance
      severed, yes or no — because `icacls` prints localised account names and no SIDs, so the
      trustee list cannot be checked. Doing better means `GetNamedSecurityInfoW` +
      `GetSecurityDescriptorControl` (the `SE_DACL_PROTECTED` flag, exactly, no parsing) and
      `GetAce` + `EqualSid` to compare the three trustees, with `SetNamedSecurityInfoW` +
      `SetEntriesInAclW` replacing the apply path for symmetry. That is ~150 lines of `unsafe`
      FFI on `windows-sys`, which this crate is allowed per item — the reason it was not done in
      T3a is that the *apply* path is verified working and the check had no caller yet. If T47
      only reports "inheritance is intact", the swap is still not worth it.
- [ ] **T93** `mix doctor --bundle`: one diagnostics archive — daemon log excerpt, `mix doctor`
      output, versions and platform facts, credentials redacted — so that "copy diagnostics"
      costs a client nothing to assemble
      ([../features/client-surface.md](../features/client-surface.md)). Carried over from the
      withdrawn Phase 6's T66, which owned the requirement
      ([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)).

**Milestone M4** — create a site and open `http://blog.test` in a fresh shell on all three OSes with
**zero elevation prompts after first-run setup**; `mix uninstall --dry-run` shows a complete cleanup.

---

Previous: [Phase 3 — Services](phase-3-services.md) · Next: [Phase 5 — HTTPS](phase-5-https.md)
