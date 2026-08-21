# Multi-version runtime management

**Goal**: install many versions of PHP, Node.js, Python and Ruby side by side, switch instantly, and
have each project use the right one without the user thinking about it.

## Model

- A **runtime install** is an immutable directory `runtimes/<kind>/<version>/`. Installing never
  mutates an existing version.
- A **runtime kind** has one *global default* and any number of *project pins*.
- PHP is special: each installed PHP version also owns a long-running `php-fpm@<version>` service
  ([services.md](services.md)). Node/Python/Ruby are invoked per-command, not supervised.

## Version resolution

One function, `core::resolve::runtime`, used by the shims, the daemon, the CLI and the GUI — and
`runtime.resolve` over the API for the clients already talking to a daemon. Order:

1. Explicit flag / env (`MIXENGINE_PHP=8.1`), read by the process the user invoked and passed in —
   never read by the daemon, whose environment is whatever started it
2. `mixengine.toml` found by walking up from cwd. The first one **that names this language** wins: a
   manifest silent about PHP is not an answer about PHP, so an outer pin still applies
3. Project record in SQLite matching the cwd or a directory above it (a registered project root)
4. Global default
5. Error `dependency_missing` with a hint naming the exact `mix runtime install` command, or — for a
   range, whose satisfying version is not knowable from here — `mix runtime available`

Constraint strings accept a **prefix** (`8`, `8.3`, `8.3.12` — as many segments as are written have
to agree, and one nobody wrote is a zero) and a **caret** (`^8.3`, up to the leftmost non-zero
segment: `^0.12` stops at `0.13`). A constraint with no pre-release in it never selects one — `8.5`
and `^8.5` both pass over `8.5.0RC1`, and naming it (`8.5.0RC1`) is how it is asked for. Everything
is resolved against installed versions — **never** silently against downloadable ones.

## Shims

`<root>/bin/` contains a small shim binary per exposed command (`php`, `php-config`, `pecl`,
`composer`, `node`, `npm`, `npx`, `python`, `pip`, `ruby`, `gem`, `bundle`). The shim:

1. Reads its own file name to know which command was invoked.
2. Calls `resolve` (in-process, reading SQLite read-only + walking for `mixengine.toml`) — **no IPC**,
   so it stays fast even when the daemon is down. Target: **< 15 ms** overhead, enforced by a bench.
3. `exec`s the real binary with the correct `PATH`, `PHPRC`, `GEM_HOME`, etc. prepended.
   On Windows there is no `exec`: spawn the child in the same Job Object and proxy the exit code and
   console signals.

Only `<root>/bin` goes on the user's PATH — one entry, never per-version directories. The directory
is filled by the daemon at every start, one shim per row in `core::shims::COMMANDS` — a hard link to
the shim binary wherever the filesystem gives one file a second name, and a copy of its bytes where
it does not, which on Windows is always: a shim there outlives the program it starts, so a link would
let a running `php -S` hold the shim binary itself open against the next upgrade. Either way the file
in `bin/` dispatches on the name it was invoked by. Putting the directory on the PATH is
`path.install`, which is asked for rather than assumed, and
`path.uninstall` reverses it. Because the command table is a constant, `bin/` does **not** depend on
what is installed and there is nothing to refresh after an install — a `node` shim on a machine with
no Node.js resolves nothing and says which command to type.

## Install flow

`runtime.install { kind, version }` returns a job:

1. Look up the artifact in the signed package index for `(kind, version, os, arch)`.
2. Download to `<root>/cache/downloads` with resume support, verify SHA-256, verify index signature.
   Not `run/`, which is scratch belonging to the daemon currently running: a partial download's whole
   value is surviving a restart, and it is named after the artifact's hash so the same file offered
   by a mirror and by the default host resumes one download rather than starting two.
