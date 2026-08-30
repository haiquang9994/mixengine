# Phase 7 — Efficiency

*Goal: deliver the promise that idle costs nothing.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [x] **T68** `ResourceLimits` per OS: Job Objects, cgroup v2, macOS QoS; the API reports only
      what the platform really supports, so no client can offer a control that does nothing. **(P)**
      Design: [2026-08-26-t68-resource-limits-design.md](../../docs/superpowers/specs/2026-08-26-t68-resource-limits-design.md).
      **The macOS watchdog came out of this task and became T71a**, below: warning and restarting at a
      threshold needs a per-process RSS sample taken repeatedly, and that sampler is T71. Building a
      second one here to serve one field on one operating system would put a loop in the supervisor
      that T71 would then replace.
      **What it does not prove, and who owes it.** Memory is proved by *outcome* — a `fakeservice`
      given 32 MB a bite and a 128 MB ceiling leaves `running` before it reaches 256 MB, and the
      suite was run once with the ceiling removed to check it can fail. **CPU is proved only by
      reading the value back out of the mechanism it was written into**, because a cap is a rate and
      asserting a rate means timing a busy loop on a shared runner. That a CPU cap *slows anything
      down* is still nobody's: **T72** settled on fixed budgets rather than a comparison against
      master, on the reasoning the other two guards in that job already follow, so a CPU cap slowing
      something down is not a number anything measures yet.
      **Two things this task found that the design did not predict.** `SetInformationJobObject`
      refuses any `JobObjectCpuRateControlInformation` whose `ControlFlags` is `0` — measured, three
      shapes tried — so on Windows there is **no way to put a job back to having no rate control**
      once it has had some; removing a CPU cap writes `ENABLE` without `HARD_CAP` at a hundred per
      cent of the whole machine, which is the nearest true statement. And `cpu_percent` is a `u8`, so
      the "no more than `100 × cores`" refusal is **unreachable on any machine with three cores or
      more**: it guards a one-core VM and nothing else, which is written beside the check so nobody
      reads it as more than it is.
      **Where the honesty is enforced rather than asserted**: `LimitSupport` answers per *field*,
      because systemd delegates `memory` far more readily than `cpu` and a single flag could only
      describe that by lying about one of them — and `Unsupported` (this system never will) is a
      different variant from `Unavailable` (this machine currently will not), because they are
      different advice. `mix doctor` prints the second and deliberately says nothing about the first.
- [x] **T69** Idle detection (connections, request counters, query counters) and `IdlePolicy`
      shutdown, with per-project "keep warm". **(P)**
      Design: [2026-08-26-t69-idle-detection-design.md](../../docs/superpowers/specs/2026-08-26-t69-idle-detection-design.md).
      **It ships switched off, and that is the task's largest decision.** Stopping a pool is only
      safe once something starts it again on the next request, and that is **T70**; so every recipe
      answers `None` to `Recipe::idle_default`, and a home that changes nothing behaves exactly as it
      did. What T70 spends to turn it on is four `None`s.
      **Which is why `services.idle_minutes` has three states rather than two, today.** `NULL` is
      *use the recipe's default*, `0` is *never, whatever the recipe says*, and `n` is minutes. Two
      states would have been enough for a build where nothing is reachable — and would have left T70
      unable to tell a home that never touched the setting from one whose owner switched it off,
      with every existing row holding `NULL` and no migration able to guess which was which. The
      distinction costs nothing while both are unreachable and cannot be added afterwards at any
      price.
      **Three things this task deliberately did not do.** *Keep-warm reaches a project's PHP pool
      and not its database*: `sites.php_service_id` is the only edge the schema has between a project
      and a service, and a `project_services` table would be a second description of a relationship
      `sites` already half-holds. **T77** is where a project declares what it needs, and
      `projects::kept_warm` widens there rather than being rewritten — asserted in both directions
      by its own test, so the day it changes, a test says so. *A php-fpm pool on a Unix socket is
      never idle-stopped*, because `IdleProbe` counts TCP and such a pool has no port; left
      unmeasured rather than measured wrongly. And *no query counter*, below.
      **The roadmap line above asked for one thing that cannot be built as written**, and this is
      the place to record it rather than leave the next reader to rediscover it: a database's query
      counter lives behind `SHOW GLOBAL STATUS` or `pg_stat_database`, which means speaking the
      database's protocol as an authenticated user — and the probe lives on a `ServiceSpec`, which
      never carries a secret ([ADR 0006](../decisions/0006-servicespec-in-proto-and-secret-free.md)
      says of that type "there is no field a password fits into"). So `IdleProbe` stays at two arms
      and a database is measured by its established connections. The error is in the safe direction:
      a client connected and idle reads as busy, so a database is kept running that could have been
      stopped and one somebody is holding open is never stopped.
      **What a failing test found that the design did not**: a fifth `IdleSource`. A duration asked
      for that produces no policy — the pool-on-a-socket case above — read as "never", which is the
      outcome without the reason and an invitation to type `--after 30m` again. `IdleSource::of`
      therefore takes the assembled policy as well as the two halves it is made of, and answers
      `Unmeasurable`.
      **The reading lives in `mixengine-supervisor`, beside `ready` and `health`.** Three questions
      about one running process — *can I route traffic to it*, *is it still fine*, *is anybody using
      it* — and the third needs the HTTP client the other two already have. Reading it from the
      daemon would have been the second HTTP stack that crate's `Cargo.toml` argues against by name.
      The daemon keeps what the supervisor cannot know: the policy, the dependency graph, keep-warm.
      **One safety property is worth naming because every layer repeats it.** An unmeasurable service
      is never stopped — `lsof` missing, `/proc/net/tcp` unreadable, a status endpoint refusing —
      because reading *I could not measure* as *there is nothing to measure* stops a database
      somebody is using. `Observation::Unmeasurable` exists so the two cannot be one arm, and the
      sweeper resets its count on it rather than advancing. The same rule skips a whole sweep when
      the keep-warm table cannot be read.
