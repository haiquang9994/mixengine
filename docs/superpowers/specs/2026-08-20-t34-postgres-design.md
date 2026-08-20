# T34 — PostgreSQL, as a service MixEngine runs

*Design for roadmap task [T34](../../../.claude/roadmap/phase-3-services.md), and for the platform
task it turned out to need first. Written 2026-08-20, before any code. What survives implementation
goes into `phase-3-services.md` and into an ADR; this file is the argument, not the record.*

## What this task is, and what it is not

A `services` row saying `postgres@main` becomes a running database: a rendered `postgresql.conf`, a
`pg_hba.conf` that trusts nothing, a cluster bootstrapped once by `initdb`, a superuser password that
exists only in the OS keyring, a readiness check that means the server answers an *authenticated*
query, a reload that a running server really honours, and a shutdown that leaves no recovery for the
next start to pay for.

It is **not** the machinery around a recipe — T30 owns the merge, the render, the diff, the staging
and the spec that comes out, and none of it changes here. It is not the first-run ritual, which T33
built and which this task is the second and last caller of before it is machinery rather than a hook.
It is not `pg_upgrade`, a second instance (T36), an application role that is not the superuser, or
backup and restore.

What it *does* add beyond a fourth recipe is one thing no recipe before it needed, and it is not a
database question at all: **`postgres` refuses to run from a token that holds Administrators.** That
is Part 0 below, and it is a separate roadmap task with an ADR of its own.

## Where the knowledge already is

The artifact was packed in `mixengine-packages` before any of this, and its smoke test
(`tools/postgres_smoke.py`) already runs this task's exact sequence on every runner: bootstrap a
cluster, start the server against a **rendered** `postgresql.conf` that lives outside the data
directory, wait for `pg_isready`, run a query, create `hstore` and `pgcrypto`, and stop with
`pg_ctl stop -m fast`. `docs/packages/postgres.md` records what that cost. **Every platform
difference below was measured there rather than assumed here.** This design's job is to move those
findings into Rust.

Three of them decide things in this document:

- **`postgres` will not start under an elevated token**, and GitHub's Windows image is elevated. The
  packaging check had to build a restricted token in Python before it could run the server at all.
- **`initdb` inherits the machine's locale when it is not told one**, and on a machine whose system
  locale is Vietnamese it reports *could not find suitable text search configuration*, sets the
  default to `simple`, and **exits zero**. Two developers, two databases that answer differently.
- **A socket path is capped at 103 characters** and the failure arrives after the server has started,
  which reads like a storage problem. The same finding `mariadb_smoke` recorded, and
  `recipes::within_socket_limit` already exists for it.

The package publishes fifteen commands by name; five of them — `postgres`, `initdb`, `pg_ctl`,
`psql`, `pg_isready` — are required for an artifact to be published at all, so a recipe may rely on
exactly those five and no more.

---

## Part 0 — T34a: a supervised child never inherits Administrators

### What refuses, and what does not

PostgreSQL's own tools call `get_restricted_token()` and re-launch themselves with Administrators
disabled — `initdb`, `pg_ctl` and their siblings therefore work perfectly well from an elevated
shell. The **server** does not: `main()` calls `check_root()`, which on Windows asks
`pgwin32_is_admin()` and exits with *Execution of PostgreSQL by a user with administrative
permissions is not permitted*.

So the affected calls are the ones that are `postgres` itself:

| Call | Path today | Elevated? |
| --- | --- | --- |
| the server | `spawn_supervised` | refuses |
| `postgres --single`, the ritual's second step | `run_once_with_input` | **refuses too** — measured, see below |
| `initdb`, `pg_ctl stop`, `pg_ctl reload`, `pg_isready`, `psql` | `run_once` | fine: they self-restrict, or do not care |

