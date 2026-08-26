# Lightweight resource isolation

**Goal**: the laptop stays cool and quiet. MixEngine's whole pitch against Docker is that idle
projects cost nothing — so idle *must* cost nothing.

Rationale for not using containers: [../decisions/0003-no-container-isolation.md](../decisions/0003-no-container-isolation.md).

## Three mechanisms

### 1. On-demand start (the big win)

Nothing but the daemon runs at login. Services start when something actually needs them:

- **Web traffic**: the front-end web server is the only always-on service (it is tiny — Caddy idles
  at a few MB). A request to a site whose php-fpm pool is stopped hits a small `mixengined` gateway
  handler, which starts the pool, waits for its `ReadyCheck`, and then proxies the request. First hit
  is slow (~1 s); the rest are normal.
- **Databases**: started when a site that declares them starts, or on the first TCP connection via
  the same activation trick (a listener the daemon holds until the real service is up).
- **CLI**: `mix` commands that need a service start it explicitly.

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

**Every recipe currently offers no default**, so nothing is idle-stopped unless somebody asks for it
per service (`mix service idle mariadb@main --after 60m`). That is deliberate and lasts until
on-demand activation exists: a stopped pool with nothing to start it again is a site that answers
502. The intended defaults, once it does, are php-fpm 30 min, databases 60 min, caches 60 min, web
server never.

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

The daemon samples per-process CPU/RSS (`sysinfo`) for each supervised process group once a second
while anyone is subscribed, and keeps a 24-hour downsampled history so a client can answer "what is
eating my battery" and "how much does MixEngine cost when I'm not using it".

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
