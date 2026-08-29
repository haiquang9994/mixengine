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
      down* is **T72**'s, which has a `bench` job that knows how to compare against master.
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
- [~] **T70** On-demand activation gateway: hold the socket, start the service, wait for ready, proxy
      the first request.
      Design: [2026-08-29-t70-on-demand-activation-design.md](../../docs/superpowers/specs/2026-08-29-t70-on-demand-activation-design.md).
      **"Hold the socket" survives for the databases and not for the web path**, and the design's D2
      says why: to let php-fpm bind its own socket the daemon has to close its listener first, and
      every request arriving in the several hundred milliseconds before the pool binds it is refused
      by the kernel. So a site's front end keeps pointing at the pool and names a second, permanently
      bound activator address after it. A database has no front end to express that, so there the
      daemon does hold the address, and the window is stated rather than hidden.
      **The rendering was measured before the rest was written, and the measurement changed it.**
      Against a real Caddy 2.10.0, the bare two-address form answers 200 to **8 of 20** requests: Caddy
      treats the addresses as peers and load-balances between them, so it would send half of a healthy
      site's traffic through the activator. `lb_policy first` with a retry budget and no passive health
      check is worse — **0 of 20**, each burning the full 5 s, because nothing ever marks the refusing
      pool unavailable. Only all three of `lb_policy first`, `lb_try_duration` and `fail_duration`
      together answer **20 of 20**. nginx needs one directive for the same thing. `fail_duration` was
      then measured to be exactly how long a *recovered* pool is still reached through the activator.
- [ ] **T71** Metrics history: 1 s sampling while subscribed, 24-hour downsampled retention.
- [ ] **T71a** The macOS memory watchdog: warn at a `memory_mb` it cannot enforce, and restart at a
      threshold when the service asks to be. **Split out of T68**, and ordered here rather than there
      because it is the one part of `ResourceLimits` that is not a call on a kernel object — macOS has
      no hard memory cap, so the limit becomes a reading taken repeatedly and compared, which is
      T71's sampler and nothing else. Until this lands, `LimitSupport` answers `Unsupported` for
      memory on macOS and means it. **(P)**, though only one of the three does anything.
- [ ] **T72** CI budgets: idle footprint < 60 MB RSS, cold path < 1.5 s — failing the build on
      regression.
- [ ] **T73** Dev-tuned defaults pass over every service template (buffer pools, memory limits).

**Milestone M7** — after 30 idle minutes only `mixengined` + the web server are running, and the next
request still succeeds within budget.

---

Previous: [Phase 5 — HTTPS](phase-5-https.md) · Next: [Phase 8 — Differentiators](phase-8-differentiators.md)
