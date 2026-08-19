# T32 — php-fpm pools

*Design, 2026-08-19. Roadmap task [T32](../../../.claude/roadmap/phase-3-services.md), Phase 3.*

## What this closes

T31a made a service reachable: a package is installed from the signed index and `service.create`
writes the row. Every part of that assumes the thing supplying the binary is a **package**, and
php-fpm is the first service where it is not. A PHP that a user installed with `runtime.install`
lives in `runtime_installs`, and the process that serves their sites lives inside it.

It is also what the other half of [T28](../../../.claude/roadmap/phase-2-runtimes.md) has been
waiting for — a per-pool reload needs a pool — and it is the first refusal `runtime.uninstall` can
make, which [todo.md](../../../.claude/roadmap/todo.md) has been carrying as an open promise.

## What was measured, not assumed

The whole shape of this task turned on one question that could not be answered by reading: what
`php-cgi.exe` on Windows actually is. **php-fpm does not exist there** — the published index says so
in its own words, and the difference is not an omission of ours:

| PHP 8.3.33 artifact | `provides` |
| --- | --- |
| linux aarch64 / x86_64, macos aarch64 / x86_64 | `php`, `php-fpm` |
| windows x86_64 | `php`, `php-cgi` |

Every PHP from 7.0.33 to 8.5.9 has the same split, so this is upstream's shape and not a version we
happened to pack badly. `tools/php_windows.py` in `mixengine-packages` already records why the
non-thread-safe build is the one taken: "`php-cgi.exe` behind FastCGI is how a Windows site is
served".

What was **not** known is whether that leaves Windows without a process manager. Measured against the
artifact this project publishes, on Windows 11:

| Probe | Result |
| --- | --- |
| 2 concurrent requests, no `PHP_FCGI_CHILDREN` | 6.21 s, one pid — a queue |
| 2 concurrent, `CHILDREN=4` | 3.04 s, two pids, 5 processes (1 master + 4 children) |
| 4 concurrent, `CHILDREN=2` | 6.05 s — two served, two queued; the master serves nothing |
| kill one child | replaced in under a second, still serving |
| terminate the master | every child goes with it, nothing orphaned |
| `PHP_FCGI_MAX_REQUESTS=2` | the child is recycled after exactly two requests |
| `-c <dir>` | loads a `php.ini` from where we point it |
| `PHP_INI_SCAN_DIR` | loads `conf.d/*.ini` over it — which is T28's model |

**Windows php-cgi is a process manager**: a master, N children, respawn, recycling, and a clean
teardown. That is php-fpm with `pm = static`, configured through the environment instead of a file.
The three middle rows are why this task does not write a process manager of its own — the work is
already done, upstream, and rewriting it in Rust would be taking on maintenance for something that
runs today. ADR 0008 refused "a helper process per service" on the same reasoning.

A supervisor of our own would also make the two systems **less** alike, not more: Windows would run
*MixEngine-PM + N php-cgi* while Unix ran *php-fpm*. Uniformity is bought at the layer a user sees,
not by replacing what each platform already does well.

## How alike the systems end up

| Layer | How alike |
| --- | --- |
| What a user touches — `ServiceId`, `mix service`, the overrides, the lifecycle | identical |
| What the supervisor runs — one master owning N workers, one process to watch and to stop | identical in shape |
| The mechanism — which binary, file or environment, whether a signal exists | different, deliberately |

Two differences survive, and both are stated rather than papered over: a Windows pool cannot be
reloaded, and it has no `request_terminate_timeout`, so a hung script holds a worker.

## Decisions

### A service has two possible parents

`services.package_id` is `NOT NULL REFERENCES packages(id)`, and php-fpm has no `packages` row and
must not be given a fake one — that would be a second table describing one directory, with
`package.uninstall` able to see and delete it, and an `install_path` that goes stale the moment the
runtime is removed.

So `services` grows a second, typed parent:

```sql
package_id         INTEGER NULL REFERENCES packages (id)         ON DELETE RESTRICT,
runtime_install_id INTEGER NULL REFERENCES runtime_installs (id) ON DELETE RESTRICT,
CHECK ((package_id IS NULL) <> (runtime_install_id IS NULL))
```

Exactly one is set, and SQLite enforces it. `Generator` picks which table to join by which column is
filled; `Context` does not change shape at all — it still receives an `install_path` and a `version`,
from whichever parent answered. The foreign key is what gives `runtime.uninstall` its refusal for
free.

**`0001_initial.sql` is edited rather than migrated**, on the precedent `services.rs` already
records: nothing has shipped, so forward-only has nothing to protect, and SQLite cannot `ALTER` a
`NOT NULL` away without a twelve-step table rebuild that would leave the definition of `services`
split across two files. `sqlx::migrate!` checksums migrations, so an existing development home must
be deleted; CI builds one from nothing every run.

### The recipe is still found by one name

`ServiceId::name()` is the lookup key T31a settled on, and it needs no exception here:
`php-fpm@8.3.33` finds the recipe `php-fpm`. Nothing about recipe lookup learns that runtimes exist.

### The instance is the full version

`php-fpm@8.3.33`, not `php-fpm@8.3`. `runtime_installs` is `UNIQUE (kind, version)` on the full
version, so 8.3.33 and 8.3.34 can both be installed — and `php-fpm@8.3` would then name neither.
`.claude/architecture/data-model.md` and `.claude/features/services.md` use the short form in their
examples and are corrected by this task.

### Nobody calls `service.create` for it

