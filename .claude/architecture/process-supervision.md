# Process supervision

`mixengine-supervisor` is the only code that spawns long-lived children. It knows nothing about PHP
or MariaDB — it consumes a `ServiceSpec`.

## ServiceSpec

The type lives in `mixengine-proto`, not in `mixengine-core` and not in this crate: those two are
siblings that cannot depend on each other, and one definition has to serve the `services` table, the
GUI's Services screen and an `extension.toml` alike. A spec is a shared vocabulary rather
than a supervisor implementation detail. See
[../decisions/0006-servicespec-in-proto-and-secret-free.md](../decisions/0006-servicespec-in-proto-and-secret-free.md).

```rust
pub struct ServiceSpec {           // fields are private; read through accessors
    id: ServiceId,                 // "php-fpm@8.3", "mariadb@main", "caddy"
    program: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, EnvValue>, // explicit; parent env is NOT inherited wholesale
    cwd: PathBuf,
    ready: ReadyCheck,
    health: Option<HealthCheck>,   // periodic, after ready
    restart: RestartPolicy,
    stop: StopBehaviour,           // signal / graceful command / just kill
    depends_on: Vec<ServiceId>,
    limits: ResourceLimits,        // see features/resource-isolation.md
    idle: Option<IdlePolicy>,      // stop after N minutes without activity
    logs: LogPolicy,
}

pub enum ReadyCheck {
    Tcp { addr: SocketAddr, timeout: Millis },
    UnixSocket { path: PathBuf, timeout: Millis },
    Http { url: String, expect_status: u16, timeout: Millis },
    LogPattern { regex: String, timeout: Millis },
    PidAlive { settle: Millis },       // last resort
}
```

**Waiting for a `ReadyCheck` races three outcomes, not two.** The third is the process *exiting*
while the probe waits — the port was taken, the config did not parse — and it is the most common way
a service fails to start. Treating it as "not ready yet" spends the whole timeout on something that
died in the first second and then reports the wrong thing, which is why `Starting → Restarting` is
an edge in the machine below. The race is biased towards the exit, so a service that printed its
ready line and then died is not called ready.

A `UnixSocket` check **connects** rather than looking for the file: a socket exists from the moment
it is bound and survives the crash of whatever bound it, so its presence answers neither question.
An `Http` check reads its URL once, before anything waits, and then retries the request until the
status the spec expects arrives — a `502` from a service whose own backend is not up yet is the
first second of an ordinary start, not a failure to report. It speaks **plaintext only**: every URL
a spec here has any business naming is on the loopback interface, so an `https://` one answers the
same typed error a check this build cannot make always has, rather than a `todo!()`.

`ReadyCheck` answers *"can I route traffic to it yet"*; `HealthCheck` answers *"is it still fine"*.
They are separate because MariaDB's first boot (schema init) is slow while its steady-state ping is
cheap. `HealthCheck` takes a `HealthProbe` rather than a `ReadyCheck`, because two of those variants
only make sense once: `LogPattern` matches a line printed during startup and would never match
again, and `PidAlive` asks something the supervisor already knows without probing — a health check
that cannot fail reports health it never measured.

Every length of time is `Millis`, not `Duration`, whose serde form is a `{secs, nanos}` object.
It is written on the wire as a plain number of milliseconds and additionally *read* as `"10s"` or
`"500ms"`, which is what an `extension.toml` author writes.

A spec is built once and then only read. `ServiceSpecBuilder::build` is where the invariants are
checked, and the fields are private so that nothing can undo one afterwards — a `pub program` could
be reassigned to a relative path after `build` refused exactly that. A changed service is a new
spec, built and checked the same way.

The checks themselves are `ServiceSpec::validate`, which `build` calls and which is public. A spec
also arrives by `Deserialize` — from an `extension.toml`, from a generated file — and that path
deliberately does not validate, because the error belongs to whoever knows which file it came from.
Those loaders call `validate` rather than restating the rules, so there is one definition of a usable
spec instead of two that drift. `build` itself only adds what a builder can see and a finished spec
cannot: which of `cwd` and `ready` was never set, as opposed to set to something unusable.

