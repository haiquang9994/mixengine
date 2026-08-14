# 0009. Log lines travel on their own stream, never on the event stream

**Status**: Accepted
**Date**: 2026-08-14

## Context

Roadmap task **T16b** owns three things: something that publishes a service's output as it arrives,
`GET /logs/{id}?follow=1`, and `mix service logs`. It cannot be built until one question is settled,
because the two obvious shapes differ in what they cost the *rest* of the API rather than in how much
code they are.

`.claude/architecture/daemon-and-ipc.md` lists `LogLine { service_id, stream, line, ts }` among the
`DaemonEvent`s. Taken literally, that puts every line of every running service on `GET /events` — the
one stream the GUI watches to learn that a service went `degraded`, that a certificate is expiring,
that an elevation is needed.

**That stream has properties chosen for state, and they are the wrong properties for output.**
It is a bounded broadcast of capacity 1024, shared by every connected client, and a receiver that
falls behind is handed a `Resync` and loses what it missed. Those are the right rules for state:
transitions are rare, they are the daemon's own, and the correct response to having missed some is to
call `service.list` and rebuild — which is why events are best-effort and never the only way state is
learned.

A log line is none of that:

- **Its volume is decided by somebody else's program.** MariaDB in debug mode, a PHP application
  logging every query, a build script — thousands of lines a second, from a process MixEngine
  supervises but does not write. Nothing about the daemon's own behaviour bounds it.
- **One chatty service would spend every client's allowance.** 1024 messages is a fraction of a
  second of that output. A GUI watching for state changes would be lagged continuously, and what it
  would lose in each gap is precisely the `ServiceStateChanged` events it opened the stream for. A
  `Resync` answers a missed state change; there is no useful answer to a missed log line, and a
  client that resynced would only be told to resync again.
- **Every client would pay for output nobody asked to see.** A `mix status` holding a stream open, a
  GUI on its dashboard, an extension watching for site events — all of them would receive, decode and
  discard the log of every running service.
- **The interesting failure is invisible in development.** With one quiet fixture service, both
  designs behave identically. The difference appears the first time a real database is started in
  debug mode, which is exactly where a GUI must not silently stop learning about state.

There is no back-pressure available on the event stream either: it is one broadcast fanned out to
every connection, so the daemon cannot slow a producer that is a third-party process, and would not
want to — a supervisor that stalls a service's pipe stalls the service.

## Decision

**`DaemonEvent` never carries log lines.** The `LogLine` variant listed in
`.claude/architecture/daemon-and-ipc.md` is not implemented, and that document is corrected rather
than left to be discovered in the GUI. `GET /events` carries state, and its 1024 messages are 1024
state changes.

**`GET /logs/{service_id}` is the whole of the log surface**, SSE-framed like `/events` and carrying
`LogLine` objects:

- `?tail=N` — the last `N` lines, then the stream ends. This is the snapshot a log panel opens with.
- `?follow=1` — the tail, and then every line as it arrives, until the client disconnects or the
  daemon shuts down.

**One connection, one service, and back-pressure per connection.** hyper polls a response body as
fast as its client reads it, so a slow reader on `/logs/caddy` slows *its own* stream and nothing
else — not another client's log, and not anybody's state. A reader slow enough to fall behind the
service's own fanout misses lines and is told so in the stream, in the shape `Resync` has on
`/events`: the gap is stated rather than papered over.

**`service.logs` leaves the `service.*` namespace.** A JSON-RPC method cannot stream, so keeping one
beside the endpoint would mean two ways to ask for the same lines, differing only in that one of them
is worse. `?tail=N` with no `follow` *is* the snapshot method, and it is one round trip either way.

**The tail and the follow are one request and not two.** A client that asked for the last 200 lines
and then subscribed would lose whatever was printed between the two calls, or see it twice, with no
way to tell which. Handing the tail over on the same connection that then carries the live lines is
what makes the seam impossible — the daemon takes both from the running capture under one lock.

**The daemon keeps a ring per service that outlives any one run of it.** That is what the tail is
served from, what makes a `follow` survive a crash and a restart without ending, and what lets
`mix service logs` explain a service that failed ten minutes ago. It is dropped when the runner ends
with nobody watching.

**Where the daemon has nothing of its own, `current.log` answers — and says that is what it is.**
The file is the service's own output and carries no timestamp and no stream tag, deliberately, so a
line read back out of it is a `historic` frame rather than a `line`: only its text is known, and a
stream and a moment invented for it would look like readings. The two are never stitched together —
the ring answers or the file does — because a file something is still appending to gives no honest
place to join them. A `follow` on a service that is not running is not an error either: the stream
stays open and starts carrying lines when the service next starts.

## Consequences

**Easy**: the event stream keeps one meaning and one budget, and nothing a supervised process prints
can cost a client its state. Log volume is bounded by who is actually watching, rather than by who is
connected. The endpoint is a plain HTTP stream, so `curl` reads it, the GUI reads it with
`EventSource`, and `mix service logs` reads it with the client it already has.

**Hard / accepted costs**:

- **A GUI watching N services' logs opens N connections.** On a local socket that is cheap, but it is
  N pipe instances on Windows rather than one. Accepted: the alternative is the shared stream this
  ADR exists to refuse. A merged endpoint (`/logs?service=a&service=b`) can be added later without
  changing this decision, and nothing before the GUI's log panel needs it.
- **There is no "everything that happened" stream.** A client that wants state *and* output correlated
  reads two streams and interleaves them by timestamp. That is the honest shape: they have different
  volumes and different failure modes, and one stream would have to be as fragile as its noisier half.
- **The daemon holds a per-service fanout for as long as somebody is watching**, independent of any
  one run of the service — that is what makes a `follow` survive a restart, and it is a small amount
  of state the registry now owns beyond what is supervising the process.
- **`.claude/architecture/daemon-and-ipc.md` and `process-supervision.md` are edited**, because both
  describe the event that is not being built. An architecture document that keeps promising it would
  be re-implemented by whoever reads it next.

## Alternatives considered

- **Per-kind subscription on `/events`** (`GET /events?kinds=service_state_changed,log_line`).
  Rejected: it makes the protocol bigger to fix the symptom and not the cause. The channel is still
  one bounded broadcast shared by every client, so a GUI that subscribes to both — the log panel is
  open *and* the service list is visible, which is the ordinary case — is back to a chatty service
  spending its state allowance. It would also mean the daemon publishing every line even when nobody
  has subscribed, since a broadcast does not know its receivers' filters.
- **A second broadcast channel for logs, with a much larger capacity.** Rejected: it moves the number
  without changing the shape. Capacity is memory the daemon spends on behalf of clients that may not
  be reading, and any fixed number is one a debug-mode service exceeds in under a second. It also
  keeps every client receiving every service's output.
- **Polling `service.logs` from the GUI.** Rejected: a log panel that updates on a timer is either
  late or expensive, and the ring would have to be re-read and de-duplicated on every poll. The
  supervisor already has a subscription; a poll would be throwing it away.
- **Writing logs only to `current.log` and letting clients tail the file.** Rejected: it puts the
  daemon's storage layout into every client, contradicting the rule that a client renders what the
  daemon returns; it does not work for a GUI that may not share a filesystem in a future remote
  setup; and rotation makes a correct cross-platform tail — a file renamed out from under an open
  handle — a harder problem than the endpoint it would be avoiding.
