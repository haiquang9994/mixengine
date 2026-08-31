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
      [`mixengine-packages`](https://github.com/mixnz/mixengine-packages). **Nothing in this
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
      by opinion. MariaDB and PostgreSQL do branch and carry `eol` entries; both are packed (T33a,
      and the same workflow that packs the rest).
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
      difference between "MixEngine can run Caddy" and "a user can ask it to". *(Both landed in
      T31a, and this suite now installs its Caddy through `package.install`.)* **Orphan removal** is
      still open and now has its shape: a file under `etc/<id>/` that the recipe no longer renders is
      left alone, which is harmless while the set is one file and is exactly wrong for `sites/` — a
      deleted site whose import file survives is a site that goes on being served. It belongs to T43,
      with the site files that make it possible to get right.
- [x] **T31a** Install a service package, and create a service: `package.install|uninstall|list`
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
      **Six methods rather than the four named above**, and each of the two extra ones is a
      decision. `package.list_available` is separate from `package.list` on `runtime_api.rs`' stated
      reasoning — what is knowable about something installed and about something merely offered is
      different, and one type carrying both would have half its fields meaningless in half its
      answers. `service.delete` is the other, and it was not optional: `services.package_id` is
      `ON DELETE RESTRICT`, so without it a `package.uninstall` could be refused with no way to reach
      the state where it is allowed.
      **What the task settled, beyond writing the methods.** *Only packages this build has a recipe
      for are offered or installed* — an index entry MixEngine cannot configure is a download ending
      in a directory nothing can start, so the refusal is at install time rather than at create time,
      where the disk is already spent. Two MixEngine versions reading one index therefore answer
      differently, which is correct: the question is *what can I run*. *A service's package is its
      id* — `ServiceId::name()` already documented itself as "the package this is an instance of", so
      `service.create` takes the id and a version and derives the rest; a second parameter would have
      been either redundant or a pair to police. *A recipe declares its instancing*, which is the
      half of T36 that could not wait: `Recipe::instancing` has no default body, Caddy answers
      `Single` and is refused an `@`, and the generator's data fallback is `data/<package>` for a
      singleton rather than `data/caddy/caddy`. Running two of them side by side is still T36's.
      *A delete never touches data* — the row and `etc/<id>/` go, the data directory is named in the
      answer and left, because generated config can be rendered again and somebody's databases
      cannot.
      **Two things landed that the task did not name.** A recipe now also says how to *prove* an
      install runs (`Recipe::smoke_test`, `caddy version` — a subcommand and not a flag, since
      `caddy --version` exits non-zero), which is T20a's finding applied to servers. And
      `packages` gained `size_bytes` in `0003_package_size.sql`: the table was described in `0001`
      ahead of its first writer, and the one column nobody had a value for was the one left out.
      **Left undone, deliberately.** T36 proper — two instances of one package with independent ports
      and data directories, and whatever port allocation that needs. There is no `service.configure`:
      changing an override is still a row edit, which the test suites do directly and a user cannot
      yet do at all. An uninstall purges nothing — `data/` and `logs/services/<id>/` survive every
      delete, and nothing offers to remove them. And nothing notices a `packages` directory with no
      row, or a data directory with no service.
      **`mixengine_testkit::declare` is half retired**, which was the promise: what it still writes is
      the `packages` row for `fakeservice`, because a fixture binary is not something any index will
      publish. Every `services` row in every suite now comes from `service.create` over a real socket,
      so the row supervision is tested against is the row the shipped method writes. `caddy.rs` goes
      further and installs the CI-fetched Caddy through `package.install` from a signed index it packs
      itself, which covers the whole install path against a real artifact on all three systems.
- [x] **T32** php-fpm pools: one service per PHP version, socket/port per pool, `SIGUSR2` reload.
      **The first service whose binary does not come from a package.** A PHP is installed with
      `runtime.install` into `runtime_installs`, and the process that serves its sites lives inside
      that directory — so `services` grew a second, typed parent (`runtime_install_id`, with a
      `CHECK` that exactly one of the two is set) rather than a fake `packages` row, which would have
      been a second table describing one directory with an `install_path` that goes stale the moment
      the runtime is removed. It is also what the other half of
      [T28](phase-2-runtimes.md) has been waiting for, and it is the first refusal
      `runtime.uninstall` has ever been able to make.
      **What was measured rather than assumed.** The whole shape of the task turned on what
      `php-cgi.exe` actually is, because **php-fpm does not exist on Windows** — every PHP the index
      publishes, 7.0 to 8.5, ships `php` + `php-fpm` on Linux and macOS and `php` + `php-cgi` on
      Windows. Against the artifact this project publishes: two concurrent requests with no
      `PHP_FCGI_CHILDREN` take 6.2 s through one pid, and 3.0 s through two with `CHILDREN=4`; a
      killed child is replaced in under a second; terminating the master takes every child with it;
      `PHP_FCGI_MAX_REQUESTS=2` recycles after exactly two. **Windows `php-cgi` is a process
      manager** — php-fpm with `pm = static`, configured through the environment instead of a file —
      so this task writes no supervisor of its own. One would also have made the systems *less*
      alike: Windows running *MixEngine-PM + N php-cgi* against Unix's *php-fpm*.
      **What the task settled.** *One recipe, two spec shapes, no `#[cfg]`* — `Recipe::spec` branches
      on `cfg!(windows)`, a value rather than an attribute, so both arms compile everywhere and a
      unit test exercises the branch the machine is not; which binary it is comes out of the
      artifact's own `provides` map (`Context::provided`), so the index decides and no recipe writes
      a path down. *One set of overrides on every system* — `max_children`, `max_requests`,
      `request_timeout`, `ready_timeout_ms`, `stop_grace_ms` — rendered into a file or an environment
      as the platform requires, and deliberately **no `pm = dynamic`**, which Windows cannot express.
      *`ReloadBehaviour::Signal`*, with `mixengine-platform` gaining `CAN_SIGNAL` and
      `Supervised::signal` — addressed at the *leader* where a stop is addressed at the group, and
      answering `unsupported` on Windows exactly as `ask_to_stop` does. *One pool per PHP version,
      shared by every site*, because a pool per site is Unix-only vocabulary. *Nobody calls
      `service.create` for it*: an idempotent hook (`services::pools::ensure`) runs after every
      install **and at boot**, which gives a PHP installed by an earlier build its pool with no data
      migration and repairs a home whose row was deleted by hand.
      **Judged against a real PHP**, on all three systems, through the FastCGI protocol — because a
      pool that is listening and cannot execute anything accepts a connection exactly like one that
      works. `mixengine-testkit` gained a minimal responder client for it, and
      `crates/mixengine-cli/tests/php_fpm.rs` installs a real artifact through `runtime.install`,
      starts the pool the hook created, reads a body back, changes an override, and asserts what each
      system actually does about it: Unix serves the new configuration from the same pid, Windows
      says in `daemon.log` that it cannot and keeps the old one.
      **Left undone, deliberately.** *No `request_terminate_timeout` on Windows* — a hung script holds
      a worker there forever, and with five of them that is a dead PHP; the fix needs no process
      manager, only a measurement of how a hung script behaves there, and that is its own task. *No
      `php.ini` and no `conf.d`* — `PHP_INI_SCAN_DIR` was measured to work on all three systems, so
      T28 has its road, but a pool's file and a runtime's ini set have different owners — and T28 took it:
      the pool now *names* that set with `PHP_INI_SCAN_DIR` without rendering any of it. *No site and no
      `pool.d/`* — php-fpm reads a glob whose directory is missing as a hard error, and that
      directory cannot exist for the first `--test`, so Phase 4 brings the two together. *No `pm.status_path` and no slowlog*, neither of
      which exists on Windows. *No `--force` on `runtime.uninstall`*, which now finally has something
      to force past. *Orphan removal under `etc/<id>/`* is still T43's, as T31 left it. And one
      thing found here rather than caused here: on macOS `kill(-pgid, ...)` answers **`EPERM`** when
      every member of the group is already a zombie, and `unix/process.rs` forgives only `ESRCH` —
      so a stop that arrives after the last process exited is reported there as a stop that failed.
      No test reaches it (a group with one live worker in it answers normally, which is what
      `stopping_a_service_whose_leader_has_died_still_reaches_its_workers` covers) and the runner
      does not either, so it is written down rather than fixed blind. A second finding of the same
      kind — a Windows CI run of this branch serving a log tail out of the file because the daemon's
      ring was still empty — belongs to T16b's path and is written up as
      [T16c](phase-1-process-supervision.md).
- [x] **T33a** Publish `mariadb` to the package index, in
      [`mixengine-packages`](https://github.com/mixnz/mixengine-packages). **Nothing in this
      repository changes**, and it is here for the reason T30a is: T33 is a recipe judged against a
      real server, and one has to exist first. T30a was the cheap version of this task; **this is the
      expensive one, and the reason is that the runtime table's MariaDB row was wrong.**
      Asked rather than assumed — the catalogue, across every release from 10.2 to 13.1 — upstream
      publishes a binary for **two** of the six cells. There has never been a macOS build of MariaDB,
      on either architecture, and there is no ARM64 archive of any kind. So the kind takes three
      recipes: `mariadb.py` borrows the Windows zip and the Linux bintar; `mariadb_deb.py` assembles
      Linux ARM64 out of upstream's own `arm64` `.deb` packages, rearranged into the layout upstream's
      bintar already uses; and `mariadb_build.py` compiles macOS on both architectures and Windows on
      ARM64 from the source release. One workflow runs all three across six legs, and takes a *list*
      of series — `all` covers the catalogue — because MariaDB maintains four at once with
      end-of-life dates years apart. The evaluation is written up in
      [`runtime-packaging.md`](../operations/runtime-packaging.md).
      **What lands on T33 directly, and none of it is in any documentation the row linked.** All
      thirty cells are green — five series across six targets — and published. The two Windows cells
      of 11.8 passed on the first run, including the compiled ARM64 one; the rest took seven rounds,
      and running the whole catalogue afterwards found four more that one series had hidden. Almost
      none of it was about compiling: the build would succeed and then the artifact could not be made
      to *be a database*.
      `mariadb-install-db` is a **different program on Windows** — C++, not the Unix shell script,
      sharing almost none of its options — so the random root password below is created by a
      different mechanism per platform rather than by one command with a flag. On Unix that script
      needs three things stated that a supervisor would not think to state: **`--no-defaults`**, or
      it reads the user's own `/etc/mysql/my.cnf` and can be pointed at somebody else's datadir,
      socket and port; **`--user`**, or it tries to hand the data directory to a `mysql` account
      MixEngine has not created; and **paths without spaces**, because `$basedir` and `$datadir` are
      both unquoted inside it. The last of those is a real constraint on where the daemon may put a
      data directory, or a reason to bootstrap with `mariadbd --bootstrap` instead.
      Two more for the generated `my.cnf`. Windows `mariadbd` writes its error log to
      `<datadir>/<hostname>.err` and sends nothing to stdout, so **`log_error` must be stated** or
      the supervisor cannot say why a service failed. And a **socket path is capped at 103
      characters** by `sockaddr_un` — the server aborts *after* InnoDB has started, which reads like
      a storage failure — so the socket cannot simply live beside a deeply nested data directory.
      Finally, a *borrowed* MariaDB is not self-contained the way Caddy is: it names the build
      machine's OpenSSL, libaio, libnuma and libsystemd by soname, and its plugin directory carries
      features linked against libraries a user will not have (`cracklib`, `libJudy`). The artifacts
      therefore bundle their libraries, drop the plugins that cannot resolve, and say so in
      `upstream.added` and `upstream.removed`.
- [x] **T33** MariaDB: install, `mariadb-install-db` first-run job, random root password in the OS
      keyring, secure defaults, dev-tuned `my.cnf`. **(P)**
      **The first recipe that has to create something before it can run**, and two pieces of
      machinery the design assumed were already here turned out not to be.
      **`ReadyCheck::Command` did not exist.** `HealthProbe::Command` did; readiness had five
      variants and none of them ran a program. A database needs it and a TCP check cannot stand in:
      an accept proves the listener is up and stays true for the whole of InnoDB's crash recovery,
      while the server refuses every query — so a supervisor watching the port reports the service
      ready and hands the next caller a connection refusal. The supervisor runs it in the service's
      own `Surroundings`, which is what lets the probe authenticate.
      **`packages` gained a `provides` map** (migration 0004). Every package until now published one
      server named after itself, so `Context::program` — the install path joined to the package name
      — could find it. MariaDB publishes seven commands under `bin/` and `scripts/`, and upstream
      renamed every one of them between 10.4 and 10.6. The index has carried the map since T20 and
      `runtime_installs` has recorded it since T25; `packages` was throwing it away.
      **`Recipe::ritual` is the hook T34 reuses.** A `Ritual` bundles the credentials the recipe
      declares with the function that builds the steps, so a recipe cannot ask for a secret and have
      no ritual, or have one nobody asks for. The recipe *declares* the credential and the daemon
      *generates and stores* it — no recipe reaches a keyring, and `mixengine-core` still has no
      platform call. The daemon stores it **before** it touches the disk, so a machine with no
      credential store fails with nothing created rather than half-way through, leaving a data
      directory whose root password exists nowhere.
      **Two markers, and the pair is what makes cleaning safe.** `services.md` says a half-finished
      data directory is cleaned; `DataDirectory::Foreign` is what keeps that from also meaning
      *MixEngine deletes a database it did not create*. Only a directory carrying our own
      in-progress evidence is ever cleared.
      **Four things were settled by running it, not by reading about it**, and each cost a failing
      run:
      `mariadbd --bootstrap` **does** read SQL from stdin on Windows, which nothing in
      `mixengine-packages` had tried — so there is no window in which a password-less root listens.
      `SET PASSWORD` **does not work there**: bootstrap mode implies `--skip-grant-tables` and
      answers `ERROR 1290`. The password is written to `mysql.global_priv` directly, with
      `JSON_SET(..., PASSWORD(...))`, which is what upstream's own installer does in that mode.
      **Every `root` row, not `root@localhost`.** The configuration says `skip-name-resolve`, so a
      client on TCP to 127.0.0.1 is matched as `root@127.0.0.1`; `mariadb-install-db` creates four
      root rows and the password goes on all of them.
      **`--no-defaults` before `--defaults-file` means the file is never read.** MariaDB honours
      whichever comes first, so the pair the spec was first written with left the server looking for
      its data directory beside its own binary — it crash-looped six times before the supervisor gave
      up. `--defaults-file` alone already means *read this and no other*.
      **And the started marker lives beside the data directory rather than inside it**, because
      Windows' `mariadb-install-db` refuses any datadir that is not empty.
      Two more came out of the first CI run, and neither is about the database.
      **A macOS keychain item belongs to the process that created it.** The daemon reads its own
      credential without a word; the *suite* asking for it raises a dialog, and on a runner nobody
      answers it — twenty-seven minutes of a job spent inside one read, after a bootstrap that had
      finished in three seconds. The suite no longer reads it, and loses nothing: this service's
      ready check is an authenticated `mariadb-admin ping` whose password the daemon resolves out of
      the keyring at spawn, so `running` already says the store holds a credential that works.
      Everything else it asks the server is a connection that must be **refused**, which needs none.
      **And Linux CI never had a credential store at all.** `gnome-keyring-daemon --unlock` with an
      empty password owns `org.freedesktop.secrets` and a `session` collection, with the `default`
      alias pointing at nothing: every store fails, which reaches `secrets.rs` as
      `UnsupportedPlatform` and is *skipped*. Eight credential tests had been green on a leg holding
      no credentials. One non-empty password creates `login` and the alias resolves.
      **Deliberately not done**, each with a task of its own: a second instance (T36),
      `mariadb-upgrade` for a directory bootstrapped by an older series, backup and restore, and a
      non-root application user. There is no reload, and there cannot be — MariaDB reads its
      configuration once, at startup.
- [x] **T34a** A supervised child never inherits Administrators. `postgres` calls `check_root()`
      before it dispatches a mode and refuses a token holding an enabled `BUILTIN\Administrators`;
      this repository's Windows CI leg holds one on purpose (T2b). So every child MixEngine starts to
      run a user's software — supervised and one-shot alike — is created from a restricted copy of
      the daemon's own token, through `CreateProcessAsUserW`. A no-op on an ordinary machine, where
      the interactive token is already filtered, and no elevation: that call needs no privilege for a
      restricted copy of the caller's own token.
      `.claude/decisions/0010-supervised-child-never-inherits-administrators.md`. What it cost
      outside the platform crate is two enum variants — `Supervised`'s streams are now `OutputPipe`.
      **Not** `spawn_detached` and **not** the shim; see the ADR for why. Read from upstream rather
      than assumed: exactly `--describe-config` and a leading `-C var` bypass that check, so
      `postgres --single` is refused on the same terms as the server, which is why the one-shot path
      is de-elevated too. **A restricted token also has to keep granting its own user**: measured on
      an elevated machine, a restricted child was created and then died at `0xC0000142` before its
      first instruction while the same spawn from the unrestricted token ran. An elevated
      administrator's token has a *default* access control list naming `SYSTEM` and
      `BUILTIN\Administrators` and nothing else, so disabling that group leaves a child with no
      access to the objects it creates itself.
      `restricted::keep_what_a_child_creates_reachable` merges the user back in. The window station
      was the plausible candidate and was measured innocent.
- [x] **T34** PostgreSQL: `initdb`, `pg_hba` local-only, superuser creation. Three generated files
      under `etc/postgres@main/`, and the cluster's own `postgresql.conf` and `pg_hba.conf` are
      never read — the server is started with `--config-file`, which is what lets generated
      configuration stay disposable while the data directory stays sacred. No `trust` on any line,
      on any platform. The superuser password is set through `postgres --single`, which listens on
      nothing, rather than through `initdb --pwfile`, which is a plaintext credential on disk for
      the length of a bootstrap. `--locale` and `--encoding` are stated because `initdb` otherwise
      reads the machine's and **exits zero** having quietly set the text-search default to `simple`.
      Readiness is an authenticated `psql -tAc "SELECT 1"` and health is `pg_isready`, which are two
      different questions — the second would pass for a cluster whose password never got set.
      **Three things were measured that the design had assumed.** `initdb` refuses
      `--auth-*=scram-sha-256` unless it is also given a password, which is the `--pwfile` this
      ritual exists to avoid, so it is asked for `--auth-*=reject` instead — stricter than the
      `trust` it would otherwise default to, in a file nothing reads. `postgres --single` **exits 0
      even when the statement it was fed failed**, so nothing may read its exit code as proof; the
      authenticated ready check is that proof. And `psql` prompts for a password on a terminal when
      it has none, so every probe that expects a refusal passes `--no-password` or hangs.
      **`pg_ctl reload` is in the spec and is the first real reload in this catalogue on all three
      systems** — MariaDB has none and php-fpm's is a signal Windows answers `unsupported` — but
      nothing can ask for one yet: there is no `service.reload` and no `mix service set`, so a
      running server cannot be handed a changed file. The behaviour is asserted where it is written
      and the end-to-end claim waits for the task that gives a service a way to be reconfigured.
      **Deliberately not done**, each with a task of its own: `pg_upgrade`, a second instance (T36),
      an application role that is not the superuser, extensions, backup and restore. No
      Windows-on-ARM cell — upstream does not compile there before 19.
- [x] **T34b** Publish `mysql` to the package index, in
      [`mixengine-packages`](https://github.com/mixnz/mixengine-packages). **Nothing in this
      repository changes**, and it is here for the reason T33a is: T34c is a recipe judged against a
      real server, and one has to exist first. **MySQL is not a MariaDB version** — it is a second
      product with the same words in its programs, and anybody maintaining an application against one
      of them can say which. Five lines are packed, 5.6 through 9.7, and the shape of the table is
      upstream's rather than a preference: 8.0 and newer are borrowed on the five cells Oracle still
      builds, while **every Unix cell of 5.6 and 5.7 is compiled**, because Oracle withdrew macOS
      from those lines *while they were alive* and never built ARM for either — so the newest patch of
      a line is less portable than one from the middle of it. There is no Windows-on-ARM cell in any
      line. What that cost is written up in
      [`docs/packages/mysql.md`](https://github.com/mixnz/mixengine-packages/blob/master/docs/packages/mysql.md),
      and the parts T34c has to know are three: **`provides` shrinks with newer versions**
      (`mysql_install_db` is 5.6 alone, `mysqlpump` and `mysql_upgrade` are gone at 8.4), which the
      `provides` map T33 added to `packages` already carries; **the 5.6 cells load an OpenSSL 1.1.1
      the recipe builds itself**, because 5.6's own `cmake/ssl.cmake` accepts no other major version,
      and each manifest names the library that artifact loads; and **there are no end-of-life dates**,
      because Oracle publishes the schedule in a support-policy PDF that `tools/eol.py` cannot
      re-read — 5.6 went out of support in February 2021, 5.7 in October 2023, 8.0 in April 2026.
- [x] **T34c** MySQL: install, first-run bootstrap, random root password in the OS keyring, secure
      defaults, dev-tuned `my.cnf`. **(P)**
      Most of the machinery was T33's and was reused rather than rebuilt: `Recipe::ritual`,
      `ReadyCheck::Command` as an authenticated `mysqladmin ping`, `StopBehaviour::Command` as
      `mysqladmin shutdown`, and the two markers that let a half-finished data directory be cleaned
      without ever clearing a database MixEngine did not create. No reload here either. **The
      bootstrap is a table of three routes and not a version test**, and the route is an argument
      rather than a `cfg!` — so all three are exercised wherever the tests run, where two of them
      would otherwise be unreachable on any one machine. `mysqld --initialize-insecure` from 5.7 on;
      `mysql_install_db` for 5.6 on Unix, reached through the space-free view that **moved up out of
      `mariadb.rs` into `recipes.rs`** because that script is the ancestor of MariaDB's and leaves
      `$basedir` unquoted in the same places, and run by the interpreter its own first line names —
      it is Perl in a tree compiled by `mixengine-packages`; and 5.6 on Windows copying the `data/`
      directory upstream's zip ships built, with `xcopy` run directly rather than through `cmd.exe`,
      which reads a program's standard input as commands. **What the task did not expect to add is
      `Step::secret_file`.** MySQL removed `--bootstrap` at 5.7.6, so the statement that sets the
      root password cannot travel on standard input the way MariaDB's does; `--init-file` takes a
      *path*. The three ways to get a generated password into that server are a file, an argument
      list every process on the machine can read, or a temporary server on a port anybody can
      connect to — so a step may now declare a file, the daemon writes it inside owner-only `run/`
      and removes it whether the step succeeded, failed or timed out, and its content never reaches
      a `Debug` line. The server that reads it is started with `--skip-networking` and stops itself
      with `SHUTDOWN`, which is the window MariaDB's `--bootstrap` closes by other means. **Two
      measurements changed the template.** `--initialize-insecure` creates only `root@localhost`,
      where MariaDB's installer creates four root rows including `root@127.0.0.1` — so
      `skip-name-resolve` could not travel from one `my.cnf` to the other, and the suite's refusal
      assertion is what proves the lookup is still on. And a modern MySQL opens a **second listener
      nobody asked for**, the X Protocol on 33060, which no allocation handed out and no `services`
      row records; `loose-mysqlx = OFF` closes it on 8.0 and newer and is a warning rather than a
      refusal on the two 5.x lines, which is one line instead of a version branch in a template.
      **The port allocation landed with it and is now every service's**, which is the half T36
      reuses: a recipe declares the port it prefers, `service.create` allocates one under a lock
      held across the insert, free means free on the *machine* (the test is a bind, and a preferred
      port lost to an unmanaged program is reported with that program's name — T38), and the number
      is written once and never recomputed. `services::pools::free_port` went into it: a pool asks
      for its 9000 by the same rule `mariadb@main` asks for 3306. `ServiceSummary` carries the port
      now, and `service.create` answers a `ServiceCreation` — the service, plus why it is not on the
      port it asked for. Judged against a real 8.4.10 end to end in
      `crates/mixengine-cli/tests/mysql.rs`, and against 5.6.51 by hand while the routes were
      written.

- [x] **T35** Redis + Memcached with dev-tuned config. Packaged already, as T34.
      Two recipes and one task, because neither is big enough to be one on its own — and between
      them they took the catalogue's two remaining assumptions away.
      **Memcached renders nothing, and that is the honest shape.** It has no configuration file
      format — not one it declines to use, one that does not exist: every setting is a command-line
      flag, and what distributions call `/etc/memcached.conf` is a list of flags an init script
      pastes onto the command line. So `files()` is empty, `etc/memcached@main/` is never created,
      and the typed overrides land in the spec's arguments. Rendering a file nothing reads, to keep
      the catalogue looking uniform, would put a document in front of the user that changes nothing
      when they edit it — which is the failure "users edit overrides, never the generated file"
      exists to prevent. It is also the one recipe with no validator worth having and no client to
      check itself with: `bin/memcached` is the whole archive, so the end-to-end suite speaks the
      text protocol over a socket, and a flag this build does not understand is caught by the start
      itself, which exits rather than ignoring it.
      **A TCP accept is honest here, and T33 is why that needs saying.** That task argued at length
      that an accept is a dishonest readiness check for a database, because it stays true for the
      whole of InnoDB's crash recovery while the server refuses every query. None of it applies to
      memcached: there is no recovery phase in which it is listening and unable to answer. Redis has
      a client, so Redis is asked — `redis-cli -p <port> ping`, with the port written out because a
      developer's machine routinely has a Redis of its own on 6379 and a ping without one would
      report this service ready against somebody else's server.
      **Redis's configuration is named relatively, and that is upstream's constraint rather than a
      Windows arm.** `getAbsolutePath()` in `server.c` decides a path is absolute with
      `relpath[0] == '/'` and otherwise joins it to `getcwd()`, so no Windows spelling survives being
      passed as an argument. `mixengine-packages` measured that on all five published cells and
      handed it over; the recipe answers with `redis.conf` beside a working directory of its own. A
      server that does not find its configuration does not fail — it starts on 6379 with its own
      defaults, which is why the suite runs on a port nothing else was listening on a moment before.
      **The cache keeps nothing, and it is proved rather than declared.** `save ""`, `appendonly no`
      and `SHUTDOWN NOSAVE` are three statements in three places; what makes them one behaviour is a
      key written, a restart, and the key being gone, which is what the suite asserts.
      **One thing landed outside these two recipes**, in `Generator::render`: the service's data
      directory is created before the render, beside the log directory that was already created
      there for php-fpm's reason. A server that names its own data directory in its own
      configuration does not create it — Redis refuses the whole file with `FATAL CONFIG FILE ERROR
      … No such file or directory`, and memcached, whose working directory it is, never reaches its
      first line. The two recipes before them hid it: a first-run ritual creates the directory it is
      about to bootstrap into, and Caddy makes its own storage. Both were found by running the
      suites rather than by reading, which is the whole argument for having them.
      **What is deliberately not here**: no reload for either. `CONFIG SET` changes a running Redis
      without touching the file, which is the opposite of what a reload means where the file is
      rendered from the database, and memcached re-reads nothing at all. `ReloadBehaviour`'s own
      documentation already said this about both.
- [x] **T36** Multiple instances of one service (`mariadb@main`, `mariadb@legacy`, and `mysql@*` on
      the same terms once T34c lands) with independent ports and data dirs.
      **Almost none of this task was new code, and that is the finding.** Every mechanism the claim
      rests on already keyed itself by service id rather than by package — the data directory, the
      socket, the log directory, the keyring address, the port a row is given — each decided in its
      own task for its own reason. What was missing was anything that ran two of them at once, so
      what this task owes is the suite: `crates/mixengine-cli/tests/instances.rs` installs **two
      versions** of MariaDB from one index, creates an instance over each, and proves both bootstrap
      their own directory, come up under their own credential, stop independently and do not
      bootstrap twice.
      **Two versions rather than two names**, and the choice is what makes the suite worth its
      minutes: 11.4.12 beside 10.6.28, the oldest line the index publishes, whose bootstrap programs
      upstream renamed wholesale and whose `share/` layout differs. Two instances of one version
      would share a `packages` row and prove only that two directories can have two names, which the
      unit tests already say. The marker each ritual leaves names the version that wrote it, and the
      two differ — an instance that had quietly reused the other's package is caught there and
      nowhere else.
      **The suite was made to fail before it was believed.** It passed on its first run, which
      proves nothing on its own; deriving the data directory from the package instead of the
      instance — the exact regression this task guards — turns it red, with the second server
      crash-looping over the first's files.
      **The one behaviour it added is a refusal.** Nothing stopped two rows naming one `data_dir`,
      and two servers over one set of InnoDB files is a cost paid in the user's data rather than in a
      start that fails. `service.create` now refuses it and names the holder, inside the lock the
      port allocation already holds, because two calls naming one directory both read a table
      neither has written to yet. **What it deliberately does not do** is ask the OS whether two
      paths are one directory: it resolves a relative path and stops, so a symlink, a bind mount or
      a case-insensitive collision goes through — being too lenient leaves the server's own lock
      file exactly where it was, while a `mixengine-platform` capability for "are these one file"
      would be a cross-OS question asked for one refusal nobody has been bitten by.
      **And it caught a live bug on its first CI run, which is the return on the whole task.**
      `first_run::patience_for` was meant to wait for the sum of a ritual's step deadlines plus a
      minute of slack. It built those steps with an empty credential map to measure them, and every
      recipe that has a credential refuses an empty one — so the measurement came back as no steps,
      and MariaDB's declared thirty minutes had been arriving at the daemon as sixty seconds since
      T33. One bootstrap fits in sixty seconds, which is why nothing noticed; two on a Windows
      runner do not, and the first was reported as a first run that never finished. `FirstRun` now
      measures itself against stand-ins of the length its own `SecretSpec`s declare — core's
      decision, because it is core that decides what a recipe accepts — and MySQL and PostgreSQL,
      which had it too, are covered by the same three-line change.
      **What is deliberately not here**: renaming an instance. The id is the config directory, the
      log directory, the socket and the keyring address, so a rename moves five things at once and
      is not a column update — it belongs with `mix service set`, which does not exist yet.
- [x] **T37** Nginx as the alternative front end; parity test suite running both generators.
      Packaged already — what was missing was the recipe, not the artifact.
      **The task's own finding is that "one front end" was a sentence nothing could break until
      now.** `.claude/features/services.md` has always said exactly one of Caddy and Nginx owns 80
      and 443, and until this task there was one front-end recipe, so the rule cost nothing to
      state. Two of them make it breakable in a way neither recipe can refuse on its own:
      `Instancing` is about a *package* — how many rows may name `nginx` — and both front ends
      answer `Single`, so a home obeying both recipes still ends up with a Caddy and an nginx
      rendered against the same ports, with the allocator helpfully offering the second one 81.
      **`Recipe::role`** is the vocabulary that closes it, and it is deliberately one distinction
      and not a taxonomy: `FrontEnd` or `Other`, defaulted to the second, so a recipe added later
      opts into an exclusivity rather than remembering to opt out. The refusal is in
      `service.create`, beside the two the recipe already drives, and the lookup is
      `core::services::front_end` — by role, so neither program is the one the code happens to know
      about. **Switching front ends is not here**: it means re-rendering every site into the other
      syntax, and there are no sites until T43.
      **nginx has no admin endpoint, so the recipe renders one.** Caddy answers both readiness and
      health on a control channel it ships; nginx ships none, and the obvious substitute is wrong
      rather than weaker — the master holds the listening socket, so a TCP accept succeeds in
      exactly the same way when every worker has died. What the template writes instead is a
      loopback `server` block answering `200` on `/mixengine/health`, which is a request a worker
      reading this configuration served. It is also the only port the spec declares for T38, because
      nothing here listens on the row's own port until sites arrive.
      **Three things were measured against nginx 1.31.3 rather than read about**, and each fails
      silently in its own way if it is guessed wrong. An `include` resolves against the **prefix**
      and not against the file it is written in, unlike Caddy's `import` — so `-p` is passed to the
      validator as `.`, with the staging directory as its working directory, and a broken staged
      site is what proves the whole rendering is judged where it is staged. `-s reload` reaches the
      master through the pid file *this configuration* names, which is why the pid goes to `run/`
      rather than to the compiled-in `logs/nginx.pid`. And the five temp directories nginx makes for
      itself are made with a single `mkdir` each, so they are children of the data directory —
      already created by `Generator::render` (T35) — rather than of a `temp/` nobody creates.
      **What the parity suite is, exactly.** The sequence a front end has to walk moved into
      `crates/mixengine-cli/tests/harness/frontend.rs` and is driven twice: a row becomes a
      configuration the server itself accepts, the server comes up, an edited override is *served*
      by the same process a moment later, a broken one is refused with the last good configuration
      still live, and a stop ends it. `caddy.rs` and `nginx.rs` are each four constants over it.
      Two copies of that arc would drift, and the copy that drifted would be green while it did.
      **It passed on its first run and was not believed until it was made to fail** (T36's rule):
      pointing the control check at a path nginx answers `404` on turns it red in a second, and
      replacing `-s reload` with `-s reopen` — a signal that reopens log files and re-reads nothing
      — turns it red at the reload, which is the failure `mixengine-packages`' own smoke test warned
      whoever wrote this recipe about. **The second mutation was green on the first attempt**, and
      the reason is worth keeping: `cargo test --test <suite>` does not rebuild `mixengined`, so a
      changed recipe reaches the suite and not the daemon that runs it.
      **And the Windows leg found a trap that was never nginx's alone.** Both tests failed there and
      nowhere else, with nginx reporting that it could not find a configuration file sitting on disk
      in front of it. nginx checks every file it opens for reading with `ngx_win32_check_filename`,
      which expands the name it was handed and **reports `ENOENT` when the expansion is not what it
      was given** — so a home reached through an 8.3 alias (`RUNNER~1` standing for `runneradmin`,
      which is how a runner's temporary directory is spelled) is a home whose every rendered file is
      missing. The fix is not in the recipe: `mixengine_platform::paths::in_full` spells a path the
      way the filesystem spells it, and both `resolve_root`s apply it, because everything — `etc/`,
      `data/`, `run/`, `packages/` — is joined onto that one answer and a second place to decide it
      would be two spellings of one home. `mix` and `mixengined` have to agree on it exactly: the
      endpoint is derived from it, and a client that skipped the rule would knock on a pipe no
      daemon is listening at. That is the whole of what the fixtures changed —
      `mixengine_testkit::Home` keeps its `TempDir` for the cleanup and takes every path it restates
      from the resolved spelling, while `mixengine-daemon/tests/api.rs` deliberately still hands
      `--home` over unresolved, so something in the suite is still watching the daemon resolve it.
- [x] **T38** Port conflict diagnosis: report the owning process name, not just `EADDRINUSE`. **(P)**
      `PortOwner` is the platform capability — one question, "who is listening on this TCP port",
      answered from `GetExtendedTcpTable` on Windows, `/proc/net/tcp[6]` plus a walk of
      `/proc/<pid>/fd` on Linux, and `lsof -t` with `ps -o comm=` on macOS, where the alternative was
      hand-declaring a `socket_fdinfo` layout that `libc` does not publish and that fails by reading
      garbage rather than by not compiling.
      **How much of the answer exists is per-OS, and `PortHolder` says so rather than averaging it.**
      Windows publishes the owning pid of every listener to anybody and refuses the *name* of a
      process belonging to another account; Linux maps a socket to a pid only through
      `/proc/<pid>/fd`, which the same refusal covers — so a listener owned by another user is a
      holder with **no pid at all**, and `ss -ltnp` is refused in exactly the same way. Both fields
      are optional and the three sentences are in `StateReason`'s `Display`, once, where every client
      reads the same one.
      **The producer is a start that already failed, never a check before one.** Asking first would
      be a race and would put an OS call in front of every start for the sake of the rare one that
      fails; what is here is a diagnosis, run on the two branches where a start ends badly, and
      **`ServiceSpec::ports` is what it is run against** — a `ReadyCheck` names an address only for
      the services proved up by connecting to one, and a database proved up by a query names none.
      Four recipes declare theirs; Caddy declares its admin endpoint **alone**, because `http_port`
      and `https_port` are in the global block and Caddy binds neither until a site asks it to (T43).
      **T34c's `mysql.rs` adds the same one line**, beside the port it renders.
      Two things it deliberately does not do. A diagnosis that cannot be made — an OS that will not
      answer, a join that failed — leaves the failure exactly as it was rather than replacing it with
      a failure to diagnose, which is asserted in `services::ports`. And a conflict replaces
      `StateReason::Exited` and never `CrashLoop`: the second carries its own count and the lines the
      service printed, which is more than a port could add. Windows's **excluded port ranges**
      (`netsh int ipv4 show excludedportrange`) look like this and are not — they are a refusal to
      bind with nobody on the port — and they stay where the roadmap already put them, in
      [T47](phase-4-sites-and-elevation.md).

**Milestone M3 — reached, on all three systems.**
`crates/mixengine-cli/tests/warm_start.rs` installs a real Caddy, MariaDB and Redis into one home,
times a single `mix service start`, and holds the **median** of five warm rounds to ten seconds —
reporting the first start, bootstrap included, beside it and gating nothing on that. In the `bench`
job, run 32637764489: **875 ms** on macOS, **2133 ms** on Windows, **3189 ms** on Linux, against
first starts of 3.1, 10.5 and 8.2 s.

Three things the measurement said that the milestone did not ask about.

**The promise was two runs in one sentence** — *fresh install* and *warm cache* — and
[../features/services.md](../features/services.md) now separates them. A fresh install's first start
*is* MariaDB's first-run ritual, tens of seconds by design, and it never had a budget anybody argued
for. Windows's 10.5 s is that number, and it is over the warm line by design rather than by fault.

**The median passes and the tail does not.** Two Linux rounds were over ten seconds — 11.8 s and
15.1 s, both in the first half of the run — beside three that were 1.2 to 3.2 s. The gate is the
median on purpose: a gate on the maximum would flap on a shared runner and say nothing about the
design. But a round that takes 15 s is a person waiting 15 s, so it is recorded rather than smoothed
away, and a round over the budget now prints the daemon's own account of itself.

**The tail is one service, and it is not the walker.** In that 11.8 s round Caddy reached `running`
in 53 ms and Redis in 256 ms; `mariadb@main` took **11.5 s** to answer its own ping, on a cold runner
two rounds after the bootstrap, and was under a second by the fourth round. A walk is sequential, so
the number is a sum — `crates/mixengine-daemon/src/services/mod.rs` has said since T18 that M3's
budget "buys concurrency by changing this walker and nothing else" — and it did not have to. On this
evidence it would not buy the tail either: the two fast services are 300 ms of the twelve seconds.
Cold I/O in MariaDB's own start is where anybody chasing it should look.

---

Previous: [Phase 2 — Runtimes](phase-2-runtimes.md) · Next: [Phase 4 — Sites, domains and on-demand elevation](phase-4-sites-and-elevation.md)
