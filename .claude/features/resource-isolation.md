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
| macOS | `setpriority`/`taskpolicy` background QoS only — **no hard cap available** | watchdog (T71a): warn, then restart where the recipe permits |

macOS honesty rule: no client may offer a memory-limit control that does nothing there, which means
the API reports what the platform actually supports rather than a uniform shape. Since T71a the
memory control *does* do something there, and says so as `Enforcement::Advisory` — watched, not
capped — which is a fourth answer rather than a promotion of the old one.


### The watchdog, where nothing caps memory

**Built at T71a.** A `memory_mb` on macOS — and on a Linux session that was never delegated the
`memory` controller — used to be a number the daemon stored and nothing read. It is now compared,
every minute, against the readings the metrics sampler already takes:

- **Where it arms**: wherever `LimitSupport::memory` is not `Hard`. The daemon asks the machine, never
  the operating system's name, so a Linux home without cgroup delegation gets the same protection
  macOS does.
- **What is judged**: the finished minute's `rss_avg` against the declared ceiling. Not the peak — a
  service twice its usual size for five seconds is a service doing its work. `MemoryMeasure::Resident`
  is what says on the wire that RSS is the quantity here, and that it overstates shared pages.
- **What follows**: over the line, the service goes `Degraded` with `StateReason::OverMemory`. Over it
  for **three consecutive finished minutes**, the service is restarted — *if its recipe permits*.
  php-fpm pools do; databases and caches do not, because a restart mid-transaction is a data question
  and restarting a cache deletes what somebody believes is still there. The person's control is
  `memory_mb` itself: nothing watches a service that declared no ceiling.
- **One restart per episode.** After a restart the service must be seen *under* the line once before
  it may be restarted again. A pool that leaks up to its ceiling every twenty minutes is rescued
  every twenty minutes; a ceiling set below what the service needs at boot costs exactly one restart
  and then a service left alone in `Degraded`, because that is a misconfiguration rather than a leak.
- **A missing minute means nobody measured**, and resets the count. A laptop that slept eight hours
  wakes with no evidence, not with eight hours of it.

**One honest wrinkle.** With nobody watching, a finished minute holds a single reading, so the
average is that reading; with a client holding `GET /metrics` open it holds sixty, and the average
genuinely smooths. The watchdog is therefore slightly more sensitive to a spike when nobody is
looking. What protects against a transient is the three consecutive minutes, and that rule is the
same at either rate.

`service.limits` reports it per service — `watchdog: { after_minutes, restarts }`, and `null` where
nothing is watching — because whether *this* service would be restarted is its recipe's answer, and
`LimitSupport` describes a machine.

### Defaults tuned for a laptop, which save more RAM than any cgroup will

**Settled at T73**, and measured rather than claimed — the paragraph that used to stand here promised
tuning the database templates had not had.

What the generated configuration now says, and none of it is a knob a recipe offers: every line is
stated in the template with the reason beside it, and `extra` renders last for anybody who wants
otherwise.

| Server | Tuned to | The server's own | Why it is memory a laptop pays for |
| --- | --- | --- | --- |
| MariaDB, MySQL | `innodb_buffer_pool_size 64M` | `128M` | allocated at startup, held with nobody connected |
| MariaDB, MySQL | `key_buffer_size 16M` | `128M` | MyISAM's key cache, and a modern application has no MyISAM tables |
| MySQL | `performance_schema OFF` | `ON` | preallocated instrumentation tables nothing here reads; MariaDB already ships it off |
| PostgreSQL | `shared_buffers 32MB` | `128MB` | one shared segment taken at startup |

And one thing that is not memory: **the log is flushed once a second rather than at every commit** —
`innodb_flush_log_at_trx_commit = 2`, `synchronous_commit = off`. A power cut costs the last second
of committed transactions and **cannot corrupt anything**: `fsync`, `full_page_writes` and
`innodb_doublewrite` are deliberately untouched, and each recipe has a test asserting they stay that
way. What it buys is the migration and the seed — thousands of tiny transactions that otherwise each
wait for a disk.

**Two knobs were examined and deliberately left alone.** `max_connections` allocates per *actual*
connection, so lowering it saves nothing on an idle machine and turns a busy afternoon into an error
that arrives from MixEngine. And php-fpm's `pm.max_children` is already worth nothing at idle,
because an idle pool is stopped outright — shrinking it would only slow down the machine while
somebody is using it.