- [x] **T70** On-demand activation, the web path: a stopped php-fpm pool is started by the request
      that needed it, and the front end is what notices. **(P)**
      Design: [2026-08-29-t70-on-demand-activation-design.md](../../docs/superpowers/specs/2026-08-29-t70-on-demand-activation-design.md),
      whose D1, D2, D3 and D5 through D9 are this task's; D4 is **T70a**'s.
      **The roadmap line this was split from said "hold the socket", and for a pool that cannot be
      done.** To let php-fpm bind its own socket the daemon has to close its listener and unlink the
      file first, and php-fpm binds it several hundred milliseconds later — every request arriving in
      between is refused by the kernel. The first request is served, which is the promise; the second
      one, for the same page's next asset, is a 502. So the pool keeps its own address, the activator
      binds a second one derived from it, and the site file names both.
      **The rendering was measured before any of it was written, and the measurement refuted two
      candidates in opposite directions.** Against a real Caddy 2.10.0 the bare two-address form
      answers **8 of 20** — Caddy treats the pair as peers and load-balances between them, so half of
      a *healthy* site's traffic would cross the activator. `lb_policy first` plus a retry budget and
      no passive health check is worse than having no fallback at all: **0 of 20**, each burning the
      full 5 s, because nothing ever marks the refusing pool unavailable and `first` keeps choosing
      it. All three of `lb_policy first`, `lb_try_duration` and `fail_duration` answer **20 of 20**,
      first request 55.8 ms and the rest ~1.5 ms. nginx needs one directive (`backup` plus
      `fastcgi_next_upstream`) for the same thing. `fail_duration` was then measured to be exactly
      how long a *recovered* pool is still reached the slow way, which makes it a number to justify
      rather than default.
      **Two things the implementation found that the design had not.** A TCP activator's port cannot
      be derived *and* the rule "every port a row holds is taken" has to read both columns, or two
      services are handed one address — so `ports::allocate_activation` is a second allocator and not
      a `+ 1`. And **D8 was unanswerable as designed**: it asks whether the daemon idled the service,
      the reason lives on a transition, and a transition is not stored — so a daemon restart would
      forget, and every pool idled before it would stay stopped with its site answering 502 for ever.
      Migration `0010` adds `services.idle_stopped`, written on every arrival at `stopped`.
      **What is not automated, stated rather than implied**: no test drives a real front end through
      a real stopped pool. The two renderings were accepted by a real Caddy and a real nginx and the
      retry behaviour was measured against both, and the activator's own halves are covered — but the
      two ends meeting is left to the suites that run a real pool, and until one of them asserts it
      the gap is real.
