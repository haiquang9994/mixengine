-- The schema from .claude/architecture/data-model.md, in dependency order so that every foreign key
-- points at a table that already exists.
--
-- Four conventions hold throughout, and none of them is worth re-deciding per table:
--
--   * STRICT. Without it SQLite stores whatever it is handed, so a version written as the number
--     8.3 instead of the string "8.3" comes back as 8.3000000000000007 and a port written as "80"
--     compares less than 9. The whole point of keeping declared state in a database rather than in
--     JSON files is that it refuses what does not fit.
--   * Times are ISO-8601 UTC text ("2026-08-11T09:14:03Z"). Text because a database a user opens in
--     a viewer during a support conversation should be readable, and because lexical order is
--     chronological order for this format anyway. **The exception is a moment the daemon does
--     arithmetic on**, which is epoch milliseconds and says so at the column: `services.last_started_at`
--     (T15) and the two on `jobs` (T22). Both are read back and compared rather than displayed, and
--     text would mean parsing a date on the path that compares it — with a civil-calendar dependency
--     this workspace has never otherwise needed.
--   * Booleans are INTEGER 0/1 with a CHECK, which is what SQLite has; the CHECK is what stops a 2.
--   * A `*_json` column is TEXT holding one JSON document, parsed by `serde_json` on the way out.
--     These are settings blobs nothing queries into — the moment something needs to filter on a
--     field, that field becomes a column in a new migration.
--
-- Closed vocabularies are CHECKed because they are fixed by the product: four runtimes, four site
-- kinds, the seven states of `mixengine_proto::ServiceState` (T14) and the four of
-- `mixengine_proto::JobState` (T22). `sites.state` is still not, because its state machine belongs
-- to a later phase and does not exist yet; a CHECK written before the vocabulary does is guesswork,
-- and SQLite has no way to drop a constraint short of rebuilding the table.

-- Runtimes --------------------------------------------------------------------------------------

CREATE TABLE runtime_installs (
    id           INTEGER PRIMARY KEY,
    kind         TEXT    NOT NULL CHECK (kind IN ('php', 'node', 'python', 'ruby')),
    version      TEXT    NOT NULL,
    channel      TEXT    NOT NULL,
    install_path TEXT    NOT NULL,
    installed_at TEXT    NOT NULL,
    size_bytes   INTEGER NOT NULL,
    source_url   TEXT    NOT NULL,
    sha256       TEXT    NOT NULL,
    is_default   INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),

    UNIQUE (kind, version)
) STRICT;

-- One default per kind. A plain UNIQUE (kind, is_default) would say something else entirely: it
-- would forbid a second *non*-default PHP, which is the normal case and the reason this product
-- exists.
CREATE UNIQUE INDEX runtime_installs_one_default_per_kind
    ON runtime_installs (kind) WHERE is_default = 1;

-- Packages: servers, databases, caches ------------------------------------------------------------

CREATE TABLE packages (
    id           INTEGER PRIMARY KEY,
    -- caddy | nginx | mariadb | postgresql | redis | memcached | mailpit … Not CHECKed: the list
    -- grows with every extension that ships a service (T80), which is a registry entry rather than
    -- a schema change.
    name         TEXT    NOT NULL,
    version      TEXT    NOT NULL,
    install_path TEXT    NOT NULL,
    installed_at TEXT    NOT NULL,
    source_url   TEXT    NOT NULL,
    sha256       TEXT    NOT NULL,

    UNIQUE (name, version)
) STRICT;

-- Service instances --------------------------------------------------------------------------------

