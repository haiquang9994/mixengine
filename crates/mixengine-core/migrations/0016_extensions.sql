-- no-transaction
-- The extension registry's half of the database — roadmap task **T81**, the design's D5 and D6.
--
-- **Why this one gives up sqlx's transaction, and no other migration here has had to.** `services`
-- carries `CHECK ((package_id IS NULL) <> (runtime_install_id IS NULL))`, which refuses a row
-- belonging to an extension, and SQLite cannot alter a CHECK in place. So the table is rebuilt —
-- and two tables point at it, each damaged differently by a drop with foreign keys enforced:
-- `sites.php_service_id` is `ON DELETE SET NULL`, so every site would quietly lose the pool it
-- names, and `site_service_links.service_id` is `ON DELETE CASCADE`, so every "this site needs that
-- database" row would be deleted outright. The second is the worse one and the easier to miss,
-- because nothing about a site's own row would look wrong afterwards.
--
-- `PRAGMA foreign_keys` is a **no-op inside a transaction**, and sqlx wraps a migration in one by
-- default — so the pragma cannot be turned off from within the transaction sqlx would open.
-- `-- no-transaction` on the first line is how sqlx is told not to open one (it matches the literal
-- prefix), which is why that line comes before this comment rather than after it. What is lost is
-- sqlx's atomicity and what is bought is the pragma applying at all; the BEGIN below takes back as
-- much of the first as SQLite will give.
--
-- **0006 is not a precedent for this.** It rebuilt `sites` by DROP, and said why it was allowed to:
-- no shipped code path had ever written into those tables, so they were empty on every machine in
-- existence. `services` is full on every developer's machine, so this one copies.

PRAGMA foreign_keys = OFF;

BEGIN;

-- **`0001_initial.sql` reserved this name, and what it reserved does not fit.** That table was
-- written before there was anything to install: `manifest_toml` (T79's finding says the stored
-- manifest must be the reader's canonical rendering, and T80's reader is format-agnostic, so the
-- column is JSON), `install_path` alone (an install has two directories with two lifetimes — D13),
-- `state` (an extension's running state is its `services` row's, and a second copy of it is a second
-- answer to "is Mailpit up") and `settings_json` (nothing declares a setting; overrides live on the
-- service row like every other service's).
--
-- **It is dropped rather than migrated**, on exactly 0006's reasoning: no shipped code path has ever
-- written a row into it — there were no `extension.*` methods before T80 and T80 installs nothing —
-- so it is empty on every machine in existence, and a copy-out and copy-back would be ceremony for
-- whoever audits this file to read past.
DROP TABLE extensions;

-- What is installed, and where it went.
CREATE TABLE extensions (
    -- The `ExtensionId`, which is also the directory name and — for a `service` — the first half of
    -- its `ServiceId`.
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    -- The extension's own version, not MixEngine's.
    version       TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('service', 'web-app', 'desktop-app', 'recipe')),
    -- **The manifest as the reader rendered it, not as its author wrote it.** T79 found what the
    -- alternative costs: a manifest kept as somebody's text makes the file on disk, this column and
    -- what the renderer reads three texts for one extension. This column is the source of truth for
    -- the spec, and nothing re-reads `extension.toml` out of `install_dir` — that file sits where a
    -- user can edit it, so a manifest read back from it is one nobody consented to.
    manifest_json TEXT NOT NULL,
    install_dir   TEXT NOT NULL,
    -- **Outside `install_dir`** — the design's D13. An uninstall removes the install directory whole
    -- and keeps this one unless asked otherwise, which a `data` nested inside it would make
    -- impossible to promise.
    data_dir      TEXT NOT NULL,
    -- Where it came from. `path` is `mix extension install --path`, which nothing vouches for.
    source        TEXT NOT NULL CHECK (source IN ('registry', 'path')),
    -- Whether a signature covered it. Two-valued because the situation is: the registry's signature
    -- covers the whole document, so an entry either arrived inside something the compiled-in key
    -- vouched for or the document was refused entirely. There is no third answer to record, which is
    -- where this differs from `blueprints.signature` (T79b).
    signed        INTEGER NOT NULL CHECK (signed IN (0, 1)),
    installed_at  TEXT NOT NULL
) STRICT;

