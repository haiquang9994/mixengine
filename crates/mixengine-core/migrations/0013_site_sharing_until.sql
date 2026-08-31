-- When a share ends without anybody ending it — roadmap task T76.
--
-- **Nullable on its own, unlike T74's three.** A share with no expiry is the ordinary case, so this
-- column is not part of the all-or-nothing rule `0012_site_sharing.sql` states: folding it in would
-- make `--for` mandatory. What it *is* part of is the other half of that rule — a site that is not
-- shared carries no deadline either, because a deadline for a share that does not exist is a value
-- nothing will ever read and something will eventually believe.
--
-- Milliseconds since the Unix epoch, a `mixengine_proto::Timestamp`, like `shared_since` beside it.
-- Stored as the instant it lands on rather than as the length that was asked for: an expiry has to
-- survive a restart, and a duration without the start it was measured from is not an expiry.
ALTER TABLE sites ADD COLUMN shared_until INTEGER;

-- SQLite has no ALTER TRIGGER, so the pair from 0012 is dropped and written again with the fourth
-- column in it. The two conditions are one sentence from both ends: the three T74 columns move
-- together, and this one may only be set when they are.
DROP TRIGGER sites_sharing_is_all_or_nothing_insert;

DROP TRIGGER sites_sharing_is_all_or_nothing_update;

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