CREATE TABLE services (
    -- The human-stable ServiceId — "mariadb@main", "php-fpm@8.3" — and not a rowid, because it is
    -- what the user types, what the log directory is named after and what an event carries.
    id                    TEXT    PRIMARY KEY,
    -- RESTRICT, not CASCADE: uninstalling a package while an instance still refers to it is a
    -- mistake to report, not one to carry out. The instance owns data_dir.
    --
    -- **Two possible parents, and exactly one of them set** — T32. Every service up to php-fpm came
    -- out of a `packages` row; php-fpm comes out of a `runtime_installs` one, because the process
    -- that serves a user's sites lives inside the PHP they installed with `runtime.install`. Giving
    -- it a `packages` row as well would be a second table describing one directory, with
    -- `package.uninstall` able to see and delete it and an `install_path` that goes stale the moment
    -- the runtime is removed. The foreign key here is also what gives `runtime.uninstall` its
    -- refusal for nothing.
    package_id            INTEGER REFERENCES packages (id) ON DELETE RESTRICT,
    runtime_install_id    INTEGER REFERENCES runtime_installs (id) ON DELETE RESTRICT,
    instance_name         TEXT    NOT NULL,
    -- `mixengine_proto::ServiceState`, spelled exactly as `ServiceState::as_str` writes it. The
    -- list is closed in Rust too, so this constraint is not a second opinion about the vocabulary —
    -- it is what stops a hand-edited database, or a future migration writing a literal, from
    -- putting a word in here that the daemon cannot read back and cannot act on.
    state                 TEXT    NOT NULL CHECK (state IN (
                              'stopped', 'starting', 'running', 'degraded',
                              'stopping', 'restarting', 'failed')),
    autostart             INTEGER NOT NULL DEFAULT 0 CHECK (autostart IN (0, 1)),
    -- Null for a service that listens on a socket rather than a port.
    port                  INTEGER,
    bind_addr             TEXT    NOT NULL DEFAULT '127.0.0.1',
    data_dir              TEXT,
    config_overrides_json TEXT    NOT NULL DEFAULT '{}',
    limits_json           TEXT    NOT NULL DEFAULT '{}',
    -- Null means "never shut this down for being idle" (T69).
    idle_minutes          INTEGER,
    -- Milliseconds since the Unix epoch — a `mixengine_proto::Timestamp` — where the `_at` columns
    -- on every other table are ISO-8601 text. The difference is what the column is for rather than
    -- an inconsistency: `installed_at` and `created_at` are records a person reads, written once and
    -- compared by nobody, while this one is read back by the supervisor to decide whether a restart
    -- falls inside a crash-loop window and is set on every start. Storing a moment the daemon does
    -- arithmetic on as text would mean parsing it back on every comparison, and formatting is a
    -- concern of whatever renders it to a person.
    last_started_at       INTEGER,
    last_exit_code        INTEGER,
    -- The pair T18 adopts a survivor by: a pid alone is reused by the OS within minutes, and
    -- signalling the wrong process is exactly the accident this product cannot have.
    pid                   INTEGER,
    pid_start_time        INTEGER,

    -- One parent or the other, never both and never neither. `(x IS NULL)` is 0 or 1 in SQLite, so
    -- `<>` over the pair is exclusive-or spelled in what this database has.
    CHECK ((package_id IS NULL) <> (runtime_install_id IS NULL)),

    -- Two constraints rather than one over a coalesced column: SQLite treats NULLs as distinct in a
    -- UNIQUE, so each of these only ever sees the rows whose parent it names, and the other kind's
    -- rows are invisible to it rather than colliding on a shared NULL.
    UNIQUE (package_id, instance_name),
    UNIQUE (runtime_install_id, instance_name)
) STRICT;

-- Blueprints ----------------------------------------------------------------------------------------
-- Before projects, which reference them.

CREATE TABLE blueprints (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    manifest_toml TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    -- builtin | captured | imported
    source        TEXT NOT NULL
) STRICT;

-- Projects & sites ------------------------------------------------------------------------------

CREATE TABLE projects (
    id                INTEGER PRIMARY KEY,
    name              TEXT    NOT NULL UNIQUE,
    root_path         TEXT    NOT NULL UNIQUE,
    -- {"php": "8.3.12", "node": "22.8.0"} — the pins `core::resolve` consults after mixengine.toml.
    runtime_pins_json TEXT    NOT NULL DEFAULT '{}',
    created_at        TEXT    NOT NULL,
    -- SET NULL rather than RESTRICT: a blueprint is where a project came from, not something it
    -- depends on, and deleting one must not strand the project it created.
    blueprint_id      TEXT    REFERENCES blueprints (id) ON DELETE SET NULL
) STRICT;