3. Extract to a staging dir, then atomic-rename into `runtimes/<kind>/<version>/`.
4. Post-install hook (per kind): PHP — write the base `php.ini` from our template and create the
   `php-fpm@<version>` service record. **The service half landed with T32 and is written differently
   from what this step implies**: it is not a PHP-shaped branch in the installer but a walk over the
   recipes whose `Recipe::source` names a runtime, and it is *idempotent and also run at boot* — so a
   PHP installed by an earlier build gets its pool with no data migration, and a home whose row was
   deleted by hand repairs itself. The `php.ini` half landed with T28 and
   became a `conf.d` instead — there is no generated `php.ini` at all, see *PHP extensions* below. **Node, Python and Ruby — nothing**, which is not what this
   step originally said. It reserved *ensure `pip`* and *ensure `bundler`*, and T27 found that both
   belong in the recipe rather than here: the only artifact missing a runnable entry point is the
   Windows CPython, and generating one at install time bakes the install directory into a launcher
   that stops working the moment `<root>` moves. `tools/python.py` writes a wrapper that computes the
   interpreter from its own location instead. **A path computed at run time beats a path written at
   install time** — the same rule the shim itself is an instance of — so the hook that would have
   fixed this would have been the bug.
5. Record in `runtime_installs`, emit events. **No shim refresh** — see the note under *Shims*: the
   command table does not depend on what is installed, so there is nothing an install changes about
   `bin/`.

Failures roll back the staging directory. A half-extracted version must never appear in `list`.

See [operations/runtime-packaging.md](../operations/runtime-packaging.md) for where the binaries come
from on each OS.

## PHP extensions

Per-version, since that is how PHP works. **Landed with T28**, and three things about it are written
differently from what this section originally said:

- `mix runtime ext list|enable|disable <name> --php 8.3`, and **not** `mix php ext …`. A per-language
  command family for one language is a noun this CLI would then owe every other runtime; `runtime` is
  where the version already lives.
- **No `install <name>`.** What can be switched on is what the archive already ships — which is 31
  modules on the Windows build and everything the Unix build compiles in. An extension from anywhere
  else is a `mixengine-packages` task before it is one here, and the state model does not change when
  one arrives: it becomes another name in the artifact's `shared` list.
- Enabling writes `etc/php/<version>/conf.d/<NN>-<name>.ini` and reloads only that php-fpm pool.
  **Under `etc/` and not inside the install**: an install is a rename of a staging directory over the
  destination, so a generated `conf.d` living inside it is destroyed by reinstalling the same version
  — and generated configuration is disposable by the project's own rule. Both consumers find it
  through `PHP_INI_SCAN_DIR`, set by the pool's spec and by the shim, so `php -m` on a terminal and
  `phpinfo()` in a browser answer the same thing.
- The GUI shows extensions as toggles per version, with the "requires restart" state made obvious —
  `runtime.set_extension` answers `reloaded`, `restart_required` or `pool_not_running`, so no client
  has to guess it from the operating system it happens to be running on.

## Uninstall

Refuses if a project pins it or a site uses its php-fpm service, listing what blocks it, unless
`--force`. Removes the directory, service record, and any orphaned pool config.

**Both refusals are in**: the running php-fpm pool as of **T32**, by name and with
`mix service stop <pool>` in the hint; the project pin as of **T39**, naming each project and the
constraint it asks for. An uninstall that is allowed removes the `services` row before the
directory.

`--force` crosses **the pin and nothing else**. A broken pin is a statement about the future — the
next `cd` into that directory fails with a message naming the install that fixes it — and somebody
who has been shown the projects is entitled to decide; a running pool is a process serving requests
now, and no flag buys a live process with no files under it.

The pin is read in **effective** order, so a row the project's `mixengine.toml` overrides refuses
nothing, and a pin nothing already satisfies refuses nothing either: what earns the refusal is the
transition from *answered* to *unanswerable*. Still open: removing an orphaned `etc/<pool>/`, which
is the same orphan-removal question T43 owns for site files.

A *site* using the pool is T39a's half of the sentence above, and is not checked yet.

## Acceptance criteria

- Two PHP versions serving two sites simultaneously, verified by `phpinfo()` in an integration test.
- `cd project-a && php -v` and `cd project-b && php -v` disagree, with no shell hook installed.
- Uninstalling the default version leaves the system in a coherent state (new default chosen or
  cleared, with a warning), never a dangling shim.
