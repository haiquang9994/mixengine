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
- [ ] **T15** Ready/health polling, restart backoff, crash-loop cutoff with the last 200 log lines
      attached to the failure reason.
      **It inherits one gap from T13.** `Supervised::stop` kills the group only while the process it
      named is still there, so a master that crashed leaves the workers it forked behind — on Unix
      for good, on Windows until the handle drops and the job closes. That is precisely the state a
      restart policy meets (a php-fpm master gone, its pool still holding the port), so the fix
      belongs here rather than as a patch to the spawn: the unconditional `SIGKILL` becomes
      `SIGTERM` to `-pgid`, a grace period, then `SIGKILL`, and it has to run against the group
      whether or not the leader is still in it. The reason the guard is there today is a pgid that
      could have been given away after the leader was reaped — the same residual race [ADR
      0007](../decisions/0007-supervised-child-owns-a-process-group.md) already accepts, so it buys
      nothing the ADR has not already paid for.
- [ ] **T16** Log capture: line splitting, per-service files, size rotation, in-memory ring buffer,
      `LogLine` events, `GET /logs/{id}?follow=1`.
- [ ] **T17** Dependency DAG start/stop ordering; cycle detection at spec-build time.
- [ ] **T18** Crash recovery: PID + start-time adoption, stale socket/pidfile cleanup on daemon boot.
- [ ] **T19** `service.*` RPC surface + `mix service start|stop|restart|status|logs`.

**Milestone M1** — kill the daemon mid-run; on restart it adopts what survived and cleans what did
not. Proven by tests against `fakeservice` on all three OSes.

---

Previous: [Phase 0 — Foundations](phase-0-foundations.md) · Next: [Phase 2 — Runtimes](phase-2-runtimes.md)
