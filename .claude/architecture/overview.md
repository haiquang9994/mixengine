# Architecture overview

## Processes at runtime

```
┌──────────────┐   ┌──────────────┐
│  mix (CLI)   │   │  any other   │      thin clients, no business logic.
│              │   │   client     │      Only `mix` ships from this repo;
└──────┬───────┘   └──────┬───────┘      a GUI is a client elsewhere (ADR 0011)
       │ JSON-RPC over IPC │
       └─────────┬─────────┘
                 ▼
        ┌────────────────────┐        user-level, autostarts at login
        │    mixengined      │  ◀── owns SQLite state, config generation,
        │  API + orchestrator│      supervision, DNS server, metrics
        └───┬────────────┬───┘
            │            │ spawns through the OS elevation prompt,
            │            │ only for a one-shot batch of operations
            │            ▼
            │   ┌───────────────────┐   root / Administrator, seconds of lifetime
            │   │ mixengine-elevate │   hosts file, trust store, resolver, firewall
            │   └───────────────────┘   validates every request itself, then exits
            ▼
   supervised child processes
   caddy · nginx · php-fpm@8.1.33 · php-fpm@8.3.33 · mariadbd · postgres · redis-server ·
   memcached · node · extensions (mailpit, phpmyadmin runner, …)
```

Three privilege levels, three lifetimes:

| Component | Privilege | Lifetime |
| --- | --- | --- |
| `mix` (and any other client) | user | on demand |
| `mixengined` | user | login → logout (autostart, restartable) |
| `mixengine-elevate` | elevated | seconds — one batch of operations, then exits. **Never resident.** |
| Managed services | user | started/stopped by the supervisor |

Ports 80/443/53 are **not** obtained through elevation; they are designed away per platform
(direct bind on Windows, pf redirect or `setcap` on Unix, DNS on an unprivileged port). See
[decisions/0005-on-demand-elevation.md](../decisions/0005-on-demand-elevation.md).

Rationale for the tier split in
[decisions/0001-rust-core-daemon-gui-split.md](../decisions/0001-rust-core-daemon-gui-split.md).

## Crate responsibilities

- **`mixengine-proto`** — the shared vocabulary: every request, response, and event type, the error
  enum, and the types that describe a service (`ServiceSpec` and its policies, see
  [decisions/0006-servicespec-in-proto-and-secret-free.md](../decisions/0006-servicespec-in-proto-and-secret-free.md)).
  Serde only, no I/O, no platform code — that, rather than the list, is the constraint. Both the
  daemon and the CLI depend on it, and the TypeScript bindings published for out-of-repo clients
  are generated from it (`ts-rs`, T56).
- **`mixengine-core`** — pure domain: what a project/site/runtime/service *is*, config template
  rendering, version resolution, blueprint diffing. Takes storage and platform as injected traits so
  it is testable without touching the machine.
- **`mixengine-platform`** — traits (`HostsFile`, `TrustStore`, `ResolverConfig`, `ProcessLimits`,
  `ServiceInstaller`, `FirewallRules`, `Elevation`) with `windows/`, `macos/`, `linux/` impls and an
  in-memory `mock/` impl used by tests.
- **`mixengine-supervisor`** — spawn, watch, restart, health-check, capture logs. Knows nothing about
  PHP or MariaDB; it only understands `ServiceSpec`.
- **`mixengine-daemon`** — wires the above together, serves the API, runs the DNS server and the
  scheduler (cert renewal, idle shutdown, metrics sampling).
- **`mixengine-elevate`** — the only elevated code. One-shot, no listener, small enough to audit in
  one sitting, and it re-validates everything the daemon sends it.
- **`mixengine-cli`** — `clap` command tree mapping 1:1 onto API methods, plus human/JSON output.
- **`mixengine-shim`** — one binary, copied into `<root>/bin` under each command name it answers to
  (`php`, `node`, `npm` …). Reads the name it was invoked by, resolves a version **in its own
  process** against the database opened read-only, and becomes the real program. The one client that
  depends on `mixengine-core`, and it has to: the whole promise is that it works with no daemon
  running.
- **`mixengine-testkit`** — the fixtures every suite shares: a `TempDir` home, the `fakeservice`
  binary supervision is tested against, and the one way this workspace stops a process by pid. A
  **dev-dependency and never anything else**, which `mixengine-proto/tests/workspace_layering.rs`
  enforces rather than trusts — see [../standards/testing.md](../standards/testing.md).
There is no graphical client here and no frontend toolchain
([decisions/0011-no-gui-in-this-repository.md](../decisions/0011-no-gui-in-this-repository.md)).
What one would need from the API is written down in
[../features/client-surface.md](../features/client-surface.md).

