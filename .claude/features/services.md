# Bundled services: web servers, databases, caches

**Goal**: working Caddy/Nginx, MariaDB/MySQL/PostgreSQL, Redis and Memcached immediately after
install, with sane defaults and no hand-edited config files.

## Catalogue

| Service | Default version line | Default bind | Notes |
| --- | --- | --- | --- |
| Caddy | 2.x | `127.0.0.1:80/443` | **default web server** — see ADR 0004 |
| Nginx | 1.27 stable | `127.0.0.1:80/443` | alternative front end, one active at a time |
| php-fpm | one per installed PHP, named by the full version (`php-fpm@8.3.33`) | unix socket / `127.0.0.1:9xxx` on Windows | created by `runtime.install`, removed by `runtime.uninstall`, never by `service.create` |
| MariaDB | 11.4 LTS | `127.0.0.1:3306` | random root password in OS keyring |
| MySQL | 8.4 LTS | `127.0.0.1:3306` | **a different product from MariaDB**, not a version of it: its own package, its own recipe, its own rows. Only one of the two holds 3306; the other is given a port of its own |
| PostgreSQL | 16 | `127.0.0.1:5432` | initdb on first start |
| Redis | 7.x | `127.0.0.1:6379` | **a cache, and it keeps nothing**: `save ""`, `appendonly no`, and stopped with `SHUTDOWN NOSAVE` so it does not write one on the way out either |
| Memcached | 1.6 | `127.0.0.1:11211` | 64 MB default |

**"Default" is which one this project picks, not one that arrives by itself.** Nothing installs a
front end. `service.create` refuses a *second* one by `Role::FrontEnd`, and
[ADR 0004](../decisions/0004-caddy-as-default-web-server.md) settles which of the two that role
should be when there is a choice — but a home with neither has neither until somebody runs
`mix package install caddy`, or `nginx`, which is a first-class alternative and not a lesser one. A
first run that offers to do it for them is not built and has no task of its own yet.

Multiple instances of the same service are supported (`mariadb@main`, `mariadb@legacy`, and
`mysql@main` beside `mysql@legacy` on the same terms) with independent ports, data dirs and
versions. Instance name is part of the `ServiceId`, and the name after the `@` is the user's: it is
what tells two of them apart, and nothing in MixEngine knows the words `main` or `legacy`. It cannot
be changed afterwards — the id is also the generated config directory, the log directory, the socket
file and the keyring address — so renaming one is creating the other and deleting this one, which
keeps the data directory.

**A data directory belongs to one service.** Two servers over one set of files corrupt them, and the
cost lands on the data rather than on a start that fails, so `service.create` refuses a `data_dir`
another row already holds and names who holds it. Only an explicit `--data-dir` can reach that
refusal: the derived layout is `data/<package>/<instance>`, where two instances cannot collide. The
comparison resolves a relative path against the working directory and stops there — a symlink, a
bind mount, or one directory reached through two cases on a filesystem that ignores case are the
server's own lock file to catch, not MixEngine's.

## Ports, and who gets 3306

MariaDB and MySQL name the same default, and so do two instances of either — which is one problem
and not two. A port is **allocated once, when the row is written, and never computed again**:

- **The number in the table above is a recipe's preferred port, not a reservation.** Which port a
  service would like is a fact about the service, so it is declared by the recipe beside its binary
  and its template rather than decided in `service.create`. A caller naming a port explicitly is
  taken at its word and gets no allocation at all.
- **First created, first served.** The first database to ask for 3306 is given it; the next is given
  the first free port above — `mysql@main` beside `mariadb@main` lands on 3307 by the same rule
  that puts `mariadb@legacy` there, which is the point of writing one rule rather than a special
  case for two products. The daemon reports the port it chose, because a port a person did not pick
  is one they have to be told.
- **Free means free on the machine, not free in the table.** 3306 on a developer's machine is
  routinely held by an XAMPP, by Windows' own `MySQL80` service or by a published container, none of
  which has a `services` row. So the test is a bind and not a query, and a preferred port lost to a
  program MixEngine does not manage is reported with as much of that program's identity as the OS
  will give up (T38) rather than as a silent renumbering. The search is bounded — running out of ports is an error, not a longer loop.
- **An allocated port belongs to its row for as long as the row lives.** Deleting whoever holds 3306
  does not promote anybody into it: the port is in a project's `.env` and in a colleague's shell
  history, and a service that quietly moved would break both. Moving one is a person's decision and
  a regeneration — `mix service set`, which does not exist yet and is the same missing task the
  reload waits for.
- **Allocating and writing the row are one critical section**, or two concurrent `service.create`
  calls are each handed the same next-free port and the second server fails to bind at start.

## Config generation

Every service's runtime config is **generated** into `etc/<service-id>/` from a template
(`minijinja`) plus the user's overrides stored in `services.config_overrides_json` — one directory
per `ServiceId`, which is why an instance's name is in the directory rather than under it:

