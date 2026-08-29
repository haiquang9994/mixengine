-- Whether this service is stopped because nothing was using it — roadmap task T70.
--
-- On-demand activation may start a service back up when a connection arrives, and must not do it to
-- a service a person stopped: `mix service stop mariadb@main` followed by the next connection
-- restarting it is the tool overruling its user.
--
-- **A column and not a fact the daemon remembers**, which is the whole reason it exists. T69 put the
-- reason on the transition rather than inventing an event for it, and a transition is not stored — so
-- a daemon that restarts would forget which of its stopped services it had stopped itself. What that
-- costs is precisely the case activation is for: every pool idle-stopped before the restart would
-- stay stopped, and every site through it would answer 502 for ever.
--
-- Written on every transition into `stopped`, so it is never stale: 1 when the reason was
-- `StateReason::Idle` and 0 for every other reason. It says nothing at all about a service that is
-- running, and nothing reads it then.
ALTER TABLE services
    ADD COLUMN idle_stopped INTEGER NOT NULL DEFAULT 0 CHECK (idle_stopped IN (0, 1));
