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