```
etc/
  caddy/Caddyfile                  ← global block + one imported file per site
  caddy/sites/blog.test.caddy
  nginx/nginx.conf + sites/
  php-fpm@8.3.33/php-fpm.conf      ← one pool per installed PHP, shared by every site on it
  mariadb@main/my.cnf
  mysql@main/my.cnf                ← a MySQL template, not MariaDB's
  postgres@main/postgresql.conf + pg_hba.conf + pg_ident.conf
  redis@main/redis.conf
```

**Memcached is not in that list, and never will be.** It has no configuration file format — not one
it declines to use, one that does not exist: every setting is a command-line flag, and what
distributions call `/etc/memcached.conf` is a list of flags their init script pastes onto the command
line. So its typed overrides become the process's arguments and it is the one service with no
`etc/<service-id>/` directory at all. Rendering a file nothing reads, so that this list looks
uniform, would put a document in front of the user that changes nothing when they edit it — which is
what the next rule exists to prevent.

**One pool per PHP version and not one per site**, which is a decision T32 made rather than a
simplification: a pool per site is Unix-only vocabulary, and Windows has one master with one set of
children and no `[pool]` sections at all. Choosing it would have created exactly the split the rest
of this design avoids, in the layer Phase 4 builds on. **No `pool.d/` yet**: php-fpm reads a glob
whose directory is missing as a hard error rather than as a pattern matching nothing, and that
directory cannot exist for the first `php-fpm --test` — `include` names the installed path while
validation runs over the staged one. Phase 4 brings the directory and the `include` together.

What each service *is* — which binary, which templates, which overrides it understands, how to tell
it is up — is a **recipe** compiled into the daemon (`mixengine_core::generate::Recipe`), found by
`packages.name`. Two instances of one server are two rows against one recipe. The package index
publishes downloads and says nothing about any of this: a template changes with a MixEngine release,
not with a repackaged upstream.

Rules:

- Users edit **overrides** (typed key/value, or a free-form `extra` blob per service), never the
  generated file. The rendered result is readable back for display only, with its path, so a client
  can show it and reveal it in a folder.
- An override naming a setting the recipe does not have is **refused**, with the ones that exist in
  the message. A silently ignored key is a setting the user believes is in effect.
- Regeneration is atomic and diffed: if the rendered output is byte-identical, skip the reload.
- Reload beats restart: Caddy `caddy reload`, Nginx `nginx -s reload`, php-fpm `SIGUSR2`. Only fall
  back to restart when the change requires it (port, user, data dir). **Windows has no signal a
  daemon can send** (ADR 0008), so a pool there keeps its old configuration until somebody restarts
  it — the daemon says so in `daemon.log` rather than restarting a thing nobody asked it to restart.
- A generated config that fails validation (`caddy validate`, `nginx -t`, `postgres --check`) is
  **not** installed; the previous config stays live and the error is surfaced with the offending
  override highlighted.

## First-start initialisation

- **MariaDB**: two programs, in this order (T33). `mariadb-install-db` bootstraps
  `data/mariadb/<instance>` — a shell script on Unix and a different C++ program of the same name on
  Windows, sharing almost none of their options. It does **not** set the password: a second
  `mariadbd --bootstrap`, fed its SQL on standard input, writes the generated password into
  `mysql.global_priv`, drops the anonymous accounts and removes the test database. Bootstrap mode
  listens on no port and no socket, so there is no window in which a password-less root is
  reachable — and it implies `--skip-grant-tables`, which is why the row is written directly rather
  than with `SET PASSWORD`.
  The password is generated by the daemon and stored in the OS keyring **before** either program
  runs, so a machine with no credential store fails with nothing created. There is no fallback to a
  file: it would be a plaintext credential on disk, which is the thing this arrangement exists to
  avoid (ADR 0006).
- **MySQL**: the same job as MariaDB's and none of the same programs, which is why it is a recipe of
  its own (T34c). **Three bootstrap routes, chosen by version and platform** rather than by a version
  test: 5.7 and newer use `mysqld --initialize-insecure`; 5.6 on Unix uses `scripts/mysql_install_db`,
  which does not quote `$basedir` and so has to be reached through a path with no spaces; 5.6 on
  Windows has neither, and upstream's zip ships a `data/` directory with the system tables already
  built. The generated password goes on afterwards, out of the keyring, on the same terms as
  MariaDB's — stored before anything is created, no fallback to a file.
  **`--initialize-insecure` creates only `root@localhost`**, where MariaDB's installer also creates
  `root@127.0.0.1`: the `skip-name-resolve` in MariaDB's template would leave every client here
  refused by a server whose own log says it is ready for connections. Two `my.cnf` files that look
  alike are not one template.
- **PostgreSQL**: `initdb` with UTF-8 + the user's locale, `pg_hba.conf` trusting local connections
  only, create a superuser named after the OS user.
- **Redis/Memcached**: nothing at all — no bootstrap, no credential, no ritual. What they do need is
  a data directory that exists, which `Generator::render` creates for every service beside the log
  directory: Redis names its own `dir` in its own configuration and refuses the whole file when it is
  missing, and memcached, whose working directory it is, never reaches its first line.

