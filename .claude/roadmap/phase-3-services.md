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
      T28 has its road, but a pool's file and a runtime's ini set have different owners. *No site and no
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
      [`mixengine-packages`](https://github.com/haiquang9994/mixengine-packages). **Nothing in this
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
      **Deliberately not done**, each with a task of its own: a second instance (T36),
      `mariadb-upgrade` for a directory bootstrapped by an older series, backup and restore, and a
      non-root application user. There is no reload, and there cannot be — MariaDB reads its
      configuration once, at startup.
- [ ] **T34** PostgreSQL: `initdb`, `pg_hba` local-only, superuser creation. Packaged already —
      every service kind this phase names is published to the index.
- [ ] **T35** Redis + Memcached with dev-tuned config. Packaged already, as T34.
- [ ] **T36** Multiple instances of one service (`mariadb@main`, `mariadb@legacy`) with independent
      ports and data dirs.
- [ ] **T37** Nginx as the alternative front end; parity test suite running both generators.
      Packaged already, as T34 — what is missing is the recipe, not the artifact.
- [ ] **T38** Port conflict diagnosis: report the owning process name, not just `EADDRINUSE`. **(P)**

**Milestone M3** — `mix service start caddy mariadb redis` → all healthy in under 10 s warm.

---

Previous: [Phase 2 — Runtimes](phase-2-runtimes.md) · Next: [Phase 4 — Sites, domains and on-demand elevation](phase-4-sites-and-elevation.md)
