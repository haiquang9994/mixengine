# MixEngine build plan

Tasks are ordered. Work top to bottom — each phase depends on the ones above it. Tick items as they
land; when new work appears, insert it **where it belongs in the order**, not at the end.

Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** = has a platform-layer component and
needs verification on Windows + macOS + Linux.

---

## Phase 0 — Foundations
*Goal: an empty but real system — daemon starts, CLI talks to it, state persists.*

- [ ] **T1** Cargo workspace scaffold: the seven crates from [CLAUDE.md](../../CLAUDE.md) with their
      dependency direction enforced, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml`.
- [ ] **T2** CI skeleton: `lint` + `test` jobs on windows/macos/ubuntu, network egress blocked for tests.
- [ ] **T3** Paths & config: `MIXENGINE_HOME` resolution per OS, directory bootstrap, `config.toml`
      loading with defaults. **(P)**
- [ ] **T4** Logging: `tracing` setup, file + stderr sinks, `MIXENGINE_LOG_FORMAT=json`, rotation of
      `daemon.log`.
- [ ] **T5** Error model: `mixengine-proto::Error` with stable codes + hints; per-crate `thiserror`
      enums and conversions at the daemon boundary.
- [ ] **T6** SQLite store: `sqlx` setup, WAL, migration runner, the schema from
      [data-model.md](../architecture/data-model.md), pre-migration backup.
- [ ] **T7** IPC transport: Unix socket + Windows named pipe with owner-only permissions and peer
      credential checks. **(P)**
- [ ] **T8** JSON-RPC server over the transport: `daemon.status`, `daemon.version`, `/health`,
      `/events` SSE, panic-to-`internal` catch, request spans.
- [ ] **T9** Daemon lifecycle: single-instance lock, `--foreground`/`--detach`, graceful shutdown with
      cancellation token.
- [ ] **T10** CLI skeleton: `clap` tree, transport client, daemon autostart-on-connect, human + `--json`
      output, `mix status`.
- [ ] **T11** Test harness: per-test `TempDir` home, `mock::Host` with operation recording,
      `fakeservice` fixture binary, `MockRegistry`.

**Milestone M0** — `mix status` prints a healthy daemon on all three OSes in CI.

---

## Phase 1 — Process supervision
*Goal: we can run and babysit arbitrary programs correctly. Everything later is built on this.*

- [ ] **T12** `ServiceSpec`, `ReadyCheck`, `HealthCheck`, `RestartPolicy`, `StopBehaviour` types.
- [ ] **T13** Spawn with process groups: Job Object (Windows), `setsid` + `PR_SET_PDEATHSIG` (Unix);
      no orphans when the daemon dies. **(P)**
- [ ] **T14** State machine + persistence + `ServiceStateChanged` events; `Degraded` vs `Failed`.
- [ ] **T15** Ready/health polling, restart backoff, crash-loop cutoff with the last 200 log lines
      attached to the failure reason.
- [ ] **T16** Log capture: line splitting, per-service files, size rotation, in-memory ring buffer,
      `LogLine` events, `GET /logs/{id}?follow=1`.
- [ ] **T17** Dependency DAG start/stop ordering; cycle detection at spec-build time.
- [ ] **T18** Crash recovery: PID + start-time adoption, stale socket/pidfile cleanup on daemon boot.
- [ ] **T19** `service.*` RPC surface + `mix service start|stop|restart|status|logs`.

**Milestone M1** — kill the daemon mid-run; on restart it adopts what survived and cleans what did
not. Proven by tests against `fakeservice` on all three OSes.

---

## Phase 2 — Runtimes
*Goal: multiple PHP/Node/Python/Ruby versions installed and selectable.*

- [ ] **T20** Package index client: fetch, Ed25519 signature verification, 6-hour cache, offline mode.
- [ ] **T21** Download pipeline: resumable download, SHA-256 verification, staging dir, atomic rename,
      rollback on failure, post-install smoke test.
- [ ] **T22** Job system: `jobs` table, `JobProgress`/`JobFinished` events, `job.wait`, cancellation.
- [ ] **T23** `runtime.install|uninstall|list_installed|list_available|set_default` — **PHP first**.
- [ ] **T24** Version resolution (`core::resolve`): flag → `mixengine.toml` → project record → default;
      exact/minor/caret constraints.
- [ ] **T25** Shim binary: name-based dispatch, in-process resolution without IPC, `exec` on Unix /
      Job-Object child on Windows, exit-code and signal passthrough. **(P)**
- [ ] **T26** PATH integration for `<root>/bin`, reversible. **(P)**
- [ ] **T27** Node.js, Python, Ruby support in the same pipeline.
- [ ] **T28** PHP extensions: `conf.d` model, enable/disable, prebuilt extension artifacts, per-pool
      reload.
- [ ] **T29** Shim overhead benchmark in CI (< 15 ms budget).

**Milestone M2** — two PHP versions installed; `php -v` differs between two directories with no shell
hook installed.

---

## Phase 3 — Services
*Goal: web server, databases and caches run with generated config.*

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

## Phase 4 — Sites, domains and the privileged helper
*Goal: `http://blog.test` works.*

