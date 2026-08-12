# 0008. A service is asked to stop with a signal on Unix and with a command on Windows

**Status**: Accepted
**Date**: 2026-08-12

## Context

Roadmap task **T15** owns stopping a service, and `StopBehaviour` in `mixengine-proto` offers three
ways to do it: send a signal and wait out a grace period, run a command and wait out a grace period,
or kill outright. T13 deferred one question to here in as many words —
`windows/process.rs` says a supervised child is deliberately started *without*
`CREATE_NEW_PROCESS_GROUP` because "stopping a service politely on Windows is roadmap task T15's
question and it should be answered there rather than pre-empted by a flag set here."

The question is what `StopBehaviour::Signal` means on a system that has no signals.

**Unix has a direct answer.** `kill(-pgid, SIGTERM)` reaches every process in the group the child
leads: the php-fpm master and every worker it forked, the wrapper script and the `mariadbd` it
started. Each may flush, close its sockets and remove its pidfile. `SIGKILL` to the same group ends
the grace period.

**Windows has one signal-shaped mechanism and it travels through a console.**
`GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pgid)` delivers to a process group *attached to the
calling process's console*. Two facts about MixEngine make that unreachable:

- `mixengined` is started detached, with `DETACHED_PROCESS`. It has no console.
- A supervised child is started with `CREATE_NO_WINDOW`, which gives it a console of its own that
  nobody is attached to and no window is shown for.

Sending an event from the daemon to a service therefore means, per stop: `FreeConsole`,
`AttachConsole(child_pid)`, `SetConsoleCtrlHandler(NULL, TRUE)` so the event does not end the daemon
itself, `GenerateConsoleCtrlEvent`, then undoing all three. Every one of those is **process-wide**
state, changed from one thread of a daemon that is supervising other services on the others, and a
process may be attached to at most one console at a time. A second service stopping concurrently
would find the console already swapped. A crash between the calls leaves the daemon with a foreign
console and no control handler.

And it buys little where it works: `mariadbd`, `caddy` and `php-fpm` on Windows do not shut down
cleanly on `CTRL_BREAK` the way their Unix builds do on `SIGTERM`. The documented graceful stop for
each of them on Windows is a command — `mariadb-admin shutdown`, `caddy stop`.

## Decision

**`mixengine_platform::process::CAN_ASK_TO_STOP` states, per OS, whether a group can be asked to
stop at all: `true` on Unix, `false` on Windows.** `Supervised::ask_to_stop` sends `SIGTERM` to the
group on Unix and returns `Error::UnsupportedPlatform` on Windows rather than succeeding quietly — a
grace period spent waiting for a message nobody sent is time added to every stop.

**The supervisor reads that constant before it starts a grace period.** On a system that answers
`false`, a `StopBehaviour::Signal` degrades to a kill *immediately*, and the service's log says so.

**A service that must shut down cleanly on Windows uses `StopBehaviour::Command`.** That variant
already exists, already carries its own grace period, and is what the upstream projects document.
Phase 3's service specs are written that way: MariaDB and PostgreSQL get a command on every OS,
because their own client tool is a better shutdown than a signal anywhere; the rest get `Signal`,
which on Windows means the kill they would have got in the end.

**No `CREATE_NEW_PROCESS_GROUP` on a supervised child.** With no way to send a console event, the
flag only makes the child unreachable by a Ctrl-C the daemon could never send it, and leaving it off
keeps the spawn identical to what T13 tested.

## Consequences

**Easy**: one call, `ask_to_stop`, with the same meaning wherever it is available, and no
process-wide console juggling anywhere in the tree. The stop path is short on both systems, and a
stop on Windows is *faster* than on Unix rather than slower, because no grace period is spent on a
request that could not be delivered.

**Hard / accepted costs**:

- **A service with no shutdown command loses its data buffer on Windows.** It is killed, so anything
  it had not flushed is gone. Accepted for services where that is cheap (Caddy, Memcached, Mailpit)
  and avoided by giving a command to the ones where it is not. Redis is the case to watch: its
  Windows builds are unofficial, and a `redis-cli shutdown save` belongs in its spec for this reason
  rather than as a nicety.
- **Two shapes of spec for the same service across OSes.** `StopBehaviour` is per spec and specs are
  built per platform already (paths differ, sockets do not exist on Windows), so this adds no
  mechanism — but it is one more thing a spec author has to know, and
  [../architecture/process-supervision.md](../architecture/process-supervision.md) says it where they
  will look.
- **A capability constant the caller has to remember to read.** A supervisor that ignored it would
  get a typed error rather than a silent no-op, which is the failure mode worth having; a test in
  `crates/mixengine-testkit/tests/supervision.rs` asserts the Windows half the way ADR 0007's macOS
  gap is asserted, so the day this becomes possible a test fails and names this document.

## Alternatives considered

- **Attach to the child's console for the length of the call**, as described above. Rejected: it
  mutates process-wide state — console attachment and this daemon's own control handler — from one
  thread of a multi-service supervisor, cannot be done for two services at once, and leaves the
  daemon in a foreign console if it crashes mid-sequence. The behaviour it buys is not the graceful
  shutdown the upstream programs actually document.
- **A helper process per service that owns a console and relays the event.** Rejected: a second
  supervisor on the user's machine, one process per running service, to deliver a message the target
  programs largely ignore. It also breaks the job object's shape by putting a process between the
  daemon and the service.
- **Windows message-based shutdown (`WM_CLOSE`/`WM_QUIT`).** Rejected: it reaches windows, and a
  managed service has none — these are console programs started without one on purpose.
- **`StopBehaviour::Signal` silently doing nothing on Windows and then killing after the grace
  period.** Rejected as the worst of both: every stop on Windows would take the full grace period,
  and the log would say the service was asked and refused when it was never asked. The whole point of
  a typed `Unsupported` answer is that the caller can shorten the path instead of waiting out a lie.
