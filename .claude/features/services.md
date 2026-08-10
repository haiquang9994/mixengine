# Bundled services: web servers, databases, caches

**Goal**: working Caddy/Nginx, MariaDB/PostgreSQL, Redis and Memcached immediately after install,
with sane defaults and no hand-edited config files.

## Catalogue

| Service | Default version line | Default bind | Notes |
| --- | --- | --- | --- |
| Caddy | 2.x | `127.0.0.1:80/443` | **default web server** — see ADR 0004 |
| Nginx | 1.27 stable | `127.0.0.1:80/443` | alternative front end, one active at a time |
| php-fpm | one per installed PHP | unix socket / `127.0.0.1:9xxx` on Windows | created by runtime install |
| MariaDB | 11.4 LTS | `127.0.0.1:3306` | random root password in OS keyring |
| PostgreSQL | 16 | `127.0.0.1:5432` | initdb on first start |
| Redis | 7.x | `127.0.0.1:6379` | appendonly off by default (dev) |
| Memcached | 1.6 | `127.0.0.1:11211` | 64 MB default |

Multiple instances of the same service are supported (`mariadb@main`, `mariadb@legacy`) with
independent ports, data dirs and versions. Instance name is part of the `ServiceId`.

## Config generation

Every service's runtime config is **generated** into `etc/<service-id>/` from a template
(`minijinja`) plus the user's overrides stored in `services.config_overrides_json`:

```
etc/
  caddy/Caddyfile             ← global block + one imported file per site
  caddy/sites/blog.test.caddy
  nginx/nginx.conf + sites/
  php-fpm/8.3/pool.d/<site>.conf
  mariadb/main/my.cnf
  postgresql/main/postgresql.conf + pg_hba.conf
  redis/main/redis.conf
```

Rules:

- Users edit **overrides** (typed key/value, or a free-form "extra directives" blob per service),
  never the generated file. The GUI shows the rendered result read-only with a "reveal in folder"
  button.
- Regeneration is atomic and diffed: if the rendered output is byte-identical, skip the reload.
- Reload beats restart: Caddy `caddy reload`, Nginx `nginx -s reload`, php-fpm `SIGUSR2`. Only fall
  back to restart when the change requires it (port, user, data dir).
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
- Ports 80/443 are bound via the helper on Unix (socket passed back to the daemon-supervised child)
  and directly on Windows (no privileged-port restriction there); if a port is taken by another
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
