-- no-transaction
-- The site a `web-app` extension is served on — roadmap task **T81b**, the design's D1 and D2.
--
-- `sites.project_id` was `NOT NULL`, and an administrative interface belongs to no project. SQLite
-- cannot drop `NOT NULL`, so the table is rebuilt — the fourth time (0001, 0006 by drop, 0012/0013 by
-- `ADD COLUMN`) and the second by **copy**, on 0016's reasoning: `sites` is full on every developer's
-- machine, and 0006's drop was allowed only because nothing had ever written into it.
--
-- **Why this gives up sqlx's transaction, exactly as 0016 did.** Two tables cascade into `sites` —
-- `site_domains.site_id` and `site_service_links.site_id` — and a `DROP TABLE sites` with foreign
-- keys enforced deletes every row of both. `PRAGMA foreign_keys` is a no-op inside a transaction, so
-- the marker on the first line is what lets the pragma below apply; the BEGIN takes back as much
-- atomicity as SQLite will give.
--
-- **What a drop takes with it and this file has to put back**: the index `sites_project`, and the two
-- triggers 0013 last wrote — SQLite drops a table's triggers with the table, and a trigger that is
-- not recreated fails silently. `tests/migration_extension_sites.rs` asserts the refusal, not the row.

PRAGMA foreign_keys = OFF;

BEGIN;

CREATE TABLE sites_new (
    id               INTEGER PRIMARY KEY,
    -- **One of two parents.** A project's site, as every site was before this migration …
    project_id       INTEGER REFERENCES projects (id) ON DELETE CASCADE,
    -- … or an extension's. CASCADE where `services.extension_id` is RESTRICT: a service is a process
    -- that may be running, a site is declared state re-rendered from the rows — and the cascade is
    -- what makes `extension_store::forget` a whole rollback and an interrupted uninstall re-runnable.
    extension_id     TEXT    REFERENCES extensions (id) ON DELETE CASCADE,
    -- Relative to the **owner's** root: `projects.root_path`, or `extensions.install_dir`.
    doc_root         TEXT    NOT NULL,
    kind             TEXT    NOT NULL
                     CHECK (kind IN ('php-fpm', 'static', 'reverse-proxy', 'node-app')),
    php_service_id   TEXT    REFERENCES services (id) ON DELETE SET NULL,
    https_enabled    INTEGER NOT NULL DEFAULT 1 CHECK (https_enabled IN (0, 1)),
    http_port        INTEGER NOT NULL DEFAULT 80,
    https_port       INTEGER NOT NULL DEFAULT 443,
    config_json      TEXT    NOT NULL DEFAULT '{}',
    state            TEXT    NOT NULL DEFAULT 'enabled'
                     CHECK (state IN ('enabled', 'disabled')),
    shared_interface TEXT,
    shared_address   TEXT,
    shared_since     INTEGER,
    shared_until     INTEGER,

    -- Exactly one owner: the exclusive-or 0001 spelled for `services` before T81 made it a sum.
    CHECK ((project_id IS NULL) <> (extension_id IS NULL))
) STRICT;

INSERT INTO sites_new
    (id, project_id, doc_root, kind, php_service_id, https_enabled, http_port, https_port,
     config_json, state, shared_interface, shared_address, shared_since, shared_until)
SELECT
     id, project_id, doc_root, kind, php_service_id, https_enabled, http_port, https_port,
     config_json, state, shared_interface, shared_address, shared_since, shared_until
FROM sites;

DROP TABLE sites;

ALTER TABLE sites_new RENAME TO sites;

-- 0006's index, back.
CREATE INDEX sites_project ON sites (project_id);

-- **One site per extension**, and the index the cascade walks — `sites_project`'s job for the other
-- parent. A `web-app` declares one `[web-app]` table, so a second site under one extension is a row
-- nothing this build writes.
CREATE UNIQUE INDEX sites_one_per_extension ON sites (extension_id) WHERE extension_id IS NOT NULL;

-- 0013's pair, verbatim.
CREATE TRIGGER sites_sharing_is_all_or_nothing_insert
BEFORE INSERT ON sites
FOR EACH ROW
WHEN (NEW.shared_interface IS NULL) <> (NEW.shared_address IS NULL)
  OR (NEW.shared_interface IS NULL) <> (NEW.shared_since IS NULL)
  OR (NEW.shared_interface IS NULL AND NEW.shared_until IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'a shared site carries an interface, an address and a start, or none of the three — and only a shared site carries a deadline');
END;

CREATE TRIGGER sites_sharing_is_all_or_nothing_update
BEFORE UPDATE ON sites
FOR EACH ROW
WHEN (NEW.shared_interface IS NULL) <> (NEW.shared_address IS NULL)
  OR (NEW.shared_interface IS NULL) <> (NEW.shared_since IS NULL)
  OR (NEW.shared_interface IS NULL AND NEW.shared_until IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'a shared site carries an interface, an address and a start, or none of the three — and only a shared site carries a deadline');
END;

-- Proof that nothing was left pointing at a row that is no longer there.
PRAGMA foreign_key_check;

COMMIT;

PRAGMA foreign_keys = ON;
