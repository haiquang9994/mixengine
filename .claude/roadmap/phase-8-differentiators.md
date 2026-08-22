# Phase 8 — Differentiators

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T74** LAN sharing: per-site opt-in, rebind, firewall rule (one elevation prompt), LAN URL +
      QR code. **(P)**
- [ ] **T75** mDNS advertisement (`<slug>.mixengine.local`) and CA download endpoint for phones.
- [ ] **T76** Auto-revoke sharing on network change; sharing reported on the event stream so a
      client can surface it; the "web ports only" enforcement test.
- [ ] **T77** Blueprint manifest, `blueprint.capture`, and plan/`--dry-run` output.
- [ ] **T78** `blueprint.apply` execution with resumable idempotent actions and scoped rollback.
- [ ] **T79** Built-in blueprint gallery (Laravel, WordPress, Symfony, static, Next.js proxy, Django),
      doubling as end-to-end tests.
- [ ] **T80** Extension model: `extension.toml`, the four kinds, scoped tokens and permission
      enforcement.
- [ ] **T81** Extension registry client + install/uninstall/start/stop.
- [ ] **T82** First extensions: Mailpit (with the `sendmail_path` recipe for every managed PHP),
      phpMyAdmin, Adminer.
- [ ] **T83** **MixDB integration**: detect installed MixDB, "Open in MixDB" on every database service,
      connection handoff with credentials read from the keyring at click time.
- [ ] **T84** MixDB as a `desktop-app` registry entry + a shared keyring naming convention.

**Milestone M8** — capture a project as a blueprint, apply it to a new one, open its database in
MixDB, and test it from a phone.

---

Previous: [Phase 7 — Efficiency](phase-7-efficiency.md) · Next: [Phase 9 — Ship](phase-9-ship.md)
