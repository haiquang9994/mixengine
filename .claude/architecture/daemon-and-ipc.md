# Daemon and IPC

## Transport

One transport abstraction, two implementations:

| OS | Endpoint | Access control |
| --- | --- | --- |
| Linux / macOS | Unix domain socket at `<root>/run/mixengined.sock` | socket mode `0600`, owner-only; peer credentials checked via `SO_PEERCRED` / `LOCAL_PEERCRED` |
| Windows | Named pipe `\\.\pipe\mixengine.<user-sid>.<home-fingerprint>` | DACL granting only the current user SID; the client is impersonated and its SID compared |

The daemon **never** opens a TCP port for its API by default. `--listen 127.0.0.1:PORT` exists for
debugging and for remote-container setups; when enabled it requires a bearer token from
`<root>/run/api.token` (mode `0600`).

**The address identifies the home, not just the machine.** On Unix that is free — the socket is a
file inside the home — and on Windows it is not: the pipe namespace is flat and machine-wide, so the
name carries a short fingerprint of `<root>/run` alongside the SID. Without it a daemon started with
`MIXENGINE_HOME` pointing at a sandbox would collide with the real install, and two tests would
collide with each other. (The original spec said `<user-sid>` alone; corrected in T7.)

**Two gates, and the second one only ever confirms the first.** Endpoint permissions are the
control that matters, because the kernel enforces them before any MixEngine code runs. The peer
check on top of them exists to notice when they were not applied the way we think — a socket
restored with somebody else's mode, a pipe whose DACL a future change got wrong. It answers *who is
this*, never *what may they do*: every client is the user, and the user may do everything. A
connection from another account is closed and logged, not an error.

**A leftover endpoint is cleaned up; a live one is never touched.** A socket file outlives the
daemon that bound it, and Windows reports a name already taken as `ERROR_ACCESS_DENIED` — the same
answer a genuine permission problem gives. Both are resolved the same way, by dialling the endpoint
before doing anything to it: something answers and the start fails with "already listening";
nothing answers and the corpse is removed. This is not the single-instance guarantee — that is the
lock below — only the far commoner case of one daemon starting after another one died. **A second
daemon never reaches it**, because the lock is taken first and answers the same question earlier and
without a race; what is left for this path is a stranger on the endpoint, which is a failure and
stays one.

## Protocol

JSON-RPC 2.0 framed over HTTP/1.1 (`hyper` over the local transport):

- `POST /rpc` — single call or batch.
- `GET /events` — Server-Sent Events stream of `DaemonEvent`.
- `GET /logs/{service_id}?follow=1` — chunked log tail (also SSE-framed).
- `GET /health` — unauthenticated liveness probe, used by clients to decide whether to autostart the
  daemon.

HTTP is a deliberate choice over a bespoke frame format: it gives us streaming, back-pressure, and
off-the-shelf clients for the GUI, the CLI, and future extensions with zero extra code.

**The HTTP status describes the envelope; the JSON-RPC error describes the call.** A method that
fails is a `200` carrying an `error` member — the request was delivered, parsed and answered. The
statuses that do appear are all about the envelope: `204` for a body of nothing but notifications
(the spec returns nothing for those, and an empty `200` would hand zero bytes to a client that parses
every response), `400` for a body that could not be read, `404` for a route that is not here, `405`
with `Allow`, `413` past the 1 MiB body limit. Their bodies are the plain `Error` below, not a
JSON-RPC response: there is no `id` to answer. `/health` answers `HEAD` as well as `GET`.

**A notification is a request with no `id` member — `"id":null` is not one.** The spec discourages a
null id and nowhere lets it mean silence, so a call that carries one is answered, to the id it gave.
The two are indistinguishable once a request is decoded (`Option<Id>` reads both as `None`), which is
why the daemon decides it from the undecoded JSON.