- [x] **T70a** On-demand activation, the database path: a stopped MariaDB, PostgreSQL, Redis or
      Memcached is started by the connection that needed it. **(P)**
      Design: [2026-08-29-t70-on-demand-activation-design.md](../../docs/superpowers/specs/2026-08-29-t70-on-demand-activation-design.md),
      D4 — on T70's mechanism, which is protocol-blind and therefore already suits a client that
      waits to be greeted rather than speaking first.
      **Split out of T70 because it is the half where "hold the socket" is still the answer**, not
      because it is optional: **M7 is unreachable without it**, since a database that idle-stops and
      never comes back moves the broken case rather than fixing it. There is no front end in front of
      a database to name a fallback in, so here the daemon does bind the service's own address while
      it is stopped and releases it on the start — accepting the refusal window T70 refused, because
      the alternatives are worse. Always proxying would put every query's bytes through the daemon
      for the connection's life and would make a *running* database unreachable when the daemon dies,
      which is a property no startup window is worth.
      **Ordered immediately after T70 and before T71** — it shares T70's activator and adds a second
      caller, so landing it late would mean `idle_default` staying `None` for four of the six recipes
      that were the point of turning it on.
      **Three things the implementation found that the design had not.**
      **D4 says "the service's own address" in the singular, and on a Unix system it is two.** A
      MariaDB, a MySQL or a PostgreSQL answers on a port *and* on a socket in `run/`, and which one
      a client uses is that client's habit rather than a setting — a generated `.env` names the
      port, the client typed with no host at all names the socket. Holding only the port would have
      left the second client dialling an address nothing holds, so `Recipe::held_while_stopped`
      returns a *set*. Redis and Memcached return one address, which is the whole of what they
      listen on and is why their module docs say so.
      **Giving a Unix socket back is not the same as closing it.** A `UnixListener` does not unlink
      its path when it is dropped, and a server told to bind a path that already exists reports that
      it exists rather than taking it — so `Activation::release` unlinks, and its test binds the way
      a `mariadbd` binds rather than the way `Activation::bind` does. That distinction is not
      pedantry: the first version of that test used `Activation::bind`, which clears a stale socket
      file before binding, and therefore passed with the unlink removed.
      **The design's one `mix doctor` addition needed no new check.** It asks for a site whose pool
      is idle-stopped and whose front end names no fallback upstream to be reported; such a home is
      one whose installed site files differ from what its rows render to, which is exactly what
      `Doctor::generated_config` already reports, with the same remedy. Adding a second check would
      have broken that file's own rule — *"Two implementations of one question are two answers to
      it."*
      **What is not automated, stated rather than implied**: no test drives a real `mysql` client
      through a real stopped MariaDB. The splice is covered in both conversational orders (T70), the
      hold, the release and the release-before-spawn ordering are covered here, and each recipe's
      addresses are covered — but the two ends meeting is left to whichever suite runs a real
      database, and it is the same gap T70 recorded rather than a second one.
