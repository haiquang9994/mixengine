# Daemon and IPC

## Transport

One transport abstraction, two implementations:

| OS | Endpoint | Access control |
| --- | --- | --- |
| Linux / macOS | Unix domain socket at `<root>/run/mixengined.sock` | socket mode `0600`, owner-only; peer credentials checked via `SO_PEERCRED` / `LOCAL_PEERCRED` |
| Windows | Named pipe `\\.\pipe\mixengine.<user-sid>` | DACL granting only the current user SID |

The daemon **never** opens a TCP port for its API by default. `--listen 127.0.0.1:PORT` exists for
debugging and for remote-container setups; when enabled it requires a bearer token from
`<root>/run/api.token` (mode `0600`).

## Protocol

JSON-RPC 2.0 framed over HTTP/1.1 (`hyper` over the local transport):

- `POST /rpc` — single call or batch.
- `GET /events` — Server-Sent Events stream of `DaemonEvent`.
- `GET /logs/{service_id}?follow=1` — chunked log tail (also SSE-framed).
- `GET /health` — unauthenticated liveness probe, used by clients to decide whether to autostart the
  daemon.

HTTP is a deliberate choice over a bespoke frame format: it gives us streaming, back-pressure, and
off-the-shelf clients for the GUI, the CLI, and future extensions with zero extra code.

## Method namespaces

Methods are `namespace.verb`. All types are defined in `mixengine-proto`.

```
daemon.*     status, version, shutdown, doctor, doctor_repair
runtime.*    list_available, list_installed, install, uninstall, set_default, resolve
service.*    list, start, stop, restart, reload, status, logs, config_get, config_set
project.*    list, create, import, delete, get, set_runtime
site.*       list, create, update, delete, start, stop, open, share_lan
domain.*     list, add, remove, dns_status
cert.*       list, issue, renew, ca_status, ca_install, ca_uninstall
blueprint.*  list, capture, apply, export, import, delete
extension.*  registry_list, install, uninstall, start, stop, configure
metrics.*    snapshot, subscribe
```

Rules:

- **Verbs are idempotent where it makes sense.** `start` on a running service succeeds.
- **Long operations return a job.** `runtime.install` returns `{ job_id }`; progress arrives as
  `job.progress` events; `job.wait` exists for scripting. Never block an RPC call for minutes.
- **Every mutating method is expressible in the CLI.** No GUI-only capabilities.

## Events

```rust
enum DaemonEvent {
    ServiceStateChanged { id: ServiceId, from: ServiceState, to: ServiceState, reason: Option<String> },
    SiteStateChanged    { id: SiteId, state: SiteState },
    JobProgress         { job_id: JobId, percent: u8, message: String },
    JobFinished         { job_id: JobId, result: JobResult },
    LogLine             { service_id: ServiceId, stream: Stream, line: String, ts: SystemTime },
    MetricsSample       { sample: MetricsSample },
    CertExpiring        { domain: String, days_left: u16 },
    ElevationRequired   { ops: Vec<PrivilegedOp> },    // GUI turns this into one elevation prompt
}
```

Events are best-effort and **must not** be the only way state is learned: a client that reconnects
calls the matching `*.list` and re-syncs. Slow consumers get dropped, not buffered without bound
(bounded broadcast channel, capacity 1024, lagging receiver gets a `Resync` marker).

## Daemon lifecycle

- **Autostart**: registered at install time — Windows: Task Scheduler logon task (not a service; it
  is user-level); macOS: `~/Library/LaunchAgents/dev.mixengine.daemon.plist`; Linux: systemd *user*
  unit `mixengined.service`.
- **Client autostart**: if a client cannot connect, it spawns `mixengined --detach` and retries with
  backoff for ~5 s. This is why `/health` is unauthenticated.
- **Shutdown**: `daemon.shutdown` stops supervised services in reverse dependency order with a
  configurable grace period (default 10 s) before escalating to kill.
- **Crash recovery**: on boot the daemon reconciles — for every service marked *running* in SQLite it
  verifies the recorded PID still belongs to that process (PID + start-time check, never PID alone),
  adopts it if so, otherwise marks it stopped and cleans stale sockets/pid files.
- **Single instance**: an advisory lock on `<root>/run/mixengined.lock`; a second instance exits 0
  after printing the endpoint.

## Errors

`mixengine-proto::Error` is a closed enum carrying a stable `code` string, a human `message`, and an
optional `hint` the GUI renders as a suggested action:

```
not_found · already_exists · invalid_argument · conflict · precondition_failed
port_in_use · privileged_required · unsupported_platform · dependency_missing
process_failed · io · internal
```

`unsupported_platform` is a first-class result, never a panic — see the cross-platform rule in
[CLAUDE.md](../../CLAUDE.md).
