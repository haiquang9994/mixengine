# T33 — MariaDB, as a service MixEngine runs

*Design for roadmap task [T33](../../../.claude/roadmap/phase-3-services.md). Written 2026-08-20,
before any code. What survives implementation goes into `phase-3-services.md`; this file is the
argument, not the record.*

## What this task is, and what it is not

A `services` row saying `mariadb@main` on port 3306 becomes a running database: a rendered `my.cnf`,
a data directory bootstrapped once, a random root password in the OS keyring, a readiness check that
means the server answers queries, and a shutdown that leaves InnoDB clean.

It is **not** the machinery around a recipe — T30 owns the merge, the render, the diff, the staging
and the spec that comes out, and none of it changes here. It is not a second instance running beside
the first, which is T36. It is not backup or restore.

What it *does* add beyond a third recipe is the one thing T30 deliberately left out: a **first-run
ritual**. `Recipe` today can describe a service completely and still cannot say "before this ever
starts, run these two programs once." MariaDB is the first service that needs it, PostgreSQL (T34) is
the second, and building it for one of them and generalising later would mean writing it twice.

## Where the knowledge already is

T33a packed MariaDB for all six targets, and its smoke test
(`mixengine-packages/tools/mariadb_smoke.py`) already runs this task's exact sequence on every
runner: bootstrap a data directory, start against a rendered `my.cnf`, wait for `mariadb-admin ping`,
run a query, shut down with `mariadb-admin shutdown`, and prove the shutdown was clean by finding
InnoDB's own line in the log. **Every platform difference below was measured there rather than
assumed here.** This design's job is to move those findings into Rust, not to rediscover them.

The package publishes seven commands by name — `mariadbd`, `mariadb`, `mariadb-admin`,
`mariadb-install-db`, and where they exist `mariadb-dump`, `mariadb-upgrade`, `mariadb-backup`. The
first four are required for an artifact to be published at all, so a recipe may rely on them.

## Part 1 — `FirstRun`, in `mixengine-core`

### The shape

```rust
trait Recipe {
    /// The secrets this ritual needs, declared rather than generated.
    fn first_run_secrets(&self) -> &'static [SecretSpec] { &[] }

    /// The ritual, built from a Context that already carries the generated secrets.
    fn first_run(&self, context: &Context) -> Option<FirstRun> { None }
}
```

**Two methods rather than one, and the reason is the keyring.** A recipe lives in `mixengine-core`,
which has no business reaching an OS credential store; the daemon has the `Keyring` and is the only
thing that should. So a recipe *declares* what it needs (`SecretSpec { key: "root", length: 32 }`),
the daemon generates the value and stores it, and the value arrives back inside the `Context` the
recipe is handed. The recipe reads `context.secret("root")` and builds its SQL. No `#[cfg]`, no
platform call from core, and one place where a secret is created.

**Storing comes before touching the disk.** The daemon writes the credential to the keyring *first*,
then runs step one. A machine with no credential store therefore fails while nothing has been
created — see "No credential store" below — rather than half-way through, leaving a data directory
whose root password exists nowhere.

**A secret must never reach a template.** `Context::rendering()` is what a Jinja template sees; the
secret map is not part of it. A `my.cnf` with a root password in it would be a plaintext credential
on disk written by the very design that refuses one. This gets its own test, not a comment.

```rust
pub struct FirstRun {
    /// Run in order. A step that fails ends the ritual; nothing after it runs.
    pub steps: Vec<Step>,
}

pub struct Step {
    /// What the progress line says: "creating the data directory".
    pub label: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Fed to the program's stdin and then closed. This is how SQL reaches `mariadbd --bootstrap`.
    pub stdin: Option<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub timeout: Millis,
}
```

`stdin` is a `String` and carries a generated password, so `Step` must not be `Debug`-printed
wholesale — it derives a manual `Debug` that writes `stdin: <{n} bytes>`.

### Where it runs, and how a half-finished one is told from a finished one

In the daemon, inside `Registry::begin`, before the spawn — and the body runs through the job system,
so it appears in `mix job list` and reports progress, which is what `services.md` promises. The RPC
signature does not change: `service.start` still waits, and a user still types one command.

The marker is two files in the data directory, and the pair is what makes cleaning safe:

| What is in the data directory | What happens |
| --- | --- |
| nothing, or it does not exist | bootstrap |
| `.mixengine-init-started`, no `.mixengine-ready` | a ritual we started and did not finish — remove the directory's contents and bootstrap again |
| `.mixengine-ready` | already done; start the server |
| content, and neither marker | **refuse, and touch nothing** — this directory was not made by MixEngine |

