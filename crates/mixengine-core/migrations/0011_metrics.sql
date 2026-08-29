-- What each subject was costing, one row per minute — roadmap task T71.
--
-- `features/resource-isolation.md` promises a 24-hour downsampled history so a client can answer
-- "what is eating my battery". A row per second would be 86,400 per subject per day; a row per
-- minute is 1,440, and `samples` says how many readings each row is made of — 1 while nobody was
-- watching, up to 60 while somebody held `GET /metrics` open.
--
-- **`subject` is 'daemon' or 'service:' followed by a service id.** `ServiceId::parse` accepts a
-- bare name, so a service may legally be called `daemon`; the prefix is what keeps its history out
-- of the daemon's own, and `:` is not in a service id's alphabet. It is the same spelling the API
-- uses, so a row read back is a subject rather than a parse of one.
--
-- **No foreign key to `services`, deliberately.** A service deleted at two in the morning is still
-- the answer to what happened at two in the morning, and a cascade would delete exactly the evidence
-- somebody came looking for. What bounds this table is the trim, which does not care whether the
-- subject still exists — the one table in this schema whose rows outlive their subject.
--
-- `minute` is epoch milliseconds truncated to the minute: `services.last_started_at`'s rule, a value
-- that exists to be compared and never to be read.
--
-- `cpu_avg` and `cpu_peak` are nullable because a CPU figure is a difference between two readings,
-- and the first reading of a group has nothing to subtract from. NULL is "not measured"; it is never
-- written as a zero, because a zero would draw an idle service during the second it is most
-- expensive.
CREATE TABLE metrics_minutes (
    subject  TEXT    NOT NULL,
    minute   INTEGER NOT NULL,
    cpu_avg  REAL,
    cpu_peak REAL,
    rss_avg  INTEGER NOT NULL,
    rss_peak INTEGER NOT NULL,
    samples  INTEGER NOT NULL CHECK (samples > 0),
    PRIMARY KEY (subject, minute)
) WITHOUT ROWID;

-- Both readers go through it in time order: the trim deletes a prefix, and a history read takes a
-- window. The primary key orders by subject first, which is what "this service's day" wants and not
-- what either of those does.
CREATE INDEX metrics_minutes_minute ON metrics_minutes (minute);
