# MixEngine

A local web development environment (ServBay-style): run and switch multiple PHP / Node.js /
Python / Ruby versions, bundled Nginx/Caddy + MariaDB/PostgreSQL/Redis/Memcached, local domains
with automatic HTTPS — without Docker, without hand-written config files.

## Architecture in one paragraph

Rust core, split into three layers. **`mixengined`** (daemon) owns all state and supervises every
managed process. **`mix`** (CLI) and the **desktop GUI** (Tauri v2 + React) are thin clients that
speak the same JSON-RPC API over a local IPC transport (Unix socket / Windows named pipe). **Nothing
runs as root.** For the few one-shot operations that need it (hosts file, OS trust store, resolver
config, firewall rules), a short-lived **`mixengine-elevate`** is spawned through the OS elevation
prompt, does the work, and exits. Cross-platform (Windows, macOS, Linux) from day one — all
OS-specific behaviour lives behind traits in `mixengine-platform`.

## Workspace layout

```
crates/
  mixengine-core/        Domain logic: projects, sites, runtimes, services, blueprints
  mixengine-proto/       Shared API types (requests, responses, events) — single source of truth
  mixengine-platform/    OS abstraction traits + per-OS impls (hosts, trust store, DNS, limits)
  mixengine-supervisor/  Process supervision, health checks, log capture
  mixengine-daemon/      `mixengined` binary: API server + orchestration
  mixengine-elevate/     One-shot elevated binary (minimal, audited, self-validating)
  mixengine-cli/         `mix` binary
  mixengine-shim/        the version-resolving shim, copied into `<root>/bin` per command name
  mixengine-testkit/     Shared test fixtures — **dev-dependency only**, never in a shipped binary
apps/desktop/            Tauri v2 + React + TypeScript GUI (thin client)
```

## Non-negotiable rules

- **No business logic in clients.** CLI and GUI only render what the daemon returns. If the GUI can
  do something the CLI cannot, that is a bug.
- **No direct OS calls outside `mixengine-platform`.** No `#[cfg(windows)]` in core/daemon code.
- **No persistent root process, ever.** Elevation is one-shot and per-operation.
  `mixengine-elevate` never runs arbitrary commands, validates every request itself rather than
  trusting the daemon, and is excluded from auto-update.
- **Generated config is disposable.** Everything under `etc/` is regenerated from state in SQLite;
  never parse a generated file back into state.
- **Cross-platform or not merged.** A feature must compile on all three OSes; unsupported paths
  return a typed `Unsupported` error, never `todo!()`.
- **No Docker, no VM.** Managed processes are native. See `.claude/decisions/0003-no-container-isolation.md`.

## Detailed documentation

All design detail lives in [.claude/](.claude/) — start at [.claude/README.md](.claude/README.md).

- Architecture → [.claude/architecture/](.claude/architecture/)
- Feature specs → [.claude/features/](.claude/features/)
- Coding standards → [.claude/standards/](.claude/standards/)
- Build & packaging → [.claude/operations/](.claude/operations/)
- Decision records → [.claude/decisions/](.claude/decisions/)
- **Ordered build plan → [.claude/roadmap/todo.md](.claude/roadmap/todo.md)**

## Common commands

```bash
cargo check --workspace --all-targets   # fast feedback loop
cargo clippy --workspace -- -D warnings  # must be clean before commit
cargo fmt --all --check                  # CI's lint job gates on this too; clippy clean != fmt clean
cargo test --workspace                   # unit + integration
cargo doc --workspace --no-deps --document-private-items  # intra-doc links, for this OS only
cargo sqlx prepare --workspace -- --all-targets --all-features  # after editing any sqlx::query!
cargo run -p mixengine-cli -- status      # drive the daemon from the CLI
npm --prefix apps/desktop run tauri dev   # GUI against a running daemon
```

## Working agreements

- Before implementing a feature, read its spec in `.claude/features/` — specs are authoritative.
- Changing a cross-cutting decision requires a new ADR in `.claude/decisions/`, not an edit to an
  accepted one.
- Keep the roadmap current: tick tasks in their phase file (`.claude/roadmap/phase-*.md`) as they
  land, add follow-ups where they belong in the order, do not append them at the end.
  [.claude/roadmap/todo.md](.claude/roadmap/todo.md) is the index over those files.
- When splitting a batch of fixes across subagents, group the work by the invariant the findings
  share, not by the file they sit in — two agents editing around one invariant undo each other.
- CI is asked for, not automatic: `master` builds itself, any other branch is pushed and then
  requested — see [.claude/operations/build-and-release.md](.claude/operations/build-and-release.md).
