# Data model

## State ownership

| Kind | Lives in | Written by | Rebuildable? |
| --- | --- | --- | --- |
| Declared state (projects, sites, versions, service settings) | `mixengine.db` (SQLite) | daemon only | no — this is the source of truth, back it up |
| Generated config (`etc/**`) | files | daemon | **yes**, always regenerate, never parse back |
| Service data (`data/**`) | files | the service itself | no — user data |
| Runtimes & packages (`runtimes/**`, `packages/**`) | files | installer | yes, re-downloadable |
| Cached downloads (`cache/**`) | files | daemon | yes — delete it and the next fetch replaces it |
| Secrets (DB root passwords, extension tokens) | OS keyring | daemon | no |
| User preferences | `config.toml` | user or GUI | — |

Rule: **if it can be regenerated, it is not state.** This keeps "reset my Nginx config" a one-liner
and makes upgrades safe.

`cache/` is not `run/`, although both are disposable. `run/` is scratch belonging to the daemon
currently running and may be emptied between runs; the whole value of a cached package index is that
it survives a reboot, because a machine that came up offline and lost its cache is a machine that can
list nothing. It is also not private, and does not need to be: everything in it is a document
published to the world, and what makes it trustworthy is the signature — re-checked on every read,
because the file is one any local process can rewrite.

## SQLite

Accessed via `sqlx` (compile-time-checked queries, same as MixDB), WAL mode, `foreign_keys=ON`,
`synchronous=NORMAL`, a five-second busy timeout and a pool of four connections. Every one of those
is set explicitly rather than left to sqlx's defaults, which the next release is free to change.

Tables are declared `STRICT`. SQLite otherwise stores whatever it is handed, so a version written as
the number `8.3` comes back as `8.3000000000000007`; with `STRICT` a value that converts losslessly
still converts (the text `'3306'` becomes the integer `3306`) and one that does not is refused,
which is the half that matters.

Migrations are numbered SQL files in `crates/mixengine-core/migrations/`, applied at boot by
`core::store::Store::open`; forward only, never edited after release. They live beside the `Store`
rather than in the daemon because `sqlx::migrate!` embeds the directory of the crate that owns the
schema, and the type every domain module is handed (`Arc<Store>`, see
[../standards/rust.md](../standards/rust.md)) is a `core` type. The daemon is still the only
process that **writes** the database, and since T25 not the only one that opens it: a shim reads it
through `Store::open_read_only`, which neither creates the file nor migrates it — a schema upgrade
decided by whichever `php -v` ran first is the one thing this file cannot afford.

```sql
-- Runtimes -----------------------------------------------------------------
runtime_installs(id, kind, version, channel, install_path, installed_at, size_bytes,
                 source_url, sha256, is_default, provides_json)
   -- kind: php | node | python | ruby ; UNIQUE(kind, version)
   -- provides_json: {"php":"bin/php","php-config":"bin/php-config"} — the artifact's own
   --   `provides` map, kept because the shim has to turn a command name into a file with no
   --   daemon to ask and no right to guess the publisher's layout

-- Packages (servers, databases, caches) ------------------------------------
packages(id, name, version, install_path, installed_at, source_url, sha256)
   -- name: caddy | nginx | mariadb | mysql | postgresql | redis | memcached | mailpit …

-- Service instances ---------------------------------------------------------
services(id, package_id, runtime_install_id, instance_name, state, autostart, port,
         bind_addr, data_dir, config_overrides_json, limits_json, idle_minutes,
         last_started_at, last_exit_code, pid, pid_start_time)
   -- id is the human-stable ServiceId, e.g. "mariadb@main", "php-fpm@8.3.33"
   -- the instance half is the FULL version for a pool: runtime_installs is
   --   UNIQUE (kind, version) over the full version, so 8.3.33 and 8.3.34 can both
   --   be installed and "php-fpm@8.3" would name neither
   -- exactly one of package_id / runtime_install_id is set, enforced by a CHECK:
   --   every service up to php-fpm comes out of a packages row, and a pool comes
   --   out of the PHP that runtime.install put on disk (T32)
   -- last_started_at is epoch milliseconds, not ISO-8601 text — see below

-- Projects & sites ----------------------------------------------------------
projects(id, name, root_path, runtime_pins_json, created_at, blueprint_id)
   -- runtime_pins_json: {"php":"8.3.12","node":"22.8.0"}
sites(id, project_id, doc_root, kind, php_service_id,
      https_enabled, http_port, https_port, config_json, state)
   -- kind: php-fpm | static | reverse-proxy | node-app
site_domains(id, site_id, domain, is_primary)  -- every domain, primary included
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
   -- id is the JobId a client is handed; kind is the method that produced it ("runtime.install")
   -- started_at/finished_at are epoch milliseconds, not ISO-8601 text — see below
   -- result_json is one JobOutcome; null exactly while state is 'running', enforced by two CHECKs
events(id, ts, kind, subject, payload_json)  -- ring-trimmed audit trail, 30 days
settings(key, value_json)
```

