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
| User preferences | `config.toml` | user or a client | — |

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
projects(id, name, root_path, runtime_pins_json, created_at, blueprint_id, keep_warm)
   -- runtime_pins_json: {"php":"8.3.12","node":"22.8.0"}
   -- keep_warm: hold this project's services out of idle shutdown while it is worked on (T69)
sites(id, project_id, extension_id, doc_root, kind, php_service_id,
      https_enabled, http_port, https_port, config_json, state,
      shared_interface, shared_address, shared_since, shared_until)
   -- kind: php-fpm | static | reverse-proxy | node-app
   -- exactly one of project_id / extension_id is set (CHECK, 0017): a project's site is
   --   rooted at projects.root_path, a web-app extension's at extensions.install_dir, and
   --   doc_root is relative to whichever owner it has (T81b)
   -- extension_id cascades where services.extension_id restricts: a site is declared state,
   --   not a process — and the cascade is what makes forgetting an extension a whole rollback
   -- sites_one_per_extension: a unique partial index, which is also the cascade's index
site_domains(id, site_id, domain, is_primary)  -- every domain, primary included
site_service_links(site_id, service_id)      -- which DBs/caches a site declares

-- TLS -----------------------------------------------------------------------
certificates(id, domain, sans_json, not_before, not_after, cert_path, key_path,
             issued_by_ca_fingerprint, revoked)
ca(id, fingerprint, cert_path, key_path, created_at, installed_in_trust_store)

-- Blueprints & extensions ----------------------------------------------------
blueprints(id, name, description, manifest_toml, created_at, source)
extensions(id, name, version, kind, manifest_json, install_dir, data_dir, source, signed,
           installed_at)
   -- rewritten by T81, which dropped 0001's placeholder: nothing had ever written to it
   -- manifest_json is the reader's canonical rendering, and the source of truth for the spec —
   --   nothing re-reads extension.toml out of install_dir, where a user could have edited it
   -- install_dir and data_dir are two directories with two lifetimes: an uninstall removes the
   --   first and keeps the second unless asked otherwise, which is why data_dir is not under it
   -- source is 'registry' or 'path'; signed is 0 for every --path install
extension_ports(extension_id, name, port)
   -- one row per allocated port, because services::ports::allocate asks the database which
   --   ports are taken: a second port kept in a JSON column would be handed out again

-- Operations ----------------------------------------------------------------
jobs(id, kind, state, percent, message, started_at, finished_at, result_json)
   -- id is the JobId a client is handed; kind is the method that produced it ("runtime.install")
   -- started_at/finished_at are epoch milliseconds, not ISO-8601 text — see below
   -- result_json is one JobOutcome; null exactly while state is 'running', enforced by two CHECKs
events(id, ts, kind, subject, payload_json)  -- ring-trimmed audit trail, 30 days
metrics_minutes(subject, minute, cpu_avg, cpu_peak, rss_avg, rss_peak, samples)
   -- what each subject cost, one row per minute, trimmed at 24 hours (T71)
   -- subject: 'daemon', or 'service:' + a ServiceId — the prefix is load-bearing, because
   --   ServiceId::parse accepts a bare name and a service may legally be called `daemon`;
   --   ':' is not in a service id's alphabet, so the two spaces cannot collide
   -- minute: epoch milliseconds truncated to the minute, an INTEGER — see below
   -- samples: how many readings the row is made of; 1 while nobody was watching and up
   --   to 60 while somebody held `GET /metrics` open, because an average of one reading
   --   and an average of sixty may be drawn but not as though they were equally supported
   -- cpu_avg/cpu_peak are nullable: a CPU figure is a difference between two readings, and
   --   NULL is "not measured" — never written as a zero
   -- **no foreign key to services, deliberately**: a service deleted at two in the morning
   --   is still the answer to what happened at two in the morning, and a cascade would
   --   delete exactly the evidence somebody came looking for. The trim is what bounds it,
   --   and this is the one table here whose rows outlive their subject
pending_privileged_ops(id, op, dedupe_key, requested_at)
   -- what is waiting for one elevation prompt (T40b); requested_at is epoch milliseconds
   -- dedupe_key is UNIQUE and holds the operation's canonical form, which is what makes
   -- "no code path elevates in a loop" a property of the schema rather than of anyone's care:
   -- a producer that enqueues on every start writes one row, and the row keeps the moment the
   -- machine first needed it