`services.md` says a half-finished data directory is "detected and cleaned rather than reused", and
the last row is what keeps that sentence from meaning "MixEngine deletes a database it did not
create". Deleting a data directory is not reversible, so it happens only where we left our own
evidence that we were mid-ritual.

`.mixengine-ready` records the package version that bootstrapped the directory. Nothing reads it yet;
the thing that will is a version upgrade needing `mariadb-upgrade`, and a marker written without it
would have to be guessed at later.

## Part 2 — The MariaDB recipe

### The spec it produces

| Field | Value | Why not the obvious alternative |
| --- | --- | --- |
| `program` | `mariadbd` | — |
| `args` | `--defaults-file=<etc>/my.cnf` | Not a pile of flags: one rendered file is the thing a user can be shown |
| `ready` | `Command`: `mariadb-admin ping` | A TCP accept stays true while the server refuses every query during InnoDB recovery |
| `health` | the same command | — |
| `stop` | `Command`: `mariadb-admin shutdown` | A signal or a kill leaves an unclean InnoDB — a recovery on next start, which a supervisor must never cause |
| `reload` | `None` | MariaDB reads `my.cnf` once. Changing an override needs a restart, and per T31's rule the daemon does not restart what nobody asked it to. `mix doctor` (T47) owes the sentence |
| `env` | `MYSQL_PWD` from the keyring, key `mariadb@<instance>/root` | So `mariadb-admin` authenticates without a password on a command line or in a file |

### The generated `my.cnf`

Three lines are **derived, not settings**, each because of something measured in T33a:

- **`log_error`** — on Windows `mariadbd` writes to `<datadir>/<hostname>.err` and sends nothing to
  stdout. A supervisor reading the process's own output finds an empty file and reports that the
  server said nothing. It said plenty.
- **`socket`**, under `run/` rather than beside the data directory — `sockaddr_un` caps a path at 103
  characters, and the server aborts *after* InnoDB has started, which reads like a storage failure.
  `php_fpm::SOCKET_PATH_LIMIT` already exists and moves up to be shared.
- **`plugin-dir`** on Windows — the plugin directory is derived from `basedir` on Unix and not always
  there, where the server has been seen looking beside its own executable instead.

Also fixed rather than offered: `bind-address` from the row, `skip-name-resolve` (nothing here should
ask a DNS server whether 127.0.0.1 may connect), and forward slashes with every path quoted —
MariaDB's option parser treats `\` as an escape and everything after an unquoted `#` as a comment, so
`C:\Users\Nguyen Hai Quang` breaks a naive rendering in two different ways at once.

