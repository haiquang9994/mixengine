# Phase 4 — Sites, domains and on-demand elevation

*Goal: `http://blog.test` works, and creating a site prompts for nothing.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

Design: [ADR 0005](../decisions/0005-on-demand-elevation.md). Nothing here installs a persistent
root process.

---

- [ ] **T39** Project & site model: create/import/update/delete, doc root, site kinds
      (`php-fpm`, `static`, `reverse-proxy`, `node-app`), `mixengine.toml` read/write.
- [ ] **T40** **`mixengine-elevate`**: one-shot binary, typed request/response over files, self
      validation, atomic writes under lock, root-owned audit log, distinct "user declined" exit code. **(P)**
- [ ] **T40a** `Elevation` trait: `ShellExecuteEx`/`runas`, osascript `with administrator privileges`,
      `pkexec` — **including polkit-agent detection and the manual-command fallback on Linux**. **(P)**
- [ ] **T40b** Elevation queue in the daemon: batch pending ops into one invocation,
      `ElevationRequired` event, decline → degraded mode with a pending list. Test: no code path
      elevates in a loop.
- [ ] **T41** `PrivilegedOp::HostsApply` — marker-block editing with atomic write, locking, and the
      "unrelated lines survive" regression test. **(P)**
- [ ] **T42** `PortAccess`: no-op on Windows, pf anchor redirect on macOS, `setcap`/nftables on Linux,
      plus **re-probe after every app update** (setcap is lost when the binary is replaced). **(P)**
- [ ] **T43** Site → config → reload end-to-end; `site.start|stop`, idempotent re-runs.
- [ ] **T44** Built-in DNS server (`hickory`): bind **5353** on macOS/Linux and **53** on Windows,
      wildcard answers for managed TLDs, upstream forwarding, loopback-only recursion, port-in-use
      detection with the owning process reported.
- [ ] **T45** Resolver wiring per OS with a custom port: `/etc/resolver` + `port`,
      `resolvectl dns …:5353` / dnsmasq `#5353`, NRPT (port 53) — TLD-scoped only, never global. **(P)**
- [ ] **T46** `domain.*` RPC + `domain.dns_status` real-lookup diagnostics.
- [ ] **T46a** Hosts-only fallback mode: wildcards disabled, batched hosts prompts, clearly signalled
      in the GUI.
- [ ] **T47** `mix doctor` / `doctor_repair`: reconcile hosts, DNS, resolver, port grant, orphans,
      stale config; flush deferred privileged ops; **detect Windows excluded port ranges**
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

**Milestone M4** — create a site and open `http://blog.test` in a fresh shell on all three OSes with
**zero elevation prompts after first-run setup**; `mix uninstall --dry-run` shows a complete cleanup.

---

Previous: [Phase 3 — Services](phase-3-services.md) · Next: [Phase 5 — HTTPS](phase-5-https.md)
