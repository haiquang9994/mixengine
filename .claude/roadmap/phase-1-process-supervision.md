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
      `Millis` joins `Timestamp` and `Uptime` in `time.rs` as the third and **last** time type — a
      moment, a length a person reads, and a length a machine waits out are all three of them, and a
      fourth would be a spelling rather than a kind. `ServiceState` is deliberately not here: it
      arrives with T14, which is what persists and emits one.
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
- [x] **T15a** `ReadyCheck::Http`, `HealthProbe::Http`, `HealthProbe::Command` and
      `StopBehaviour::Command`.
      Done **before** T30 rather than during it, which was the whole reason it is a task: the four
      were one gap in two halves, and specs written around a gap are written twice.
      **`hyper`, not `reqwest`.** [../standards/rust.md](../standards/rust.md) names the second for
      downloads and it stays there; what a check makes is a `GET` to a **loopback** URL for a status
      code, so there is no redirect, no cookie jar and no TLS to want, and `hyper` was already in
      the tree for the IPC transport. An `https://` check therefore answers
      `Error::UnsupportedCheck` naming what is missing, rather than a certificate store being pulled
      in to verify `127.0.0.1`. The URL is read **once**, before anything waits —
      `Error::Url` is the sibling of `Error::Pattern`, on the same rule: a check that can never pass
      is the spec's fault, and reporting it as a service that never came up sends the reader to look
      at the service. A *different* status is retried rather than reported, because a `502` from a
      service whose own backend is not up is the first second of an ordinary start.
      **One bug in that was worth the test that found it.** The connection future is what reads the
      socket, so for a server that answers and closes at once — which is every one of these, nothing
      asking to keep a connection alive — the response arriving and the connection ending are the
      *same* poll. A `select!` biased towards the connection reported a service that had replied
      perfectly well as one that hung up. The answer is asked for again after the connection ends
      rather than assumed absent, which also keeps the real hang-up honest.
      **The one-shot is `process::run_once`, in the platform layer**, where the `CREATE_NO_WINDOW`
      it needs is allowed to be written: a health probe every ten seconds would otherwise be a
      terminal window opening on the user's desktop six times a minute, which is the lesson
      `windows/process.rs` already records once. `tokio::process` and not the standard library's,
      because a probe with a deadline has to be able to give up and a blocking `wait` cannot be.
      Deliberately **no session or group on Unix**, unlike both other spawns: a one-shot that forks
      is out of scope, and a group would put the process out of reach of the very deadline that has
      to kill it.
      **The deadline is the process's, not its pipes'**, and the two are only usually the same
      moment. End of file arrives when the *last holder* of a pipe exits, so a one-shot behind a
      wrapper script — exited in milliseconds, with a helper still holding a copy of its stdout —
      was reported as having timed out however cleanly it had exited: a `caddy stop` that worked
      would have been followed by a kill of the service it had just shut down properly, and a
      command health probe would have degraded a healthy service every interval, for ever. So the
      wait is read off `Child::wait` alone and the tail of the output is then given its own short
      bound, which is the same thing `FLUSH` does for a service's last log lines. The streams are
      drained *alongside* that wait rather than after it: that is what keeps the output of a run
      that timed out, and it is also what stops a program with a screenful of complaint blocking on
      its own write and being timed out as though it had hung.
      **A service's own commands run where the service runs.** `Surroundings` is its working
      directory and the environment it was actually started with, resolved once at the spawn and
      kept: a credential reaches `mariadb-admin` through the environment, which is what ADR 0006
      exists for and why it must not travel in an argv every process table on the machine can read,
      and re-deriving it per probe would be an OS keyring read every ten seconds per service, for
      ever. A service this daemon *adopted* has none — the environment belonged to a daemon that is
      gone — so a stop command there resolves one at the moment it is needed, and an environment
      that cannot be resolved still runs the command rather than skipping to the kill.
      **The grace period now starts before the command runs, not after.** For `Signal` that changes
      nothing; for `Command` it is T9a's rule one level down — whatever the spec allows, minus what
      has already been spent — because running `mariadb-admin shutdown` is itself part of what the
      grace was written to cover. A command that fails or runs out of it falls through to the kill,
      loudly and carrying the program's own last line, because `ERROR 1045: Access denied` is the
      whole of what a user can act on and the kill is a recovery on the database's next start.
      **Unless the service went by itself while being asked**, which a whole grace period's worth of
      running a command is long enough for: a server that took the instruction and exited inside
      that window has stopped exactly as it was told to, even where the program carrying the
      instruction then returned non-zero or ran out of patience waiting for a server that had
      already gone. So both failing arms read the process once before they answer, and the row keeps
      the real exit code rather than recording a kill that never happened. The window is *staged*
      rather than waited for: the stop command creates the file first and only fails four hundred
      milliseconds later, so the service is reliably gone before the failure is reported.
      **The fixture is what makes a stop command provable rather than plausible.** `fakeservice`
      grew `--touch` and `--exit-when`: a service told to ignore every request to stop cannot be
      ended politely by any signal, so a *clean* exit inside the grace period is evidence that the
      command ran and nothing else. The test asserts both sides of that one claim — the file is
      there, and the row records exit code 0, which a kill cannot produce.