An ordinary user never meets this: an interactive administrator carries a UAC-*filtered* token where
`BUILTIN\Administrators` is present deny-only and grants nothing, and `pgwin32_is_admin()` answers
no. The machine that meets it is CI, deliberately — the Windows leg holds a full token and
`.github/workflows/ci.yml` fails the job if it ever stops doing so (T2b).

### The decision

**ADR 0010 — a supervised child never inherits Administrators.** Every process MixEngine starts in
order to run a user's software is created from a *restricted* copy of the daemon's own token, with
`BUILTIN\Administrators` (S-1-5-32-544) and `BUILTIN\Power Users` (S-1-5-32-547) disabled — the same
two SIDs PostgreSQL's `src/common/restricted_token.c` drops, for the same reason.

Three things make this the right shape rather than a workaround for one server:

- It is **a no-op on a normal machine**. Disabling a group that is already deny-only changes nothing,
  so Caddy, php-fpm and MariaDB behave exactly as they do today on every user's laptop.
- It agrees with the rule the project already has. CLAUDE.md says *no persistent root process, ever*;
  a service inheriting Administrators because somebody happened to launch the daemon from an elevated
  terminal is that rule failing quietly.
- **`CreateProcessAsUserW` needs no privilege** when the token is a restricted version of the
  caller's own. That special case is exactly why `initdb` can do this to itself, and it is why this
  needs no elevation helper and no change to `mixengine-elevate`.

Unix is not in this ADR. "Do not run as root" is a different mechanism and is T40's.

### The shape

A new `windows/restricted.rs` in `mixengine-platform`, built on what is already there:
`windows/sid.rs` has `Token` with a `Drop` and `open_process_token`, and `Win32_Security`,
`Win32_System_Threading` and `Win32_System_Pipes` are already enabled in `Cargo.toml`. No new
dependency.

```rust
/// A child created from a restricted copy of this process's token.
pub(crate) fn spawn(
    program: &Path,
    args: &[OsString],
    directory: &Path,
    env: &BTreeMap<String, String>,
    stdin: Stdio,
) -> Result<RawChild>;
```

Handles are passed **explicitly**, through `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, rather than by
setting `bInheritHandles` and hoping. That is a small improvement this rewrite gets for free:
`hide_stdio_from_children` exists today because `bInheritHandles` is per-`CreateProcessW` and
therefore process-wide for the length of a spawn, and an explicit handle list closes that window
instead of guarding it.

### What breaks in public, and it is small

`Supervised::take_stdout()` returns `std::process::ChildStdout`, which cannot be constructed from a
handle this code creates. It becomes a platform type — `OutputPipe`, `impl Read` — which on Unix
wraps the standard library's pipe unchanged and on Windows wraps a `File` over the read end.

The whole blast radius outside this crate is two enum variants and their matches, in
`mixengine-supervisor/src/logs.rs`:

```rust
enum Source {
    Stdout(std::process::ChildStdout),   // ->  Stdout(mixengine_platform::OutputPipe)
    Stderr(std::process::ChildStderr),   // ->  Stderr(mixengine_platform::OutputPipe)
}
```

### The one-shot path

`run_once` and `run_once_with_input` use `tokio::process::Command`, and a `tokio::process::Child`
cannot be built from a foreign handle either. On Windows they gain a branch: spawn restricted, drain
both pipes on threads, and wait with a deadline through `spawn_blocking` — which
`.claude/standards/rust.md` already requires of anything that waits. Unix does not change by one
line, and the two paths meet again at `Ran`.

### What proves it

A test in `mixengine-platform` that asserts **structurally**, never by attempting an access —
`.claude/standards/testing.md` is explicit that an elevated suite proving an exclusion by trying it
proves nothing. It reads the child's own token with `GetTokenInformation(TokenGroups)` and asserts
that S-1-5-32-544 is present **and not** `SE_GROUP_ENABLED`. That assertion means the same thing on a
runner holding a full token and on a developer's filtered one, which is the property the standard
asks for.

---

## Part 1 — The PostgreSQL recipe

### The name

The index publishes `"kind": "postgres"`, and a recipe is found by `packages.name`, so the ids are
`postgres@main` and the generated directory is `etc/postgres@main/`.
`.claude/features/services.md` currently writes `postgresql@main/postgresql.conf`; the publisher is
the authority on its own name, so that line is corrected as part of this task rather than worked
around.

`Instancing::Named` — a machine serving two projects with incompatible schemas needs two clusters.
`Source::Package`, like every recipe except php-fpm.

### Three generated files, not one

```
etc/postgres@main/
  postgresql.conf    # data_directory, hba_file, ident_file, listen_addresses, port, …
  pg_hba.conf        # three lines, none of them `trust`
  pg_ident.conf      # empty, and it has to exist
