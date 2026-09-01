-- Why a blueprint is or is not trusted — roadmap task T79b.
--
-- `trusted` stays the answer: decided once, when the row was written, and never re-examined. This
-- is the evidence beside it, so that a file which arrived with no signature at all and one whose
-- signature did not verify stop reaching a person as the same sentence. Only the second is the
-- event the gallery key exists to catch.
--
-- NULL means no check happened — this build's own gallery, this machine's own captures — or that
-- the row predates this column.
ALTER TABLE blueprints ADD COLUMN signature TEXT;

-- The one thing the old schema proves. `blueprint.import` is the only writer of `imported`, and it
-- sets `trusted` from the signature check alone, so a trusted imported row can only have come from
-- a signature that verified. An untrusted one is either of the other two and nothing on disk says
-- which, so it stays NULL and its client keeps the sentence it has always had. Nothing is guessed.
UPDATE blueprints SET signature = 'verified' WHERE source = 'imported' AND trusted = 1;