- [x] **T15b** Tell a Linux with no secret service apart from one whose store refused.
      **The entry was right about the bug and wrong about its size, in three ways the work had to
      measure rather than reason out.** It recorded one misreading; there are two. `keyring`'s
      secret-service backend maps `Locked`, `NoResult` and `Prompt` to `NoStorageAccess` and
      everything else to `PlatformFailure` — so a session with no provider arrived as a failure of
      ours, **and a keyring that was merely locked arrived as a machine with no credential store**.
      Rule 4 of [../architecture/platform-abstraction.md](../architecture/platform-abstraction.md)
      inverted in both directions, from the same four lines.
      It recorded one D-Bus error name; there are **three**. `ServiceUnknown` is what a CI runner
      answers, because a runner has a session bus. A headless machine — the machine this entry
      exists for — never reaches that name at all: it fails a step earlier, at the bus, with
      `NotSupported`, and a stale `DBUS_SESSION_BUS_ADDRESS` gives `FileNotFound`. **A match on the
      recorded name alone would have fixed CI and left the case the task was written for exactly as
      broken**, which is the shape of mistake this whole entry was about.
      And it treated the two readings as a live choice. They are not. Matching the message text is
      wrong *today*, not the day dbus-daemon rephrases: `dbus::Error`'s `Display` prints
      `message()` and never `name()`, and Ubuntu 24.04 answers an unreachable bus with "Using X11
      for dbus-daemon autolaunch was disabled at compile time" rather than the "without a $DISPLAY
      for X11" every account of this failure quotes. Two bus implementations are already deployed
      and are not obliged to agree; the names are in the D-Bus specification.
      Waiting for upstream turned out to be a fourth non-option: `keyring` 4.1.6 restructures into
      `keyring-core` plus per-store crates and carries the same four lines, and
      `dbus_secret_service::Error::Unavailable` — documented for exactly this case — is constructed
      **nowhere** in 4.1.0.
      So the direct edge onto `keyring`'s backend was taken and argued in
      [ADR 0013](../decisions/0013-reading-the-d-bus-error-name-to-tell-an-absent-store.md), the
      reading moved into three `sys::secrets` modules — one capability, three ways of spelling
      "there is nothing here" — and the list of names is closed, with everything off it staying
      `Error::Secret`, which is the safe direction to be wrong in.
      **What the task deliberately did not do**, and what it cost elsewhere. It adds no `mix doctor`
      check: nothing there asks about the keyring today, and a `Note` for a machine with no store is
      a task of its own rather than a corner of this one. And it quietly turned four loud CI failures
      into eight quiet skips — a Linux leg whose `gnome-keyring` never arrives now reads as a machine
      with no store — so the leg's missing-keyring warning became an error, the wait for
      `org.freedesktop.secrets` stopped being a convenience, and the absent branch is now walked on
      every run by `test-absent-secret-service.sh` under `MIXENGINE_TEST_NO_KEYRING=1`, which makes
      *finding* a store a failure. Two versions of `dbus-secret-service` in one tree would kill the
      downcast silently, so `lint` counts them.
