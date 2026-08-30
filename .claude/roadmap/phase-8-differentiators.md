# Phase 8 — Differentiators

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T74** LAN sharing: per-site opt-in, rebind the web server and nothing else, firewall rule
      (one elevation prompt), the LAN URL, and the site certificate reissued with the LAN IP among
      its SANs so HTTPS does not break the moment it leaves loopback. Where the firewall cannot be
      managed, say so and give the manual command rather than reporting success. A QR code is a
      rendering of that URL and not a screen this repo owns: the daemon answers the URL, `mix`
      prints the code in the terminal, a graphical client draws its own. **(P)**
- [ ] **T75** mDNS advertisement (`<slug>.mixengine.local`), that name added to the certificate SANs
      beside the LAN IP, and the CA download endpoint for phones — served only while sharing is on,
      only the public certificate.
- [ ] **T76** Revoking, however it starts: a network change disables sharing and says why, and so
      does turning it off — both remove the firewall rule, stop the advertisement, rebind to
      loopback and reissue the certificate without the LAN SANs. Optional `--for 2h` expiry.
      Sharing reported on the event stream so a client can surface it. Two enforcement tests: the
      "web ports only" scan, and no firewall rule left behind, enumerated by label. **(P)**
- [ ] **T77** Blueprint manifest, `blueprint.capture` — capturing what a project actually uses rather
      than the global defaults, and never data, credentials or absolute paths — and the plan output
      that `mix blueprint apply --dry-run` prints.
- [ ] **T78** `blueprint.apply` execution with resumable idempotent actions and rollback scoped to
      what this apply created; a version mismatch is answered as a choice (install / use the
      installed one / cancel), never decided quietly.
- [ ] **T78a** Scaffold trust: `[scaffold]` is arbitrary code from whoever wrote the blueprint.
      It never runs on import, only on apply, only after a confirmation showing the exact command,
      with output streamed to the job log; gallery blueprints are signed and a hand-imported one is
      marked untrusted for good.
- [ ] **T79** Built-in blueprint gallery (Laravel, WordPress, Symfony, static, Next.js proxy, Django),
      doubling as end-to-end tests — one of them exported on Windows and applied on macOS.
- [ ] **T80** Extension model: `extension.toml` read through the `ServiceSpec` vocabulary in
      `mixengine-proto`, the four kinds, scoped tokens and permission enforcement — `network =
      "loopback"` is what stops an extension reaching the LAN, and it is enforced rather than
      documented.
- [ ] **T81** Extension registry client + install/uninstall/start/stop: signed `index.json` verified
      against the compiled-in Ed25519 key, artifacts by SHA-256, `--path` installs carrying a loud
      unsigned marker, an unparsable entry skipped instead of failing the whole index.
- [ ] **T82** First extensions: Mailpit (with the `sendmail_path` recipe for every managed PHP),
      phpMyAdmin, Adminer.
- [ ] **T83** **MixDB integration**: detect an installed MixDB behind the platform layer, a daemon
      method answering the connection handoff for one database service and the `mix` command that
      asks for it, credential read from the keyring at that moment and never placed in an argument
      or a URL, "not installed" answered as a state rather than an error. **(P)**
- [ ] **T84** MixDB as a `desktop-app` registry entry + a shared keyring naming convention.

**Milestone M8** — capture a project as a blueprint, apply it to a new one, open its database in
MixDB, and test it from a phone.

---

Previous: [Phase 7 — Efficiency](phase-7-efficiency.md) · Next: [Phase 9 — Ship](phase-9-ship.md)