- [x] **T71** Metrics history: 1 s sampling while subscribed, 24-hour downsampled retention.
      Design: [2026-08-30-t71-metrics-history-design.md](../../docs/superpowers/specs/2026-08-30-t71-metrics-history-design.md).
      **The line above and `features/client-surface.md` could not both be kept, and that was the
      task's first decision.** *"Sampled only while watched"* cannot produce a history worth having:
      *what was eating my battery last night* is a question about a night nobody was watching, so a
      history kept only while somebody looked would hold exactly the minutes that needed no
      recording — and **T71a** would be a memory watchdog that watches memory only while a client is
      open. So there are two rates in one loop: a reading a second while `GET /metrics` is held open,
      and a reading a minute when it is not. The feature document was corrected rather than
      satisfied.
      **The cost was measured before the slow rate was fixed**, because these documents criticise
      polling a sleeping laptop by name and a default that spends it is owed a number rather than an
      argument. One refresh of the process table: **about 10 ms on Windows 11** with 276 processes,
      **about 2 ms under WSL Ubuntu 24.04** — the whole machine, not per group, since the parent map
      has to be built before any group can be walked. Once a minute is 0.02% of one core; the
      one-second rate is about 1%, is spent only while somebody is looking, and is visible in
      `mix metrics --watch` as the daemon's own CPU figure.
      **One loop and not two**, because a slow loop for the history beside a fast one for the stream
      would measure the same processes at two different moments and hand a client two answers to one
      question. The rate changes on a `watch` channel the loop holds *before* it sleeps, not on a
      flag read at the top of each iteration: a receiver made at the moment of waiting counts the
      current state as already seen, so a client that opened the stream a moment earlier would wait
      out a whole sixty-second sleep for its first frame.
      **`DaemonEvent::MetricsSample` was declared in `daemon-and-ipc.md` and is now removed**, on
      [ADR 0009](../decisions/0009-logs-travel-on-their-own-stream.md)'s argument plus one of its
      own — and **no new ADR**, because a second record making 0009's case again about metrics would
      be two descriptions of one decision. The argument of its own: an event stream cannot tell a
      client watching metrics from one listening for state, so switching sampling on and off would
      need a subscribe/unsubscribe pair, and a client that crashed without the second call would
      leave the machine measured every second for as long as the daemon ran. A socket cannot forget
      to close.
      **Three things the implementation found that the design had not.** `ServiceId::parse` accepts a
      bare name, so `daemon` is a legal service id — the subject column and the wire spelling are
      therefore `daemon` or `service:<id>`, and `:` is not in a service id's alphabet, which makes
      the two spaces disjoint by the same rule that validates the ids. `#[non_exhaustive]` on the new
      types would have made them unconstructible outside `mixengine-proto`, which is why every
      neighbour that a daemon *builds* is a plain struct and only the enums carry it. And
      `metrics.snapshot` had to be answered **by the sampling loop** rather than by a reader of its
      own: a CPU figure is a difference against the previous refresh, so two callers refreshing
      independently would each measure the interval since the other.
      **What is deliberately not measured.** Windows' Job Objects already account for CPU time and
      peak memory per job, exactly and without a pid walk — refused, because it would measure one of
      the three systems by a different mechanism at the moment **T72** is about to hold all three to
      one threshold. The pid walk overstates shared pages identically everywhere, which is the safe
      direction for a number defended in a README, and is named as an overestimate in the type's own
      documentation rather than left to be discovered.
      **What this task does not do, and who owns it**: the macOS memory watchdog is **T71a**, which
      reads this sampler and compares; the CI budget that gates the number is **T72**. No client in
      this repository draws a chart — `mix metrics --since` prints the rows, and the ages rather than
      clock times, because this workspace still has no civil-calendar dependency.
- [x] **T71a** The macOS memory watchdog: warn at a `memory_mb` it cannot enforce, and restart at a
      threshold when the service asks to be. **Split out of T68**, and ordered here rather than there
      because it is the one part of `ResourceLimits` that is not a call on a kernel object — macOS has
      no hard memory cap, so the limit becomes a reading taken repeatedly and compared, which is
      T71's sampler and nothing else. **(P)**
      Design: [2026-08-30-t71a-macos-memory-watchdog-design.md](../../docs/superpowers/specs/2026-08-30-t71a-macos-memory-watchdog-design.md).
      **It is not macOS-only, and that was the first decision.** The daemon arms the watchdog
      wherever `LimitSupport::memory` is not `Hard`, which is `CLAUDE.md`'s rule about asking the
      platform rather than the operating system's name — and it turns out to buy something: a Linux
      session that was never delegated the `memory` controller had exactly the same dead number, and
      now has the same protection. `Enforcement::Advisory { why }` is the fourth variant that says
      so, and its `Option` carries the distinction T68 spent two variants on — `Some` is a machine
      somebody could start differently and the line `mix doctor` prints, `None` is an operating
      system with nothing to fix.
      **Two things the runner turned out to require, neither of them in the design until the code was
      read.** The health loop sits behind `let Some(watching) = health.as_mut() else { continue; }`,
      so a service whose recipe declares no `HealthCheck` never reaches the transitions at all — a
      fold living inside that branch would have watched such a service's ceiling and never said a
      word. And `ServiceState::can_become` has no self-loops, so a second move to `Degraded` is an
      `IllegalTransition` that `record` logs an `error!` for: once a minute, for as long as a service
      stayed over. Only a change of *state* is written, which costs a reason that can lag — a service
      that recovers its health while still over its ceiling goes on reading `unhealthy` — and the
      alternative was publishing a `Running` it never was in order to correct one word.
      **The design's own weak point, found while writing it rather than after.** The judged quantity
      is the finished minute's `rss_avg`, argued for as smoothing out transients — and at the idle
      sample rate a minute holds exactly one reading, so it smooths nothing and the watchdog is
      slightly more sensitive when nobody is watching than when somebody is. What actually carries
      the argument is the three consecutive minutes, which is the same rule at either rate. Written
      down in the spec's D3 and in `features/resource-isolation.md` rather than quietly left.
      **What is deliberately not here.** No CPU watchdog: a rate has no equivalent of *holding too
      much*, since a service at 100% for three minutes may be doing exactly what was asked of it, so
      `cpu` still answers `Unsupported` on macOS and means it. No column and no history of warnings —
      the counts live in the task, as `idle::Tally`'s do, and a daemon that restarts forgets, which
      is correct. And no user override of the recipe's permission: nothing is persisted, so the day
      somebody wants one it arrives as a column whose `NULL` means *what the recipe says*, which is
      the three-state shape T69 had to buy in advance and this gets for nothing.