- [x] **T16** Log capture: line splitting, per-service files, size rotation, in-memory ring buffer.
      Line splitting and the ring came with T15, which needed them; what landed here is the file
      under `logs/services/<service-id>/current.log` and the rotation that bounds it.
      **Serving any of it to a client is T16b**, split off for the reason T15 split the runner off:
      it starts from a `ServiceId` and has to find the `Capture` it belongs to, and that registry is
      the daemon's, arriving with T19. Building it here would mean building it twice.
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
- [x] **T19c** Give `Registry::begin` a way to *ask* a live runner to start, not only to read it.
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
      **It landed as an `Arc<Notify>` per entry**, which is the primitive the question actually is: it
      carries nothing but the asking, two requests arriving together are one restart, and one arriving
      while the runner is not waiting is kept as a permit rather than dropped — so there is no window
      between `Restarting` being persisted and `wait_out` being entered in which a request is lost.
      `wait_out` returns a `Released` instead of a `bool` and the priority is the bias: a stop beats a
      request, because a daemon on its way out will not spawn one more process, and a request beats
      the rest of the wait, which is the whole task. Only `recovered` is called and not a fresh
      `Restarts` — the wait is reset, the failure history is not, on the same rule recovery already
      followed.
      **The half that was not obvious is what `begin` then waits for.** A runner being asked is
      publishing the attempt *before* the request, so waiting on `Readiness` as it stood would have
      answered the caller with the very failure their request was correcting. The value is read and
      marked seen in the same breath as the request is sent, under the lock, and what is waited for is
      the next thing the runner says — which makes the race harmless in both directions: a start that
      is up again before the caller is next polled is a change that cannot be missed, and a runner
      that was already ending (`Stopping`, or three statements from `Failed`) drops the request with
      itself and is reported by what it last managed to say rather than by the `None` that means the
      daemon's own problem.
      Requests go to **every service in the plan** rather than to the one that was typed. A plan is
      already the transitive set, and a `db` in its fourth crash is exactly what somebody typing
      `mix service start web` needs unstuck; the alternative threads a "root" through the walk to
      tell them to go and start `db` by hand.
      The test's timeout **is** its assertion: `crash_looping`'s backoff is longer than `EVENTUALLY`
      and the second walk is not answered until the attempt the request causes has been decided, so a
      request that never arrived cannot be answered at all — checked by removing the one line and
      watching it fail there. What the events then say is which of the two put the service back:
      `Starting` with `Requested`, never `BackoffElapsed`.
      **A permit must not outlive the start that answers it**, which is the other half of keeping one
      rather than dropping it. `wait_out` is the only thing that consumes a permit and a runner
      mid-start is not in one, so a request landing while `ready::wait` runs — two walks sharing a
      dependency, the ordinary case — would sit in the `Notify` for as long as the service then
      stayed up, and release the next crash the instant it entered its backoff: the wait skipped, the
      ladder reset, the move published as `Requested` on behalf of somebody who asked an hour earlier
      and got what they asked for. Reaching `Running` therefore *takes* the permit
      (`Runner::answered_by_this_start`, a `Notified::enable` that reads without waiting), and the
      test asserting it is a silence: after the fixture's own crash, thirty seconds of backoff must
      produce no event at all.
      **What it does not reach is the window between a process exiting and the policy speaking.** The
      `Deciding` published there (see T19's note above) is indistinguishable to `begin` from the
      `Deciding` of a start in flight, so a request landing inside it — bounded by `FLUSH` plus two
      writes — is answered by the *previous* attempt's `Restarting`, which under `Always` reads as a
      failure and blocks the tier below, while the runner is in fact honouring it. Unchanged from
      before T19c rather than introduced by it, and closing it means a `Readiness` that distinguishes
      "starting" from "ended, policy deciding" — a change to the readiness vocabulary, which is T19's
      and not this task's.
- [x] **T19a** `service.*` RPC surface: `list`, `status`, `start`, `stop`, `restart`.
      Method names and payloads in `mixengine-proto` beside `daemon.*` — `service_api.rs`, which is
      to `service.rs` what `daemon.rs` is to the daemon: the vocabulary a spec is *written* in stays
      one file, what the daemon made of it is another. Handlers in
      `crates/mixengine-daemon/src/api/rpc.rs`, over `Registry::graph` and a `Plan` built from it,
      which is what T19 left ready. Errors keep the mapping T17 fixed, now including `Undeclarable`
      itself: `ToWire` gained the arm for it, so a declaration that is not a graph reaches the user as
      `invalid_argument` with the hint that says where such a thing is written, and only a *source*
      that could not answer is `internal`. The `Api` gained the `Arc<Registry>` and the `Store`.
      **It reversed its own note about waiting, and the exit code is why.** "A start returns as soon
      as the plan is accepted" was written before anything could act on the answer, and it makes
      `mix service start db && mix …` exit `0` for a database that never came up — leaving a client
      no way to know better except to re-derive the verdict from the event stream, which is the
      business-logic-in-a-client bug `CLAUDE.md` forbids. So `wait` defaults to true and a GUI sends
      `false`; the note's own case is still served, and `ServiceWalk::complete` says which of the two
      answers this is rather than letting an accepted plan look like a walk that did nothing. A walk
      nobody waits for is **cancelled by the root token** rather than detached, and its outcome goes
      to `daemon.log`, because that summary is the one thing `ServiceStateChanged` does not carry.
      **`restart` is not stop-then-start-the-same-id.** Stopping `mariadb` takes `php-fpm` with it, so
      starting `mariadb` again would leave the dependent where the stop put it — down, on behalf of
      somebody who asked for a restart and got half of one. What is started is what the stop **took
      down**, which needed no new graph function: `start_plan` already takes a set and orders it.
      Took down, not *covered* — a stop plan is what the graph says a stop reaches and not what it
      finds there, so feeding the plan itself back into a start would read "restart the database" as
      "and start every site that names it", including the ones somebody stopped on purpose. The
      service the caller named is the one exception, restarted whether or not it was up, because
      `restart` on something stopped is a request for it to be running.
      **Two smaller decisions, each where it is paid for.** `ServiceSummary::state` is an *option*: a
      service that is declared with no `services` row has no state to be in, and saying `stopped`
      would be a service that claims to be stopped and then refuses to start. And `supervised` is
      beside the state rather than folded into it — a row saying `running` with nothing supervising it
      is what a killed daemon leaves behind, and until **T18** adopts or clears those, this is the
      only place that gap is visible instead of implied.
      `mixengine_core::services` gained `record` and `records` — one query for a listing rather than
      one per service — and `ServiceGraph::ids`, because the order a listing wants is id order and
      neither plan gives it. The test fixtures moved to `services::fixture` on the way, since the
      registry's own tests and these now build the same home and the same `fakeservice` specs.
- [x] **T19b** `mix service list|status|start|stop|restart`, both renderings.
      Thin against T19a, on `crates/mixengine-cli/tests/status.rs`'s pattern: an end-to-end test that
      starts a daemon, drives a `fakeservice` spec through it and asserts what the human and
      `--json` outputs say. `mix service logs` is deliberately **not** here — it is a client of the
      endpoint T16b builds, and a CLI that read `current.log` off the disk itself would be the
      business-logic-in-a-client bug `CLAUDE.md` forbids.
      **`list` is in it although the task title did not name it**, because T19a's `service.list`
      would otherwise be a method with no client at all — and `status` keeps its **required** id
      rather than listing when it is left out, which is `ServiceQuery` read as it was written: a
      status with no subject is a mistyped list, and answering it as one hides the mistake.
      **The end-to-end test could not be written until a real daemon could be told about a service**,
      which is the gap T19 left behind and this task had to close before it could prove anything: the
      shipped `SpecSource` is `Undeclared` and the test drives the binary that is built, so no
      fixture inside the daemon crate is reachable from here. Two pieces of scaffolding close it,
      both with an expiry date written on them. `MIXENGINE_DEV_SPECS` names a JSON file of
      `ServiceSpec`s and is read by `services::spec::DevSpecs`, **gated on `debug_assertions`** — a
      release binary that read one would be a supervisor that runs whatever a variable points at, and
      the release build says so out loud rather than ignoring the variable in silence. And
      `mixengine_testkit::declare` writes the `packages` and `services` rows a service needs before
      anything can transition it, which is Phase 3's `service.create` in the same sense. **[T30](phase-3-services.md)
      has since deleted the first** — a real daemon renders a row into a spec now, and the fixture is
      a debug-only recipe rather than a file of arbitrary programs; Phase 3's create replaces the
      second.
      **A failed walk is an answer and not an error**, which is the one thing the exit code had to
      get right: the walk goes to stdout in both renderings — what was reached, what stopped it, what
      was blocked — and only the exit status changes, so `mix service start db && …` stops where a
      person reading the output would. Failures of the *call* stay on stderr as the wire error, as
      they were.
      `StateReason` gained a `Display` in `mixengine-proto` on the way, because the sentence a user
      reads about a failure belongs beside the vocabulary and not in each client: `mix` and the GUI
      would otherwise disagree about what `crash_loop` means the week one of them is updated. What is
      left to a client is layout — the `tail` is printed as lines under the sentence, never inside it.
      **One thing this leaves for Phase 3 rather than deciding now.** `mix service start caddy
      mariadb redis` in [../features/services.md](../features/services.md)'s acceptance criteria names
      three services in one command, and `ServiceTarget` carries **one** id or all of them. The CLI
      follows the wire type rather than papering over it with three calls that would each be a
      separate plan; making a target a set is T19a's type to change, and T30 is when something needs
      it.
- [x] **T16b** `GET /logs/{id}?tail=N&follow=1`, and `mix service logs`.
      **There is no `DaemonEvent::LogLine`, and that is the task's first output.** The question this
      was held back for is answered in
      [ADR 0009](../decisions/0009-logs-travel-on-their-own-stream.md): output travels on its own
      stream, `/events` keeps its 1024 messages for state, and both architecture documents are
      corrected rather than left promising an event nobody should build. The alternative — per-kind
      subscription on `/events` — loses to the case it was meant to fix: a GUI with a log panel open
      *and* a service list visible subscribes to both, and is back to a chatty service spending its
      state allowance on one shared broadcast. `service.logs` left the namespace with it: a JSON-RPC
      call cannot stream, and `?tail=N` with no `follow` is the snapshot such a method would have
      been.
      **The daemon keeps a ring per service that outlives any one run of it**
      (`services/logs.rs`), which is the piece neither T16 nor T19 had. A `Capture` belongs to one
      run of the process and dies with it, so a `follow` reading it would end at every crash; what a
      client subscribes to is the registry's, and each attempt relays its capture into it. That also
      makes the seam impossible: `ServiceLog::read` hands over the tail *and* the subscription under
      one lock, so no line can arrive between the two and none can be delivered twice — which is what
      the integration test asserts by counting a once-printed line across a whole connection.
      **A relay task rather than a fourth sink inside the capture.** The reader threads run outside
      the runtime and their one obligation is to drain a pipe the service blocks on; putting the
      daemon's locks on the path of every line would let a moment's contention stall the process
      itself. What the relay costs the threads is one more subscriber on a send they already make.
      **What the file can honestly answer is less than it looks.** `current.log` is the service's own
      output and carries no timestamp and no stream tag — deliberately, since T16 — so a line read
      back out of it cannot be a `LogFrame::Line`. It is a `LogFrame::Historic`, carrying only the
      text it really has, and the two are never stitched together: the ring answers or the file does.
      Left for later, deliberately: **no merged endpoint** (`/logs?service=a&service=b`). A GUI
      watching N services opens N connections, which is N pipe instances on Windows and cheap
      everywhere else; the merged shape can be added without revisiting the ADR, and nothing before
      the GUI's log panel needs it.
- [ ] **T16c** Let a service's first lines reach the ring and not only its file.
      **Seen once on Windows CI and not reproduced since.**
      `a_follow_hands_over_the_tail_and_then_carries_on_from_it` failed one run of the T32 branch on
      a `LogFrame::Historic` where it asserts every tail frame is the daemon's own `Line` — so the
      ring was *entirely* empty when the connection arrived, although `service.start` had already
      answered `wait: true` and `current.log` held the lines the file reader served instead. It
      passed five runs out of five locally, and on the CI runs either side of that one, so what is
      written here is the mechanism the code supports rather than a diagnosis a test forced.
      **The daemon's ring is fed by a task ordered against nothing.** `Capture::start` puts the
      reader threads on the pipes before it returns, and from that moment they append to
      `current.log` and broadcast; `Runner::relay` subscribes to that broadcast *afterwards*, inside
      a `tokio::spawn` the runtime is free not to poll for as long as it likes. A `broadcast`
      delivers nothing to a receiver that did not exist when the line was sent, so every line printed
      in that window reaches the file and never the ring — permanently, not until something catches
      up. It is the window a service's *first* lines fall in, which are the ones that explain a start
      nobody was watching.
      **The fix this should start from is the one `ServiceLog::read` already is.** `Capture` keeps a
      ring of its own that the reader threads fill synchronously, so `subscribe` can hand over that
      ring *and* the receiver under one lock, exactly as `read` hands a client its tail and its
      subscription — and `relay` records what it is given before it begins pumping. No line can
      arrive in between and none is delivered twice, and the capture still knows nothing about the
      daemon-side log. The alternative — reading `current.log` to fill a ring that is short — is
      worse than it looks: it is the "the ring answers or the file does" rule in
      `services/logs.rs` given up in order to work around a gap that has a real fix.
      **Not caused by T32**, whose run happened to catch it and which changed nothing on this path,
      and left without a test because provoking it means losing a race deliberately and none of the
      timing this suite can reach does that.
- [x] **T18** Crash recovery: PID + start-time adoption, stale socket/pidfile cleanup on daemon boot.
      **Moved below T19 on purpose**, where it can be proved rather than only written: a survivor is
      a `services` row with `state = 'running'`, a `pid` and a `pid_start_time`, and until T19 starts
      something there is no such row to meet. The pair is the point — a pid alone is reused by the OS
      within minutes, and signalling the wrong process is the one accident this product cannot have —
      so reading a process's start time is the platform work this task owns **(P)**, and it is also
      what closes the macOS gap [ADR 0007](../decisions/0007-supervised-child-owns-a-process-group.md)
      writes down honestly instead of averaging.
      **One platform reading answers both halves of the question**, which is why `Adopted` needs
      almost no per-OS code: `process::started_at` says when the process bearing a pid began, so
      *identity* is that value matching what was recorded and *liveness* is there being a value at
      all. Everything that is not a running process is `None` rather than an error, and the two cases
      that had to be found rather than designed are both corpses the OS still remembers — a Unix
      zombie, and a Windows process object kept openable by a handle somebody else holds. Either
      would have been adopted, supervised for ever and never restarted; the state field and the exit
      time are what rule them out.
      **Linux is the one system whose value is boot-relative** — `/proc/<pid>/stat` field 22 is clock
      ticks since boot, where Windows and macOS both report a wall-clock moment — so a pid *and* a
      centisecond colliding across two boots is a residual this task accepts and writes down, in the
      same spirit as ADR 0007's pid-recycling race. The obvious fix is worse: building an absolute
      moment out of `/proc/stat`'s `btime` would refuse to adopt a healthy service after any clock
      step, trading a rare wrong identification for a rare killed database.
      **Three outcomes, not the two the architecture document names.** Adopted, cleared — and
      *stopped*, which is the survivor this daemon cannot supervise: one nothing declares any more,
      and one left in `starting`, `stopping` or `restarting`, whose readiness was never decided and
      cannot be re-decided without the pipes that went with the old daemon. Leaving either running
      would leave the port and the data directory held by exactly what the next start collides with.
      They are killed **without a grace period**, deliberately: a boot is not the moment to spend one
      `StopBehaviour` per service on processes the daemon has already decided to abandon, and the
      cost — a database recovering on its next start — is stated in the row rather than hidden, as
      a stop command that cannot be run already is. `StateReason` grew the two words for it, `Vanished` and
      `Unadopted`, and `ServiceState::is_supervised` is the set being reconciled at all.
      **Adoption writes no transition**, which is the difference between adopting and restarting:
      nothing happened to the service, its row said `running` before this daemon existed and says
      `running` still, and an event there would tell a client that a service somebody has been using
      all day had just started. What *is* published is the readiness, because that lives in the new
      process and only it has just learned it.
      **A survivor that will not die leaves its row where it is**, on both paths — the runner's stop
      of a service it adopted, and recovery's refusal of one it will not: the row keeps its pid and
      the state it was found in, and a line says so, because recording `Stopped` for a process still
      holding the port is the orphan this task exists to prevent, written down as a fact. The next
      daemon then meets a case recovery already handles, which makes the failure self-healing rather
      than a lie. Both wait for the process to have actually gone before they write anything, which
      is the ordering that makes that true.
      **Stopping a foreign process falls back from the group to the process, and CI is what found
      it.** `kill(-pid, …)` is what every stop in this crate sends, and it rests on a survivor being
      a `spawn_supervised` child whose `setsid` made its pgid its pid. When no group has that id the
      kernel answers `ESRCH` — indistinguishable from a group that has already gone, which is
      forgiven — so a process leading no group was signalled, forgiven and left running. Windows
      never showed it, `TerminateProcess` taking a pid; ubuntu and macOS failed all three
      stop-a-survivor tests at once. The group is still tried first, because it is what reaches the
      workers; the process alone is tried when there was no group to reach. What makes signalling a
      bare pid defensible is that `Adopted` re-reads the identity immediately before every signal
      rather than trusting the one it was built with, which narrows the recycling window to the two
      instructions between the check and the `kill` — the same residual ADR 0007 already accepts.
      Adopting means more than believing the row: a survivor's pipes belong to a daemon that is gone,
      so its log capture cannot be resumed and only its liveness and its exit can be observed. What
      that costs a user is the honest sentence T47 owes `mix doctor`. The health check is left out
      for a reason of its own rather than by inheritance — a TCP probe would work perfectly well, and
      a service degraded by one would be put back by its policy on evidence this daemon has no log to
      explain. The moment the adopted process ends, everything is ordinary again: the policy decides,
      and what it starts is a child of *this* daemon with its pipes, its group and its capture
      restored, which is why the runner splits into `adopt` and `live` rather than growing a second
      loop.
      An adopted exit **carries no code on any platform**, although Windows would give one through a
      handle this could keep open. Uniform on purpose: a restart policy that behaved differently on
      one system for a service that merely disappeared is a difference nobody could act on.
      **The stale socket and pidfile half of the title needed no code, and that is worth saying once
      rather than discovering twice.** `ipc::Listener::bind` already unlinks a socket nothing answers
      on and binds again (T7), and there is no pid file to go stale: `run/mixengined.lock` is an open
      handle the OS releases even for a daemon that was killed, so the file surviving means nothing
      and its contents are rewritten by whoever takes the lock next (T9).
      **The M1 test makes its own survivors, and that is not a shortcut.** What a killed daemon
      leaves behind differs by system — everything dies on Windows, the immediate child on Linux,
      nothing on macOS — so a test that produced one by killing a daemon would assert three different
      things and prove the recovery on one of them. Started by the test, both cases exist on every
      system, and neither reaches the code under test as anything but a row.

**Milestone M1 — reached.** Kill the daemon mid-run; on restart it adopts what survived and cleans
what did not. Proven against `fakeservice` by
`crates/mixengine-daemon/tests/lifecycle.rs`, green on ubuntu, windows and macos.

The survivors in that test are its own children rather than the killed daemon's, and the milestone is
worth reading with that in mind: what is proved on all three systems is the *recovery*. Whether a
daemon's own child is still there to be recovered is the question [ADR
0007](../decisions/0007-supervised-child-owns-a-process-group.md) answers three different ways, and
`crates/mixengine-testkit/tests/supervision.rs` is where each of the three is held.

---

Previous: [Phase 0 — Foundations](phase-0-foundations.md) · Next: [Phase 2 — Runtimes](phase-2-runtimes.md)
