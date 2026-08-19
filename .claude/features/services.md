# Bundled services: web servers, databases, caches

**Goal**: working Caddy/Nginx, MariaDB/PostgreSQL, Redis and Memcached immediately after install,
with sane defaults and no hand-edited config files.

## Catalogue

| Service | Default version line | Default bind | Notes |
| --- | --- | --- | --- |
| Caddy | 2.x | `127.0.0.1:80/443` | **default web server** — see ADR 0004 |
| Nginx | 1.27 stable | `127.0.0.1:80/443` | alternative front end, one active at a time |
| php-fpm | one per installed PHP, named by the full version (`php-fpm@8.3.33`) | unix socket / `127.0.0.1:9xxx` on Windows | created by `runtime.install`, removed by `runtime.uninstall`, never by `service.create` |
| MariaDB | 11.4 LTS | `127.0.0.1:3306` | random root password in OS keyring |
| PostgreSQL | 16 | `127.0.0.1:5432` | initdb on first start |
| Redis | 7.x | `127.0.0.1:6379` | appendonly off by default (dev) |
| Memcached | 1.6 | `127.0.0.1:11211` | 64 MB default |

Multiple instances of the same service are supported (`mariadb@main`, `mariadb@legacy`) with
independent ports, data dirs and versions. Instance name is part of the `ServiceId`.

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
  postgresql@main/postgresql.conf + pg_hba.conf
  redis@main/redis.conf
```

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
  generated file. The GUI shows the rendered result read-only with a "reveal in folder" button.
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

- **MariaDB**: `mariadb-install-db` into `data/mariadb/<instance>`, then set a random root password,
  store it in the keyring, drop anonymous users and the test DB.
- **PostgreSQL**: `initdb` with UTF-8 + the user's locale, `pg_hba.conf` trusting local connections
  only, create a superuser named after the OS user.
- **Redis/Memcached**: nothing, just config.

Init runs inside a job with progress, and is idempotent — a half-finished data dir is detected
(marker file) and cleaned rather than reused.

## Web server integration

- Exactly one of Caddy/Nginx is the active front end (owns 80/443). Switching regenerates all site
  configs and hands the ports over.
- Sites map to a generated per-site config file; there is no shared file that all sites append to,
  so one broken site cannot take down the others' config (it just fails validation and is skipped,
  with the site marked `Degraded`).
- Ports 80/443 are bound **without any elevated process**: directly on Windows, and on Unix via a
  one-time `PortAccessGrant` (pf redirect or `setcap`) arranged at first run — see
  [../decisions/0005-on-demand-elevation.md](../decisions/0005-on-demand-elevation.md). If a port is taken by another
  program, report `port_in_use` **with the owning process name** — `mix doctor` resolves PID→name via
  the platform layer.

## Database management scope

MixEngine manages *lifecycle* (install, start, stop, config, credentials, data dir, backup/restore
snapshots of the data dir). Browsing and querying data is **out of scope** — that is
[MixDB](https://github.com/haiquang9994/mixdb), integrated as an extension
([extensions.md](extensions.md)).

## Acceptance criteria

- Fresh install → `mix service start caddy mariadb redis` → all three healthy in under 10 s on a
  warm cache.
- Breaking an override produces a clear validation error and does **not** interrupt running traffic.
- Two MariaDB instances of different versions run simultaneously with separate data dirs.
