# Daemon and IPC

## Transport

One transport abstraction, two implementations:

| OS | Endpoint | Access control |
| --- | --- | --- |
| Linux / macOS | Unix domain socket at `<root>/run/mixengined.sock` | socket mode `0600`, owner-only; peer credentials checked via `SO_PEERCRED` / `LOCAL_PEERCRED` |
| Windows | Named pipe `\\.\pipe\mixengine.<user-sid>.<home-fingerprint>` | owner and DACL naming only the current user SID; the client is impersonated and its SID compared, and the client compares the pipe's owner before it sends |

The daemon **never** opens a TCP port for its API. `--listen 127.0.0.1:PORT`, with a bearer token
read from `<root>/run/api.token` (mode `0600`), is a **design and not a build**: T8 left it out on
purpose — *"a second transport and a second access-control story for a case nobody has yet"* — and
nothing since has needed one. It is named here in the future tense deliberately, because a reader who
takes it for the present concludes the API has an authenticated network path. It has none. There is
one transport and it is the one in the table above.

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

**Both of those protect the daemon, so the client has a gate of its own.** They say nothing about a
client that dialled a stranger, and on Windows a stranger is reachable: the pipe namespace is flat,
the name is derivable from a public SID and a fingerprint the source spells out, and
`CreateNamedPipeW` needs no privilege — so another account can hold the name before the daemon comes
up and collect every request, `elevation.*` included. The daemon's own `FILE_FLAG_FIRST_PIPE_INSTANCE`
only stops it from *joining* that pipe. So a client reads the **owner of the pipe object** and hangs
up before the first byte if it is not this account, which fails with "is held by …, not by this
account" rather than with a timeout. The daemon's own start says the same thing: the probe above
already dials the name to tell "taken" from "refused", so it asks who answered while it is there —
"another process is already listening" would send the user looking for a daemon of their own to stop.
The owner is read and not the creating process: a pid can be
reused between being handed over and being looked up, while an object's owner is stamped on at
creation and cannot be set to an account the creator does not hold. For the same reason the daemon
*states* the owner in the pipe's descriptor instead of letting the token's default owner supply one —
that default is a machine policy, and on a machine set to "Administrators" every client would refuse
its own daemon. Unix needs none of this: the socket is a file inside a `run/` this account owns, and
no other account can put one there to be found instead. Found by the 2026-08-27 review as R1.

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
- `GET /logs/{service_id}?tail=N&follow=1` — one service's output, SSE-framed like `/events`:
  `tail` alone is a snapshot that ends, `follow` keeps the connection open. **This is the whole of
  the log surface** — log lines are never events, per
  [ADR 0009](../decisions/0009-logs-travel-on-their-own-stream.md).
- `GET /metrics` — Server-Sent Events stream of `MetricsFrame`, one per reading, while the
  connection is open. **Opening it is the subscription and closing it is the end of it** (T71), which
  is also what puts the daemon on its one-second sampling rate: with nobody watching it measures once
  a minute, which is what the 24-hour history is made of. There is no `metrics.subscribe`, for the
  reason under **Events** below.
- `GET /health` — unauthenticated liveness probe, used by clients to decide whether to autostart the
  daemon.

HTTP is a deliberate choice over a bespoke frame format: it gives us streaming, back-pressure, and
off-the-shelf clients for the CLI, for out-of-repo clients and for extensions, with no extra code.

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

**A member added to a response is optional on the wire, and the protocol version does not bump for
it** — [ADR 0019](../decisions/0019-an-added-response-member-is-optional.md). `#[serde(default,
skip_serializing_if = "Option::is_none")]` on an `Option<T>`, so a peer that predates the member
sends nothing and one that has it encodes exactly what it encoded before. `PROTOCOL_VERSION` is for
the changes an older peer cannot survive — a member removed, a type changed, a meaning changed, a
method's contract changed. `daemon.version` and `GET /health` are the exception in the other
direction: they are what a client reads before it knows whether to trust the rest, so they gain no
member at all.

## Method namespaces

Methods are `namespace.verb`. All types are defined in `mixengine-proto`.

```
daemon.*     status, version, shutdown, doctor, doctor_repair, bundle,
             uninstall_plan, uninstall  (T87; the plan is a strict read, the act is a job that
                                         raises one prompt and then ends this daemon)
runtime.*    list_available, list_installed, install, uninstall, set_default, resolve
path.*       status, install, uninstall
autostart.*  status, enable, disable   (T85b; enable registers and does not start, disable
                                        removes and does not stop — ADR 0016)
service.*    list, start, stop, restart, reload, status, config_get, config_set
database.*   create, client, open       (T77a and T83; `open` starts a process this daemon does not supervise)
                                        all three answer `secret: { service, key }`, the credential's whole
                                        keyring address and never its value — T84. `client` composes it from
                                        the recipe and the service id, so it still reads nothing.
job.*        list, status, wait, cancel
elevation.*  status, grant, drop
project.*    list, create, import, delete, get, set_runtime
site.*       list, create, update, delete, start, stop, open, share_lan
domain.*     list, add, remove, dns_status
cert.*       issue, status, ca_status, ca_rotate, ca_uninstall
blueprint.*  list, capture, apply, export, import, delete
extension.*  registry_list, install, uninstall, start, stop, configure
                                        `plan` answers `homepage`, and — for kind `desktop-app` alone —
                                        `client`, whether the application is on this machine. T84.
metrics.*    snapshot, history          (the live stream is `GET /metrics`, not a method)
```

