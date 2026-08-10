# MixEngine documentation map

This folder holds the detail that [CLAUDE.md](../CLAUDE.md) deliberately keeps out. Each folder has
one job:

| Folder | Answers | Read it when |
| --- | --- | --- |
| [architecture/](architecture/) | *How the system is put together* | Adding a subsystem, changing a boundary |
| [features/](features/) | *What each user-facing feature must do* | Implementing or changing a feature |
| [standards/](standards/) | *How we write code here* | Writing anything |
| [operations/](operations/) | *How it gets built, packaged, shipped* | Touching CI, installers, runtime bundles |
| [decisions/](decisions/) | *Why it is this way* | Questioning an existing choice |
| [roadmap/](roadmap/) | *What to build next, in order* | Picking up work |

## Reading order for a newcomer

1. [architecture/overview.md](architecture/overview.md) — the whole system on one page
2. [architecture/daemon-and-ipc.md](architecture/daemon-and-ipc.md) — how clients talk to the core
3. [architecture/data-model.md](architecture/data-model.md) — the nouns and their relationships
4. [roadmap/todo.md](roadmap/todo.md) — where we are

## Index

### architecture
- [overview.md](architecture/overview.md) — layers, processes, on-disk layout, request lifecycle
- [daemon-and-ipc.md](architecture/daemon-and-ipc.md) — transport, JSON-RPC surface, event stream
- [process-supervision.md](architecture/process-supervision.md) — service specs, restarts, health, logs
- [platform-abstraction.md](architecture/platform-abstraction.md) — the OS traits and their impls
- [data-model.md](architecture/data-model.md) — SQLite schema, config files, state ownership
- [security-model.md](architecture/security-model.md) — privilege split, authn, secrets, threat notes

### features
- [runtime-versions.md](features/runtime-versions.md) — multi-version PHP/Node/Python/Ruby
- [services.md](features/services.md) — web servers, databases, caches
- [domains-and-dns.md](features/domains-and-dns.md) — `.test` domains, hosts file, internal DNS
- [tls.md](features/tls.md) — internal CA, per-site certs, trust store, renewal
- [gui.md](features/gui.md) — desktop app scope, screens, live metrics
- [lan-sharing.md](features/lan-sharing.md) — access from phones/tablets on the same Wi‑Fi
- [blueprints.md](features/blueprints.md) — capture and clone an environment
- [extensions.md](features/extensions.md) — plugin model, registry, MixDB integration
- [resource-isolation.md](features/resource-isolation.md) — lightweight limits, on-demand start

### standards
- [rust.md](standards/rust.md) · [frontend.md](standards/frontend.md) ·
  [testing.md](standards/testing.md) · [git-and-reviews.md](standards/git-and-reviews.md)

### operations
- [build-and-release.md](operations/build-and-release.md) · [runtime-packaging.md](operations/runtime-packaging.md)

### decisions
- [decisions/README.md](decisions/README.md) — ADR index and template