**A `services` row is not one of those sources.** It carries `package_id`, `port`, `data_dir`,
`config_overrides_json` and `limits_json`, which is the input to config generation and not a spec;
what turns them into one is T30's generator. The daemon therefore reaches a spec through a port —
`SpecSource`, which is asked for the whole **declared set** rather than for one spec by id, because
what the registry does with the answer is build a `ServiceGraph`, and dependencies, cycles and start
order are properties of a set. So the registry T19 builds depends on the question and not on the
answer: a fixture source under test, the generator in Phase 3.

**Three fields are a program, and the same rule applies to all three**: `program`, the command a
`StopBehaviour::Command` runs, and the one a `HealthProbe::Command` runs. Each must be absolute,
because a relative path is resolved by the OS against the child's `PATH` and working directory at
the moment it runs. Enforcing it on `program` alone would leave two unguarded ways to execute
whatever happens to be first on a `PATH`.

**Two environment names may not differ only by case.** Windows compares them case-insensitively and
Unix does not, so a spec carrying both `Path` and `PATH` is two variables on one OS and one — silently,
whichever the block ended with — on the other. Same trap as the lowercase rule on `ServiceId` below,
and refused for the same reason.

## A service id is also a directory name

`ServiceId` is validated on construction **and** on deserialisation, unlike the rest of the wire
vocabulary, because it names `logs/services/<id>/` and `etc/<id>/`: a bad one is a broken install
rather than a failed lookup, and it fails far from the value that caused it. The shape is `name` or
`name@instance`, each half starting with a lowercase ASCII letter or digit and continuing with those
plus `-`; an instance may also contain `.`, because it carries version numbers (`php-fpm@8.3`).

Three of the rules exist for one OS, which is what makes them worth writing down — each one fails
only on Windows, and only at the moment a directory is created:

- **Lowercase only**, so two ids cannot differ by case alone and then collide on a case-insensitive
  filesystem.
- **No Windows device name**: `con`, `prn`, `aux`, `nul`, `com0`–`com9`, `lpt0`–`lpt9`. `con` is a
  plausible id for a console tool. The list is hand-written and a gap in it is invisible, so
  `every_windows_device_name_is_refused` generates all twenty-four rather than trusting it — the
  first version of the list was missing `lpt8` and `lpt9`. It is matched against the **whole id**,
  because the whole id is the directory name: Windows refuses `con`, not every name containing one,
  so `mariadb@aux` and `lpt1@main` are ordinary ids.
- **No trailing `.`**, which Windows strips from a directory name, making `php-fpm@8.` and
  `php-fpm@8` the same directory by a different route than the case rule already closed.

## A spec cannot express a secret

```rust
pub enum EnvValue {
    Literal { value: String },                // non-secret by contract; also read as a bare string
    Keyring { service: String, key: String }, // resolved at spawn time
}
```

A `value` written beside `from = "keyring"` is **refused**, not ignored. It is the one mistake that
puts a password into a spec, and a deserialiser that dropped the field silently would let one sit in
a manifest looking like it had been accepted.

MariaDB's generated root password lives in the OS keyring, and a `ServiceSpec` names it rather than
holding it. The supervisor resolves a `Keyring` entry through the platform `Keyring` capability at
the moment it builds the child's `Command`; the value exists nowhere else and is never persisted,
serialised or logged.

**A spec's environment is the child's whole environment.** `spawn_supervised` clears the block and
applies what it was handed, so a variable the user exported into the shell the daemon was started
from, or one an installer left in the machine's environment, cannot change what a managed service
does. Underneath it goes a short per-OS floor, taken from the daemon's own environment and only where
it has the variable: `PATH`, `HOME`, `TMPDIR` and the locale on Unix; on Windows those plus the names
a program needs to load the system DLLs it was linked against, `SystemRoot` first among them — a
Windows process with a cleared block cannot initialise Winsock. Nothing in the floor is invented, and
a spec that names one of its variables overrides it.

## State machine