**Three `cert.*` names this table used to carry do not exist, and each was refused for a reason** —
corrected at T54, which is where the namespace was finished. There is no `cert.list`, because
`cert.status` answers per site and a list that said less would be a second reading of the same
files. There is no `cert.renew`: `cert.issue` already replaces anything inside the renewal window,
and a second name for one operation is two things to keep in step (T52). And there is no
`cert.ca_install`, because installing already happens in two places that must not disagree — every
daemon start, through first-run setup's single grant, and `mix doctor --repair`.

Rules:

- **Verbs are idempotent where it makes sense.** `start` on a running service succeeds.
- **Long operations return a job.** `runtime.install` returns a `JobSummary`; progress arrives as
  `JobProgress` events; `job.wait` exists for scripting. Never block an RPC call for minutes.
  **`job.wait` is the one method that waits on purpose**, and it takes a timeout to stay inside that
  rule rather than outside it — a wait that runs out answers with the job as it stands, and
  `JobState::is_finished` is what a script branches on. The daemon caps what it grants, so a client
  asking for an hour does not get to hold a connection for one (T22). The namespace was missing from
  the table above until that task, which is why `job.progress` is named there as an event: log lines
  and job progress are the two things this document promised as events before either existed, and
  only one of them turned out to belong on the stream — see the note under **Events**.
  **`service.start`, `service.stop` and `service.restart` are the deliberate exception** and take a
  `wait` instead (T19a). Three things separate them from a download: the wait is bounded by the ready
  timeouts the plan's own specs declare rather than by a network; every move inside it is already on
  the event stream, so a blocked call is never an opaque one; and the verdict — what came up, what
  failed, what was blocked behind it — is what gives `mix` an exit code. A job would put that verdict
  behind a second round trip and make every client re-derive "is it finished" for itself. `wait:
  false` is the same answer a job id would have been, for the client that wants it.
- **Every mutating method is expressible in the CLI.** No client-only capabilities — and since
  `mix` is the only client this repository ships, a gap in the CLI is a gap in the product
  ([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)).
- **A method that writes outside `MIXENGINE_HOME` is never called on the daemon's own initiative**
  (T26). `path.*` is the first of them: the daemon fills `<root>/bin` at every start, because that is
  inside the root and is a projection of a table it compiles in, and it puts that directory on the
  user's PATH only when `path.install` asks — a shell profile and a registry hive belong to the
  person, not to a process that happened to start at login. The namespace was missing from the table
  above until T26, on `job.*`'s precedent. **`autostart.*` is the second and the same rule one step
  louder** (T85b): a logon task, a LaunchAgent and a systemd user unit are all outside the home and
  all belong to the person, and a daemon that registered one because it happened to be running would
  be arranging its own future without being asked. Neither namespace is elevated — both write
  something this account already owns.
- **The daemon never raises an elevation prompt on its own initiative** (T40b). It is the same rule
  as the one above and the same reason: everything `mixengine-elevate` will ever do is outside
  `MIXENGINE_HOME` by definition — that is why it needs root. So producers enqueue and only a client
  calls `elevation.grant`, which is also what makes T64's "explain every operation *before* the
  prompt" expressible at all rather than something a client arranges afterwards. `daemon.doctor_repair`
  (T47b) is the second door onto that grant and takes a `grant` flag rather than always flushing, for
  the same reason: a call that enqueued and raised the prompt together would leave no moment in which
  the batch could be shown. A machine where
  nobody ever grants is in degraded mode forever, and that is correct: `daemon.status` says so, and
  `elevation.drop` is the way out for an operation nobody intends to allow.
- **`daemon.bundle` is the one read that writes a file** (T93), and the file is inside
  `MIXENGINE_HOME` — `cache/diagnostics/`, never a path a caller named. The method takes no
  destination on purpose: one would be a way for any local caller to have the daemon write
  wherever that daemon can reach, and a client that wants the archive elsewhere copies it with
  its own permissions. What goes in is a closed list rather than a walk of the home, which is
  what keeps `run/`, `certs/` and `data/` out of an archive somebody emails.

## Events

```rust
enum DaemonEvent {
    ServiceStateChanged { id: ServiceId, from: ServiceState, to: ServiceState, reason: Option<String> },
    SiteStateChanged    { id: SiteId, state: SiteState },
    JobProgress         (JobProgress),                 // { job, percent, message, at }
    JobFinished         (JobFinish),                   // { job, ending, …, at }
    CertExpiring        { domain: String, days_left: u16 },
    ElevationRequired   { ops: Vec<PrivilegedOp> },    // a client turns this into one prompt
}
```

