# Phase 3 — Services

*Goal: web server, databases and caches run with generated config.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T30** Config generation engine: `minijinja` templates, typed overrides, atomic write,
      no-op-if-identical diffing, validation hook before install.
- [ ] **T31** Caddy integration: global Caddyfile + per-site imports, `caddy validate`, graceful
      reload, admin API health.
- [ ] **T32** php-fpm pools: one service per PHP version, socket/port per pool, `SIGUSR2` reload.
- [ ] **T33** MariaDB: install, `mariadb-install-db` first-run job, random root password in the OS
      keyring, secure defaults, dev-tuned `my.cnf`. **(P)**
- [ ] **T34** PostgreSQL: `initdb`, `pg_hba` local-only, superuser creation.
- [ ] **T35** Redis + Memcached with dev-tuned config.
- [ ] **T36** Multiple instances of one service (`mariadb@main`, `mariadb@legacy`) with independent
      ports and data dirs.
- [ ] **T37** Nginx as the alternative front end; parity test suite running both generators.
- [ ] **T38** Port conflict diagnosis: report the owning process name, not just `EADDRINUSE`. **(P)**

**Milestone M3** — `mix service start caddy mariadb redis` → all healthy in under 10 s warm.

---

Previous: [Phase 2 — Runtimes](phase-2-runtimes.md) · Next: [Phase 4 — Sites, domains and on-demand elevation](phase-4-sites-and-elevation.md)
