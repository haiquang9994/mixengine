# 0003. Native processes, no Docker or VM isolation

**Status**: Accepted
**Date**: 2026-08-10

## Context

The competing approach to local development is Docker Compose (or Lando/DDEV/Laradock on top of it).
It gives excellent reproducibility and true isolation. It also costs, on macOS and Windows, a Linux
VM: multi-gigabyte memory reservation, constant background CPU, slow bind-mount filesystem I/O, fans
that never stop, and a laptop battery that dies by lunchtime. That cost is the entire reason products
like ServBay exist, and it is the reason a user would choose MixEngine.

## Decision

Run every managed service as a **native process on the host**. No Docker, no VM, no filesystem or
network namespaces. Isolation is achieved through:

- **On-demand start** — nothing runs until something needs it.
- **Idle shutdown** — services stop again when they go quiet.
- **OS-native resource limits** — Job Objects on Windows, cgroup v2 on Linux, priority/QoS plus a
  watchdog on macOS.
- **Per-project separation at the logical level** — separate databases, separate php-fpm pools,
  separate config, shared server processes.

Details in [../features/resource-isolation.md](../features/resource-isolation.md).

## Consequences

**Easy**: near-zero idle cost, which is the product's core promise and a published, CI-enforced
number (< 60 MB idle); native filesystem speed, so `composer install` and file watchers behave
normally; every host tool (editor, debugger, profiler) works without container plumbing; no VM to
install or update.

**Hard / accepted costs**:

- **Reproducibility is weaker than a container image.** Two machines running the same blueprint can
  still differ in OS libraries. Blueprints pin versions, which narrows but does not close the gap.
- **No true isolation.** A project can reach another project's database, and a runaway process
  affects the machine. This is acceptable on a single-user dev machine and is stated plainly in
  [../architecture/security-model.md](../architecture/security-model.md).
- **We own the packaging problem.** Every runtime must be built or sourced as a relocatable artifact
  for six OS/arch combinations — the largest ongoing operational cost of this project
  ([../operations/runtime-packaging.md](../operations/runtime-packaging.md)).
- Projects whose production environment genuinely needs Linux-only extensions are better served by
  Docker; we should say so rather than pretend otherwise.

## Alternatives considered

- **Docker under the hood with a nicer UI** (the DDEV/Lando model). Solves packaging and
  reproducibility instantly. Rejected: it inherits exactly the resource cost we are selling against.
- **Hybrid: native by default, optional container for exotic services.** Attractive, and not
  permanently ruled out — but it doubles the execution model, and a half-container product is
  confusing. Revisit as an *extension* once the native path is solid.
- **microVMs (Firecracker/krunkit).** Lighter than Docker Desktop, but still a VM, still immature on
  Windows, and still a bind-mount performance story.