- [x] **T72** CI budgets: idle footprint < 60 MB RSS — failing the build on regression. **(P)**
      Design: [2026-08-30-t72-ci-budgets-design.md](../../docs/superpowers/specs/2026-08-30-t72-ci-budgets-design.md).
      **The cold path is not in it, and that is the task's largest finding.** The number was to be a
      real `GET` through Caddy to an idle-stopped php-fpm pool — and on Linux and macOS such a pool
      listens on a *Unix socket*, so `activation_port_needed` and `activator` both answer nothing and
      `held_while_stopped` is the trait's empty default. There is no stopped site on two of three
      systems for a first request to arrive at; T69 had already written down the other half of the
      same fact — *"a php-fpm pool on a Unix socket is never idle-stopped"* — without drawing out
      what it costs the published promise. Split into **T72a**, below. Measuring it on Windows alone
      was refused: it would gate a cross-platform promise on one system, which is what this task's
      own design argued against when it settled on a single 60 MB for all three.
      **Two defects in T71's sampler, found by pointing it at a number somebody had argued for.**
      The first: `sysinfo` lists **threads alongside processes** on Linux, each carrying its
      process's parent pid and its process's whole resident size — so a group was counted once per
      thread and its memory multiplied by the thread count. A single-binary Caddy read as 445 MB and
      the first measurement of this budget came out at **1558 MB**. The second, underneath it: every
      supervised service is a child of the daemon, so the daemon's row was *the daemon and everything
      it runs* — the largest consumer on every chart, and a set of rows that counted each service
      twice when summed. A group now stops where another group begins, which is what makes the rows
      disjoint and the sum meaningful. 1558 MB → 132 MB → **90 MB**, debug.
      **The second of those would have been invisible without this task, and the first was worse than
      cosmetic**: T71a's memory watchdog had shipped two commits earlier, and on a Linux machine
      without cgroup delegation — exactly the machine T71a widened itself to protect — a pool with
      ten threads would have read ten times its size and been restarted for a ceiling it was nowhere
      near.
      **What the measurement honestly is.** A daemon that has just installed, rendered and started
      something, read thirty seconds later — not one idle for an afternoon, whose allocator has given
      more back. The CI number is therefore *worse* than the promise is about, which is the safe
      direction for a gate. Restarting the daemon and re-adopting the web server would be closer and
      cannot be done on two of three systems, per
      [ADR 0007](../decisions/0007-supervised-child-owns-a-process-group.md).
- [ ] **T72a** The cold path, and the activator a php-fpm pool on a Unix socket has not got: hold the
      socket while the pool is stopped, render it as the site's second upstream, and gate the
      published **< 1.5 s** on a real `GET` through the front end. **Split out of T72**, which found
      that the number cannot be measured on two of three systems as things stand rather than that it
      is slow there. The machinery exists — **T70a** taught `Activation` to hold and release Unix
      sockets for the databases, and `Recipe::held_while_stopped` already returns a set of addresses
      — so what is missing is the php-fpm recipe using it and Caddy's `php_fastcgi` naming it. **(P)**,
      and Windows is the one system where the path already works today.
- [ ] **T73** Dev-tuned defaults pass over every service template (buffer pools, memory limits).

**Milestone M7** — after 30 idle minutes only `mixengined` + the web server are running, and the next
request still succeeds within budget.

**Its second half waits on T72a.** T72 gates what is running and what it costs on all three systems;
that the *next request* succeeds within the published 1.5 s is measurable today only on Windows,
because a php-fpm pool on a Unix socket is neither idle-stopped nor woken. The milestone is claimed
when T72a lands, not before.

---

Previous: [Phase 5 — HTTPS](phase-5-https.md) · Next: [Phase 8 — Differentiators](phase-8-differentiators.md)