```
Stopped ──start──▶ Starting ──ready──▶ Running ──health fail──▶ Degraded
   ▲                   │                  │                        │
   │                   │ready timeout     │stop                    │restart policy
   │                   ▼                  ▼                        ▼
   └────────────── Failed ◀────────── Stopping ────────────────▶ Restarting
```

The diagram is the shape; `ServiceState::can_become` in `mixengine-proto` is the whole edge set, and
it draws five edges this picture compresses. A process that **exits on its own** goes straight from
`Running` to `Restarting` or `Failed` depending on the policy — it never passes through `Degraded`,
because there is nothing left to be degraded about, and that is the most common thing that ever
happens to a service. The same is true one step earlier: a process that **dies before it is ever
ready** — the port was taken, the config did not parse — goes `Starting → Restarting`, so that a
`RestartPolicy` covers the ordinary way a service fails to come up rather than only the ways it
fails after succeeding once. A **stop arriving mid-flight** takes `Starting` or `Restarting` to
`Stopping` rather than being queued behind a start nobody wants any more.

`Failed` is where a service is kept, not where it is stuck: the only ways out are an explicit
`service.start` (to `Starting`) and an explicit `service.stop` (to `Stopped`, clearing the failure).
And **a state never becomes itself** — a transition is an event, and one that changed nothing would
still be persisted, published and rendered.

`ServiceState` is a *closed* enum, unlike most of the wire vocabulary: a state machine with room for
one more state is one nobody can reason about. The supervisor matches it exhaustively so that adding
a state is a compile error everywhere that has to decide what to do about it, and `services.state`
carries the same closed list as a `CHECK` constraint. The wire spelling and the stored spelling are
one string, not two that can drift.

Every transition is persisted and emitted as `ServiceStateChanged` with a reason — **one value used
twice, not two descriptions of the same event**. `core::services::transition` writes the row and
returns the `ServiceTransition` it wrote, inside a **`BEGIN IMMEDIATE`** transaction: two supervisors
reaching the same service at once serialise at the `BEGIN`, so the second reads what the first
committed and re-judges its move against it. A deferred `BEGIN` — sqlx's default — would not do,
because its `UPDATE` has to upgrade a read snapshot into a write, and in WAL mode that fails with
`SQLITE_BUSY_SNAPSHOT` against any concurrent writer, for which SQLite deliberately does not run the
busy handler. The `UPDATE`'s compare-and-swap on the previous state stays as the assertion that this
is really so. The daemon publishes exactly the returned value. A transition that was not persisted
therefore cannot be announced, and an announcement cannot describe something other than what is in
the database.

Unlike `ServiceState`, the *reason* is open-ended: `StateReason` is `#[non_exhaustive]` and grows
each time a later phase learns to distinguish two failures a user currently sees as one. A client
renders what it knows and shows the state alone for what it does not.

A spec naming a check this build or this machine cannot make is `StateReason::Uncheckable` and not a
ready timeout. The distinction is the whole reason `ready::wait` returns `Error::UnsupportedCheck`
rather than answering "not ready": a check that cannot be made was never going to pass, and calling
it a timeout thirty seconds later sends whoever wrote the spec to look at the service. Both strings
travel from the supervisor's error to the user unchanged, so there is one sentence about it and not
one per layer.

`Degraded` is distinct from `Failed`: the process is alive but failing health checks, which is what
the GUI shows in amber and what `mix doctor` explains.

## Restart policy

```rust
pub enum RestartPolicy {
    Never,
    OnFailure { max_retries: u32, window: Millis, backoff: Backoff },  // default: 5 in 5 min
    Always    { backoff: Backoff },                   // exp 500ms → 30s, ×2, ±20% jitter
}
```

`Backoff`'s multiplier is an integer percentage (`200` doubles) rather than an `f64`: these types are
compared and hashed like everything else in `mixengine-proto`, and a `NaN` arriving from a
hand-written manifest should not be representable at all. Jitter is a percentage of the wait and must
stay **under** 100: at 100 the low end of the spread is zero, which is the tight retry loop an
`initial` of zero is already refused for.