Events are best-effort and **must not** be the only way state is learned: a client that reconnects
calls the matching `*.list` and re-syncs. Slow consumers get dropped, not buffered without bound
(bounded broadcast channel, capacity 1024, lagging receiver gets a `Resync` marker).

**A job's two variants carry the value that was persisted, not a second description of it** (T22) —
the rule `ServiceStateChanged` already followed, so an ending that did not survive its transaction
cannot be announced. `JobId` is the rowid of the `jobs` row, which is why the type could not be
declared before the table existed: naming it here was a promise, and the shape it took was decided by
the code that mints one. Progress is the one thing on this stream allowed to repeat itself, and it is
bounded by its producer rather than by the type — a download reporting every socket read would spend
a client's whole 1024 on a progress bar.

**This stream carries state and nothing else.** An earlier draft of this document listed a `LogLine`
variant here, and a `MetricsSample` one beside it; neither is built and neither will be. Those 1024 messages are 1024 state changes, and a
service in debug mode would otherwise spend a client's whole allowance on output nobody asked for —
losing exactly the transitions the client opened the stream for. Output has its own endpoint, its own
back-pressure and its own subscribers: [ADR 0009](../decisions/0009-logs-travel-on-their-own-stream.md).

**`MetricsSample` was removed at T71 on that same argument, and one more of its own.** Ten services
at a sample a second is ten messages a second onto a bus of 1024 shared by every client: a hundred
seconds of a live view would evict exactly the `ServiceStateChanged` the client opened the stream
for. And a reading is only worth taking while somebody is looking at it, which an event cannot
express — the bus cannot tell a client watching metrics from one listening for state, so turning
sampling on and off would need a `metrics.subscribe`/`metrics.unsubscribe` pair, and a client that
crashed without the second call would leave the machine measured every second for as long as the
daemon ran. `GET /metrics` has no such failure: opening the connection is the subscription, closing
it is the end of it, and a socket cannot forget to close.

## Daemon lifecycle

- **Autostart**: registered when somebody asks, by `autostart.enable`, and by **no installer** —
  [ADR 0016](../decisions/0016-autostart-is-registered-by-mixengine.md). Windows: a Task Scheduler
  logon task named `MixEngine` (not a service; it is user-level); macOS:
  `~/Library/LaunchAgents/dev.mixengine.daemon.plist`; Linux: systemd *user* unit
  `mixengined.service`. Three formats install as root and cannot know which account will use
  MixEngine, three install as the user — so this is MixEngine's to do and nobody else's, which is the
  helper's argument reversed. It registers and does not start; `autostart.disable` removes and does
  not stop.
- **Foreground by default**: `mixengined` runs in the foreground unless given `--detach`. That is
  what every service manager above wants — each supervises the process itself and reads a fork as a
  death — so the flag exists for the one caller that cannot hold the daemon: a client autostarting
  one. **Task Scheduler adds one thing the other two do not**, measured at T85b: it hands a
  console-subsystem program a *visible* console window in the user's session, and `<Hidden>true</Hidden>`
  does not stop it. So the daemon releases a console it is the only process attached to — 1 attached
  process under Task Scheduler, 4 from a shell — which leaves `mixengined` in a terminal exactly as
  it was. See the [T85b design](../../docs/superpowers/specs/2026-09-04-t85b-autostart-design.md), D4.
- **Client autostart**: if a client cannot connect, it spawns `mixengined --detach`, which returns
  only once the daemon answers on its endpoint and prints that endpoint on stdout. No backoff loop in
  the client: the wait belongs to the process that knows whether its child is still alive. This is
  why `/health` is unauthenticated.
- **Stopping**: the OS's own request to stop is honoured — `SIGINT`/`SIGTERM` on Unix, the five
  console control events on Windows — and cancels the daemon's root token, which is what every
  shutdown path in the process is a branch of. `daemon.shutdown` stops supervised services in
  reverse dependency order **first**, cancels the same token afterwards, and answers last: a client
  that was told only "accepted" would have to re-derive from the event stream whether its database
  was flushed or killed, and the connection closing after the answer is the shutdown rather than a
  failure of one. Neither path refuses to stop — a declaration that will not assemble, or a service
  that will not die, is reported and the daemon goes anyway.
- **The shutdown budget is one number with two ceilings.** It bounds the *total* spent stopping
  services, not each one: a service's own grace period says what that service needs, and each gets
  that or whatever is left of the budget, whichever is less. Over the API it is
  `[daemon] shutdown_grace_seconds` (default 10 s) entire. When the OS is the one asking it is the
  smaller of that and what the OS allows — `mixengine_platform::signal::STOP_CEILING`, which is
  about five seconds on Windows and nothing at all elsewhere — less the margin the connections and
  the WAL checkpoint still need after the last service has stopped. A second shutdown may shorten a
  budget and never extend one.
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
optional `hint` a client renders as a suggested action:

```
not_found · already_exists · invalid_argument · conflict · precondition_failed
port_in_use · privileged_required · unsupported_platform · dependency_missing
process_failed · io · internal
```

`unsupported_platform` is a first-class result, never a panic — see the cross-platform rule in
[CLAUDE.md](../../CLAUDE.md).
