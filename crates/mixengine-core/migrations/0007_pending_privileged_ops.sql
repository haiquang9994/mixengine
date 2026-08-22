-- The queue of privileged operations waiting for one prompt — roadmap task T40b.
--
-- **Durable, and that is the decision rather than the default.** T64 says `mix status` keeps showing
-- the pending list "until it is granted or dropped", and a restart is neither of those. The daemon
-- restarts on update, on a crash and on every `mix daemon shutdown`; a machine that still lacks its
-- hosts entries after one of those should still say so. An in-memory queue would report a healthy
-- machine while the user's site stayed unreachable, which is the exact failure a degraded mode
-- exists to prevent.
--
-- **`dedupe_key` is where "no code path elevates in a loop" stops being a matter of discipline.**
-- `.claude/decisions/0005-on-demand-elevation.md` calls elevating inside a loop a defect. A producer
-- that enqueues the same operation on every start, on every `site.create`, or inside a retry writes
-- one row — enforced by the index below, so no caller has to remember it and no reviewer has to
-- check for it. The runtime half of the same rule is the daemon's one-grant-at-a-time slot.
--
-- **Why two columns hold what is, today, the same bytes.** `op` is the operation as it was asked
-- for; `dedupe_key` is its canonical form, and canonical is not the same as serialised the moment an
-- operation carries a set — T41's `HostsApply` is a list of domains whose *order* must not make two
-- requests for the same change into two rows. Today the two are equal and
-- `mixengine_core::elevation::canonical` is one call to serde; the column is separate so that T41 is
-- an edit to that function rather than a migration.
CREATE TABLE pending_privileged_ops (
    id           INTEGER PRIMARY KEY,
    op           TEXT    NOT NULL,
    dedupe_key   TEXT    NOT NULL UNIQUE,
    -- Milliseconds since the epoch, as everywhere else in this schema. The **first** time this
    -- operation was asked for: a conflicting insert leaves this row exactly as it is, so "pending
    -- since" reads honestly rather than resetting every time a producer retries.
    requested_at INTEGER NOT NULL
) STRICT;
