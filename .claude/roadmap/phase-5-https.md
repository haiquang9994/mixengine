# Phase 5 — HTTPS

*Goal: green padlock, automatically, forever.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T48** Internal CA generation (`rcgen`), key permissions, fingerprint, `cert.ca_status`.
      Note from T3b: `restrict_to_owner` covers directories, not the files in them, and that is only
      safe because `certs/` is stripped of its ACL *before* anything is written into it — an
      inheritable ACE on a parent reaches new files too (`file_inherit`). A key written to a
      directory that has not been restricted yet keeps the inherited ACE for its whole life, since
      nothing revisits it. Keep the order, or restrict the key file itself.
- [ ] **T49** Trust store install/remove per OS, including Linux NSS DBs for Firefox/Chrome —
      **batched with T42 and T45 into the single first-run elevation prompt**. **(P)**
- [ ] **T50** Leaf issuance: 90 days, site SANs, `serverAuth` only, idempotent reuse.
- [ ] **T51** Web server TLS wiring; **disable Caddy's automatic ACME** explicitly.
- [ ] **T52** Renewal scheduler: daily + on-boot check, < 30 days threshold, reload without restart.
- [ ] **T53** `mix cert status` with a live handshake and SAN-mismatch detection; one-click reissue.
- [ ] **T54** `cert.ca_rotate` and complete `ca_uninstall`, verified by enumerating the stores.

**Milestone M5** — `https://blog.test` is trusted in Chrome, Firefox, Safari and Edge on their
platforms; adding a domain keeps the padlock green.

---

Previous: [Phase 4 — Sites, domains and on-demand elevation](phase-4-sites-and-elevation.md) · Next: [Phase 7 — Efficiency](phase-7-efficiency.md)
