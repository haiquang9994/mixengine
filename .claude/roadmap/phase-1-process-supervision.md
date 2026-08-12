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
      **`StopBehaviour::Command` is the same gap and belongs to the same one-shot spawn.** T19's
      runner honours `Signal` and `Kill` and can only kill for a `Command`, saying so through
      `tracing::error!` rather than quietly — which for a database is a recovery on its next start.
      No spec names one yet, and T33's `mariadb-admin shutdown` is the first that will, so the
      platform call this task adds has three callers and not two.
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
- [x] **T16** Log capture: line splitting, per-service files, size rotation, in-memory ring buffer.
      Line splitting and the ring came with T15, which needed them; what landed here is the file
      under `logs/services/<service-id>/current.log` and the rotation that bounds it.
      **The `LogLine` event and the endpoint are T16b**, split off for the reason T15 split the
      runner off: both start from a `ServiceId` and have to find the `Capture` it belongs to, and
      that registry is the daemon's, arriving with T19. Building it here would mean building it
      twice.
      **The file writer is a third reader of one stream, not a second copy of it**, and it runs on
      the reader threads T15 already has rather than on a task of its own — so the supervisor keeps
      the property that makes T19 possible (no loop, no clock), and a line is on disk before it is
      broadcast. The order matters in one direction only: a line that reached a subscriber and not
      the disk is a line the GUI showed and `current.log` will never explain. The file's lock is
      held across all three steps, because the two reader threads race and that race has to resolve
      to *one* order — the ordering between stdout and stderr is what somebody reading a failure is
      looking at, and a file that disagrees with the event stream about it is worse than either.
      The cost is stated where it is paid: the disk write now sits on the thread that drains the
      pipe, so a log directory on a stalled mount is a service's problem and not only a log's.
      **A service's log is plain text and carries nothing of ours.** No timestamp, no `[stderr]`
      tag: `current.log` is read by whoever reads MariaDB's or Caddy's log, with their tools and
      their expectations, and a prefix would break all of them to restate what the ring and the
      event carry anyway. Both streams interleave into the one file, because the ordering *between*
      them is what somebody reading a failure is looking for. The same rule is why a failed rotation
      is reported through `tracing` — into `daemon.log`, where the supervisor's own voice belongs —
      and never written into the service's file.
      **`RotatingFile` moved down rather than being written twice.** The 10 MB × 5 rule was the
      daemon's, private to its `logging` module, and the supervisor is the process that holds a
      service's handle — so the type now lives in `mixengine-supervisor::logs::rotating` and the
      daemon uses it from above. Moving it forced the one behavioural change: it no longer *writes*
      the complaint, it hands the `io::Error` back and the caller decides, because the daemon owes
      that note to `daemon.log` in whatever shape `log.format` asks for while a service's file must
      not be given a sentence at all. The move also gave it a retry rule it did not need before: a
      rotation that failed waits for another `max_bytes` of growth rather than being tried on the
      next line, because four syscalls per attempt was nothing at `daemon.log`'s few lines a minute
      and is a measurable share of the machine at a service's few thousand a second.
      `LogLine` and `Stream` moved to `mixengine-proto` on the way, for the reason ADR 0006 gives
      and T14 set the precedent for: the line a ring holds, the line a file is written from and the
      line an event will carry are one value, so the third cannot describe something the first two
      did not see.