Settings a user may override: `innodb_buffer_pool_size` (dev-tuned, not upstream's), `max_connections`,
character set and collation, the start/stop/health timeouts, and `extra`.

### The ritual, in two steps

**Step 1 — `mariadb-install-db`, which is a different program per platform.** On Unix a shell script;
on Windows a C++ program of the same name sharing almost none of its options.
`--auth-root-authentication-method=normal` is Unix-only and makes the Windows build exit 7 with
`unknown variable`; it is needed on Unix alone, because without it root authenticates through
`unix_socket` against an OS account of the same name and cannot be reached by whoever MixEngine runs
as. Windows already creates a password-less root, so the same end state is reached by not asking.
`--service` is never passed on Windows: a first-run job that registers a system service has installed
something the daemon cannot see.

On Unix the call also needs `--no-defaults` (or the script and its bootstrap server read the
machine's own `/etc/mysql/my.cnf` — **a user with their own MariaDB installed has one naming a
datadir, a socket and a port, and an instance that inherited any of them would be writing into
somebody else's database**), `--user=<current user>` (or it tries to hand the directory to a `mysql`
account nobody created), and `/usr/sbin` on the PATH (`chown` is there on macOS and in `/usr/bin` on
Linux).

**Step 2 — `mariadbd --bootstrap`, SQL on stdin.** Four statements, and each is named here rather
than left to "secure defaults": set root's password to the generated one; delete every anonymous
account (`user = ''`); delete every root row that is not `localhost` or `127.0.0.1`; `FLUSH
PRIVILEGES`. The `test` database is already gone on Unix through `--skip-test-db`, and whether the
Windows program creates one is a thing to check rather than assume — if it does, a `DROP DATABASE IF
EXISTS test` joins the list, which is harmless on the platform where it never existed.
`--bootstrap` listens on no port and no socket, so there is
no moment where a password-less root sits on 127.0.0.1:3306 waiting for whoever is quickest. The two
rejected alternatives: a temporary server with `--skip-networking` (Unix vocabulary — Windows would
need a named pipe or a temporary port, i.e. two shapes per OS, exactly the split T32 had to accept
for php-fpm and this can avoid), and starting the real server and setting the password afterwards
(a window, measured in seconds, in which anyone on the machine is root).

**Unproven, and it is the assumption this whole choice rests on:** that `mariadbd --bootstrap` reads
SQL from stdin the same way on Windows. Nothing in `mixengine-packages` has run it there. If it does
not, the fallback is the rejected `--skip-networking` shape **for Windows alone** rather than for all
three.

### Paths with a space in them

Upstream's script leaves **both** `$basedir` and `$datadir` unquoted, so either containing a space
splits into two arguments. This is upstream's escaping, has nothing to do with relocation, and fails
identically for a user whose home has a space in it — which on macOS and Linux is a real user, not a
hypothetical.

`mariadb_smoke.py` solved the `basedir` half with a symlink from a space-free temporary path, used
only for the bootstrap; the server, the client and the shutdown all run from the real path. **The
`datadir` half has not been solved by anyone yet** — the smoke test puts its instance under a
temporary directory that happens to have no space. The plan is the same instrument applied twice:
during step 1 only, symlink both into a space-free temporary directory, and let every later step use
the real paths. If that turns out not to work, the fallback is bootstrapping into a space-free
temporary directory and moving the result — more expensive and it crosses filesystems, so it is the
second choice rather than the first.

## No credential store

A session with no secret service (WSL, a headless Linux, CI without `gnome-keyring`) **fails the
bootstrap** with the `Unsupported` answer `mixengine-platform` already produces, naming what is
missing and what provides one. It does not fall back to a file.

The reasoning, so that whoever reverses this knows what they are trading: the root password has
exactly one home, and a second one means `EnvValue` grows a variant, a credential gains two sources
of truth, and a plaintext secret exists on disk where this project has never had one. Against that,
the cost is borne by nobody using MixEngine as intended — every desktop, on all three systems, has a
store. It is borne by WSL, by SSH sessions and by CI, and CI already builds one.

**This is the same wound as [T15b](../../../.claude/roadmap/phase-1-process-supervision.md), which is
still open**: `secrets.rs` maps only `NoStorageAccess` to `Unsupported`, and a Linux session with no
provider currently arrives as `Error::Secret` — a capability the machine lacks, reported as a
failure. Whoever hits the refusal designed here on a headless machine will see the wrong error until
T15b lands. T33 does not fix it; T33 is the reason somebody will finally be bitten by it, which is
precisely what T15b says it is waiting for.

## What proves it

A `#[ignore]`d suite, in the shape T31 established and T32 followed: CI fetches a real MariaDB from
`mixengine-packages`' own release on all three systems and the test drives the shipped methods —
`package.install`, `service.create`, `service.start` — through the whole life:

1. a row and overrides become a `my.cnf` the server accepts
2. the first start bootstraps: the data directory appears, both markers in the right order
3. `mariadb-admin ping` answers, and the service is reported ready only then
4. a query runs **as root with the generated password**, read from the keyring — which is the only
   proof that the password was set and stored as the same value
5. `mariadb-admin shutdown` ends it, and InnoDB's own clean-shutdown line is in the log
6. a second start does **not** bootstrap again
7. a data directory with foreign content and no marker is refused, and its content is still there
   afterwards

Ignored rather than skipped, for T31's reason: a test that returns early when it finds no MariaDB is
a green suite that proved nothing.

**A cost worth stating in advance:** MariaDB is much larger than Caddy or PHP, and this adds a
download plus a bootstrap to every `test` job on every runner. If it pushes CI past what is
tolerable, the answer is a separate job rather than a quieter test.

## Deliberately not done

- **Instances beyond the first** — T36. The recipe is `Instancing::Named` so `mariadb@legacy` is
  expressible, and nothing here runs two at once or checks that they do not collide.
- **`mariadb-upgrade`** — nothing moves a data directory between versions yet. `.mixengine-ready`
  records the version so that whoever writes it has something to compare against.
- **Backup and restore** — `mariadb-backup` is published and unused.
- **Reload** — MariaDB cannot; see the table above.
- **A non-root user for applications.** `services.md` promises a root password in the keyring and
  nothing else. Blueprints (Phase 8) are where per-project credentials belong.
