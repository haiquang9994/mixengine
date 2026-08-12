# Phase 1 — Process supervision

*Goal: we can run and babysit arbitrary programs correctly. Everything later is built on this.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [x] **T12** `ServiceSpec`, `ReadyCheck`, `HealthCheck`, `RestartPolicy`, `StopBehaviour` types.
      They land in `mixengine-proto`, with a builder that validates, and an `EnvValue` that names a
      keyring entry instead of holding a password — [ADR
      0006](../decisions/0006-servicespec-in-proto-and-secret-free.md), which this task forced and
      which Phase 4 reuses for `PrivilegedOp` (see T40).
- [x] **T13** Spawn with process groups: Job Object (Windows), `setsid` + `PR_SET_PDEATHSIG` (Unix);
      no orphans when the daemon dies. **(P)**
      `spawn_supervised` returns a `Supervised` that *is* the group's ownership, so dropping it stops
      the group — the mirror image of `Detached`, and the pair is the whole of
      `mixengine_platform::process`. One group per service rather than one for the daemon, which is
      also the object T68's caps hang on. The task forced [ADR
      0007](../decisions/0007-supervised-child-owns-a-process-group.md): "no orphans when the daemon
      dies" is three different promises, total on Windows, the immediate child only on Linux, and
      nothing at all on macOS, where T18 is what covers it. `mix doctor` owes the honest sentence
      (see T47).
      The assertion is a **lock, not a pid**, which was most of the work: `try_stop` on Unix
      succeeds against a zombie and so answers a question about pids rather than about processes
      (see [../standards/testing.md](../standards/testing.md)), while a lock is released by the
      kernel when the process really ends and by nothing else. `fakeservice` grew `--hold-lock`,
      `--supervise` and `--child` for it, and `crates/mixengine-testkit/tests/supervision.rs` is the
      ADR written as code — including the macOS test that asserts the *gap*, so the day somebody
      closes it is a day a test fails and says so.
      **One race is left in those tests rather than papered over.**
      `a_supervisor_that_goes_away_takes_its_child_with_it` signals as soon as the *child's* lock is
      held, not once the supervisor has written its `READY_LINE`, so a `SIGTERM` arriving before
      `Signals::listen` has run would end the supervisor by default disposition with no destructor
      — which Windows (killed outright anyway) and Linux (`PR_SET_PDEATHSIG`) cannot notice and
      macOS would fail on. The window is one process's `exec` wide and has never been seen to lose;
      `Running::wait_for_stdout` is the one-line fix the first time a macOS runner flakes.
- [x] **T14** State machine + persistence + `ServiceStateChanged` events; `Degraded` vs `Failed`.
      The first `sqlx::query!` in the workspace lands here, so it brings the offline data with it:
      committed `.sqlx/`, `cargo sqlx prepare --check` in CI, and no `DATABASE_URL` needed to build
      (see T6). The `lint` job installs `sqlx-cli` from a prebuilt binary rather than compiling it,
      and [../operations/build-and-release.md](../operations/build-and-release.md) has the four
      commands to run after editing a query — the failure that step exists for is invisible on the
      machine that caused it.
      `ServiceState` is a **closed** enum where the rest of the wire vocabulary is `non_exhaustive`,
      because a state machine with room for one more state is one nobody can reason about; the
      *reason* is the open half. One spelling serves the wire and `services.state`, checked by a
      test rather than trusted, and the column's `CHECK` carries the same closed list — which is why
      `0001_initial.sql` was edited rather than followed by a table rebuild: nothing has shipped, so
      the forward-only rule has nothing yet to protect.
      The diagram in
      [../architecture/process-supervision.md](../architecture/process-supervision.md) turned out to
      compress five real edges, now written down: a process that exits on its own goes `Running →
      Restarting|Failed` without passing through `Degraded`; one that dies before it is ever ready
      goes `Starting → Restarting`, without which a `RestartPolicy` would cover none of the ordinary
      ways a service fails to come up; and a stop arriving mid-flight is not queued behind a start
      nobody wants. `can_become` is the authority and the spec was corrected to match.
      **Persisted and emitted are one value, not two.** `core::services::transition` returns the
      `ServiceTransition` it wrote and `DaemonEvent::ServiceStateChanged` carries that same value, so
      a transition that did not happen cannot be announced. The transaction opens with `BEGIN
      IMMEDIATE` rather than sqlx's deferred default, because two supervisors reaching one service
      is the ordinary case and a deferred `BEGIN` would leave the `UPDATE` to upgrade a read
      snapshot — which WAL refuses with `SQLITE_BUSY_SNAPSHOT` and does not even run the busy
      handler for. The compare-and-swap on the previous state stays as the assertion.
      **One column is deliberately not written here.** `last_started_at` is ISO-8601 text and this
      workspace has no date library — `Timestamp` is a number of milliseconds and nothing has needed
      to *format* a moment. Writing it means either a new dependency or a hand-written civil-date
      conversion, and that choice belongs to T15 along with the code that would use it.