```

`postgresql.conf` names the other two and names the data directory, and the server is started with
`--config-file` pointing at it. **That is the whole reason generated configuration and the data
directory can keep opposite policies**: `initdb` writes a `postgresql.conf` and a `pg_hba.conf` of
its own into the cluster, and nothing here ever reads, edits or regenerates them — the server never
looks at them again. `etc/` stays disposable and the data directory stays sacred, which is the same
separation `basedir`/`datadir` gives MariaDB, reached through a different door.

`pg_hba.conf` is three lines, and it is the only wall between the data and anybody else on the
machine:

```
local   all   all                scram-sha-256
host    all   all   127.0.0.1/32 scram-sha-256
host    all   all   ::1/128      scram-sha-256
```

No `trust`, on any line, on any platform. `initdb`'s default would have written one for local
connections, which is why the ritual asks it for `reject` instead; ours is what the server actually
reads.

`pg_ident.conf` is generated empty because `ident_file` must name a file that exists, and pointing it
into the data directory would be `etc/` reaching into the one place it must not.

Every path in all three is quoted and forward-slashed, for the reason `my.cnf` already is:
PostgreSQL's configuration parser treats `\` inside a quoted string as an escape, and a home under
`C:\Users\Nguyen Hai Quang` is a real user rather than a hypothetical one.

### The spec it produces

```
program   provided("postgres")
args      --config-file=<etc>/postgres@main/postgresql.conf
cwd       <etc>
env       PGPASSWORD  <- keyring, at `postgres@main/postgres`
ready     psql       --host … --port … --username postgres --dbname postgres -tAc "SELECT 1"
health    pg_isready --host … --port … --username postgres   every 10s, timeout 5s, 3 to degrade
stop      pg_ctl stop   --pgdata <data> --mode fast          grace from `stop_grace_ms`
reload    pg_ctl reload --pgdata <data>
```

`--config-file` alone, with no `-D`: `data_directory` is stated *in* that file, and naming the
cluster twice is two places for it to drift.

The credential is **named, not carried** — `EnvValue::Keyring`, per ADR 0006. The server itself does
not read `PGPASSWORD`; the readiness check does, and it gets it because the supervisor resolves the
spec's environment once at spawn and keeps it for the life of the process, which is the same
mechanism that lets MariaDB's health probe authenticate.

### Readiness and health are two different questions here

This is the one place this recipe deliberately departs from what the packaging check does.

`pg_isready` is a genuinely good liveness probe and a **weak readiness probe**: it sends a startup
packet and reads the response, so it distinguishes *accepting connections* (0) from *rejecting* (1) —
a server that has bound the port and is still in crash recovery — which is precisely what a TCP
accept cannot do. But it never authenticates. A cluster whose superuser password did not get set
would pass it every time.

So readiness is `psql -tAc "SELECT 1"` over TCP as `postgres`, with the generated password: the same
sentence T33 arrived at for MariaDB — *running* means the server answers an **authenticated** query —
and the check that the two halves of the ritual agreed with each other. Health stays `pg_isready`,
because it is cheap, it needs no credential, and it is the question a probe repeated every ten
seconds for weeks should be asking.

### Stop and reload

`pg_ctl stop --mode fast` rather than a signal or a kill: `fast` disconnects clients and shuts down
cleanly, where terminating the postmaster leaves an unclean shutdown and makes the next start pay for
it with recovery. `pg_ctl` finds the postmaster through `postmaster.pid` in the data directory, so it
does not need to have started it.

**This is the first service in the catalogue with a real reload on all three systems.** MariaDB has
none and cannot have one; php-fpm has `ReloadBehaviour::Signal`, which Windows answers `unsupported`.
`pg_ctl reload` is one shape everywhere, and a running server re-reads both `postgresql.conf` and
`pg_hba.conf`. What it does *not* re-read is stated in the doc comment rather than discovered:
`shared_buffers`, `port` and `listen_addresses` wait for a restart somebody asked for.

### The ritual, in two steps

```
1. initdb --pgdata <data> --username postgres
          --auth-local=reject --auth-host=reject
          --encoding=<encoding> --locale=<locale>          # both from settings, not from a template
