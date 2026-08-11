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
also arrives by `Deserialize` — from a `services` row, from an `extension.toml` — and that path
deliberately does not validate, because the error belongs to whoever knows which row or which file it
came from. Those loaders call `validate` rather than restating the rules, so there is one definition
of a usable spec instead of two that drift. `build` itself only adds what a builder can see and a
finished spec cannot: which of `cwd` and `ready` was never set, as opposed to set to something
unusable.

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

## State machine

```
Stopped ──start──▶ Starting ──ready──▶ Running ──health fail──▶ Degraded
   ▲                   │                  │                        │
   │                   │ready timeout     │stop                    │restart policy
   │                   ▼                  ▼                        ▼
   └────────────── Failed ◀────────── Stopping ────────────────▶ Restarting
```

Every transition is persisted and emitted as `ServiceStateChanged` with a reason. `Degraded` is
distinct from `Failed`: the process is alive but failing health checks, which is what the GUI shows
in amber and what `mix doctor` explains.

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
was. The last 200 log lines are attached to the failure reason so the GUI can show *why* without the
user opening a log viewer.

## Process groups — no orphans, ever

- **Windows**: every child is assigned to a Job Object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. If the daemon dies, Windows tears the tree down for us.
- **Unix**: `setsid()` in `pre_exec` to create a process group; stop sends `SIGTERM` to `-pgid`, then
  `SIGKILL` after the grace period. `prctl(PR_SET_PDEATHSIG)` on Linux as an extra guard.
- PIDs are always recorded together with process start time; adoption after a daemon restart
  verifies both (see crash recovery in [daemon-and-ipc.md](daemon-and-ipc.md)).

## Logs

- stdout/stderr are piped, line-split, tagged, and written to
  `logs/services/<service-id>/current.log`.
- Rotation: size-based (default 10 MB × 5 files), enforced by the supervisor, not by an external
  logrotate.
- The last N lines (default 500) are kept in a ring buffer in memory so `service.logs` and the GUI
  log panel are instant, and `LogLine` events stream new lines to subscribers.
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
`ReadyCheck`; stop walks it in reverse. A cycle is a programming error and fails at spec-build time,
not at runtime.

## Testing

The supervisor is tested against a purpose-built `tests/fixtures/fakeservice` binary that can be told
to: start slowly, never become ready, exit with a code after N ms, ignore SIGTERM, or spawn a child
that outlives it. Every policy above has a test using it. Do not test supervision against real
MariaDB — it is slow and hides races.
