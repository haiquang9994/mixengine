-- The vocabulary `sites.state` was left open for — roadmap task T39a.
--
-- `0001_initial.sql` said why it had none: "its state machine belongs to a later phase and does not
-- exist yet; a CHECK written before the vocabulary does is guesswork, and SQLite has no way to drop
-- a constraint short of rebuilding the table." This is that phase, and the vocabulary is two words.
--
-- **Two words, because a site is not a process.** `enabled` and `disabled` say whether the web
-- server should have a server block for this site. `starting`, `running` and `failed` belong to the
-- *services* a site uses, which are `services` rows with seven states of their own; a site with a
-- lifecycle beside them would be a second answer to "is blog.test up".
--
-- **The rebuild drops rather than copies**, and that is a fact rather than a shortcut: no shipped
-- code path has ever written a row into any of these three tables — there were no `site.*` methods
-- before this migration — so on every machine in existence they are empty. A copy-out and copy-back
-- would be ceremony for whoever audits this file to read past.
--
-- **A new file rather than an edit to 0001.** T14 edited that one in place because nothing had
-- shipped. Five migrations later every developer database has run it and sqlx records its checksum,
-- so changing a byte of it turns the next `cargo test` into a migration failure.
--
-- Children first, because they hold the foreign keys.
DROP TABLE site_service_links;
DROP TABLE site_domains;
DROP TABLE sites;

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
    -- The kind's remaining payload: {"upstream": "…"} or {"port": 3000}, and {} for the two kinds
    -- that carry none. A blob rather than columns because nothing queries into it.
    config_json    TEXT    NOT NULL DEFAULT '{}',
    state          TEXT    NOT NULL DEFAULT 'enabled'
                   CHECK (state IN ('enabled', 'disabled'))
) STRICT;

-- "Which sites belong to this project" — what `mix status` and the GUI's project view both ask —
-- and the path the cascade takes when a project is deleted.
CREATE INDEX sites_project ON sites (project_id);

-- Every domain a site answers to, the primary one included. There is deliberately no
-- `sites.primary_domain` beside this table: two unique indexes on two tables cannot constrain each
-- other, so a primary column would leave `blog.test` free to be site A's primary *and* site B's
-- alias at the same time.
CREATE TABLE site_domains (
    id         INTEGER PRIMARY KEY,
    site_id    INTEGER NOT NULL REFERENCES sites (id) ON DELETE CASCADE,
    domain     TEXT    NOT NULL,
    is_primary INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0, 1))
) STRICT;

-- The one that decides ownership.
CREATE UNIQUE INDEX site_domains_domain ON site_domains (domain);

-- At most one primary per site. "At least one" is not expressible here, and stays an invariant the
-- site module upholds inside the transaction that creates a site.
CREATE UNIQUE INDEX site_domains_one_primary_per_site
    ON site_domains (site_id) WHERE is_primary = 1;

-- Deleting a site cascades into this table by `site_id`, which the unique index above cannot serve.
CREATE INDEX site_domains_site ON site_domains (site_id);

-- Which databases and caches a site declares, so `site.start` knows what to start with it.
CREATE TABLE site_service_links (
    site_id    INTEGER NOT NULL REFERENCES sites (id) ON DELETE CASCADE,
    service_id TEXT    NOT NULL REFERENCES services (id) ON DELETE CASCADE,

    PRIMARY KEY (site_id, service_id)
) STRICT;

-- "Which sites are still using this service" — asked before stopping one, and the path the cascade
-- takes when a service is deleted.
CREATE INDEX site_service_links_service ON site_service_links (service_id);