- [x] **T15** Ready/health polling, restart backoff, crash-loop cutoff with the last 200 log lines
      attached to the failure reason.
      **It inherited one gap from T13 and closed it.** `Supervised::stop` killed the group only
      while the process it named was still there, so a master that crashed left the workers it
      forked behind — which is precisely the state a restart policy meets, and "gone" is also the
      state a stop is *trying* to reach, so making it a precondition read the question backwards.
      The kill is now unconditional and the handle remembers having killed: on Unix an **unreaped**
      leader keeps its pgid reserved, so terminating before waiting is always sound, and doing it
      twice afterwards is the residual race [ADR
      0007](../decisions/0007-supervised-child-owns-a-process-group.md) already accepts.
      **The polite half forced [ADR 0008](../decisions/0008-no-signal-stop-on-windows.md)**, which is
      the question T13 explicitly left here. Windows has one signal-shaped mechanism and it travels
      through a console the daemon does not have and the child was deliberately not given; reaching
      it would mean swapping this process's console and disabling its own control handler, from one
      thread of a supervisor running other services on the others. So `process::CAN_ASK_TO_STOP` is
      false there, the supervisor reads it *before* starting a grace period rather than waiting out
      a request nobody sent, and a service that must shut down cleanly on Windows uses
      `StopBehaviour::Command` — which is what MariaDB and Caddy document anyway.
      **Three decisions the task made on its way through**, each recorded where it applies: a
      supervised child now gets the environment its spec states and not the daemon's, under a short
      per-OS floor (Windows cannot load a system DLL without `SystemRoot`); `services.last_started_at`
      became epoch milliseconds rather than ISO-8601 text, closing what T14 left open, because the
      supervisor reads it back on every exit to place a restart inside the crash-loop window; and
      the `Keyring` capability landed, since ADR 0006 means a spec *names* a credential and something
      has to resolve it at spawn time.
      **Log capture came first, in the shape T16 will build on.** A crash-loop cutoff that says
      "it kept crashing" explains nothing without the line saying `Address already in use`, so
      `StateReason::CrashLoop` grew a `tail` — the one reason that cannot explain itself, and the
      only variant carrying evidence. `ReadyCheck::LogPattern` is the second user. Reader threads
      rather than tasks: an anonymous pipe on Windows cannot be read asynchronously at all, and
      draining both is not optional — a pipe holds tens of kilobytes and then the service blocks on
      its next line, looking exactly like one that has hung.
      Waiting for readiness **races three outcomes, not two**: the process exiting while the probe
      waits is the most common way a service fails to start, and treating it as "not ready yet"
      spends the whole timeout on something that died in the first second. The `select` is biased
      towards the exit, so a service that printed its ready line and then died is not called ready.
