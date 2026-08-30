-- Whether this site is reachable from the local network, and where — roadmap task T74.
--
-- **Three columns on `sites` rather than a table of its own.** Sharing is at most one row per site,
-- it has no history worth keeping, and every reader that wants it — `site.show`, config generation,
-- the certificate — is already reading the site. A table beside this one would buy a join on every
-- one of those reads to model a relationship that cannot be one-to-many.
--
-- **Nullable together, and that is the state.** All three are NULL for a site that is not shared and
-- all three are set for one that is; there is no boolean beside them, because a boolean and an
-- address can disagree and the address is the thing every consumer actually needs. What enforces it
-- is the CHECK below rather than a convention in the code that writes them.
--
-- `shared_address` is IPv4 as text (T74, D4) — the address of the interface named, not `0.0.0.0`:
-- it is bound, it goes in the certificate as an IP SAN, and it is what the URL and the QR code say,
-- so one column keeps those three from ever disagreeing.
--
-- `shared_since` is milliseconds since the Unix epoch, a `mixengine_proto::Timestamp`, exactly as
-- every other `_at`/`_since` column in this schema. T76 adds `shared_until` beside it, which is why
-- this is stored rather than derived: an expiry is a comparison against a start nobody can
-- reconstruct after a restart.
ALTER TABLE sites ADD COLUMN shared_interface TEXT;

ALTER TABLE sites ADD COLUMN shared_address TEXT;

ALTER TABLE sites ADD COLUMN shared_since INTEGER;

-- SQLite cannot add a table-level CHECK to an existing table, and rebuilding `sites` to gain one
-- would be the third rebuild of it (0006 was the second). A trigger holds the same invariant at the
-- same moment a constraint would, and names it in the same words.
CREATE TRIGGER sites_sharing_is_all_or_nothing_insert
BEFORE INSERT ON sites
FOR EACH ROW
WHEN (NEW.shared_interface IS NULL) <> (NEW.shared_address IS NULL)
  OR (NEW.shared_interface IS NULL) <> (NEW.shared_since IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'a shared site carries an interface, an address and a start, or none of the three');
END;

CREATE TRIGGER sites_sharing_is_all_or_nothing_update
BEFORE UPDATE ON sites
FOR EACH ROW
WHEN (NEW.shared_interface IS NULL) <> (NEW.shared_address IS NULL)
  OR (NEW.shared_interface IS NULL) <> (NEW.shared_since IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'a shared site carries an interface, an address and a start, or none of the three');
END;
