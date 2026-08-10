# 0002. Cross-platform from day one via a platform trait layer

**Status**: Accepted
**Date**: 2026-08-10

## Context

ServBay started macOS-first and added Windows later. Development happens on Windows 11 here, but the
audience for a local dev environment is split across all three OSes, and the *hardest* parts —
privileged ports, hosts file, trust stores, DNS resolver wiring, process limits — are exactly the
parts that differ per OS. Retrofitting a second OS onto code that assumed the first one usually means
rewriting the parts that were hardest to get right.

## Decision

Support Windows, macOS and Linux from the first release. All OS-specific behaviour lives behind
traits in `mixengine-platform` (`HostsFile`, `TrustStore`, `ResolverConfig`, `Elevation`,
`ServiceInstaller`, `ProcessLimits`, `FirewallRules`, `NetworkInfo`, `Keyring`, `PathIntegration`).
No `#[cfg(target_os = …)]` anywhere else in the workspace.

A capability that genuinely cannot exist on a platform returns
`Error::UnsupportedPlatform { capability, reason }` with a hint — a typed, user-visible, testable
outcome — never a panic and never a silently-degraded lie.

CI runs the full test suite on all three runners; the platform-specific suites run elevated.

## Consequences

**Easy**: no rewrite when the second OS arrives; the trait boundary gives us a mock implementation,
which makes the daemon fully testable without touching a real machine; users get identical behaviour
and identical docs everywhere.

**Hard / accepted costs**: every platform feature is three implementations plus a mock, so features
land slower; CI is three times the cost; we must own knowledge of NRPT, `/etc/resolver`,
`systemd-resolved`, NSS, Job Objects and cgroups. Some capabilities are simply weaker on one platform
(hard memory caps on macOS) and the UI must reflect that honestly instead of pretending.

**Enforcement**: a `#[cfg(windows)]` outside `mixengine-platform` fails review; a `todo!()` in a
platform impl fails review.

## Alternatives considered

- **Windows-first, port later.** Fastest for the immediate developer. Rejected — the port would land
  after the architecture had calcified around Windows assumptions (no `exec`, different privileged
  port rules, no cgroups), which is precisely the expensive kind of rework.
- **macOS-first, mirroring ServBay.** Same objection, plus it is not the machine this is being built
  on, so the feedback loop would be slow.
- **Linux-only + WSL for Windows users.** Cheap and tempting, but WSL's networking and filesystem
  performance make it a poor local dev environment for the workflows we target, and it abandons
  macOS entirely.
