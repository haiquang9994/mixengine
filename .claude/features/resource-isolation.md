# Lightweight resource isolation

**Goal**: the laptop stays cool and quiet. MixEngine's whole pitch against Docker is that idle
projects cost nothing — so idle *must* cost nothing.

Rationale for not using containers: [../decisions/0003-no-container-isolation.md](../decisions/0003-no-container-isolation.md).

## Three mechanisms

### 1. On-demand start (the big win)

Nothing but the daemon runs at login. Services start when something actually needs them:

- **Web traffic**: the front-end web server is the only always-on service (it is tiny — Caddy idles
  at a few MB). The site file names *two* upstreams — the pool's own address first, and a second,
  permanent address the daemon holds — so a request to a stopped pool is refused by the first, and
  the front end retries it against the second. That one starts the pool, waits for its `ReadyCheck`,
  and proxies. First hit is slow (~1 s); the rest are normal, and go straight to the pool without
  touching `mixengined` at all.
- **Databases**: started when a site that declares them starts, or on the first connection. There is
  no front end in front of a database to name a fallback in, so here the daemon holds *the
  database's own* address while it is stopped, and gives it back on the start.
- **CLI**: `mix` commands that need a service start it explicitly.

**What holding a database's own address costs, stated plainly.** Between the moment the daemon lets
go of the address and the moment the server binds it — the server's own start time — nothing is
listening there, and a connection arriving inside that interval is refused by the operating system.
**The connection that woke the service is never the one refused**: it is already accepted, and it
waits on the service rather than on the address. Only a *second* client, dialling while the first
one's start is still running, meets the window, and its client will report a refused connection.

The alternative would be for the daemon to keep 3306 for itself for ever and forward every query
through — which would put every byte of every query through `mixengined` for the connection's whole
life, and would make a *running* database unreachable the moment the daemon died. Today a crashed
daemon leaves a working database, and that is not a property worth trading for a startup window.

### 2. Idle shutdown

A service may have an `IdlePolicy { after: Millis, probe: IdleProbe }`, and it is made of two halves
from two places: the **recipe** says how the service is measured, because only it knows which port
its pool listens on; the **row** says for how long, because that is the machine owner's to choose.
Idle is measured by real signals — established connections, and a counter read from a status endpoint
where the service publishes one — never by wall-clock alone. A service with an open connection is
never idle, nor is one something running depends on, nor is one that could not be measured at all.

`services.idle_minutes` has three states, and the third is what makes a later default safe:

| Value | Means |
| --- | --- |
| `NULL` | use the recipe's default |
| `0` | never idle-stop, whatever the recipe says |
| `n` | idle-stop after `n` minutes |

**A recipe offers a default only once something can start its service again**, because a stopped
service with nothing to wake it is a site that answers 502 for ever. Each number therefore arrived
with the task that made its service wakeable:

| Recipe | Default | Since |
| --- | --- | --- |
| php-fpm | 30 min | T70 — the request that finds the pool down is what wakes it |
| MariaDB, MySQL, PostgreSQL | 60 min | T70a — the connection that finds the server down is what wakes it |
| Redis, Memcached | 60 min | T70a |
| Caddy, nginx | never | — the thing that starts everything else back up cannot be the thing that gets stopped |

A database waits longer than a pool on purpose: a pool starts in tens of milliseconds and a server
replays its log first, so an hour is the point at which stopping it is worth the wait to start it.
Any of these is overridden per service — `mix service idle mariadb@main --after 0` never stops it.

**A database is measured by its connections and not by its query counter**, which the counter would
be the better signal for. Reading `Queries` means speaking the database's own protocol as an
authenticated user, and the probe lives on a `ServiceSpec`, which never carries a secret
([ADR 0006](../decisions/0006-servicespec-in-proto-and-secret-free.md)). The error is in the safe
direction: a client connected and idle reads as busy.

