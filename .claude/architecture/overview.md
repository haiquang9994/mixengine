# Architecture overview

## Processes at runtime

```
┌──────────────┐   ┌──────────────┐
│  Desktop GUI │   │  mix (CLI)   │      thin clients, no business logic
│  Tauri+React │   │              │
└──────┬───────┘   └──────┬───────┘
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
   caddy · nginx · php-fpm@8.1 · php-fpm@8.3 · mariadbd · postgres · redis-server ·
   memcached · node · extensions (mailpit, phpmyadmin runner, …)
```

Three privilege levels, three lifetimes:

| Component | Privilege | Lifetime |
| --- | --- | --- |
| GUI / CLI | user | on demand |
| `mixengined` | user | login → logout (autostart, restartable) |
| `mixengine-elevate` | elevated | seconds — one batch of operations, then exits. **Never resident.** |
| Managed services | user | started/stopped by the supervisor |

Ports 80/443/53 are **not** obtained through elevation; they are designed away per platform
(direct bind on Windows, pf/nftables redirect or `setcap` on Unix, DNS on 5353). See
[decisions/0005-on-demand-elevation.md](../decisions/0005-on-demand-elevation.md).

Rationale for the tier split in
[decisions/0001-rust-core-daemon-gui-split.md](../decisions/0001-rust-core-daemon-gui-split.md).

## Crate responsibilities

- **`mixengine-proto`** — every request, response, and event type, plus the error enum. Serde only,
  no I/O, no platform code. Both the daemon and the Tauri backend depend on it; the TypeScript
  types in the GUI are generated from it (`ts-rs`).
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
- **`apps/desktop`** — Tauri v2 shell. Its Rust side is a proxy to the daemon socket; its React side
  is the only place with UI concerns.

Dependency direction is strictly downward: `cli`/`desktop` → `proto` → (nothing); `daemon` → `core`,
`supervisor`, `platform`, `proto`. **`core` never depends on `daemon`.**

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
config, firewall rules, the port-80/443 redirect rule, and the daemon's autostart entry — all via
`mixengine-elevate`, all reversible by `mix doctor --repair` / uninstall.

The one exception the user controls: `runtimes/`, `packages/`, `data/` and `logs/` can be moved to
another disk through `[paths]` in `config.toml`. They are still MixEngine's to create and remove;
the uninstaller reads their real location out of the same file rather than assuming.

## Lifecycle of a request (example: "start site `blog.test`")

1. GUI calls `site.start { id }` over IPC.
2. Daemon loads the site + its project from SQLite, resolves the PHP version
   (project manifest → global default), and computes the required service set.
3. `core` renders the Caddy site block and the php-fpm pool config into `etc/`.
4. Supervisor ensures `php-fpm@8.3` is running (starting it if idle) and reloads Caddy.
5. Daemon ensures the domain resolves and a valid leaf certificate exists. With the internal DNS
   server wired up this needs **no elevation at all** — wildcards are answered by pattern, so
   creating a site prompts for nothing.
6. Daemon emits `site.state_changed` on the event stream; GUI and any attached CLI update live.

Each step is idempotent — re-running `site.start` on a healthy site is a no-op that still verifies
state, which is what `mix doctor` reuses.

## Guiding constraints

- **State lives in exactly one place** (SQLite). Generated files are projections.
- **Everything is reversible.** Any change we make to the machine has an undo path.
- **Fast cold start.** Nothing heavyweight starts at login; services start on demand
  ([features/resource-isolation.md](../features/resource-isolation.md)).
- **The CLI is the reference client.** New API surface ships with CLI coverage in the same change.