Crash-loop protection: after `max_retries` inside `window` the service goes `Failed` and stays there
until an explicit `service.start`. The window is a field rather than a constant because a service
that crashes once a day is not in a crash loop, and counting since boot would eventually say it
was. The last 200 log lines are attached to the failure reason — `StateReason::CrashLoop` carries a
`tail`, the only variant that carries evidence — so the GUI can show *why* without the user opening
a log viewer.

**Becoming healthy again resets the backoff and not the history.** A service that starts, works for
four seconds and dies, five times in a minute, is exactly the thing the cutoff exists for; a counter
cleared on every success would restart it forever while reporting that all was well. What recovery
does reset is the wait, so the next crash backs off from half a second rather than from thirty.

**An explicit `service.start` is asked of the runner, and it resets the same half.** A service
crash-looping under `Always` never reaches `Failed` and its runner never ends, so a start that could
only *read* it would report the crash the backoff is being served for and spawn nothing, however many
times a person typed it. The registry therefore sends a request into the runner (T19c), which
abandons the rest of the wait, restarts through `StateReason::Requested` rather than
`BackoffElapsed`, and resets the backoff exactly as a recovery does — while keeping the failure
history, because a service somebody has restarted four times has still crashed four times. A start
that names a service pulls in its dependencies, so every service in the plan is asked and not only
the one that was typed.

## Process groups — one per service, and three different promises

Every supervised child leads a group of its own, created at spawn and owned by the handle the
supervisor holds — `mixengine_platform::process::spawn_supervised` returns a `Supervised`, and
**dropping it stops the group**, which is the exact opposite of the `Detached` next to it. One group
per *service*, not one for the daemon: `TerminateJobObject` against a daemon-wide job would mean
"stop everything", so per-service stop would go back to walking a process tree, and Phase 7's
per-service caps ([resource-isolation.md](../features/resource-isolation.md), T68) hang on the very
object created here.

- **Windows**: a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, created before the spawn and
  holding this child and its descendants. Stop is `TerminateJobObject`.
- **Unix**: `setsid()` in `pre_exec`, so the child is a session and process-group leader with
  `pgid == pid` and its own children inherit that group; stop sends `SIGTERM` to `-pgid`, then
  `SIGKILL` after the grace period. `prctl(PR_SET_PDEATHSIG)` on Linux as an extra guard — Linux
  only, so it lives in `linux/process.rs` rather than in `unix/`.

**A stop reaches the group whether or not the leader is still in it.** `Supervised::stop` kills the
group unconditionally, because the state it most often meets is a master that has already died with
its workers still holding the port — a crashed php-fpm, a wrapper script that `exec`ed and went. The
handle remembers that it killed, so it does not do it twice; on Unix that matters because an
*unreaped* leader keeps its pgid reserved, and once it has been waited for the number is the
residual race [ADR 0007](../decisions/0007-supervised-child-owns-a-process-group.md) already accepts.

**Asking politely is not available everywhere.** `process::CAN_ASK_TO_STOP` says whether it is:
`SIGTERM` to `-pgid` on Unix, and **nothing on Windows**, where a console control event would mean
detaching the daemon's own console and disabling its control handler for the length of the call. A
supervisor reads that constant *before* it starts a grace period, so a `StopBehaviour::Signal` on
Windows becomes an immediate kill rather than a five-second wait for a message nobody sent; a service
that has to shut down cleanly there uses `StopBehaviour::Command`, which is what its own project
documents. [ADR 0008](../decisions/0008-no-signal-stop-on-windows.md) has the alternatives that lost.

**"No orphans" is not one sentence, because the mechanisms are not equivalent.** What each system
delivers when the daemon goes away, weakest cell last:

| | daemon exits normally | daemon is killed | grandchildren |
| --- | --- | --- | --- |
| Windows | group dies | group dies (kernel) | covered |
| Linux | group dies | immediate child dies (`PR_SET_PDEATHSIG`) | **not covered** |
| macOS | group dies | **nothing dies** | not covered |

macOS has neither a job object nor `PR_SET_PDEATHSIG`, and that gap is stated rather than papered
over: `mix doctor` and the GUI say which of the three they are on (T47) instead of repeating a
guarantee only Windows keeps. [ADR
0007](../decisions/0007-supervised-child-owns-a-process-group.md) has the reasoning and the
alternatives that lost.