**Two error codes, and only one of them is MixEngine's.** JSON-RPC requires `error.code` to be an
integer, so it is one: the five reserved values, plus `-32000` for everything MixEngine itself
refuses. The stable string from the closed set below travels in `error.data.code`, with `data.hint`
beside it, and *that* is what clients branch on. `error.message` is the sentence, written once.

**Events are internally tagged and carry no SSE `event:` line** — one `data:` line holding
`{"type": "…", …}`, so a client needs one handler rather than one subscription per variant, and a
variant added in a later phase arrives at an older client as an object it can ignore instead of as an
event type it never subscribed to. An idle stream sends a `:` comment every 15 s so that a live
connection stays distinguishable from a dead one. (Both settled in T8.)

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
  **`service.start`, `service.stop` and `service.restart` are the deliberate exception** and take a
  `wait` instead (T19a). Three things separate them from a download: the wait is bounded by the ready
  timeouts the plan's own specs declare rather than by a network; every move inside it is already on
  the event stream, so a blocked call is never an opaque one; and the verdict — what came up, what
  failed, what was blocked behind it — is what gives `mix` an exit code. A job would put that verdict
  behind a second round trip and make every client re-derive "is it finished" for itself. `wait:
  false` is the same answer a job id would have been, for the GUI that wants it.
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
- **Foreground by default**: `mixengined` runs in the foreground unless given `--detach`. That is
  what every service manager above wants — each supervises the process itself and reads a fork as a
  death — so the flag exists for the one caller that cannot hold the daemon: a client autostarting
  one.
- **Client autostart**: if a client cannot connect, it spawns `mixengined --detach`, which returns
  only once the daemon answers on its endpoint and prints that endpoint on stdout. No backoff loop in
  the client: the wait belongs to the process that knows whether its child is still alive. This is
  why `/health` is unauthenticated.
- **Stopping**: the OS's own request to stop is honoured — `SIGINT`/`SIGTERM` on Unix, the five
  console control events on Windows — and cancels the daemon's root token, which is what every
  shutdown path in the process is a branch of. `daemon.shutdown` cancels the same token and
  additionally stops supervised services in reverse dependency order with a configurable grace
  period (default 10 s) before escalating to kill.
- **Crash recovery**: on boot, before the first client is served, the daemon reconciles every service
  whose row claims a supervisor — *starting*, *running*, *degraded*, *stopping*, *restarting*. It
  verifies the recorded PID still belongs to that process (PID + start-time check, never PID alone)
  and there are **three** outcomes, not two: a survivor that is *running* or *degraded* and still
  declared is **adopted** and supervised again with no state change written, because nothing happened
  to it; a survivor that is not — nothing declares it any more, or it was left mid-start or mid-stop,
  where readiness can no longer be decided — is **stopped**, since leaving it would leave the port
  and the data directory held against the next start; and a row whose process is gone is **cleared**
  and marked stopped, with nothing signalled at all. An adopted service is watched for its liveness
  and its exit only: its pipes went with the daemon that started it, so its log is not captured until
  its restart policy next starts it here. Stale endpoint files need no step of their own — the
  listener already unlinks a socket nothing answers on and binds again, and the lock below is a
  handle rather than a pid file.
- **Single instance**: a lock held on `<root>/run/mixengined.lock` for the life of the process —
  `flock` on Unix, an exclusive share mode on Windows — so the OS releases it even when the daemon is
  killed. A second instance exits 0 after printing the endpoint: it was asked for a running daemon
  and there is one. The lock is taken **before SQLite is opened**, because `sqlx-sqlite` implements
  the migration lock as a no-op and two daemons that got that far could both migrate the same
  database. The file's contents are the holder's pid, for the message; its *existence* means
  nothing. One consequence differs by OS and is left as it is: the Windows share mode withholds
  `FILE_SHARE_DELETE`, so a home cannot be deleted while its daemon runs, where on Unix an `rm -rf`
  of a live home succeeds and the daemon carries on writing into files that have no names.

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
