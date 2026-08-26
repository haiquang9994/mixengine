-- The per-project opt-out from idle shutdown — roadmap task T69.
--
-- `features/resource-isolation.md`: "Sites can opt out per project ('keep warm') for the one project
-- being worked on all day." Default 0, because *the one project being worked on all day* is one
-- project: a home where every project were kept warm would be a home where the whole mechanism is
-- switched off, which is what `services.idle_minutes = 0` is already for.
--
-- On `projects` rather than on `services`, because the thing a person is working on is a project.
-- Which services that reaches is a join — see `projects::kept_warm` — and today it reaches the PHP
-- pool a project's sites name and nothing else, because `sites.php_service_id` is the only edge the
-- schema has between the two. T77's blueprint manifest is what widens it.
ALTER TABLE projects
    ADD COLUMN keep_warm INTEGER NOT NULL DEFAULT 0 CHECK (keep_warm IN (0, 1));