- [ ] **T39** Project & site model: create/import/update/delete, doc root, site kinds
      (`php-fpm`, `static`, `reverse-proxy`, `node-app`), `mixengine.toml` read/write.
- [ ] **T40** **Helper binary**: install as a system service/daemon per OS, control channel, peer
      identity verification, argument validation, audit log, idle exit. **(P)**
- [ ] **T41** `PrivilegedOp::HostsApply` — marker-block hosts editing with atomic write, locking, and
      the "unrelated lines survive" regression test. **(P)**
- [ ] **T42** Privileged port binding for 80/443 (socket passing on Unix; direct on Windows). **(P)**
- [ ] **T43** Site → config → hosts → reload end-to-end; `site.start|stop`, idempotent re-runs.
- [ ] **T44** Built-in DNS server (`hickory`): wildcard answers for managed TLDs, upstream forwarding,
      loopback-only recursion, port-53-in-use detection with the owning process reported.
- [ ] **T45** Resolver wiring per OS: `/etc/resolver`, `systemd-resolved`/NM, NRPT — TLD-scoped only,
      never global. **(P)**
- [ ] **T46** `domain.*` RPC + `domain.dns_status` real-lookup diagnostics.
- [ ] **T47** `mix doctor` / `doctor_repair`: verify and reconcile hosts, DNS, ports, orphans, stale
      config.

**Milestone M4** — create a site, open `http://blog.test` in a fresh shell on all three OSes, then
`mix uninstall --dry-run` shows a complete cleanup.

---

## Phase 5 — HTTPS
*Goal: green padlock, automatically, forever.*

- [ ] **T48** Internal CA generation (`rcgen`), key permissions, fingerprint, `cert.ca_status`.
- [ ] **T49** Trust store install/remove per OS, including Linux NSS DBs for Firefox/Chrome. **(P)**
- [ ] **T50** Leaf issuance: 90 days, site SANs, `serverAuth` only, idempotent reuse.
- [ ] **T51** Web server TLS wiring; **disable Caddy's automatic ACME** explicitly.
- [ ] **T52** Renewal scheduler: daily + on-boot check, < 30 days threshold, reload without restart.
- [ ] **T53** `mix cert status` with a live handshake and SAN-mismatch detection; one-click reissue.
- [ ] **T54** `cert.ca_rotate` and complete `ca_uninstall`, verified by enumerating the stores.

**Milestone M5** — `https://blog.test` is trusted in Chrome, Firefox, Safari and Edge on their
platforms; adding a domain keeps the padlock green.

---

## Phase 6 — Desktop GUI
*Goal: the terminal becomes optional.*

- [ ] **T55** Tauri v2 shell + Rust proxy to the daemon socket + SSE relay to the webview.
- [ ] **T56** `ts-rs` binding generation + the CI check that committed bindings are current.
- [ ] **T57** Frontend foundation: Vite, strict TS, TanStack Query, event→invalidation mapping,
      `ui/` primitives, i18n (English + Vietnamese), light/dark.
- [ ] **T58** Dashboard: service tiles, metrics (`metrics.subscribe`, sampling only while subscribed),
      disk usage, recent events.
- [ ] **T59** Sites screen: list, create/edit drawer, open in browser/folder/terminal.
- [ ] **T60** Runtimes screen: installed/available, install jobs with progress, PHP extension toggles.
- [ ] **T61** Services screen: settings forms, rendered config read-only, validation errors,
      credential reveal.
