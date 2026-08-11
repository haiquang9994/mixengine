# 0007. A supervised child owns a process group, and "no orphans" means three different things

**Status**: Accepted
**Date**: 2026-08-11

## Context

Roadmap task **T13** implements the promise
[../architecture/process-supervision.md](../architecture/process-supervision.md) makes under *Process
groups — no orphans, ever*: a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` on Windows,
`setsid` plus `PR_SET_PDEATHSIG` on Unix. Writing it forces two questions the sentence leaves open,
and the second one is the uncomfortable half.

**Who owns a group?** The document says "every child is assigned to a Job Object" without saying
whether that is one job for the daemon or one job per service. The two are not variations on a
theme: with a single daemon-wide job, `TerminateJobObject` is unusable — it would take every service
down — so stopping one service goes back to addressing a pid, which is the thing a group exists to
avoid. Unix has no such ambiguity to begin with, because `setsid` gives the child a group of its own
whether anybody wanted one or not.

**What does "no orphans when the daemon dies" actually buy on each OS?** The mechanisms are not
equivalent, and the sentence reads as if they were:

- **Windows.** `KILL_ON_JOB_CLOSE` is a kernel guarantee. The last handle to the job closing — which
  a killed process does as surely as an exiting one — terminates everything in it, grandchildren
  included. Nothing has to run in the daemon for this to hold.
- **Linux.** `setsid` does *not* kill anything when the parent dies; a session is a grouping, not a
  lifetime. `PR_SET_PDEATHSIG` does kill, and it is narrower than it looks: it fires for the
  **immediate child only**, so a php-fpm master dies and its pool workers do not; it is keyed to the
  parent *thread*, so in a threaded daemon it arrives when whichever tokio worker did the spawn
  exits, not when the process does; and it is cleared across a setuid `exec`.
- **macOS.** There is no `PR_SET_PDEATHSIG` and no job object. A supervised child of a killed daemon
  keeps running, and nothing in the child or the kernel will notice.

So the guarantee cannot be stated once. Either the weakest platform is written down honestly, or the
GUI and `mix doctor` end up claiming something that is false on one of the three.

## Decision

**One group per service, created at spawn and owned by the handle the supervisor holds.**
`mixengine_platform::process::spawn_supervised` returns a `Supervised`, and that value *is* the
group's ownership:

- Windows: a Job Object created before the spawn, with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, holding
  exactly this child and its descendants.
- Unix: `setsid` in `pre_exec`, so the child is a session and process-group leader with `pgid ==
  pid`, and its own children inherit that group.

`Supervised` is the mirror image of the existing `Detached`, and the pair is the whole API surface:
**dropping a `Detached` deliberately does not stop the child; dropping a `Supervised` deliberately
does.** The daemon owns what it supervises, and a supervisor handle that went out of scope while its
process kept running would be an orphan produced by the one crate that exists to prevent them.

**The guarantee is stated per OS, not once.** T13 delivers, in order of strength:

| | daemon exits normally | daemon is killed | grandchildren |
| --- | --- | --- | --- |
| Windows | group dies | group dies (kernel) | covered |
| Linux | group dies | immediate child dies (`PR_SET_PDEATHSIG`) | **not covered** |
| macOS | group dies | **nothing dies** | not covered |

`PR_SET_PDEATHSIG` is a Linux-only extra guard and lives in `linux/process.rs`, not in `unix/`, with
the `getppid` re-check that closes the fork/prctl race. macOS gets the `setsid` half and a comment
saying what it does not get.

**What the weak cells rest on instead is T18, and that is not a downgrade.** Crash recovery already
has to exist for the case no mechanism can cover — a machine that lost power, a daemon killed while a
service was mid-start — and it works by recording pid *and* process start time and reconciling both
at boot. A macOS daemon that was killed adopts what survived and stops what it no longer wants,
which is the same code path Windows and Linux run anyway.

**The GUI and `mix doctor` say which of the three they are on.** Claiming a kernel guarantee we do
not have is the specific failure this ADR exists to prevent; the honest sentence is short enough to
show.

## Consequences

**Easy**: `service.stop` is one call against a group on every OS — `TerminateJobObject`, or `kill` to
`-pgid` — so a runtime that forks (php-fpm, a Node cluster) is stopped completely without walking a
process tree. Per-service jobs also make Phase 7's `ResourceLimits` (T68) land where they belong: a
Job Object already takes CPU and memory caps, so the object T13 creates is the one T68 configures,
and per-daemon jobs would have had to be rebuilt into per-service ones to get there.

**Hard / accepted costs**:

- **A window between spawn and assignment on Windows.** `AssignProcessToJobObject` runs after
  `CreateProcessW`, because resuming a `CREATE_SUSPENDED` child needs the thread handle that
  `std::process::Child` does not expose, and taking that handle means reimplementing `Command`
  against raw `CreateProcessW`. A daemon killed inside that window leaves the child behind. Accepted:
  the window is one call wide, and T18 covers exactly this case. Revisit if a spawn ever has to be
  hardened against a hostile child rather than a crashing parent.
- **macOS is genuinely weaker**, and a user reading the Services screen on two machines sees two
  different promises. Accepted rather than papered over. A watchdog in the child was considered and
  rejected below.
- **Three implementations of a sentence that used to be one.** This is the standing cost of
  [0002](0002-cross-platform-from-day-one.md) and is the reason that ADR exists.
- **`Supervised` kills on drop**, which is a destructor with a side effect on another process. It
  checks that the child has not already been reaped first, so the ordinary path — a service that
  exited on its own and was waited for — kills nothing. The residual race is the one every process
  supervisor has: a pid can be recycled between the check and the signal.

## Alternatives considered

- **One Job Object for the whole daemon.** Simpler: one handle, one flag, everything inside it dies
  with the daemon. Rejected: `TerminateJobObject` then means "stop every service", so per-service
  stop returns to addressing a pid and its children — precisely the walk that a group replaces — and
  Windows would model services differently from Unix, which gives each child a group regardless. It
  also forecloses T68, whose caps are per service and configured on a job.
- **A watchdog inside the child on macOS**: poll `getppid()`, or take a `kqueue` `NOTE_EXIT` on the
  parent, and exit when it goes. Rejected: it only works for a child we wrote. MixEngine supervises
  `php-fpm`, `mariadbd` and `caddy`, so the mechanism would have to be a wrapper process injected in
  front of every service — a second supervisor, running on the user's machine, to cover a case T18
  already covers on every OS.
- **A `launchd` user agent per service on macOS**, letting the OS own the lifetime. Rejected for
  Phase 1: it moves state out of the daemon and into plists that
  [../../CLAUDE.md](../../CLAUDE.md)'s "generated config is disposable" rule would then have to
  reconcile, and it makes a service's lifetime survive MixEngine being uninstalled. `ServiceInstaller`
  in [../architecture/platform-abstraction.md](../architecture/platform-abstraction.md) uses launchd
  for the **daemon's** autostart, which is a different question with a different answer.
- **Say "no orphans, ever" and leave it.** Rejected: it is the kind of claim that is true on the
  developer's machine and false on a user's, and `mix doctor` would have no vocabulary for what it
  found.