- [ ] **T15a** `ReadyCheck::Http`, `HealthProbe::Http` and `HealthProbe::Command`.
      Deferred deliberately, not forgotten: each needs a dependency the supervisor should not invent
      before something wants it. HTTP needs a client in a crate that has none — `reqwest` per
      [../standards/rust.md](../standards/rust.md), or the `hyper` already in the tree — and a
      command probe needs a **one-shot spawn in `mixengine-platform`** that suppresses a console
      window on Windows, which the supervisor cannot write itself without the `#[cfg]` it is not
      allowed to contain. Until then both answer `Error::UnsupportedCheck` naming what is missing,
      per `CLAUDE.md`'s rule against `todo!()`.
      Lands with its first real user in Phase 3: `HealthProbe::Command` is the honest check for
      MariaDB and PostgreSQL (a TCP accept only proves the listener is up, which stays true while
      the server refuses every query), and Caddy's admin endpoint is the first `Http` one. Do it
      **before** T30 writes those specs, or they will be written around the gap.
- [ ] **T15b** Tell a Linux with no secret service apart from one whose store refused.
      `crates/mixengine-platform/src/secrets.rs` maps only `KeyringError::NoStorageAccess` to
      `Error::UnsupportedPlatform`, on the assumption that a machine without a store answers that
      way. It does not. `keyring`'s secret-service backend maps `Locked`, `NoResult` and `Prompt` to
      `NoStorageAccess` and **everything else** to `PlatformFailure`, so a session with no provider
      arrives as `Error::Secret` — rule 4 of
      [../architecture/platform-abstraction.md](../architecture/platform-abstraction.md) inverted: a
      capability the machine does not have, reported as a failure, with no workaround to act on.
      The evidence, from the first CI run that ever compiled this crate on Linux:
      `Platform secure storage failure: DBus error: The name org.freedesktop.secrets was not
      provided by any .service files` — a `dbus_secret_service::Error::Dbus` carrying the D-Bus
      error name `org.freedesktop.DBus.Error.ServiceUnknown`, and *not* the `Unavailable` variant
      that exists for exactly this case and that this backend never returns here.
      **Deferred because every way of reading it costs something.** Reaching the error name means
      depending on `dbus-secret-service` and `dbus` directly, which pins this crate to `keyring`'s
      current Linux backend — one it has changed before — and goes against the one-crate-per-concern
      table in [../standards/rust.md](../standards/rust.md), so it wants an ADR rather than a quiet
      import. Matching the message text instead needs no dependency and breaks silently the day
      dbus-daemon rephrases it. Neither is urgent while CI runs these tests against a real
      gnome-keyring and a developer sees the whole cause chain, so this waits for somebody who has
      actually been bitten on a headless machine to say which of the two they want.
- [ ] **T16** Log capture: line splitting, per-service files, size rotation, in-memory ring buffer,
      `LogLine` events, `GET /logs/{id}?follow=1`.
- [ ] **T17** Dependency DAG start/stop ordering; cycle detection at spec-build time.
- [ ] **T18** Crash recovery: PID + start-time adoption, stale socket/pidfile cleanup on daemon boot.
- [ ] **T19** `service.*` RPC surface + `mix service start|stop|restart|status|logs`.
      **The runner belongs here, and that is why T15 does not contain one.** T15 delivers the
      mechanisms — capture, ready, health, restart — as pieces with no loop, no clock and no state
      row, because the thing that owns the timing is also the thing that owns the registry of
      running services, the `CancellationToken` they hang off and the `core::services::transition`
      that persists each move. That is the daemon, and it has no such registry until something can
      ask it to start a service. Building the loop before its owner would mean writing it twice.

**Milestone M1** — kill the daemon mid-run; on restart it adopts what survived and cleans what did
not. Proven by tests against `fakeservice` on all three OSes.

---

Previous: [Phase 0 — Foundations](phase-0-foundations.md) · Next: [Phase 2 — Runtimes](phase-2-runtimes.md)
