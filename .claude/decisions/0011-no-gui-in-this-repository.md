# 0011. MixEngine ships a CLI; a GUI is a client in another repository

**Status**: Accepted — supersedes the GUI tier of [0001](0001-rust-core-daemon-gui-split.md)
**Date**: 2026-08-22

## Context

[ADR 0001](0001-rust-core-daemon-gui-split.md) put all logic in `mixengined` and named two thin
clients over one JSON-RPC API: `mix`, and a Tauri v2 + React application in `apps/desktop`. The CLI
half has been built since Phase 0. The GUI half was scheduled as Phase 6 — thirteen tasks, T55–T67,
milestone M6 *"the terminal becomes optional"* — and was never started. `apps/desktop` does not
exist. Nothing has been written against it.

That is what makes this decision cheap now and expensive in six months: today it costs an ADR and a
documentation pass; after T55 it costs a Tauri shell, a Vite/TanStack/i18n foundation, a design
system, a Playwright suite and a second release pipeline, all built and then abandoned.

Three forces argue against building it here.

**It is the second of something that already exists.** ADR 0001 chose this architecture partly
because the neighbouring project [MixDB](https://github.com/mixnz/mixdb) is Tauri + React +
Rust and the stack was already known. MixDB does not merely know the stack — it *is* an installed,
released Tauri application with a design system, i18n, an installer and an updater. Phase 6 rebuilds
each of those next door (T55 shell, T57 Vite + TanStack Query + i18n + theming, T67 Playwright), and
leaves two application shells, two updaters, two installers and two release pipelines to maintain.

**It is not on the critical path.** Phase 4 stands at 4 of 14 and Phase 5 has not begun. What stops
`https://blog.test` from working is sites, elevation and TLS, not screens. Thirteen tasks of
anticipated presentation work sit in the plan describing requirements that will have moved by the
time anyone reaches them.

**A client in another repository tests the API harder than a client in this one.** This is the
argument that decides it, and it runs against intuition. A GUI inside the workspace can always
cheat: import a type from `mixengine-proto` directly, read a file the daemon generated, or acquire
one RPC method that exists because one screen needed it. A client on the far side of a published
API, on its own release cadence, has none of those moves available. The rule at
[daemon-and-ipc.md](../architecture/daemon-and-ipc.md) — *every mutating method is expressible in
the CLI; no GUI-only capabilities* — stops being a thing reviewers must watch for and becomes a
property of the workspace: there is no GUI here to grant a capability to.

## Decision

This repository ships `mixengined`, `mix`, `mixengine-elevate` and `mixengine-shim`. It contains no
graphical client and no frontend toolchain.

1. **The API is the product surface.** The JSON-RPC API and the TypeScript bindings generated from
   `mixengine-proto` (`ts-rs`) are a released artifact, versioned like any other. Generating and
   publishing them stays on the plan; consuming them does not happen here.
2. **A GUI is a client, and no client is privileged.** MixEngine names no official front end. MixDB
   is expected to be the first consumer and is welcome to be, but nothing in this repository depends
   on it, detects it as a client, or shapes an API method around it. The one-directional coupling
   already stated in [extensions.md](../features/extensions.md) — MixEngine knows how to hand a
   connection to MixDB, MixDB need not know MixEngine exists — is unchanged by this ADR, because it
   describes a different relationship: a database client we launch, not a front end we depend on.
3. **The screens survive as requirements on the API.** `features/gui.md` becomes
   [client-surface.md](../features/client-surface.md): the nine screens stay, stripped of stack and
   frontend architecture, as the statement of what any full graphical client must be able to ask for.
   It is the checklist that answers *is the API sufficient* on paper, before a client discovers the
   answer by hitting a wall.
4. **The CLI is the reference client, and now the only one.** Every capability reaches a person
   through `mix`. A gap in the CLI is a gap in the product, not a gap to be covered by a GUI.

## Consequences

**Easy.** Thirteen tasks, a frontend toolchain, a design system, an E2E suite and a second release
pipeline leave the plan. The critical path to a usable product shortens to sites, elevation and
TLS. "No business logic in clients" and "no GUI-only capabilities" become unfalsifiable here rather
than enforced by review. Every daemon affordance designed with a GUI in mind is still built and
still justified — `ElevationRequired` batching one prompt out of many operations, `Error::hint`,
jobs with progress, `metrics.subscribe` sampling only while subscribed — because an out-of-repo
client needs exactly those, and needs them more.

**Hard, and accepted.** The API loses its most demanding consumer from this repository's CI. Under
the old plan, an insufficient API surfaced as a GUI task that could not be finished; now it surfaces
in another repository, later, across a boundary. `client-surface.md` and the CLI-completeness rule
are the mitigation, and they are weaker than a compiler.

Packaging and updates lose their foundation. [updates.md](../features/updates.md) and
[build-and-release.md](../operations/build-and-release.md) were written on the Tauri v2 bundler and
the Tauri updater — mandatory minisign signing, `latest.json` produced by `tauri-action`, the
updater's exit-and-replace dance. None of that arrives with a CLI. How `mix` is installed and how it
updates itself is now an open question, answered in Phase 9 rather than inherited.

Milestone M6 is withdrawn. *The terminal becomes optional* is not a promise this repository makes.
MixEngine v0.1 is a terminal product; a user escapes the terminal by installing a client. This is a
deliberate narrowing of the audience to developers who already live in one, and it is the cost that
would have to be revisited first if that audience assumption ever changes.

## Alternatives considered

- **Build the GUI here, as Phase 6 planned.** The strongest case for it is the fast feedback loop:
  an API gap fails in the same CI run that caused it. Rejected because the loop is paid for with a
  duplicate application shell, and because the same gap is found by `client-surface.md` at a
  fraction of the cost.
- **Park Phase 6 rather than delete it.** Cheaper today and closes nothing. Rejected because it
  leaves `updates.md` and `build-and-release.md` resting on a Tauri updater that may never ship —
  a contradiction that costs nothing right up to the day it costs a release.
- **Make MixDB the official front end and let it carry `mixengined`.** Least total work: one
  installer, one updater, one release. Rejected on two grounds — MixEngine could no longer be
  released, scripted or run on a server without a database client attached, and an API with exactly
  one consumer drifts into that consumer's shape, which is the failure ADR 0001 was written to
  avoid.
