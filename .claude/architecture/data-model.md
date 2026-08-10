# Data model

## State ownership

| Kind | Lives in | Written by | Rebuildable? |
| --- | --- | --- | --- |
| Declared state (projects, sites, versions, service settings) | `mixengine.db` (SQLite) | daemon only | no — this is the source of truth, back it up |
| Generated config (`etc/**`) | files | daemon | **yes**, always regenerate, never parse back |
| Service data (`data/**`) | files | the service itself | no — user data |
| Runtimes & packages (`runtimes/**`, `packages/**`) | files | installer | yes, re-downloadable |
| Secrets (DB root passwords, extension tokens) | OS keyring | daemon | no |
| User preferences | `config.toml` | user or GUI | — |

Rule: **if it can be regenerated, it is not state.** This keeps "reset my Nginx config" a one-liner
and makes upgrades safe.

## SQLite

Accessed via `sqlx` (compile-time-checked queries, same as MixDB), WAL mode, `foreign_keys=ON`.
Migrations are numbered SQL files in `crates/mixengine-daemon/migrations/` applied at boot; forward
only, never edited after release.

```sql
-- Runtimes -----------------------------------------------------------------
runtime_installs(id, kind, version, channel, install_path, installed_at, size_bytes,
                 source_url, sha256, is_default)
   -- kind: php | node | python | ruby ; UNIQUE(kind, version)

-- Packages (servers, databases, caches) ------------------------------------
packages(id, name, version, install_path, installed_at, source_url, sha256)
   -- name: caddy | nginx | mariadb | postgresql | redis | memcached | mailpit …

-- Service instances ---------------------------------------------------------
services(id, package_id, instance_name, state, autostart, port, bind_addr,
         data_dir, config_overrides_json, limits_json, idle_minutes,
         last_started_at, last_exit_code, pid, pid_start_time)
   -- id is the human-stable ServiceId, e.g. "mariadb@main", "php-fpm@8.3"

-- Projects & sites ----------------------------------------------------------
projects(id, name, root_path, runtime_pins_json, created_at, blueprint_id)
   -- runtime_pins_json: {"php":"8.3.12","node":"22.8.0"}
sites(id, project_id, primary_domain, doc_root, kind, php_service_id,
      https_enabled, http_port, https_port, config_json, state)
   -- kind: php-fpm | static | reverse-proxy | node-app
site_domains(id, site_id, domain, is_primary)
site_service_links(site_id, service_id)      -- which DBs/caches a site declares

-- TLS -----------------------------------------------------------------------
certificates(id, domain, sans_json, not_before, not_after, cert_path, key_path,
             issued_by_ca_fingerprint, revoked)
ca(id, fingerprint, cert_path, key_path, created_at, installed_in_trust_store)

-- Blueprints & extensions ----------------------------------------------------
blueprints(id, name, description, manifest_toml, created_at, source)
extensions(id, name, version, manifest_toml, install_path, state, settings_json)

-- Operations ----------------------------------------------------------------
jobs(id, kind, state, percent, message, started_at, finished_at, result_json)
events(id, ts, kind, subject, payload_json)  -- ring-trimmed audit trail, 30 days
settings(key, value_json)
```

Indexes: `sites(primary_domain)` unique, `site_domains(domain)` unique,
`runtime_installs(kind, is_default)` partial-unique, `events(ts)`.

## Project manifest (`mixengine.toml`, in the user's repo)

Optional, checked into the user's project, and the reason `mix` can be used without the GUI:

```toml
[project]
name = "blog"

[runtimes]
php = "8.3"          # range or exact; resolved against installed versions
node = "22"

[site]
domain = "blog.test"
doc_root = "public"
kind = "php-fpm"
https = true

[[services]]
name = "mariadb"
version = "11.4"
database = "blog"

[[services]]
name = "redis"
```

Resolution order for "which PHP is this?": explicit CLI flag → `mixengine.toml` walking up from cwd →
project record in SQLite → global default. Implemented once in `core::resolve`, used by shims, the
daemon and the GUI alike.

## Blueprint manifest

Same schema as the project manifest plus a `[blueprint]` header with name/description and pinned
exact versions. `blueprint.capture` snapshots a project; `blueprint.apply` creates a new project from
it. See [features/blueprints.md](../features/blueprints.md).

## Migration & compatibility rules

- Adding a column: new migration with a default. Never rewrite an existing migration file.
- Renaming a `ServiceId` or `kind` value requires a data migration in the same change.
- `mixengine.db` is backed up to `mixengine.db.bak-<version>` before any migration that drops or
  rewrites data.
- Manifest files are versioned by `schema = N` when a breaking change lands; the daemon upgrades old
  manifests in place and tells the user.