- [ ] **T62** Logs viewer: live tail, filter, search, pause-on-scroll.
- [ ] **T63** Domains & TLS screen: the diagnostic table, CA install/uninstall, per-site reissue.
- [ ] **T64** Elevation UX: `HelperRequired` → a dialog showing exactly what will change and why.
- [ ] **T65** Tray/menu-bar item: state, start/stop all, quick-open sites, sharing indicator.
- [ ] **T66** Settings screen + `doctor_repair` surface; "copy diagnostics" on every error.
- [ ] **T67** GUI cold-start benchmark (< 1.5 s) and Playwright E2E for create-site → open.

**Milestone M6** — a user installs, creates a Laravel-shaped site with HTTPS, and never opens a
terminal.

---

## Phase 7 — Efficiency
*Goal: deliver the promise that idle costs nothing.*

- [ ] **T68** `ResourceLimits` per OS: Job Objects, cgroup v2, macOS QoS + watchdog; the GUI shows only
      what the platform really supports. **(P)**
- [ ] **T69** Idle detection (connections, request counters, query counters) and `IdlePolicy`
      shutdown, with per-project "keep warm".
- [ ] **T70** On-demand activation gateway: hold the socket, start the service, wait for ready, proxy
      the first request.
- [ ] **T71** Metrics history: 1 s sampling while subscribed, 24-hour downsampled retention.
- [ ] **T72** CI budgets: idle footprint < 60 MB RSS, cold path < 1.5 s — failing the build on
      regression.
- [ ] **T73** Dev-tuned defaults pass over every service template (buffer pools, memory limits).

**Milestone M7** — after 30 idle minutes only `mixengined` + the web server are running, and the next
request still succeeds within budget.

---

## Phase 8 — Differentiators

- [ ] **T74** LAN sharing: per-site opt-in, rebind, firewall rule via the helper, LAN URL + QR code. **(P)**
- [ ] **T75** mDNS advertisement (`<slug>.mixengine.local`) and CA download endpoint for phones.
- [ ] **T76** Auto-revoke sharing on network change; sharing visible in the tray; the "web ports only"
      enforcement test.
- [ ] **T77** Blueprint manifest, `blueprint.capture`, and plan/`--dry-run` output.
- [ ] **T78** `blueprint.apply` execution with resumable idempotent actions and scoped rollback.
- [ ] **T79** Built-in blueprint gallery (Laravel, WordPress, Symfony, static, Next.js proxy, Django),
      doubling as end-to-end tests.
- [ ] **T80** Extension model: `extension.toml`, the four kinds, scoped tokens and permission
      enforcement.
- [ ] **T81** Extension registry client + install/uninstall/start/stop + GUI store screen.
- [ ] **T82** First extensions: Mailpit (with the `sendmail_path` recipe for every managed PHP),
      phpMyAdmin, Adminer.
- [ ] **T83** **MixDB integration**: detect installed MixDB, "Open in MixDB" on every database service,
      connection handoff with credentials read from the keyring at click time.
- [ ] **T84** MixDB as a `desktop-app` registry entry + a shared keyring naming convention.

**Milestone M8** — capture a project as a blueprint, apply it to a new one, open its database in
MixDB, and test it from a phone — all from the GUI.

---

## Phase 9 — Ship

- [ ] **T85** Installers: MSI + portable zip, notarised `.dmg`, AppImage/`.deb`/`.rpm`. **(P)**
- [ ] **T86** Code signing across all platforms; the daemon verifies the helper's signature before
      trusting it.
- [ ] **T87** Complete uninstall path + a clean-VM smoke test proving nothing is left behind.
- [ ] **T88** Auto-update with opt-in prompts (never silent while services are under load) and
      client/daemon protocol negotiation.
- [ ] **T89** Upgrade test: an old `mixengine.db` migrated by a new binary, in CI.
- [ ] **T90** User documentation site + in-app help; English and Vietnamese.
- [ ] **T91** Crash reporting that is opt-in and contains no project paths or credentials.
- [ ] **T92** Public beta: the packaging pipeline running for all runtimes across six OS/arch targets
      ([../operations/runtime-packaging.md](../operations/runtime-packaging.md)).

**Milestone M9 — v0.1.0.**

---

## Parked (revisit deliberately, do not start early)

- Optional Docker escape hatch for exotic services (see
  [ADR 0003](../decisions/0003-no-container-isolation.md)).
- Remote tunnels (Cloudflare/ngrok) as an extension.
- Team-shared blueprint registries.
- Editor extensions (VS Code / JetBrains) as additional API clients.
- Xdebug one-click profiles and a built-in profiler view.