settings(key, value_json)
```

**Two kinds of moment, stored two ways.** Most `_at` columns are ISO-8601 text: they are written
once, read by a person, and compared by nobody — `installed_at` records when a package arrived and
nothing branches on it. `services.last_started_at` is the other kind and is `INTEGER` epoch
milliseconds, a `mixengine_proto::Timestamp` verbatim, because the supervisor reads it back on
every exit to decide whether a restart falls inside the crash-loop window. Text would mean parsing a
date on the hot path of a restart, and would put a civil-calendar conversion — a dependency this
workspace does not otherwise need — between the daemon and an arithmetic comparison. Formatting a
moment is the job of whatever shows one to a person, which is a client, not the store.
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
`certificates(domain, not_after)` for the renewal check, `events(ts)`, which both readers of
that table go through in time order, and `metrics_minutes(minute)` for the same reason on its own
table — the trim deletes a prefix and a history read takes a window, while that table's primary key
orders by subject first. `projects.name` and `projects.root_path` are unique columns:
one directory is one project. `root_path` is written spelled the way the filesystem spells it —
`paths::in_full`, which settles Windows' 8.3 aliases — and read back through the same call, or the
same directory under two spellings would be two projects and only one of them findable (T39).

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

Optional, checked into the user's project, and the reason a checkout carries its own setup:

```toml
[project]
name = "blog"

[runtimes]
php = "8.3"          # range or exact; resolved against installed versions
node = "22"

[site]
domain = "blog.test"
aliases = ["api.blog.test"]
doc_root = "public"
kind = "php-fpm"
https = true

[[services]]
name = "mariadb"
instance = "main"     # optional; the lookup tries `mariadb` and then `mariadb@main`
version = "11.4"
database = "blog"     # preserved, not interpreted — Phase 8's `blueprint.apply` reads it

[[services]]
name = "redis"
```

Resolution order for "which PHP is this?": explicit CLI flag → `mixengine.toml` walking up from cwd →
project record in SQLite → global default. Implemented once in `core::resolve`, used by shims, the
daemon and every client alike.

## Blueprint manifest

**Overlapping** with the project manifest rather than the same schema — corrected at T77, which is
where the two were first read by code. A blueprint carries a `[blueprint]` header, `domain_pattern`
where `mixengine.toml` carries `domain` and `aliases`, and the `database` and `user` a project
manifest only passes through. They also have two lifetimes: `mixengine.toml` is a file a person owns
and this workspace edits byte-preservingly, while a blueprint is generated and disposable. So there
are two types — `core::manifest` and `core::blueprints::manifest` — sharing the leaf vocabulary
(`RuntimeKind`, `VersionConstraint`, `SiteKind`, `ServiceId`) and nothing else.

`blueprint.capture` snapshots a project; `blueprint.apply` creates a new project from it. The row is
the truth and `blueprints/<slug>.toml` is a rendering of it, never parsed back into state. See
[features/blueprints.md](../features/blueprints.md).

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
- **Every migration path a release will perform is checked against a frozen database, not a
  reconstructed one** (T89). `crates/mixengine-testkit/fixtures/upgrade/` holds a `mixengine.db`
  captured at a schema version and committed as bytes, with the seed it was captured from beside it
  so the blob is reviewable; `crates/mixengine-core/tests/upgrade.rs` copies each one aside, opens
  it with the real `Store::open` and compares a census of every row before and after. It is what
  makes the first rule on this list enforceable: the checksums in a committed `_sqlx_migrations` are
  the only thing in this repository that can catch a migration edited after it shipped, because
  every other migration test builds "the old database" out of today's migration files. A captured
  fixture is **frozen** — never regenerate one to make a test pass.
  `cargo run -p mixengine-core --example capture-upgrade-fixture -- <schema>` makes a new one and
  refuses a destination that exists.
- **Two migrations in this tree empty a table rather than carrying its rows across**, and the suite
  names them rather than working around them: `0006_site_state.sql` drops `sites`, `site_domains`
  and `site_service_links` outright, and `0016_extensions.sql` drops `extensions` — no
  `INSERT … SELECT` in either, while the `services` rebuild beside the second one does carry its
  rows over. Nothing has ever been released from this repository, so no database in the world is
  below schema 17 and no user will ever perform either upgrade, which is why they are **recorded
  rather than repaired**: rewriting a shipped migration would break the first rule on this list and
  invalidate every developer's local database in exchange for nothing. The list in the suite is
  keyed by version, so a future migration that empties a table without an entry fails the census
  like any other loss, and a second test asserts the loss is *total* — an exception that quietly
  covered a partial one would be worse than none.
- **A reader does not migrate, and there is a window in that.** `Store::open_read_only` neither
  creates nor migrates, so between a binary upgrade and the next daemon start the file on disk is at
  the old schema while the shim's queries were compiled against the new one, and a column the
  pending migration adds is one the shim asks for and does not get. Measured by
  `the_shims_door_opens_an_old_database_and_leaves_it_old` in the upgrade suite and left open: what
  a shim should say when it finds a database older than itself is a design and not a patch.