What covers the weak cells is crash recovery, which has to exist anyway for the machine that lost
power: PIDs are always recorded together with process start time, and adoption after a daemon
restart verifies both (see crash recovery in [daemon-and-ipc.md](daemon-and-ipc.md)).

**What an adopted service is, and is not.** `mixengine_platform::process::Adopted` is the third
relationship the platform layer has with a process, after "started and let go" and "started and
owned": one it did not start at all. Its identity is the pid *and* the moment it began — the same
reading answers "is it still there", because a pid that carries a different start time is somebody
else's program — and what can be done with it is that question and a stop. Its output is not
captured, because the pipes belong to a process that no longer exists, so `current.log` has a hole in
it from the moment the old daemon died; its readiness is not re-decided, the check that proved it
being a log pattern most of the time; and it is not health-checked, because a service degraded by a
probe would be put back by its policy on evidence this daemon has no log to explain. All of that ends
the moment the process does: the restart policy decides as usual, and what it starts is a child of
this daemon with everything restored.

## Logs

- stdout/stderr are piped, line-split, tagged with the stream they came from, and written to
  `logs/services/<service-id>/current.log`. The tag travels in the `LogLine` value; the file itself
  carries none, for the reason below.
- **The file is plain text and carries nothing of MixEngine's**: exactly what the service printed,
  one line per line, both streams interleaved in the order they were written. It is read by whoever
  reads MariaDB's or Caddy's log, so a timestamp or a `[stderr]` prefix would break their tools to
  restate what the ring and the event carry anyway — the tag lives in `LogLine::stream`, not in the
  file. Line endings are normalised to `\n`, because the file and the ring hold the same line.
- Rotation: size-based (default 10 MB × 5 files), enforced by the supervisor, not by an external
  logrotate — it is the process holding the handle. `RotatingFile` is one implementation for this
  and for `daemon.log`, and it *reports* a failed rotation rather than writing one: the daemon owes
  that note to its own log in `log.format`'s shape, and a service's file must not be given a
  sentence of ours. A rotation that failed is retried once the file has grown another `max_bytes`,
  not on the next line — a rename that cannot work costs four syscalls per attempt, and a service in
  debug mode writes thousands of lines a second.
- The last N lines (default 500) are kept in a ring buffer in memory so a log panel opens instantly.
  The ring, the line splitting and the subscription landed with T15, which needed them for the
  crash-loop tail and for `ReadyCheck::LogPattern`; T16 added the file and the rotation as a third
  reader of the same stream rather than as a second copy of it, written from the same reader threads
  and under the file's lock, so a line is on disk before it is broadcast and all three agree on the
  order the two streams interleaved in.
- **Output reaches a client on `GET /logs/{id}`, never as an event** — T16b, and
  [ADR 0009](../decisions/0009-logs-travel-on-their-own-stream.md) for why: the event stream is a
  bounded broadcast sized for state, and a service in debug mode would spend every client's
  allowance on it. `?tail=N` is the snapshot the ring answers, `?follow=1` keeps the connection
  open, and the two arrive on one request so there is no seam to lose a line in. A `follow` outlives
  any one run of the service: the fanout it reads belongs to the daemon's registry, so a restart
  swaps the capture underneath it without ending the stream.
- **One reader thread per stream, not a task.** `spawn_supervised` hands back the standard library's
  pipes and an anonymous pipe on Windows cannot be read with overlapped I/O, so there is nothing to
  await. Draining both is not optional either way: a pipe holds tens of kilobytes, after which the
  *service* blocks on its next line and looks exactly like one that has hung.
- **End of file is not the service exiting**, so waiting for the last lines takes a deadline. A pipe
  closes when the last process holding its write end goes, and a crashed service is exactly where
  those differ — nobody killed its group, so a worker it forked still holds the pipe open. That is
  also the moment the crash-loop tail is wanted, so `Capture::finish` waits for a bounded time and
  answers whether it got there; a wait that runs out costs the last few lines, never the daemon.
