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

One function, `core::resolve::runtime(kind, cwd, flags) -> ResolvedRuntime`, used by the shims, the
daemon, the CLI and the GUI. Order:

1. Explicit flag / env (`MIXENGINE_PHP=8.1`)
2. `mixengine.toml` found by walking up from cwd (first hit wins)
3. Project record in SQLite matching the cwd (a directory registered as a project root)
4. Global default
5. Error `dependency_missing` with a hint naming the exact `mix runtime install` command

Constraint strings accept exact (`8.3.12`), minor (`8.3`), and caret (`^8.3`) forms, resolved against
installed versions — **never** silently against downloadable ones.

## Shims

`<root>/bin/` contains a small shim binary per exposed command (`php`, `php-config`, `pecl`,
`composer`, `node`, `npm`, `npx`, `python`, `pip`, `ruby`, `gem`, `bundle`). The shim:

1. Reads its own file name to know which command was invoked.
2. Calls `resolve` (in-process, reading SQLite read-only + walking for `mixengine.toml`) — **no IPC**,
   so it stays fast even when the daemon is down. Target: **< 15 ms** overhead, enforced by a bench.
3. `exec`s the real binary with the correct `PATH`, `PHPRC`, `GEM_HOME`, etc. prepended.
   On Windows there is no `exec`: spawn the child in the same Job Object and proxy the exit code and
   console signals.

Only `<root>/bin` goes on the user's PATH — one entry, never per-version directories.

## Install flow

`runtime.install { kind, version }` returns a job:

1. Look up the artifact in the signed package index for `(kind, version, os, arch)`.
2. Download to `<root>/tmp` with resume support, verify SHA-256, verify index signature.
3. Extract to a staging dir, then atomic-rename into `runtimes/<kind>/<version>/`.
4. Post-install hook (per kind): PHP — write the base `php.ini` from our template and create the
   `php-fpm@<version>` service record; Node — nothing; Python — ensure `pip`; Ruby — ensure
   `bundler`.
5. Record in `runtime_installs`, emit events, refresh shims.

Failures roll back the staging directory. A half-extracted version must never appear in `list`.

See [operations/runtime-packaging.md](../operations/runtime-packaging.md) for where the binaries come
from on each OS.

## PHP extensions

Per-version, since that is how PHP works:

- `mix php ext list|enable|disable|install <name> --php 8.3`
- Prebuilt extension binaries ship in the index for common ones (redis, imagick, xdebug, mongodb,
  swoole); source builds are a later, opt-in path.
- Enabling writes `runtimes/php/<v>/conf.d/<ext>.ini` and reloads only that php-fpm pool.
- The GUI shows extensions as toggles per version, with the "requires restart" state made obvious.

## Uninstall

Refuses if a project pins it or a site uses its php-fpm service, listing what blocks it, unless
`--force`. Removes the directory, service record, and any orphaned pool config.

## Acceptance criteria

- Two PHP versions serving two sites simultaneously, verified by `phpinfo()` in an integration test.
- `cd project-a && php -v` and `cd project-b && php -v` disagree, with no shell hook installed.
- Uninstalling the default version leaves the system in a coherent state (new default chosen or
  cleared, with a warning), never a dangling shim.
