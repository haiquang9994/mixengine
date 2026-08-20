-- What each installed PHP can load, and the extensions the user turned round — roadmap task T28.
--
-- Three columns and two different kinds of fact, which is why they are three and not one.
--
-- `extension_dir` and `extensions_json` are **the artifact's own, copied down at install time**, on
-- 0002's argument: the index is a cache with a six-hour life and a network behind it, and whether
-- `redis` can be enabled for a PHP that is on this disk must not depend on either.
--
-- `extension_choices_json` is the user's, and it holds **deviations rather than a set**:
-- `{"xdebug": true, "mongodb": false}`. The effective set is what the build enables, plus what was
-- turned on, minus what was turned off, intersected with what the build ships as loadable. Storing
-- the resulting list instead would freeze 8.3.33's answer and carry it silently onto 8.3.34 — a
-- reinstall or a patch upgrade is supposed to bring the new build's defaults with it and keep only
-- the extensions somebody deliberately touched.
--
-- `*_json` for 0002's other reason: nothing queries into these. One runtime's whole map is read and
-- looked up in memory.
--
-- The defaults are what make this additive. A row from before these columns describes a runtime
-- whose extensions nobody recorded: no directory, nothing offered, nothing chosen — which is
-- exactly what a listing for it should say, and is repaired by reinstalling that version.
ALTER TABLE runtime_installs ADD COLUMN extension_dir          TEXT NOT NULL DEFAULT '';
ALTER TABLE runtime_installs ADD COLUMN extensions_json        TEXT NOT NULL DEFAULT '{}';
ALTER TABLE runtime_installs ADD COLUMN extension_choices_json TEXT NOT NULL DEFAULT '{}';