**Two kinds of moment, stored two ways.** Most `_at` columns are ISO-8601 text: they are written
once, read by a person, and compared by nobody — `installed_at` records when a package arrived and
nothing branches on it. `services.last_started_at` is the other kind and is `INTEGER` epoch
milliseconds, a `mixengine_proto::Timestamp` verbatim, because the supervisor reads it back on
every exit to decide whether a restart falls inside the crash-loop window. Text would mean parsing a
date on the hot path of a restart, and would put a civil-calendar conversion — a dependency this
workspace does not otherwise need — between the daemon and an arithmetic comparison. Formatting a
moment is the job of whatever shows one to a person, which is the CLI and the GUI, not the store.
`pid_start_time` was already an integer for the same reason: it exists to be compared, never read.

`jobs.started_at` and `jobs.finished_at` joined them at T22, which was the first task that had to
*write* an `_at` column at runtime rather than put a literal in a fixture — and found that this
workspace still has no date library. Both readings are compared rather than displayed: a listing
orders by the first, a duration is the difference, and a job's ending is placed against its start.
Text would have meant buying a civil-calendar dependency to parse it back on every one of those.

`jobs` is also the one table here that grows without bound — every job a home has ever run stays in
it, and nothing trims it. What bounds the cost is the read: `job.list` takes a limit, defaulting to
fifty. Deleting history needs a retention policy, and one invented before anything had produced a
single job would have been a guess.

A site's domains live in `site_domains` and nowhere else — there is no `primary_domain` column on
`sites`. Two unique indexes on two tables cannot constrain each other, so splitting them would let
one domain be site A's primary and site B's alias at once, which is precisely the collision the
uniqueness is there to prevent. The primary is the row with `is_primary = 1`.

Indexes: `site_domains(domain)` unique — the one that decides who owns a domain —
`site_domains(site_id)` unique *where* `is_primary = 1`, `runtime_installs(kind)` unique *where*
`is_default = 1` — a plain unique `(kind, is_default)` would forbid a second non-default PHP, which
is the normal case — `site_domains(site_id)` and `sites(project_id)` for the cascades and for "the
domains of this site" / "the sites of this project", `site_service_links(service_id)` because the
primary key `(site_id, service_id)` cannot answer "which sites still use this service",
`certificates(domain, not_after)` for the renewal check, and `events(ts)`, which both readers of
that table go through in time order. `projects.name` and `projects.root_path` are unique columns:
one directory is one project.

The remaining foreign keys have no index of their own on purpose. They are checked against tables
holding a few dozen rows, where the scan is not measurable and the index is a write on every insert
in exchange for nothing.

"At least one primary per site" is not expressible in SQLite, which has no deferred constraints: the
site row and its primary domain row cannot both be required to exist before either is written. It is
an invariant the site module upholds inside the transaction that creates a site.

## User preferences (`config.toml`, in `MIXENGINE_HOME`)

Read once at boot, before anything else exists — it is the only file that can move the directories
the rest of the layout is built from. Written commented-out on first run and never rewritten, so a
user's edits survive every update; a missing file means "all defaults".

```toml
[log]
level = "info"          # error | warn | info | debug | trace
format = "text"         # text | json

[daemon]
ipc_path = "…"          # unset: a socket under run/, a named pipe on Windows

[paths]                 # absolute, or relative to MIXENGINE_HOME
runtimes = "…"
packages = "…"
data = "…"
logs = "…"
```

Two rules hold the file together:

- **Unknown keys are an error**, not a warning (`serde(deny_unknown_fields)`). A typo that is
  silently ignored is indistinguishable from a setting that does not work. So is a relocation that,
  once `.` and `..` are resolved, names no directory of its own (`""`, `"."`, `".."`, `"bulk/.."`,
  `"/"`): `Path::join("")` returns the root, so it would silently make the relocated directory *be*
  `MIXENGINE_HOME` or a directory containing it. So is one that cannot be anchored on Windows —
  starting at a drive root without naming the drive (`"/bulk"`, `'\bulk'`) or naming a drive
  without starting at its root (`'C:bulk'`), both of which `join` resolves against the *current*
  directory of that drive rather than against the root. A sibling (`"../bulk"`) is allowed.
- **Keys arrive with the task that reads them.** A section nothing honours yet is a promise the
  build does not keep. Only `bin/`, `etc/`, `certs/`, `extensions/`, `blueprints/`, `run/` and
  `mixengine.db` are *not* relocatable — an uninstaller can only promise to remove a home it can
  find.

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
- `mixengine.db` is backed up to `mixengine.db.bak-<version>` before **any** migration runs against a
  database that already has one applied — not only the ones that drop or rewrite data, because
  nothing can tell from the SQL which those are and being wrong once costs the user their sites. A
  database with nothing applied yet is not copied; there is nothing in it. A backup that already
  exists is kept rather than replaced: same-version backups only happen when an upgrade ran twice,
  and the older file is the one from before the first, possibly half-finished attempt.
- The copy is taken with `VACUUM INTO`, never with a file copy. Under WAL the most recent commits
  live in the `-wal` sidecar until a checkpoint moves them, so copying the main file alone produces
  a backup missing exactly the work that was done most recently.
- It is written to `mixengine.db.bak-<version>.partial` and renamed into place. "Is there already a
  backup?" is answered by looking for a file, so a copy that died half way would answer it wrongly
  and the next upgrade would step over a truncated database. After the rename, a file at the backup
  path can only have come from a copy that finished; a leftover `.partial` is discarded and retried.
- Manifest files are versioned by `schema = N` when a breaking change lands; the daemon upgrades old
  manifests in place and tells the user.
