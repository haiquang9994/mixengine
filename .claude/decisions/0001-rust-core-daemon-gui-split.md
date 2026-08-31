# 0001. Rust core + daemon, thin CLI and GUI clients

**Status**: Accepted — the helper tier is superseded by [0005](0005-on-demand-elevation.md), the
GUI tier by [0011](0011-no-gui-in-this-repository.md)
**Date**: 2026-08-10

## Context

MixEngine must supervise long-lived processes (php-fpm pools, databases, a DNS server, a web server)
that outlive any window the user has open. It must also be usable from a terminal, from a GUI, from
CI, and eventually from an editor extension. The obvious shortcut — put the logic inside the Tauri
app — makes the GUI a mandatory dependency for background work and makes "the app is closed but my
sites still work" impossible without an ugly hidden-window hack.

The neighbouring project [MixDB](https://github.com/mixnz/mixdb) is Tauri + React + Rust, so
the team already knows that stack; the question was only where the *logic* lives.

## Decision

Three tiers:

1. **`mixengined`** — a user-level daemon owning all state (SQLite), all supervision, config
   generation, the DNS server and the scheduler. It is the only writer of state.
2. **`mix` (CLI) and the Tauri GUI** — thin clients over one JSON-RPC API on a local IPC transport.
   They contain presentation only.
3. **`mixengine-helper`** — a separate elevated binary with a closed, typed, allowlisted API for the
   few privileged operations.

Domain logic lives in `mixengine-core`, independent of the daemon, so it is testable without any
process or socket.

## Consequences

**Easy**: headless/CI/scriptable usage for free; the GUI can crash or be closed without affecting
running services; one API to test, with the CLI as its reference client; a future editor extension or
web UI is another client, not a rewrite; the elevated surface is small enough to audit.

**Hard / accepted costs**: an IPC layer to build and version; daemon lifecycle management (autostart,
crash recovery, single-instance) on three OSes; every feature needs API + CLI + GUI work, which is
more upfront effort per feature; debugging spans two processes.

**Enforcement**: "no business logic in clients" is a review blocker. The smell to watch for is a GUI
that can do something the CLI cannot.

## Alternatives considered

- **All logic inside the Tauri app.** Fastest to a demo. Rejected: no background operation, no
  scripting, and the logic would be untestable without a webview.
- **CLI-only, no daemon; state on disk, processes detached.** Simple, but nobody owns supervision —
  restarts, health checks and crash recovery all become cron-shaped hacks, and concurrent CLI
  invocations race on state.
- **A system-wide daemon running as root.** Simplifies privileged operations, massively expands the
  attack surface, and breaks multi-user machines. Rejected in favour of a user daemon plus a minimal
  helper.
