-- Whether a blueprint's own `[scaffold]` command may be offered to run — roadmap task T78a.
--
-- Written once, by whatever put the row there, and raised by nothing afterwards: `builtin` is this
-- build's own and `captured` is this machine's own, while `imported` earns it only from a signature
-- that verified against the compiled-in gallery key. That is what "untrusted for good" means when
-- it is a column rather than a promise.
--
-- Every row that exists on any machine today is `captured` — `blueprint.import` is what this task
-- adds, and nothing before it could write another source — so the update below is what keeps a
-- home's own blueprints where they were. The default is 0 because that is the direction a mistake
-- should fall in.
ALTER TABLE blueprints ADD COLUMN trusted INTEGER NOT NULL DEFAULT 0;

UPDATE blueprints SET trusted = 1 WHERE source IN ('builtin', 'captured');
