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
| [0013](0013-reading-the-d-bus-error-name-to-tell-an-absent-store.md) | The D-Bus error name is what tells an absent credential store from a refusing one | Accepted |
| [0014](0014-an-extension-is-not-an-api-client.md) | An extension is not an API client, and gets no token | Accepted |
| [0015](0015-the-helper-installs-itself.md) | The privileged helper installs itself, and the installer does not | Accepted |
| [0016](0016-autostart-is-registered-by-mixengine.md) | MixEngine registers the daemon's autostart entry, and the installer does not | Accepted |
| [0017](0017-smart-app-control-is-an-unsupported-configuration.md) | A machine with Smart App Control enforcing is a configuration MixEngine does not support | Accepted |
| [0018](0018-a-signed-candidate-is-what-lets-a-path-cross-the-boundary.md) | A path may cross into the elevated process only when that process itself checks a signature over the bytes at it | Accepted |
| [0019](0019-an-added-response-member-is-optional.md) | A member added to a response is optional on the wire, and the protocol does not bump for it | Accepted |
| [0020](0020-the-published-contract-is-the-shape-the-daemon-writes.md) | The published contract is the shape the daemon writes, not everything it accepts | Accepted |
| [0021](0021-the-handbook-is-one-corpus-published-three-ways.md) | The handbook is one Markdown corpus published three ways, and help is not an API method | Accepted |
| [0022](0022-a-crash-report-is-recorded-by-default-and-sent-by-nothing.md) | A crash report is recorded by default and sent by nothing | Accepted |

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
