# Phase 3 — Services

*Goal: web server, databases and caches run with generated config.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

**Precondition [T15a](phase-1-process-supervision.md) is met**, which is what these specs may now be
written against rather than around: Caddy's admin endpoint is a `ReadyCheck::Http` the supervisor
makes, MariaDB is health-checked by `mariadb-admin ping` rather than by a TCP accept that stays true
while the server refuses every query, and `mariadb-admin shutdown` is a `StopBehaviour::Command` that
is really run — over **plaintext HTTP only**, and with the service's own environment and working
directory, which is where a generated defaults file and a keyring credential reach it from.

---

- [x] **T30** Config generation engine: `minijinja` templates, typed overrides, atomic write,
      no-op-if-identical diffing, validation hook before install.
      **It is also the real `SpecSource`.** T19 left the daemon asking a port for the whole declared
      set of `ServiceSpec`s, with a fixture source behind its tests, precisely because turning a
      `services` row plus a package into a runnable spec is this task. Implementing it here is what
      makes the registry start something a user declared rather than something a test wrote.
      **The knowledge that turns a row into a service is compiled in, not published.** A row says
      *that* MariaDB 11.4 is installed as `mariadb@main` on port 3306; what MariaDB **is** — which
      binary, which template, how to tell it is up, what stops it — is a `Recipe` in
      `core::generate::recipe`. Not in the package index, which was the other candidate and would
      have made this task a change to `mixengine-packages`: the index describes a *download*, and a
      template that has to change with a MixEngine release cannot be published by a pipeline that
      runs on its own schedule and is consumed by clients of every older version. The lookup key is
      `packages.name`, so `mariadb@main` and `mariadb@legacy` are two rows, two data directories and
      two ports against **one** recipe — the whole difference between them is the `Context` it is
      handed.
      **`Catalogue::builtin()` is therefore empty, and that is the shape of the task rather than an
      omission.** Every real recipe is a template, a set of overrides worth having and a first-start
      ritual, each judged against the real server, and each is its own task below (T31–T35). What
      this one owns is everything around them, proved against recipes written in tests: the merge,
      the render, the diff, the staging, the validation and the spec that comes out.
      **`MIXENGINE_DEV_SPECS` is deleted**, as [phase 1](phase-1-process-supervision.md) said it
      would be, and what replaced it is *narrower and more honest*: a `fakeservice` recipe compiled
      into debug builds only (`daemon/src/services/fakeservice.rs`). The variable was a supervisor
      that ran whatever program a JSON file named, with whatever arguments; the recipe runs one
      program — the one inside the package a row points at — configured by the settings it declares.
      It is also the only version of this that tests the code under test: a spec read from a file
      exercised no part of generation, and a `fakeservice` row exercises all of it. `service.rs`,
      `lifecycle.rs` and `logs.rs` now declare rows and overrides the way a user will.
      **An override that names nothing is refused.** `config.toml`'s rule one directory down: a
      recipe declares its keys with typed defaults, and `{"prot": 3307}` is an error naming the keys
      that exist rather than a setting the user believes is in effect. One key belongs to no recipe
      and every service has it — `extra`, the free-form blob
      [services.md](../features/services.md) promises, because a config format this build models
      incompletely is the normal case and the alternative is somebody editing the generated file.
      **The whole set is staged, then judged, then renamed in.** Not file-by-file, because a
      validator has to be shown a *complete* configuration — a Caddyfile with two of its six site
      imports from before this render is not a thing anybody can have an opinion about. So the
      rendering goes into `etc/.<service-id>.staging/`, `caddy validate` (T31's, when it exists) is
      pointed at it from inside that directory, and each file is then renamed into place. The
      staging directory is removed on every path, including the failing ones.
      **A rendering identical to what is on disk is not written at all**, which is what makes
      "rendered on every walk" affordable: `Generator::declared` renders and installs at the top of
      every `service.*` call, so the configuration and the row cannot drift, and a home whose state
      has not changed does no writing and asks nothing to reload. The `Written` per file is reported
      and acted on by nobody yet — the reload it is for is T31 and T32.
      **One `services` row that cannot be generated fails the whole set**, deliberately: a service
      that quietly vanishes from `mix service list` is one somebody goes looking for in the wrong
      place. Which made the daemon's `Undeclarable::Unavailable` arm wrong the moment it became
      reachable — it classified everything as `internal`, on the grounds that T30 owned those
      failures and had not been written. It now downcasts to `mixengine_core::Error`, so a misspelled
      override is `invalid_argument` and a broken template of ours is still `internal`.
      **`ResourceLimits` gained `#[serde(default)]`** in `mixengine-proto`, which is where the row's
      `limits_json` is read into: serde asks for a field even when its type is `Option`, so the
      column's own default of `{}` — the state of every service nobody has capped — did not parse.
      Left for the tasks that need them, none of it guessed at here: **orphan removal** under
      `etc/<id>/` (a file the recipe no longer renders is left alone, which T31's per-site imports
      are what makes worth doing properly); **reload versus restart**, which needs a service that can
      reload; **`idle_minutes`**, read from no row into no `IdlePolicy` until T69; and **creating a
      data directory**, which is a first-start ritual (T33, T34) and not a side effect of writing a
      config file. `service.create` is still missing, so `mixengine_testkit::declare` still writes
      the rows — the second piece of scaffolding [todo.md](todo.md) names, and the one this task did
      not touch.
- [x] **T30a** Publish `caddy` to the package index, in
      [`mixengine-packages`](https://github.com/haiquang9994/mixengine-packages). **Nothing in this
      repository changed**, and the task is here rather than there because of what it unblocks: T30
      shipped an empty `Catalogue`, and each of T31–T35 is a recipe *judged against the real server*
      — which needs a real server to exist first. This is T20a's shape one layer along, and far
      cheaper: Caddy publishes one statically linked Go binary per target, so it is borrowed on all
      six and the recipe is the shortest in that repository.
      **What is not short is the proof, and that is the reusable part of this task.** A runtime is
      packed to be *executed* — `php -v` answering from a moved tree is the whole claim. A service is
      packed to be *run, configured, health-checked and stopped*, and each of those is a mechanism
      T31 depends on, so the smoke test does all four from a directory the archive was moved to:
      `caddy validate` on a rendered Caddyfile, `caddy run`, `GET /config/` on the admin endpoint, a
      request served, and `caddy stop` against that endpoint. An artifact that answers
      `caddy version` and cannot be health-checked is one T31 would find out about against a user's
      site. **T33–T35 should each cost that same test in their own terms** — `mariadb-admin ping`
      and `mariadb-admin shutdown` are the same two claims for MariaDB, and the precondition at the
      top of this phase is already written in those words.
      **`caddy run`, not `caddy start`**: `start` hands its child the parent's stdout and returns, so
      anything capturing that output waits for the *server* to exit. A hang rather than a failure,
      and worth knowing before T31 writes the `ServiceSpec` — `run` is what the supervisor execs.
      **Deliberately standard, not `xcaddy`.** A plugin set baked into an artifact cannot change
      without repacking six targets, and it would make a blueprint pinning Caddy 2.11.4 mean
      something no upstream release means. Nothing T31 or Phase 5 needs is outside the standard
      distribution: MixEngine issues from its own CA into the OS trust store rather than solving an
      ACME DNS challenge. If a plugin is ever genuinely needed it wants a `kind` of its own, not a
      quieter `caddy`.
      **Left for T31**, and none of it guessed at here: the index says a package *exists*, and
      nothing installs one. `Package::kind` is an open `String` — `caddy` needed no proto change —
      and `core::install::install` already takes an `&Artifact` and a destination rather than
      anything runtime-shaped, so what is missing is the call in front of it and the answer to where
      a service package lands. `paths.packages()` is that place, documented as "installed servers,
      databases and caches, one directory per `name/version`" and, at the time of writing, written
      to by nobody. **There is no `eol` entry for Caddy** and there should not be: upstream publishes
      no schedule, supports one line, and `mkindex.py` leaves a package undated rather than dating it
      by opinion. MariaDB and PostgreSQL do branch and will get entries when they are packed.
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