[runtime-versions.md](../../../.claude/features/runtime-versions.md) already decided this: PHP's
post-install hook creates the `php-fpm@<version>` service record. The hook is written **idempotent
and also run at boot**, so a PHP installed before this task has a service without a data migration,
and a home whose row was deleted by hand repairs itself. `service.create` refuses `php-fpm` with a
message naming `runtime.install` instead.

`runtime.uninstall` deletes it, and refuses while it is running — the first refusal that method has
ever been able to make, and the one `--force` now has something to force past.

### One recipe, two spec shapes, no `#[cfg]`

| | Unix | Windows |
| --- | --- | --- |
| program | `provides["php-fpm"]` | `provides["php-cgi"]` |
| args | `--nodaemonize --fpm-config <etc>/php-fpm.conf` | `-b 127.0.0.1:<port>` |
| workers | `pm = static`, `pm.max_children` | `PHP_FCGI_CHILDREN` |
| recycling | `pm.max_requests` | `PHP_FCGI_MAX_REQUESTS` |
| listen | `<run>/php-fpm-<version>.sock` | `services.port`, allocated from 9000 |
| ready / health | `UnixSocket` | `Tcp` |
| validator | `php-fpm -t` | none — there is no file to test |
| stop | `StopBehaviour::Signal` | ADR 0008 degrades it to a kill, which is safe here because the children were measured to go with the master |
| reload | `ReloadBehaviour::Signal { Usr2 }` | `None` |

The program is read out of the artifact's `provides` rather than written down, so the index decides
which binary this is and no recipe carries a platform conditional. The socket-versus-port split is
`.claude/features/services.md`'s own — "unix socket / `127.0.0.1:9xxx` on Windows".

**Where each of those comes from.** The Windows port is allocated when the hook creates the row: the
lowest free port from 9000 that no `services` row already holds, written into `services.port` so it
is stable across restarts rather than re-derived from a version number two PHPs could collide on.
On Unix the column stays `NULL` and the socket path is the one thing here that can fail for a reason
nobody would guess — **T33a measured `sockaddr_un` at 103 characters**, and a server that exceeds it
aborts late, in a way that reads like a different failure entirely. `<run>/php-fpm-<version>.sock` is
short and `run/` is near the top of the home, but the recipe checks the length and refuses with the
measurement in the message rather than letting php-fpm do it.

The overrides a user sets are **one set on every OS**: `max_children` (5), `max_requests` (500),
`ready_timeout_ms`, `stop_grace_ms`. The recipe renders them into a file or an environment as the
platform requires. There is deliberately **no `pm = dynamic|ondemand`**: Windows cannot express it,
and an override that works on two systems out of three is the divide this task exists to avoid.

### One pool per version, shared by every site

`.claude/features/services.md` sketches `php-fpm/8.3/pool.d/<site>.conf`, and a pool per site is
Unix-only — Windows has one master with one set of children and no pool vocabulary at all. Choosing
it would create exactly the split this design refuses everywhere else, in the layer Phase 4 builds
on. So every site on a PHP version shares that version's pool.

`include=pool.d/*.conf` is still rendered and still matches nothing, on T31's reasoning for
`import sites/*.caddy`: the path is relative to the file it is written in, so whoever renders the
first per-pool file must render it through here, or it is invisible to `php-fpm -t` and present at
run time.

### `ReloadBehaviour::Signal`

`ReloadBehaviour` has only `Command`, and php-fpm's reload is `SIGUSR2` to the master — there is no
program to run, and the daemon already holds the pid.

```rust
pub enum ReloadBehaviour {
    Command { .. },
    Signal { signal: ReloadSignal, patience: Millis },
}

pub enum ReloadSignal { Hup, Usr1, Usr2 }
```

A closed enum and not an `i32`: `ServiceSpec` is proto, is cross-platform and must not leak `libc` —
the same reason `StopBehaviour::Signal` names no number. `mixengine-platform` grows one function to
send it, answering `Unsupported` on Windows exactly as `ask_to_stop` does, and the supervisor reads
that before it starts waiting, as `CAN_ASK_TO_STOP` taught it to.

Windows therefore does not reload. A changed override leaves the running pool on its old
configuration until somebody restarts it, and the daemon does not restart a thing nobody asked it to
restart — T31's rule, and `mix doctor` (T47) owes the sentence.

## What this task deliberately does not do

- **No `request_terminate_timeout` on Windows.** A hung script holds a worker forever, and with
  `max_children = 5` five of them are a dead PHP. The fix is thin and needs no process manager —
  because the master respawns a killed child, the daemon only has to *kill* a worker that has run too
  long. It is left out because doing it right needs its own measurement of how a hung script behaves
  there, and that is a separate task.
- **No php.ini and no `conf.d`.** `PHP_INI_SCAN_DIR` was measured to work, so T28 has its road, but
  what a pool renders and what a runtime's ini set contains are different files with different
  owners.
- **No site is rendered.** `pool.d/` matches nothing until Phase 4.
- **No `pm.status_path`, no slowlog.** Neither exists on Windows, and nothing reads them yet.

## Testing

Unit tests render the templates through a `Context::for_test`, as T31's do, so a misspelled variable
is caught without fifty megabytes of PHP.

The real judgement is an `#[ignore]`d integration test that CI runs with a PHP fetched from
`mixengine-packages`' own release, the way `caddy.rs` does: install PHP through `runtime.install`,
find the service the hook created, start it, **send a real FastCGI request and read the body back**,
change `max_children` and reload, confirm the same pid still serves on Unix, then stop it and confirm
nothing is left running. A minimal FastCGI responder client — about eighty lines, and a working
prototype exists from the spike — goes into `mixengine-testkit`, because a test that proves php-fpm
is up by connecting to its socket proves only that something is listening.
