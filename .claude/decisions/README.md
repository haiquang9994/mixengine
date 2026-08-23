# Architecture decision records

One file per decision, numbered, immutable once accepted. To change a decision, write a **new** ADR
that supersedes the old one and update the old one's status line — never edit its body.

## Index

| # | Decision | Status |
| --- | --- | --- |
| [0001](0001-rust-core-daemon-gui-split.md) | Rust core + daemon, thin CLI and GUI clients | Accepted |
| [0002](0002-cross-platform-from-day-one.md) | Cross-platform from day one via a platform trait layer | Accepted |
| [0003](0003-no-container-isolation.md) | Native processes, no Docker/VM isolation | Accepted |
| [0004](0004-caddy-as-default-web-server.md) | Caddy as the default web server, Nginx optional | Accepted |
| [0005](0005-on-demand-elevation.md) | On-demand elevation, no persistent privileged helper | Accepted |
| [0006](0006-servicespec-in-proto-and-secret-free.md) | `ServiceSpec` lives in `mixengine-proto` and never carries a secret | Accepted |
| [0007](0007-supervised-child-owns-a-process-group.md) | A supervised child owns a process group, and "no orphans" means three different things | Accepted |
| [0008](0008-no-signal-stop-on-windows.md) | A service is asked to stop with a signal on Unix and with a command on Windows | Accepted |
| [0009](0009-logs-travel-on-their-own-stream.md) | Log lines travel on their own stream, never on the event stream | Accepted |
| [0010](0010-supervised-child-never-inherits-administrators.md) | A child started to run a user's software never inherits Administrators | Accepted |
| [0011](0011-no-gui-in-this-repository.md) | MixEngine ships a CLI; a GUI is a client in another repository | Accepted |
| [0012](0012-a-boot-time-job-enables-the-packet-filter-on-macos.md) | A boot-time job enables the packet filter on macOS | Accepted |

## Template

```markdown
# NNNN. <Short title>

**Status**: Proposed | Accepted | Superseded by [NNNN](…) | Deprecated
**Date**: YYYY-MM-DD

## Context
What forces are at play? What did we know at the time?

## Decision
What we are doing, stated plainly.

## Consequences
What becomes easy, what becomes hard, what we accept as the cost.

## Alternatives considered
Each with the reason it lost.
```

Write an ADR when a choice is expensive to reverse, spans more than one crate, or will otherwise be
re-litigated in six months by someone (possibly you) who has forgotten the reasoning.