CREATE TABLE sites (
    id             INTEGER PRIMARY KEY,
    project_id     INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    doc_root       TEXT    NOT NULL,
    kind           TEXT    NOT NULL
                   CHECK (kind IN ('php-fpm', 'static', 'reverse-proxy', 'node-app')),
    -- Only a php-fpm site has one, and it may outlive the pool it names being reconfigured.
    php_service_id TEXT    REFERENCES services (id) ON DELETE SET NULL,
    https_enabled  INTEGER NOT NULL DEFAULT 1 CHECK (https_enabled IN (0, 1)),
    http_port      INTEGER NOT NULL DEFAULT 80,
    https_port     INTEGER NOT NULL DEFAULT 443,
    config_json    TEXT    NOT NULL DEFAULT '{}',
    state          TEXT    NOT NULL
) STRICT;

-- "Which sites belong to this project" — what `mix status` and the GUI's project view both ask —
-- and the path the cascade takes when a project is deleted. The other foreign keys in this schema
-- deliberately have no index of their own: they are checked against tables holding a few dozen rows
-- and would buy a scan nobody can measure at the cost of a write nobody asked for.
CREATE INDEX sites_project ON sites (project_id);

-- Every domain a site answers to, the primary one included. There is deliberately no
-- `sites.primary_domain` beside this table: two unique indexes on two tables cannot constrain each
-- other, so a primary column would leave `blog.test` free to be site A's primary *and* site B's
-- alias at the same time — the web server would then answer with whichever import it read last,
-- which is the bug a user reports as "it randomly serves the wrong project". One table means one
-- index decides who owns a domain, and it cannot disagree with itself.
CREATE TABLE site_domains (
    id         INTEGER PRIMARY KEY,
    site_id    INTEGER NOT NULL REFERENCES sites (id) ON DELETE CASCADE,
    domain     TEXT    NOT NULL,
    is_primary INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0, 1))
) STRICT;

-- The one that decides ownership.
CREATE UNIQUE INDEX site_domains_domain ON site_domains (domain);

-- At most one primary per site. "At least one" is not expressible here — SQLite has no deferred
-- constraint, so the row and its site cannot both be required to exist before either is written —
-- and it stays an invariant the site module upholds inside the transaction that creates a site.
CREATE UNIQUE INDEX site_domains_one_primary_per_site
    ON site_domains (site_id) WHERE is_primary = 1;

-- Deleting a site cascades into this table by `site_id`, which the unique index above cannot serve
-- (it is keyed on `domain`); the partial index does not cover a non-primary row. Without this, each
-- delete scans every domain in the database.
CREATE INDEX site_domains_site ON site_domains (site_id);

-- Which databases and caches a site declares, so `site.start` knows what to start with it.
CREATE TABLE site_service_links (
    site_id    INTEGER NOT NULL REFERENCES sites (id) ON DELETE CASCADE,
    service_id TEXT    NOT NULL REFERENCES services (id) ON DELETE CASCADE,

    PRIMARY KEY (site_id, service_id)
) STRICT;

-- The primary key answers "what does this site need" and cannot answer the reverse: an index on
-- (site_id, service_id) is no help to a lookup that knows only the second column. "Which sites are
-- still using this service" is the question asked before stopping one, and the cascade when a
-- service is deleted takes the same path.
CREATE INDEX site_service_links_service ON site_service_links (service_id);

-- TLS -------------------------------------------------------------------------------------------

CREATE TABLE ca (
    id                        INTEGER PRIMARY KEY,
    fingerprint               TEXT    NOT NULL UNIQUE,
    cert_path                 TEXT    NOT NULL,
    key_path                  TEXT    NOT NULL,
    created_at                TEXT    NOT NULL,
    installed_in_trust_store  INTEGER NOT NULL DEFAULT 0
                              CHECK (installed_in_trust_store IN (0, 1))
) STRICT;

