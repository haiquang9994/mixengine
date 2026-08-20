# 0010. A child started to run a user's software never inherits Administrators

**Status**: Accepted
**Date**: 2026-08-20

## Context

Roadmap task **T34** adds PostgreSQL to the catalogue, and PostgreSQL will not start under the token
this repository's Windows CI leg runs with.

`postgres` calls `check_root()` before it dispatches a mode, and on Windows that asks
`pgwin32_is_admin()`: a process whose token holds `BUILTIN\Administrators` as an **enabled** group is
refused with *Execution of PostgreSQL by a user with administrative permissions is not permitted*.
The refusal is unconditional. Reading `src/backend/main/main.c` on `REL_18_STABLE`, exactly two
invocations clear the `do_check_root` flag — `--describe-config`, and `-C var` as the first argument
— so `postgres --single`, which the first-run ritual feeds an `ALTER ROLE`, is refused on the same
terms as the server.

PostgreSQL's own tools do not meet this, and the reason is worth stating because it is what makes
this ADR cheap: `initdb`, `pg_ctl` and their siblings call `get_restricted_token()` in
`src/common/restricted_token.c` and re-launch *themselves* from a restricted copy of their token.
They de-elevate on purpose. The server is the one binary that refuses instead.

An ordinary user never meets it either. An interactive administrator on Windows carries a UAC-
*filtered* token, where `BUILTIN\Administrators` is present but marked deny-only and grants nothing;
`pgwin32_is_admin()` answers no, and PostgreSQL starts. The machine that meets it is CI: GitHub's
Windows runner executes steps with a full, unfiltered token, and this repository asserts that it
still does rather than assuming it (T2b, `.github/workflows/ci.yml`). Without an answer here, the
Windows leg could not run the PostgreSQL suite at all — which is the leg the suite exists for.

## Decision

**Every process MixEngine starts in order to run a user's software is created from a restricted copy
of the daemon's own token**, with `S-1-5-32-544` (`BUILTIN\Administrators`) and `S-1-5-32-547`
(`BUILTIN\Power Users`) disabled — the two SIDs `restricted_token.c` drops, for the same reason and
in the same order.

That is supervised children and one-shots alike: the server, and the `initdb` / `postgres --single`
/ `pg_ctl` / `psql` / `pg_isready` that surround it. Not only PostgreSQL's: the rule is about what
kind of process it is, not about which package published it.

The mechanism is `CreateRestrictedToken` followed by `CreateProcessAsUserW`, in
`crates/mixengine-platform/src/windows/restricted.rs`. Three reasons make it the cheap answer:

1. **It is a no-op on an ordinary machine.** Disabling a group that is already deny-only changes
   nothing about what the child can do. The machines it changes anything on are the ones running as
   a full administrator, where it is the correct behaviour anyway.
2. **It agrees with the rule the project already has.** *No persistent root process, ever* is about
   what MixEngine's own daemon may hold; a child of that daemon holding the administrator's whole
   token is the same claim one level down.
3. **It needs no privilege and so no elevation.** `CreateProcessAsUserW` is documented to require
   none when the token is a restricted version of the caller's own. That special case is exactly why
   `initdb` can already do this to itself, and it is why nothing here goes through
   `mixengine-elevate`.

## Consequences

- **`Supervised`'s streams are `mixengine_platform::process::OutputPipe`** rather than
  `std::process::ChildStdout` / `ChildStderr`. `CreateProcessAsUserW` hands back raw handles and the
  standard library offers no way to build a `Child` from one, so a Windows supervised child is not a
  `Child` at all. Outside `mixengine-platform` the whole cost of that is two enum variants in
  `mixengine-supervisor`'s log reader.
- **The Windows one-shot loses `kill_on_drop` and keeps its deadline.** `tokio::process` cannot adopt
  a foreign handle either, so the one-shot runs on a blocking thread. A caller that abandons the
  future does not kill the child; the blocking task still ends it when `patience` runs out. No
  process outlives its deadline — the moment it dies is the deadline rather than the drop.
- **Handles cross into a child by explicit list.** `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` names the
  three that may be inherited, instead of `bInheritHandles` letting through whatever happened to be
  inheritable. That is strictly narrower than the `Command` path, whose process-wide window
  `hide_stdio_from_children` exists to guard.
- **A daemon that really is an administrator has to grant its own children the window station.**
  Measured rather than predicted, on this repository's own runner: a child created from the
  restricted token was created and then died at `0xC0000142` — `STATUS_DLL_INIT_FAILED`, before its
  first instruction — while the *same* spawn from the process's own token ran and printed.
  `CreateProcessAsUserW` requires the token it is given to have access to the window station and the
  desktop, and a station granted to `BUILTIN\Administrators` rather than to a logon SID is one a
  token holding that group deny-only cannot open. So `restricted::admit` adds an entry for the
  child's **own user** — the account the daemon is already running as — to both objects, once per
  process and only where this process holds an enabled `BUILTIN\Administrators`. On every machine
  reason 1 above describes, nothing is written to anything.
- **Two spawn paths no longer share a `Command`,** so the environment rule is computed once —
  `process::whole_environment` — and applied by each. A probe and the server it is asking about
  cannot see different environments.

**Deliberately outside this decision:**

- **`spawn_detached`.** It starts MixEngine's own daemon, not a user's software. A daemon that could
  not write where the administrator who installed it can write would be a different bug.
- **`hand_over`, the shim.** It stands in the middle of somebody's shell session and must be the
  process they invoked, with their terminal and their token. `php -v` is theirs to run.
- **Unix.** "Do not run as root" is a different mechanism there — `geteuid() == 0`, checked by the
  same `check_root` — and the daemon does not run as root to begin with. Dropping privileges on Unix
  belongs to **T40**, not here.

## Alternatives considered

**Start only `postgres` from a restricted token.** Narrower, and worse: it makes the token a property
of one recipe rather than of what a managed process is, and the next package that refuses an
administrator would have to discover this all over again. It would also split the one-shot path in
two — `initdb` restricted and `psql` not — for no stated reason.

**Run the Windows CI leg unelevated instead.** It moves the problem into the CI configuration and
leaves the product wrong: a user who runs MixEngine as a full administrator, which nothing stops them
doing, would still be unable to start PostgreSQL. T2b asserts the runner's token deliberately so that
a change there is noticed, not so that it can be worked around.

**Elevate through `mixengine-elevate` to drop privileges.** Backwards, and unnecessary:
`CreateProcessAsUserW` needs no privilege for a restricted copy of the caller's own token, so this
would add an elevation prompt to buy something already available without one — and put a spawn path
inside the binary ADR 0005 keeps minimal and audited.
