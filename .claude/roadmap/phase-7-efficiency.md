# Phase 7 — Efficiency

*Goal: deliver the promise that idle costs nothing.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [~] **T68** `ResourceLimits` per OS: Job Objects, cgroup v2, macOS QoS; the API reports only
      what the platform really supports, so no client can offer a control that does nothing. **(P)**
      Design: [2026-08-26-t68-resource-limits-design.md](../../docs/superpowers/specs/2026-08-26-t68-resource-limits-design.md).
      **The macOS watchdog came out of this task and became T71a**, below: warning and restarting at a
      threshold needs a per-process RSS sample taken repeatedly, and that sampler is T71. Building a
      second one here to serve one field on one operating system would put a loop in the supervisor
      that T71 would then replace.
- [ ] **T69** Idle detection (connections, request counters, query counters) and `IdlePolicy`
      shutdown, with per-project "keep warm".
- [ ] **T70** On-demand activation gateway: hold the socket, start the service, wait for ready, proxy
      the first request.
- [ ] **T71** Metrics history: 1 s sampling while subscribed, 24-hour downsampled retention.
- [ ] **T71a** The macOS memory watchdog: warn at a `memory_mb` it cannot enforce, and restart at a
      threshold when the service asks to be. **Split out of T68**, and ordered here rather than there
      because it is the one part of `ResourceLimits` that is not a call on a kernel object — macOS has
      no hard memory cap, so the limit becomes a reading taken repeatedly and compared, which is
      T71's sampler and nothing else. Until this lands, `LimitSupport` answers `Unsupported` for
      memory on macOS and means it. **(P)**, though only one of the three does anything.
- [ ] **T72** CI budgets: idle footprint < 60 MB RSS, cold path < 1.5 s — failing the build on
      regression.
- [ ] **T73** Dev-tuned defaults pass over every service template (buffer pools, memory limits).

**Milestone M7** — after 30 idle minutes only `mixengined` + the web server are running, and the next
request still succeeds within budget.

---

Previous: [Phase 5 — HTTPS](phase-5-https.md) · Next: [Phase 8 — Differentiators](phase-8-differentiators.md)