Dependency direction is strictly downward: `cli` → `proto` → (nothing); `shim` → `core`,
`platform`, `proto`; `daemon` → `core`, `supervisor`, `platform`, `proto`. **`core` never depends on
`daemon`.** `testkit` sits outside that graph: it may depend on `platform`, and nothing may depend on
it outside `[dev-dependencies]`.

## On-disk layout

Root directory (`MIXENGINE_HOME`, overridable):

- Windows: `%LOCALAPPDATA%\MixEngine`
- macOS: `~/Library/Application Support/MixEngine`
- Linux: `$XDG_DATA_HOME/mixengine` (fallback `~/.local/share/mixengine`)

```
<root>/
  bin/            version-resolving shims: php, node, npm, python, ruby, composer …
  runtimes/       php/8.3.12/  node/22.8.0/  python/3.12.6/  ruby/3.3.5/
  packages/       caddy/2.8.4/  nginx/1.27.1/  mariadb/11.4.3/  postgresql/16.4/ …
  data/           mariadb/<instance>/  postgres/<instance>/  redis/<instance>/
  etc/            GENERATED config — safe to delete, regenerated from state
  certs/          ca/root.crt ca/root.key  sites/<domain>.{crt,key}
  logs/           daemon.log  services/<service-id>/current.log + rotated
  extensions/     <extension-id>/
  blueprints/     <name>.toml
  run/            pid files, sockets (Unix), health markers
  mixengine.db    SQLite — the single source of truth
  config.toml     user preferences the daemon reads at boot (paths, ports, telemetry off)
```

Nothing is written outside this root except: the hosts file, the OS trust store, resolver/NRPT
config, firewall rules, the port-80/443 redirect rule, and — since T85 — **`mixengine-elevate`
itself**, which the helper copies into the one directory on this system an ordinary account cannot
write ([ADR 0015](../decisions/0015-the-helper-installs-itself.md)) — and the root-owned audit log
beside it. All via `mixengine-elevate`, all reversible by `mix doctor --repair` and all removed by
**`mix uninstall`** (T87), which lists every one of them by name and by location first:
`mix uninstall --dry-run` changes nothing and needs no administrator.

**One of them does not go at once, and it is Windows'.** A file whose image is mapped cannot be
unlinked, and `mixengine-elevate.exe` is the running program when it removes itself — so there the
operating system is asked to remove it at the next restart, and the report says so rather than
claiming a removal that has not happened. See the
[T87 design](../../docs/superpowers/specs/2026-09-04-t87-uninstall-design.md), D8.

**Two more, and they are the ones that are not elevated**, because both belong to this account rather
than to the machine. Neither is ever written on the daemon's own initiative.

The first is this user's `PATH`, so that `<root>/bin` is on it. It is `HKEY_CURRENT_USER\Environment`
on Windows and a marked block in the user's own shell profiles on both others — user-writable
everywhere, which is why it needs no elevated helper — and it is written only when `path.install`
asks. `path.uninstall` takes it back off, leaving the rest of the file or the value exactly as it
was.

The second is **the daemon's autostart entry** (T85b): a Task Scheduler logon task, a LaunchAgent in
`~/Library/LaunchAgents`, or a systemd *user* unit — one per user, per-user on all three systems, and
written only when `autostart.enable` asks. **No installer registers it**, because the three formats
that run as root cannot know which account will use MixEngine and the three that run as the user are
not where a consent question belongs:
[ADR 0016](../decisions/0016-autostart-is-registered-by-mixengine.md). `autostart.disable` removes
it, and stops nothing that is running.

The one exception the user controls: `runtimes/`, `packages/`, `data/` and `logs/` can be moved to
another disk through `[paths]` in `config.toml`. They are still MixEngine's to create and remove;
the uninstaller reads their real location out of the same file rather than assuming.

## Lifecycle of a request (example: "start site `blog.test`")

1. A client calls `site.start { id }` over IPC.
2. Daemon loads the site + its project from SQLite, resolves the PHP version
   (project manifest → global default), and computes the required service set.
3. `core` renders the Caddy site block and the php-fpm pool config into `etc/`.
4. Supervisor ensures `php-fpm@8.3.33` is running (starting it if idle) and reloads Caddy.
5. Daemon ensures the domain resolves and a valid leaf certificate exists. With the internal DNS
   server wired up this needs **no elevation at all** — wildcards are answered by pattern, so
   creating a site prompts for nothing.
6. Daemon emits `site.state_changed` on the event stream; every attached client updates live.

Each step is idempotent — re-running `site.start` on a healthy site is a no-op that still verifies
state, which is what `mix doctor` reuses.

## Guiding constraints

- **State lives in exactly one place** (SQLite). Generated files are projections.
- **Everything is reversible.** Any change we make to the machine has an undo path.
- **Fast cold start.** Nothing heavyweight starts at login; services start on demand
  ([features/resource-isolation.md](../features/resource-isolation.md)).
- **The CLI is the reference client.** New API surface ships with CLI coverage in the same change.