-- Every port an extension holds, one row each — the design's D8.
--
-- **A table rather than a JSON column, because the allocator reads SQL.**
-- `services::ports::allocate` asks the database which ports are taken; a second port kept inside a
-- blob would be invisible to it, and a database created next week would be handed the port Mailpit
-- answers SMTP on. The failure would arrive as a refused bind with nothing attached to it explaining
-- why — which is the hazard `allocate_activation` is already annotated against: every port any row
-- holds is taken, whichever column holds it.
CREATE TABLE extension_ports (
    extension_id TEXT    NOT NULL REFERENCES extensions (id) ON DELETE CASCADE,
    -- The `[ports]` key, which is also the placeholder `{ui_port}` renders from.
    name         TEXT    NOT NULL,
    port         INTEGER NOT NULL UNIQUE,

    PRIMARY KEY (extension_id, name)
) STRICT;

-- `services`, rebuilt for the third origin.
--
-- Every column is `0001_initial.sql`'s, plus `activation_port` (0009) and `idle_stopped` (0010) —
-- the only two migrations that have touched this table. No index has ever been created on it, so
-- there is none to re-create here.
CREATE TABLE services_new (
    id                    TEXT    PRIMARY KEY,
    package_id            INTEGER REFERENCES packages (id) ON DELETE RESTRICT,
    runtime_install_id    INTEGER REFERENCES runtime_installs (id) ON DELETE RESTRICT,
    -- **The third parent** — T81. RESTRICT for the reason the other two have it: removing an
    -- extension while a service still runs out of it is a mistake to report, not one to carry out.
    extension_id          TEXT    REFERENCES extensions (id) ON DELETE RESTRICT,
    instance_name         TEXT    NOT NULL,
    state                 TEXT    NOT NULL CHECK (state IN (
                              'stopped', 'starting', 'running', 'degraded',
                              'stopping', 'restarting', 'failed')),
    autostart             INTEGER NOT NULL DEFAULT 0 CHECK (autostart IN (0, 1)),
    port                  INTEGER,
    activation_port       INTEGER,
    bind_addr             TEXT    NOT NULL DEFAULT '127.0.0.1',
    data_dir              TEXT,
    config_overrides_json TEXT    NOT NULL DEFAULT '{}',
    limits_json           TEXT    NOT NULL DEFAULT '{}',
    idle_minutes          INTEGER,
    idle_stopped          INTEGER NOT NULL DEFAULT 0 CHECK (idle_stopped IN (0, 1)),
    last_started_at       INTEGER,
    last_exit_code        INTEGER,
    pid                   INTEGER,
    pid_start_time        INTEGER,

    -- Exactly one parent of three. `(x IS NOT NULL)` is 0 or 1 in SQLite, so a sum of one is how
    -- "one of three" is spelled where "one of two" was an exclusive-or.
    CHECK (((package_id IS NOT NULL)
            + (runtime_install_id IS NOT NULL)
            + (extension_id IS NOT NULL)) = 1),

    -- Three constraints rather than one over a coalesced column, for 0001's reason: SQLite treats
    -- NULLs as distinct in a UNIQUE, so each of these only ever sees the rows whose parent it names.
    UNIQUE (package_id, instance_name),
    UNIQUE (runtime_install_id, instance_name),
    UNIQUE (extension_id, instance_name)
) STRICT;

INSERT INTO services_new
    (id, package_id, runtime_install_id, instance_name, state, autostart, port, activation_port,
     bind_addr, data_dir, config_overrides_json, limits_json, idle_minutes, idle_stopped,
     last_started_at, last_exit_code, pid, pid_start_time)
SELECT
     id, package_id, runtime_install_id, instance_name, state, autostart, port, activation_port,
     bind_addr, data_dir, config_overrides_json, limits_json, idle_minutes, idle_stopped,
     last_started_at, last_exit_code, pid, pid_start_time
FROM services;

DROP TABLE services;

ALTER TABLE services_new RENAME TO services;

-- What the pragma was turned off for: proof that nothing was left pointing at a row that is no
-- longer there. A violation here fails the migration rather than leaving a home to find out later.
PRAGMA foreign_key_check;

COMMIT;

PRAGMA foreign_keys = ON;
