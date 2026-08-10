# Process supervision

`mixengine-supervisor` is the only code that spawns long-lived children. It knows nothing about PHP
or MariaDB — it consumes a `ServiceSpec` produced by `mixengine-core`.

## ServiceSpec

```rust
pub struct ServiceSpec {
    pub id: ServiceId,                 // "php-fpm@8.3", "mariadb@main", "caddy"
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>, // explicit; parent env is NOT inherited wholesale
    pub cwd: PathBuf,
    pub ready: ReadyCheck,
    pub health: Option<HealthCheck>,   // periodic, after ready
    pub restart: RestartPolicy,
    pub stop: StopBehaviour,           // signal / graceful command / just kill
    pub depends_on: Vec<ServiceId>,
    pub limits: ResourceLimits,        // see features/resource-isolation.md
    pub idle: Option<IdlePolicy>,      // stop after N minutes without activity
    pub logs: LogPolicy,
}

pub enum ReadyCheck {
    Tcp { addr: SocketAddr, timeout: Duration },
    UnixSocket { path: PathBuf, timeout: Duration },
    Http { url: String, expect_status: u16, timeout: Duration },
    LogPattern { regex: String, timeout: Duration },
    PidAlive { settle: Duration },     // last resort
}
```

`ReadyCheck` answers *"can I route traffic to it yet"*; `HealthCheck` answers *"is it still fine"*.
They are separate because MariaDB's first boot (schema init) is slow while its steady-state ping is
cheap.

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
    OnFailure { max_retries: u32, backoff: Backoff },  // default: 5, exp 500ms → 30s, jitter
    Always    { backoff: Backoff },
}
```

Crash-loop protection: after `max_retries` inside a 5-minute window the service goes `Failed` and
stays there until an explicit `service.start`. The last 200 log lines are attached to the failure
reason so the GUI can show *why* without the user opening a log viewer.

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