Init runs inside a job with progress, and is idempotent — a half-finished data dir is detected and
cleaned rather than reused. **Two markers, and the second one is what keeps that sentence honest**:
one written before the first step and one after the last, so a directory carrying the first alone is
ours to clear and a directory with contents and *neither* is somebody else's database. The second is
refused and left exactly as it was. The started marker sits beside the data directory rather than
inside it, because Windows' `mariadb-install-db` refuses any datadir that is not empty.

## Web server integration

- Exactly one of Caddy/Nginx is the active front end (owns 80/443). Switching regenerates all site
  configs and hands the ports over.
- Sites map to a generated per-site config file; there is no shared file that all sites append to,
  so one site's configuration is a file somebody can read on its own. **The whole set is judged
  together, and a refusal installs nothing** — T43, D3. `SiteState` has two words on purpose, and a
  site carries no free text today: its domains are normalised, its doc root is refused if it resolves
  outside the project, its upstream is checked, and its pool must exist. A rendered site file the
  front end's own checker refuses is therefore a bug in this repository's template, not a mistake a
  user made — and skipping it would hide the bug while serving eleven sites out of twelve. So the
  front end goes on reading the configuration that worked and the error names the file the checker
  complained about. A `Degraded` site becomes worth having when a site can carry a snippet somebody
  wrote, which is the extension surface's ([extensions.md](extensions.md)) to introduce.
- The front end **answers** on 80 and 443 on every system, and **binds** 80 and 443 on Windows and
  Linux and 8080 and 8443 on macOS. Which of those a program must listen on is not a `#[cfg]`
  anywhere above the platform layer: it is what `Host::port_access().probe(…)` returns, one
  `PortBinding` per port, and **T43** is what renders it into a front end's configuration.
- Neither is bound by an elevated process. Windows reserves nothing below 1024; Linux is granted
  `cap_net_bind_service` on the front end's binary; macOS gets a packet-filter redirect plus the
  boot-time job that enables pf ([ADR 0012](../decisions/0012-a-boot-time-job-enables-the-packet-filter-on-macos.md)). All of it is arranged by a
  one-time `PortAccessGrant` — see
  [../decisions/0005-on-demand-elevation.md](../decisions/0005-on-demand-elevation.md). If a port is taken by another
  program, report `port_in_use` **with the owning process name** — the platform layer's `PortOwner`
  is what answers that, and how much of an answer there is depends on the OS: the program's name
  where this account may read it, the pid alone where the OS refuses the name, and "another program
  on this machine" where a socket cannot be traced to a process at all. All three are better than
  the symptom, and none of them is the same answer as *nothing is listening*.

## Database management scope

MixEngine manages *lifecycle* (install, start, stop, config, credentials, data dir, backup/restore
snapshots of the data dir). Browsing and querying data is **out of scope** — that is
[MixDB](https://github.com/mixnz/mixdb), integrated as an extension
([extensions.md](extensions.md)).

**Making a database is part of that lifecycle, and "credentials" is what makes it one.**
`database.create` — `mix database create mariadb@main --name blog` — creates a database and an
account that reaches it on a running instance, generating the account's password and storing it in
the OS keyring at `<service-id>/<user>`. Nothing prints it or puts it on the wire: what a caller is
told is the address, and handing a credential to a program that needs one is T83's handoff.

Two rules make it repeatable and safe to run twice. **A keyring entry is the deed of ownership**: an
account already on the server that MixEngine holds no credential for is refused by name rather than
having its password reset, because "make sure this account exists" must not mean taking over
somebody else's. And the last statement **logs in as the account just made** and creates a table with
it, so the call cannot report a success the account cannot use — on PostgreSQL that account *owns*
its database, since `GRANT ALL ON DATABASE` has not carried `CREATE` on `public` since version 15.

There is no `database.drop`. Removing a database destroys data, and nothing has asked for it — see
[blueprints.md](blueprints.md) for what a blueprint rollback does instead.

## Acceptance criteria

- `mix service start caddy mariadb redis` → all three healthy in under 10 s, **warm**: installed,
  bootstrapped, and started at least once before. Measured by
  `crates/mixengine-cli/tests/warm_start.rs` in the `bench` job, on all three systems.

  **Warm and *fresh install* are two different runs**, and this line used to say both. A fresh
  install has an empty data directory, so its first start is MariaDB's first-run ritual —
  `mariadb-install-db` building a system schema, a generated root password reaching the credential
  store — which is tens of seconds of work by design. That number is measured and reported beside
  this one and is held to nothing: nobody has said what it should be, and a budget nobody argued
  for is a budget that gets turned off rather than met.
- Breaking an override produces a clear validation error and does **not** interrupt running traffic.
- Two MariaDB instances of different versions run simultaneously with separate data dirs.
- MariaDB and MySQL run side by side, each bootstrapped by its own programs, neither reading the
  other's generated `my.cnf`.