- A line has a ceiling (8 KiB). A service that never writes a newline would otherwise grow one
  buffer until the machine ran out of memory, with nothing recorded from it at all; past the
  ceiling the run is emitted as a line and the rest continues in the next.
- Service logs are plain text (they are the upstream program's output). **Daemon** logs are
  `tracing` output, JSON when `log.format = "json"`, `--log-format json` or
  `MIXENGINE_LOG_FORMAT=json` asks for it, written to `logs/daemon.log` **and** to stderr at the
  same level — the file is what a bug report can attach, so it never carries colour.
  `daemon.log` rotates by the same rule as a service log (10 MB × 5 copies beside the live file, so
  around 60 MB, `daemon.log.1` … `daemon.log.5`), enforced by the daemon itself for the same reason.
  Around, not at most: a line is never split across two files, so one long backtrace can carry the
  live file past its limit, and so can the next bullet.
- A rotation that cannot happen never costs a log line: the file grows past its limit and says why,
  on both sinks and in the format the rest of the log is written in, once per run of failures. It
  cannot be a `tracing` event — the event would re-enter the writer that produced it.

## Dependency ordering

`depends_on` forms a DAG. Start walks it in topological order and waits for each dependency's
`ReadyCheck`; stop walks it the other way. The graph is `mixengine_core::services::graph`
(`ServiceGraph`, `Plan`) — domain logic over a set of declared specs, so it lives in `core` rather
than in `proto`, which owns the vocabulary, or in the supervisor, which owns no registry.

**A cycle fails when the specs are assembled, never at runtime**, and so do the other two things a
single spec cannot see: an id declared twice, and a dependency naming a service that is not there. A
spec on its own rejects only the case it *can* see — depending on itself — because a cycle is a
property of a set. The error carries the loop written out (`caddy → php-fpm → mariadb → caddy`), not
the fact of one: the names are what say which edge to delete.

**The graph holds edges, not the list as it was written.** `depends_on` is deduplicated as the graph
is assembled, and both directions are kept as sets. `ServiceSpecBuilder::build` already refuses an
edge written twice, but a spec deserialised from a row or an `extension.toml` has been through no
builder — `ServiceSpec` documents that as the loader's job — and counting one edge twice leaves a
service waiting on a dependency that can only ever be discharged once. From the inside that is
indistinguishable from a loop, and would be reported as one with no loop to name. A graph is never
the thing that mistakes an unvalidated spec for a broken one.

**A plan is tiers, not a flat list.** Services in one tier have no path between them, so they may be
started at once; T19's runner walks them one at a time to begin with, and the concurrency M3's
ten-second budget wants is then a change to the walker rather than a recomputation. Within a tier the
order is by `ServiceId`, so the same specs always produce the same plan — a boot order that varies
run to run turns one broken dependency into a bug that only reproduces on somebody else's machine.

**Starting and stopping pull in opposite directions, and neither is the other reversed.** Starting
`php-fpm` pulls in everything it depends on: asking for a service is asking for what it needs.
Stopping `mariadb` pulls in everything that depends on *it*, and takes those down first — a site left
pointed at a database that is going away is worse than one told its database is down. For the whole
set the two orders do coincide; for a subset they name different services, so `stop_order` is
computed from the reverse edges rather than derived from `start_order`.

**A dependency that fails takes its dependents with it, without spawning them.** When a service does
not come up, everything transitively downstream (`ServiceGraph::blocked_by`) goes straight to
`Failed` with `StateReason::DependencyFailed`, naming the direct dependency each one declared. The
alternative — spawning them anyway — is a crash against a database that is not there, a restart
backoff, and a `CrashLoop` a minute later whose tail says `connection refused`: an accurate report of
the wrong problem. Each service names the edge it declared rather than the root of the chain, so four
failures read as four honest sentences leading to the one service to fix.

## Testing

The supervisor is tested against a purpose-built `tests/fixtures/fakeservice` binary that can be told
to: start slowly, never become ready, exit with a code after N ms, ignore SIGTERM, or spawn a child
that outlives it. Every policy above has a test using it. Do not test supervision against real
MariaDB — it is slow and hides races.
