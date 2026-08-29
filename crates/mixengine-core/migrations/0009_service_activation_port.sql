-- The port the activator listens on for a service that listens on TCP — roadmap task T70.
--
-- A second port for one service, and a column rather than arithmetic on the first. `port + 1` is the
-- obvious rule and it is a silent collision: with pools on 9000 and 9001, the first pool's activator
-- is the second pool's own port, and what a user sees is one service refusing to bind and a conflict
-- reported about a number nobody chose. So this is allocated by `services::ports` — the same
-- allocator, the same rule, *free means free on the machine and not free in the table* — in the same
-- critical section as the row's own port.
--
-- **Nullable, and null is the ordinary answer.** A service that listens on a Unix socket derives its
-- activator's address from its own instead (`recipes::activator_socket`) and needs no port, and a
-- service with no activation at all needs none either. So this is filled in for exactly the rows that
-- listen on TCP and can be started by a connection, which today is a php-fpm pool on Windows.
--
-- **No backfill.** Every existing row is null, and a row that needs one is given one when it is next
-- allocated for — a port chosen at migration time against a machine's listening table would be a
-- number decided months before anything binds it, which is the one thing T34c's allocator refuses to
-- do.
ALTER TABLE services
    ADD COLUMN activation_port INTEGER;