CREATE TABLE certificates (
    id                       INTEGER PRIMARY KEY,
    domain                   TEXT    NOT NULL,
    sans_json                TEXT    NOT NULL DEFAULT '[]',
    not_before               TEXT    NOT NULL,
    not_after                TEXT    NOT NULL,
    cert_path                TEXT    NOT NULL,
    key_path                 TEXT    NOT NULL,
    -- RESTRICT keeps a rotation (T54) honest: the old CA row cannot be deleted while leaves it
    -- signed are still on disk and still in a web server's configuration.
    issued_by_ca_fingerprint TEXT    NOT NULL REFERENCES ca (fingerprint) ON DELETE RESTRICT,
    revoked                  INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1))
) STRICT;

-- The certificate a renewal check looks up, and the one a handshake needs: the newest unrevoked
-- leaf for a domain.
CREATE INDEX certificates_domain ON certificates (domain, not_after);

-- Extensions ------------------------------------------------------------------------------------

CREATE TABLE extensions (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    version       TEXT NOT NULL,
    manifest_toml TEXT NOT NULL,
    install_path  TEXT NOT NULL,
    state         TEXT NOT NULL,
    settings_json TEXT NOT NULL DEFAULT '{}'
) STRICT;

-- Operations ------------------------------------------------------------------------------------

CREATE TABLE jobs (
    id          INTEGER PRIMARY KEY,
    -- The method that produced the job — "runtime.install", "cert.issue". Not CHECKed, for the same
    -- reason `packages.name` is not: the set grows with every phase that has something long to do,
    -- and from T80 with every extension that ships one. `mixengine_proto::JobKind` is what refuses a
    -- value that is not a name.
    kind        TEXT    NOT NULL,
    -- `mixengine_proto::JobState`, spelled exactly as `JobState::as_str` writes it, and closed in
    -- Rust for the reason `services.state` is. The CHECK arrived with T22, which is what the note at
    -- the top of this file said it was waiting for.
    state       TEXT    NOT NULL CHECK (state IN (
                    'running', 'succeeded', 'failed', 'cancelled')),
    percent     INTEGER NOT NULL DEFAULT 0 CHECK (percent BETWEEN 0 AND 100),
    message     TEXT    NOT NULL DEFAULT '',
    -- Milliseconds since the Unix epoch — `mixengine_proto::Timestamp` — where this schema's other
    -- `_at` columns are ISO-8601 text. T22 is the first task that actually had to *write* one of
    -- these, and it found that this workspace still has no date library: `installed_at` and
    -- `created_at` are literals in fixtures and nothing has yet had to produce one at runtime. That
    -- leaves two ways to write a moment here — add a civil-calendar dependency, or store the number
    -- the daemon already holds — and the second is also what the column is *for*: a job's duration
    -- is subtraction, `job.list` orders by this, and `job.wait` compares against it. The same
    -- argument `services.last_started_at` made at T15, reaching the same answer.
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    -- The `mixengine_proto::JobOutcome` of a finished job: null exactly while `state` is 'running'.
    -- Written in the same statement as the state it belongs to, so the two cannot disagree.
    result_json TEXT,

    -- A finished job has an ending and a moment; a running one has neither. Two nullable columns can
    -- otherwise express a third thing that never happens — a job still going with a result, or one
    -- that ended with nothing to show — and the pair would then have to be re-checked by every
    -- reader instead of once here.
    CHECK ((state = 'running') = (finished_at IS NULL)),
    CHECK ((state = 'running') = (result_json IS NULL))
) STRICT;

-- What `job.list` reads: newest first, optionally one state. Both readers go through it in that
-- order, and the table is the one thing in this schema that grows without bound — every job a home
-- has ever run stays in it.
CREATE INDEX jobs_state_started ON jobs (state, started_at DESC);
CREATE INDEX jobs_started ON jobs (started_at DESC);

-- The audit trail behind the GUI's "recent events", trimmed to 30 days.
CREATE TABLE events (
    id           INTEGER PRIMARY KEY,
    ts           TEXT NOT NULL,
    kind         TEXT NOT NULL,
    -- What the event is about: a site id, a service id, a domain. Free text because the subject of
    -- an event is not one kind of thing.
    subject      TEXT NOT NULL DEFAULT '',
    payload_json TEXT NOT NULL DEFAULT '{}'
) STRICT;

-- Both readers of this table go through it in time order: the trim deletes a prefix, the GUI reads
-- a suffix.
CREATE INDEX events_ts ON events (ts);

CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
) STRICT;
