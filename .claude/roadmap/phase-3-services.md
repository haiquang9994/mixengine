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
- [x] **T31** Caddy integration: global Caddyfile + per-site imports, `caddy validate`, graceful
      reload, admin API health.
      **`Catalogue::builtin` is no longer empty**, and the recipe is what T30 said one would be: a
      template, the overrides worth having, a validator and a spec — about a hundred lines of
      `core::generate::recipes::caddy`, because everything around it was already there. The three
      mechanisms the task names all hang off the admin endpoint: `GET /config/` is both the
      `ReadyCheck::Http` and the `HealthProbe::Http`, `caddy stop --address` is the
      `StopBehaviour::Command`, and `caddy reload --address` is the new one below.
      **Graceful reload is the one thing that needed new vocabulary**, and it is
      `ReloadBehaviour` in `mixengine-proto` beside `StopBehaviour`, plus one edge from the registry
      into a runner. T30 left `Written` "reported and acted on by nobody"; this is the acting.
      `SpecSource` now answers `Generated` rather than a bare spec, because *what moved on disk*
      exists for one instant — by the time a caller holds the spec the file has been overwritten —
      and `Registry::graph` is the only place that fact meets the map of what is running. What it
      does with it is leave a permit on the runner's `Notify`, exactly as an explicit start does
      (T19c): the command then runs in the service's own `Surroundings`, so a reload is run where the
      service runs like every other command of its own, and `service.list` does not wait on a
      subprocess to answer. Two walks that both find a change while the runner is busy collapse into
      the one reload that was needed.
      **Reload versus restart is decided by what a config file can carry, and nothing here guesses.**
      A rendering that changed is handed over; a *spec* that changed — a different admin port, a
      different program — is not something a reload delivers, and the daemon does not restart a web
      server nobody asked it to restart. `mix doctor` owes the sentence (T47). A service the daemon
      **adopted** (T18) is not reloaded either: it is watched for exiting and nothing else, which is
      what adoption already costs.
      **Three findings, measured against Caddy 2.11.4 and written beside the lines they explain.**
      *Paths are backtick-quoted*: a Caddyfile token in double quotes processes `\"` and `\\`, so
      `C:\srv\caddy\` ends its string one character early — the failure is a parse error naming the
      wrong line, on one OS only. *`persist_config off`*, or Caddy writes the configuration it last
      loaded to the user's own config directory and reads it back next start, which is both a write
      outside `MIXENGINE_HOME` and a second source of truth for a file rendered from the database.
      *`caddy run`, not `caddy start`* — T30a's finding, now in a spec.
      **The admin endpoint is loopback whatever the row binds.** It loads arbitrary configuration
      into the running server, so it is a control channel; `bind_addr` is where *sites* are served
      and is what LAN sharing (T74) is about. `admin_port` is an override because a developer's own
      Caddy may hold 2019, and it is Caddy's default so that a `caddy` command typed by hand reaches
      the server MixEngine is running.
      **`import sites/*.caddy` matches nothing, deliberately, and is here rather than in Phase 4
      because of where it has to point.** The glob resolves against the directory holding the file it
      is written in, so a site file rendered anywhere but into this recipe's own set would be
      invisible to `caddy validate` and present at run time — the one arrangement that cannot be
      checked. Whoever renders the first site (T39, T43) renders it through here. `auto_https` is
      `off` and is a *setting* rather than a constant, so Phase 5 moves a default instead of editing
      a template.
      **It is judged against the real server, on all three systems.** `crates/mixengine-cli/tests/caddy.rs`
      is `#[ignore]`d and the `test` job fetches a pinned Caddy from `mixengine-packages`' own release
      to run it: a row becomes a Caddyfile, `caddy validate` judges it, the admin endpoint says when
      it is up, an override adding a site is *served by the same pid* a moment later, a broken one is
      refused with the good configuration still live and the site still answering, and `caddy stop`
      ends it. Ignored rather than skipped on purpose — a test that returns early when it finds no
      Caddy is a green suite that proved nothing.
      **Left undone, and none of it guessed at here.** Nothing installs a package yet: `paths.packages()`
      is still written to by nobody, and the test declares a row against a directory it unpacked
      itself — which is **T31a** below, along with `service.create`, since between them they are the
      difference between "MixEngine can run Caddy" and "a user can ask it to". **Orphan removal** is
      still open and now has its shape: a file under `etc/<id>/` that the recipe no longer renders is
      left alone, which is harmless while the set is one file and is exactly wrong for `sites/` — a
      deleted site whose import file survives is a site that goes on being served. It belongs to T43,
      with the site files that make it possible to get right.
- [ ] **T31a** Install a service package, and create a service: `package.install|uninstall|list`
      over the signed index into `paths.packages()`, and `service.create` over the row
      `mixengine_testkit::declare` writes by hand.
      **The two halves of "a user can ask for this"**, and they are one task because either alone is
      unreachable: a package with no `services` row is a directory, and a row with no package is a
      foreign key violation. T23 is the shape for the first — the job system's second producer, with
      `core::install` already taking an `&Artifact` and a destination — and the second is what
      [todo.md](todo.md) has been promising as the expiry date on `mixengine_testkit::declare`.
      Ordered after T31 rather than before it because T31 needed a *Caddy*, which a test can unpack
      for itself, and this needs a *design* for what a second instance of one package means
      (T36) — settling that against a real recipe is cheaper than settling it against none.
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