2. postgres --single -D <data> postgres      <- stdin: ALTER ROLE postgres PASSWORD '…';
```

**`--auth-*=reject`, and it was measured rather than chosen.** This section first said
`scram-sha-256` on both, and `initdb` refuses that: *must specify a password for the superuser to
enable password authentication*. Naming a method that needs a verifier obliges `initdb` to be given
one, which is the `--pwfile` this design exists to avoid. `reject` needs none — and it is a better
answer than the `trust` `initdb` would default to, because the `pg_hba.conf` it writes **inside the
data directory** then permits nothing at all. That file is never read: the generated
`postgresql.conf` names `hba_file`, and the generated `pg_hba.conf` is the one above. Between step 1
and the first supervised start there is no server, and step 2 opens no port.

**No `--pwfile`.** `initdb` will only take a password from a file, and a file is a plaintext
superuser credential on disk for the whole of a bootstrap that can take minutes — and one that a
half-failed ritual leaves behind. Step 1 therefore creates the role with **no** password, and step 2
sets it through single-user mode, which opens no port and no socket: there is no instant at which a
password-less superuser is reachable by anybody. `Step::stdin` exists for exactly this, and MariaDB
is its only current caller.

The generated value is guarded the way `mariadb.rs` guards its own: the step builder refuses a
credential that is not alphanumeric, because the only producer is
`mixengine_platform::generate_secret`, whose alphabet is chosen so that the interpolation into that
statement needs no escaper. A refusal is a bug report about whatever made the value; an escaper would
be a second thing to get right for a case that cannot arise.

`password_encryption` defaults to `scram-sha-256` from PostgreSQL 14, and 14 is the floor the index
publishes, so the stored verifier is SCRAM without the recipe stating anything.

There is no space-free view of the install, as MariaDB needed: that was upstream's unquoted
`$basedir` in a shell script, and PostgreSQL has no equivalent.

### Settings, and the two that only the ritual reads

| Key | Default | Read by |
| --- | --- | --- |
| `shared_buffers` | `128MB` | the server, on start |
| `max_connections` | `100` | the server, on start |
| `ready_timeout_ms` | `120000` | the supervisor |
| `stop_grace_ms` | `60000` | the supervisor |
| `locale` | `C` | **`initdb`, once, ever** |
| `encoding` | `UTF8` | **`initdb`, once, ever** |

`C` is not a preference, it is the only locale name that means the same thing on all three systems:
Windows spells one `English_United States.1252` and Unix spells it `en_US.UTF-8`, and no single
string is right on both. It is the same reasoning that made `mariadb.rs` pick `utf8mb4_general_ci`
over 11.4's own `utf8mb4_uca1400_ai_ci` — a default that cannot start everywhere is a default nobody
can use. What it costs is stated plainly: `ORDER BY` on text is byte order, so `Z` sorts before `a`.
A user who wants their production collation says so, and owns the per-OS spelling when they do.

`C.UTF-8` was the tempting middle and is rejected: it is a glibc locale, macOS and Windows do not
have it, and the `builtin` provider that knows it only exists from PostgreSQL 17 — a recipe running
anything from 14 to 18 would have two behaviours decided by the machine, which is the disease this
setting exists to cure. ICU is out for the same reason one version further back:
`--locale-provider=icu` is 15 and later.

**The last two rows are a trap, and the doc comment says so out loud**: they are baked into the
cluster by `initdb`, and changing them afterwards does nothing at all. Changing them for real means
deleting the data directory.

---

## What was measured

In the spirit `docs/packages/postgres.md` sets — the findings there came from running a server, not
from reading about one. Measured on 2026-08-20 against a real `postgres-18.6-windows-x86_64` on the
development machine. Each question is kept above its answer: a section that records only answers
loses why they were asked.

1. **Does `postgres --single` refuse an elevated token?** `check_root()` runs before the mode is
   dispatched, so it should; if it does not, Part 0 is still needed for the server and the ritual can
   stop caring.

   **Yes — and the bypass list is now read rather than assumed.** `src/backend/main/main.c` on
   `REL_18_STABLE` guards the check with a `do_check_root` flag, and exactly two invocations clear
   it: `--describe-config`, and `-C var` **as the first argument** — the latter because, in that
   file's own words, *pg_ctl may try to invoke it while still holding administrator privileges on
   Windows*. `--single` dispatches through `DISPATCH_SINGLE` well after `check_root(progname)` has
   already run. The ritual's second step is therefore refused by an elevated token exactly as the
   server is, and **the Windows one-shot needs the restricted token too**.

   The interactive elevated run this question asked for was not performed: the session that took
   these measurements holds a UAC-filtered token — `whoami /groups` reports the Administrators alias
   as *Group used for deny only* — which is precisely the token that cannot reproduce the failure.
   The substitute is upstream's own source plus this repository's existing evidence:
   `tools/postgres_smoke.py` in `mixengine-packages` has to build a restricted token on every Windows
   CI run because the postmaster refuses the runner's full one.

2. **Does `pg_ctl stop` work from a restricted token against a postmaster it did not start?** It
   signals through `postmaster.pid`; the question is whether the de-elevated child is still permitted
   to.

   **Yes, and it was already being proved on every Windows CI run.** `tools/postgres_smoke.py:617`
   stops a server it started under a restricted token with `pg_ctl stop -D … -m fast -w`, and asserts
   the clean-shutdown line rather than the exit code. `pg_ctl` calls `get_restricted_token()` itself,
   so both ends of that exchange are de-elevated whichever way MixEngine spawns it. Measured again
   here, directly, against a server started from this session's own token: `pg_ctl stop --mode fast
   -w` answered *server stopped* with exit 0, and the server's own last line was *database system is
   shut down*.

3. **How long is `unix_socket_directories` on the macOS runner?** `run/` is near the top of the home
   and `.s.PGSQL.<port>` is short, but 103 characters is 103 characters.

   **It fits, with room worth writing down.** A macOS home gives
   `/Users/<name>/.mixengine/run/postgres@main` — 30 fixed characters plus the user's name — and the
   file PostgreSQL creates inside it is `.s.PGSQL.<port>`, a further **17**. A five-letter user name
   therefore spends 52 of the 103. `recipes::within_socket_limit` is measured against the **file**
   rather than the directory, and refuses by name if a long enough home makes it not fit. The macOS
   CI leg is what actually proves it.

Three further things the same session measured, none of them asked for:

- **`initdb` refuses `--auth-*=scram-sha-256` without a password** — *must specify a password for the
  superuser to enable password authentication*. See *The ritual, in two steps*; its first step
  changed because of this.
- **`postgres --single` exits 0 even when the statement it was fed fails.** A malformed `ALTER ROLE`
  wrote `ERROR:  syntax error at or near …` to stderr and still returned 0. The ritual therefore
  cannot read step 2's exit code as proof that the password was set — and does not have to, because
  the readiness check *is* that proof: it is an authenticated `psql` query, and a password that was
  never set fails it.
- **`psql` prompts for a password on a terminal when it has none.** Every probe that expects a
  refusal must pass `--no-password` or it hangs instead of failing. With it, a missing password
  answers `fe_sendauth: no password supplied` and a wrong one answers `FATAL:  password
  authentication failed for user "postgres"` — the second being the sentence worth asserting on,
  because only a server enforcing SCRAM can say it.

The design was then run end to end on that machine: `initdb --auth-*=reject` → `postgres --single`
setting the password → the postmaster started against a generated `--config-file` naming `hba_file` →
`pg_isready` answering *accepting connections* → an authenticated `SELECT 1` returning `1` →
`log_min_duration_statement` changed in the generated file, `pg_ctl reload`, and the running server
answering `250ms` where it had answered `-1` → `pg_ctl stop --mode fast`.

## What proves it

**Unit, in `mixengine-core`**, through `Context::for_test`, so it costs no server: `Instancing::Named`;
every path in all three files quoted and forward-slashed; **no line of `pg_hba.conf` says `trust`**;
`unix_socket_directories` empty on Windows and `run/` on Unix; the ritual refusing a non-alphanumeric
credential; an install without `initdb` producing `ServiceProvidesNothing` and listing what it does
publish.

**Platform, for T34a**: the token assertion described in Part 0.

**Integration, `crates/mixengine-cli/tests/postgres.rs`**, `--ignored`, over a real socket, in the
shape `mariadb.rs` set: `package.install` a real PostgreSQL → `service.create postgres@main` → start →
ready, which *is* the proof that the password the daemon generated reaches the server it bootstrapped
→ the same superuser refused without it → change an override, `reload`, and observe the running
server serving the new value → stop cleanly → start again **without bootstrapping twice**.

The sentence only this suite can say: *a data directory MixEngine bootstrapped becomes a server that
answers an authenticated query with the password MixEngine generated, and honours a configuration
change without being restarted.*

**CI**: a step that fetches a real PostgreSQL on all three runners, and a step of its own for the
suite — its own, so a red build names the recipe without anybody reading the log, which is the reason
the php and MariaDB steps are separate. The Linux leg runs it from inside `test-no-network.sh`, where
a `gnome-keyring` is started on a session bus of its own, because the ritual refuses a machine with
no credential store **by design**. The Windows leg runs it at all, which is what Part 0 buys.

## Deliberately not done

Each with the task that is expected to pick it up:

- **`pg_upgrade`.** A cluster bootstrapped by one major cannot be read by the next. `READY_MARKER`
  already records the version that performed the bootstrap, so the task that does this has something
  to read.
- **A second instance running beside the first** — T36, as for MariaDB. `Instancing::Named` is this
  task's whole share of it.
- **An application role that is not the superuser**, and the database creation that would go with it.
  Everything here authenticates as `postgres`.
- **Extensions.** The artifact ships 46 of them, and `CREATE EXTENSION` is the user's to run inside
  their own database. Nothing is loaded by configuration and `shared_preload_libraries` stays empty.
- **Backup and restore.** `pg_dump` and `pg_restore` are published by the package and called by
  nothing here.
- **Windows on ARM.** Upstream does not compile PostgreSQL there before 19; the index says so, and
  `package.list_available` simply offers nothing on that cell.

## Order of work

1. **T34a** — `windows/restricted.rs`, `OutputPipe`, the `logs.rs` variants, the one-shot branch, the
   token test, ADR 0010. No PostgreSQL in it.
2. **T34** — the recipe, its three templates, its unit tests.
3. The integration suite and the two CI steps.
4. `phase-3-services.md` ticked, `features/services.md` corrected on the name, `todo.md`'s counters
   moved.