- [x] **T17** Dependency DAG start/stop ordering; cycle detection when the specs are assembled.
      `mixengine_core::services::graph` — `ServiceGraph` and `Plan`. In `core` rather than `proto`
      on ADR 0006's own line: `proto` owns the vocabulary a spec is written in and gains nothing
      from a topological sort, while the supervisor is deliberately without a registry, a loop or a
      clock, which is what leaves T19 owning the timing.
      **"At spec-build time" turned out to be the one thing it could not be.** A cycle is a property
      of a *set* of specs, and `ServiceSpecBuilder::build` sees one spec — which is why it rejects
      only the case a spec can see for itself, depending on itself. The same is true of the other
      two invariants that landed here: an id declared twice, and a dependency naming a service that
      is not in the set. All three are checked once, when the graph is assembled, and after that a
      graph answers questions and cannot fail on its own account — so no caller downstream ever
      handles "what if it is a cycle" again. The roadmap's wording was corrected rather than the
      check moved somewhere it cannot work.
      **A plan is tiers, not a flat list**, which is the decision that keeps T19 free. Services in
      one tier have no path between them and may start at once; T19 walks them sequentially through
      `Plan::flat`, and M3's ten-second budget then buys concurrency by changing the walker rather
      than recomputing the plan. Within a tier the order is by `ServiceId`, because a start order
      that varies run to run turns one broken dependency into a bug that only reproduces on somebody
      else's machine — pinned by a test that builds the same graph from specs in two orders.
      **Start and stop are opposite walks, not one walk reversed.** Starting `php-fpm` pulls in what
      it depends on; stopping `mariadb` pulls in what depends on *it* and takes those down first.
      For the whole set the two coincide, which is exactly why deriving one from the other would
      have been a coincidence waiting to be relied on: for a subset they name different services.
      **The failure path is fail-fast**, and it brought the `StateReason` variant
      `.claude/architecture/` had reserved for this task: `DependencyFailed { dependency }`, fed by
      `ServiceGraph::blocked_by`. A dependent spawned anyway would crash against a database that is
      not there, be restarted by its policy, and arrive at `CrashLoop` a minute later with a tail
      saying `connection refused` — an accurate report of the wrong problem. Each service names the
      direct edge it declared rather than the root of the chain, so a chain of four reads as four
      sentences leading to the one service to fix. It needed **no new edge in the state machine**:
      `SpawnFailed` already reaches `Failed` from `Starting` without a process ever existing, so
      `Starting` was already "somebody asked and it is not usable yet" — its doc comment said
      otherwise and was corrected to what the machine has done since T14.
      **Review found the one place this trusted an invariant nothing enforces**, and it is fixed
      here: `depends_on` is deduplicated as the graph is assembled and both directions are held as
      sets. `ServiceSpecBuilder::build` refuses an edge written twice, but `ServiceSpec` says in so
      many words that deserialisation checks nothing and a loader calls `validate` — so a row or an
      `extension.toml` can hand the graph the same edge twice, and counting it twice left the
      service waiting forever on a dependency the reverse edges could only discharge once. That
      reported a perfectly good set of specs as `Cycle { path: [] }`, rendering as "the loop could
      not be recovered". Its two neighbours were fixed with it: `mixengine_core::Error::Graph` was
      landing in the daemon's `_ => internal` arm, telling a user that their own `extension.toml`
      was a bug in MixEngine, and now answers `invalid_argument` — or `not_found` for
      `NoSuchService`, which is not a declaration failure at all.