Sites can opt out per project ("keep warm") for the one project being worked on all day —
`mix project keep-warm <name>`. It reaches the PHP pool that project's sites name; it does not yet
reach the database they query, because nothing in the schema records which database a project uses.

### 3. Hard limits

`ResourceLimits { cpu_percent: Option<u8>, memory_mb: Option<u32>, priority: Priority }` applied per
service via the platform layer:

| OS | CPU | Memory |
| --- | --- | --- |
| Windows | Job Object `CpuRateControlInformation` (hard cap, % of one core × N) | Job Object `ProcessMemoryLimit` / `JobMemoryLimit` |
| Linux | cgroup v2 `cpu.max` in a per-service scope under the user slice | `memory.max` + `memory.high` |
| macOS | `setpriority`/`taskpolicy` background QoS only — **no hard cap available** | watchdog: warn, then optional restart at threshold |

macOS honesty rule: no client may offer a memory-limit control that does nothing there, which means
the API reports what the platform actually supports rather than a uniform shape.

Defaults are conservative — MariaDB's `innodb_buffer_pool_size` and PHP's `memory_limit` are tuned
down for a dev machine in our config templates, which saves more RAM than any cgroup will.

## Measuring, not guessing

The daemon samples per-process CPU/RSS for each supervised process group — and for `mixengined`
itself, which is half of the footprint below — and keeps a 24-hour history of one row per subject per
minute, so a client can answer "what is eating my battery" and "how much does MixEngine cost when I'm
not using it".

**Two rates, settled at T71.** A reading a second while a client holds `GET /metrics` open, and a
reading a minute when nobody is watching. The second rate is what makes the history worth keeping:
the night nobody was watching is exactly the night the battery question is about. One reading costs
about 10 ms on Windows and about 2 ms on Linux, measured, which is a fiftieth of a percent of one
core at the slow rate.

**The reading is `mixengine-platform`'s `ProcessMetrics`, not `sysinfo` in the daemon** — the same
place every other question about this machine is asked, with a programmable mock beside it, which is
what lets the minute arithmetic and the retention be tested from invented numbers. A group is walked
from its root pid, and a root is identified by pid **and** the moment that process began: a pid the
system handed round would otherwise draw a stranger's memory on a service's chart.

**What `rss_bytes` overstates, said rather than hidden.** Shared pages are counted once per process,
so a php-fpm master and its four workers add up to more than they occupy. There is no cross-platform
way to do better, the error is identical on all three systems, and it is in the safe direction for a
number defended in a README. It is also **not** the quantity a `memory_mb` limit is judged against —
that is commit charge on Windows and charged pages on Linux, per `MemoryMeasure`.

**A minute with no row means nobody measured** — the service was stopped, or the machine was asleep.
It never means nothing was used, and `cpu_percent` is null rather than zero where no figure could be
taken.

Two numbers we publish and defend in the README, enforced by a benchmark in CI:

- **Idle footprint** (daemon + Caddy, nothing else running): target **< 60 MB RSS**, ~0 % CPU.
- **Cold path**: first request to a stopped site served in **< 1.5 s**.

## What we deliberately do not do

- No filesystem or network namespaces: projects share the user's filesystem, which is the point —
  editing files is instant and every tool works normally.
- No per-project database isolation by default: one shared MariaDB with per-project databases. A
  blueprint can request a dedicated instance when a project genuinely needs a different version.
- No attempt to hide processes from Activity Monitor / Task Manager. Every process we start is named
  clearly (`mixengine: php-fpm 8.3`) so users can see exactly what we run.

## Acceptance criteria

- After 30 minutes of inactivity, `ps`/Task Manager shows only `mixengined` and the web server.
- A request to an idle site succeeds (no error page, no manual start) within the cold-path budget.
- Setting a memory limit on Windows/Linux is observably enforced by an integration test that allocates
  past it.
- The CI benchmark fails the build if the idle footprint regresses beyond the budget.