**The saving is measured, and gated.** `crates/mixengine-cli/tests/tuned_footprint.rs` runs two
MariaDB instances in one home — one on these defaults, one put back to the server's own — and holds
the *difference* between them to a floor in the `bench` job. Measured on MariaDB 10.11: **77.2 MB
against 98.7 MB, a saving of 21.6 MB — 21.8 % of what the server's own values held**, for one
database on one machine doing nothing.

The gate is a **fraction** (five per cent) rather than a number of megabytes, and both halves of that
are deliberate. A budget on MariaDB's own RSS would be a promise held hostage to next month's
MariaDB, a number this project does not control — the same reasoning that has the idle footprint
gate `mixengined` and merely report the total. And a floor at a quarter of what was measured leaves
room for another series and another operating system to allocate differently, while still being ten
times the 0.4 % that two *identical* servers were measured apart.

PHP's ini set was tuned at T28 and is unchanged here: `memory_limit = 512M`, `upload_max_filesize =
128M`, `display_errors = On`, and an opcache that revalidates every request.

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

Two numbers we publish and defend in the README:

- **Idle footprint** (daemon + Caddy, nothing else running): target **< 60 MB RSS**, ~0 % CPU.
  **Measured by the `bench` job on all three systems since T72, and reported rather than enforced** —
  57 MB on Windows, 67 MB on Linux, 69 MB on macOS, as the median of five readings taken thirty
  seconds after the last command through the daemon's own `metrics.snapshot`.
  **What is enforced is `mixengined` alone, under 36 MB** — measured at 21 MB on Windows, 25 MB on
  Linux and 30 MB on macOS — because the split is roughly a third daemon to two thirds Caddy: most of
  the published number belongs to a Go program this project neither wrote nor tunes, and a gate on
  the total would go red for a reason no commit here could fix. The daemon is the half that regresses
  when this code grows, and it is the half a budget can defend.
- **Cold path**: first request to a stopped site served in **< 1.5 s**. **Measured and enforced by
  the `bench` job on all three systems since T72a**, three rounds per run against three PHP versions
  — measured in release at **108 ms on Linux, 129 ms on macOS and 574 ms on Windows**, as the median
  of three rounds. Windows is five times the others because a pool there is `php-cgi.exe` and process
  creation is what a cold path mostly is; it is still well inside the budget.
  **What had been missing was not the activator** — T70 gives a pool on a socket one, derived beside
  the pool's own path, and both site templates have rendered it as the second upstream since then.
  It was the *probe*: `IdleProbe` could only count connections to a port, a pool on a socket has
  none, and a service with no probe is never idle-stopped however its row is set. So there was no
  stopped site for a first request to arrive at. T72a asks php-fpm about itself over FastCGI
  instead — `pm.status_path`, on the pool's own socket, on every PHP from 7.0 upwards.

  **A pool is idle when no worker is serving**, and deliberately not when a connection counter has
  stopped moving: the daemon's own health check is a connection every ten seconds, so that counter is
  mostly the daemon's own footprints. What that costs is that a site used in short bursts can be
  stopped between two readings and pay one cold path; what it never costs is a request in flight.

**What the CI reading is, and is not.** The daemon it measures has just installed a package, rendered
configuration and walked a start plan, so its RSS carries the high-water mark of all of it — a real
machine idle for an afternoon holds less. The number is therefore *worse* than the promise is about,
which is why it is usable: passing at 60 MB there means passing comfortably here. Restarting the
daemon and re-adopting the web server would be closer and cannot be done on two of three systems —
[ADR 0007](../decisions/0007-supervised-child-owns-a-process-group.md): a daemon leaving takes its
whole job down on Windows and its immediate children on Linux.

## What we deliberately do not do

- No filesystem or network namespaces: projects share the user's filesystem, which is the point —
  editing files is instant and every tool works normally.
- No per-project database isolation by default: one shared MariaDB with per-project databases. A
  blueprint can request a dedicated instance when a project genuinely needs a different version.
- No attempt to hide processes from Activity Monitor / Task Manager. Every process we start is named
  clearly (`mixengine: php-fpm 8.3`) so users can see exactly what we run.

## Acceptance criteria

- After 30 minutes of inactivity, `ps`/Task Manager shows only `mixengined` and the web server.
- A request to an idle site succeeds (no error page, no manual start) within the cold-path budget —
  `cold_path.rs` in the `bench` job, on all three systems, since **T72a**.
- Setting a memory limit on Windows/Linux is observably enforced by an integration test that allocates
  past it.
- The CI benchmark fails the build if the idle footprint regresses beyond the budget — `bench`, on
  all three systems, since **T72**; and if the cold path does, since **T72a**.