- [x] **T19** The runner and the registry of running services, in the daemon.
      **The runner belongs here, and that is why T15 does not contain one.** T15 delivers the
      mechanisms — capture, ready, health, restart — as pieces with no loop, no clock and no state
      row, because the thing that owns the timing is also the thing that owns the registry of
      running services, the `CancellationToken` they hang off and the `core::services::transition`
      that persists each move. That is the daemon, and it has no such registry until something can
      ask it to start a service. Building the loop before its owner would mean writing it twice.
      One task per service, holding the `Supervised` T13 returns and driving T15's pieces in order:
      spawn, wait for ready, then the health loop, with `restart::decide` on every exit and
      `transition` persisting *and* announcing each move — the registry never writes `services.state`
      behind `core`'s back, or the row and the event would drift apart again.
      **It walks `Plan::flat` sequentially**, which is what T17 left it free to do: tiers are already
      computed, so M3's ten-second budget buys concurrency by changing this walker and nothing else.
      A tier that fails stops the walk and the services below it are marked
      `DependencyFailed { dependency }` from `ServiceGraph::blocked_by`, never spawned.
      **It writes the three columns nothing has written yet** — `last_started_at`, `pid` and
      `pid_start_time` — which is the whole reason this task now comes before T18 rather than after
      it: adoption needs rows to adopt, and until something starts a service there are none. Reading
      a process's start time is T18's platform work; T19 writes what it can and leaves the column
      null on an OS that cannot answer yet, so no consumer is written against a value that will
      change shape.
      **Where a `ServiceSpec` comes from was answered by a port, not by this task.** There is no
      source of one in the workspace: `services` holds no spec — `package_id`, `port`, `data_dir`,
      `config_overrides_json` and `limits_json` are the row — and turning those plus a package into a
      runnable spec is the config generation [../features/services.md](../features/services.md)
      describes for **T30**. So the registry asks a `SpecSource` for the **declared set** and does not
      know where it came from; the set rather than one spec by id, because the caller's next move is a
      `ServiceGraph` and dependencies, cycles and order are properties of a set. `Undeclared` is what
      this build ships — an empty set, which is the honest answer for a home that cannot create a
      service yet and one the registry, the graph and the walk all handle without a special case —
      and **T30** replaces it. The two answers not taken cost more than they save: a `spec_json`
      column duplicates three columns that already exist and keeps a second copy of what generation
      renders, which is the disposable-generated-config rule in `CLAUDE.md` read backwards; and
      building the spec from package + row here is T30 done early against packages Phase 2 has not
      installed yet.
      [../architecture/process-supervision.md](../architecture/process-supervision.md) said a spec
      "arrives by `Deserialize` — from a `services` row", written before the schema existed and
      naming no column; it is corrected with this task, because the row is what a `SpecSource`
      *reads*, never what it deserialises.
      The fixture source lives in the daemon's own test module rather than in `mixengine-testkit`,
      and that is forced rather than chosen: `SpecSource` is this crate's trait, so nothing outside
      it can implement one. What the tests do share is `FakeService` — the specs are built around it
      — and each seeds its own `packages` row first, because `services.package_id` is
      `NOT NULL REFERENCES packages (id)`.
      **What landed beside it, because the runner is the first thing that had to answer them:**
      `core::services::started` and `ended`, so the pid pair and `last_exit_code` are written by
      `core` and not by the daemon reaching around it; `StateReason::Uncheckable`, which is what a
      spec naming a check this build cannot make becomes — not a ready timeout, which would send its
      author looking at the service instead of at the spec; and `Events` becoming clonable, since a
      runner outlives every request and cannot borrow from the `Api` that serves one.
      `Registry::shut_down` is what the daemon calls on its way out: the root token has already
      cancelled every runner, and this is where the process *waits* for them rather than leaving the
      job to `Supervised`'s destructor, which kills rather than asks. The order is still T9a's, and
      so is the *budget* for it — see the note there.
      **Two things a review of this task found, fixed in place.** The walk asked "is a runner alive?"
      where it meant "is the service up", and those are different questions: a runner is alive through
      a restart backoff, through a stop and through a start that has not finished yet. So a
      `service.start` arriving while a dependency was in its fourth crash was answered *reached*, and
      the tier below was started against it — the exact outcome T17's fail-fast exists to prevent, and
      it would have arrived as `web` reporting `connection refused` about a `db` nobody was looking at.
      Second, the walk waited for the *policy* to settle rather than for the attempt in flight, which
      `RestartPolicy::Always` never does: no ceiling means nothing under it ever reaches `Failed`, so
      the walk never came back at all and the tier below waited for ever.
      **Both are one mechanism now.** A runner publishes a `Readiness` — `Up`, `Deciding`, or `Down`
      with what was persisted — derived from the transition it has just written, on the same rule as
      the event: one move, one description, so readiness cannot disagree with the row. `Running` and
      `Degraded` are up, because amber is not absent and a dependent that refused to start against a
      slow database would turn one of them into a machine with nothing running; `Starting` is
      undecided, so a second walk waits for the same answer instead of inventing one; `Stopping`,
      `Stopped` and `Failed` are down. The one-shot the walk used to hold is gone, and with it the
      `announce` argument that was threaded through six functions of the runner.
      **`Restarting` is the policy's to answer, and reading it as down cost the default policy its
      recovery.** The first version made it down outright, which does bound the wait — but it bounds
      it at *one attempt* for every policy, and `OnFailure`'s first crash is a transient the runner
      comes back from a backoff later. A walk that took it for an answer left the tier below `Failed`
      and unsupervised beside a service that then came up fine. So the bound follows the policy: one
      with a ceiling arrives at `Running` or `Failed` by itself and the walk waits through its
      backoffs for whichever it reached, and only `Always` — which never reaches `Failed` at all — is
      answered after the first attempt.
      **A readiness is also published once without a row behind it**, between a process exiting and
      `after_exit` persisting what that meant: a drain bounded by `FLUSH` and a write, during which
      the runner would otherwise still be advertising the `Up` the service had stopped being, and a
      concurrent `service.start` would spawn the tier below against a database that had gone. It
      publishes `Deciding` there and not `Down`, because what happens next is the restart policy's to
      say.
- [ ] **T19c** Give `Registry::begin` a way to *ask* a live runner to start, not only to read it.
      **Ordered before T19a although it is lettered after it**, because T19a is where a user can
      reach the gap: today `begin` joins an existing runner under the lock that registers and then
      only waits on its `Readiness`. That is right for a service already up and for one mid-start,
      and wrong for the case a person is most likely to type `mix service start` at — a service
      crash-looping under `RestartPolicy::Always`, whose runner never deregisters. Every attempt
      re-walks the tier below `Starting` → `Failed`, emits two more events, and spawns nothing,
      because nothing in the path can shorten the backoff the runner is sitting in or reset the
      failure count it is counting against. T19 answered `Ready` here and was wrong in the other
      direction; neither version gives `start` an action.
      What it wants is a request the registry can send *into* the runner — a second token, or a
      `watch` in the other direction that `wait_out` selects on beside `cancel` — so an explicit
      start cuts the backoff short and calls `Restarts::recovered`, which is the difference between
      "a person asked for this again" and "the policy came round again". Both are already the shape
      the runner is built in, so this is a small task; it is separate only because it is a new edge
      between the two halves rather than a rule about an existing one.
- [ ] **T19a** `service.*` RPC surface: `list`, `status`, `start`, `stop`, `restart`.
      Method names and payloads in `mixengine-proto` beside `daemon.*`, handlers in
      `crates/mixengine-daemon/src/api/rpc.rs`, over `Registry::graph` and a `Plan` built from it —
      which is what T19 left ready: `graph()`, `start(graph, plan)` and `stop(plan)` are there and
      have no caller but the tests until this task. Errors keep the mapping T17 fixed: `Error::Graph`
      is `invalid_argument`, `NoSuchService` is `not_found`, and nothing about a user's own
      declaration arrives as an internal error — which is why `Undeclarable` has two variants, one
      per side of that line.
      The `Api` gains the `Arc<Registry>` here; T19 deliberately left it in `serve`, where the
      shutdown wait needs it, rather than adding a field nothing reads.
      A start returns as soon as the plan is accepted rather than when everything is running —
      readiness takes as long as the service takes, and the client already has
      `ServiceStateChanged` to watch. The alternative is an RPC that blocks for thirty seconds and a
      GUI that cannot show what it is waiting for.
- [ ] **T19b** `mix service start|stop|restart|status`, both renderings.
      Thin against T19a, on `crates/mixengine-cli/tests/status.rs`'s pattern: an end-to-end test that
      autostarts a daemon, drives a `fakeservice` spec through it and asserts what the human and
      `--json` outputs say. `mix service logs` is deliberately **not** here — it is a client of the
      endpoint T16b builds, and a CLI that read `current.log` off the disk itself would be the
      business-logic-in-a-client bug `CLAUDE.md` forbids.
- [ ] **T16b** `DaemonEvent::LogLine`, `GET /logs/{id}?follow=1`, and `mix service logs`.
      What is already here: `Capture::subscribe` is the whole of what both need from the supervisor,
      and `Paths::service_logs` plus `logs::CURRENT_LOG_FILE_NAME` name the file the historical half
      of the endpoint reads. What it was waiting for is T19's registry: both begin by looking a
      `ServiceId` up in it.
      **It arrives with a question that wants an ADR.** `.claude/architecture/daemon-and-ipc.md`
      lists `LogLine` among the `DaemonEvent`s, which puts every line of every running service on
      the one bounded broadcast the GUI watches for state changes — capacity 1024, slow consumers
      dropped. One chatty service in debug mode would then spend a client's whole allowance and hand
      it a `Resync` storm, losing the `ServiceStateChanged` events that actually matter. Either the
      log lines travel on their own stream (`GET /logs/{id}` only, and the architecture is corrected)
      or `/events` grows per-kind subscription. Decide it there, not by discovering it in the GUI.
- [ ] **T18** Crash recovery: PID + start-time adoption, stale socket/pidfile cleanup on daemon boot.
      **Moved below T19 on purpose**, where it can be proved rather than only written: a survivor is
      a `services` row with `state = 'running'`, a `pid` and a `pid_start_time`, and until T19 starts
      something there is no such row to meet. The pair is the point — a pid alone is reused by the OS
      within minutes, and signalling the wrong process is the one accident this product cannot have —
      so reading a process's start time is the platform work this task owns **(P)**, and it is also
      what closes the macOS gap [ADR 0007](../decisions/0007-supervised-child-owns-a-process-group.md)
      writes down honestly instead of averaging.
      Adopting means more than believing the row: a survivor's pipes belong to a daemon that is gone,
      so its log capture cannot be resumed and only its liveness and its exit can be observed. What
      that costs a user is the honest sentence T47 owes `mix doctor`.

**Milestone M1** — kill the daemon mid-run; on restart it adopts what survived and cleans what did
not. Proven by tests against `fakeservice` on all three OSes.

---

Previous: [Phase 0 — Foundations](phase-0-foundations.md) · Next: [Phase 2 — Runtimes](phase-2-runtimes.md)
