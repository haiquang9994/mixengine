# Phase 7 — Efficiency

*Goal: deliver the promise that idle costs nothing.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T68** `ResourceLimits` per OS: Job Objects, cgroup v2, macOS QoS + watchdog; the API reports only
      what the platform really supports, so no client can offer a control that does nothing. **(P)**
- [ ] **T69** Idle detection (connections, request counters, query counters) and `IdlePolicy`
      shutdown, with per-project "keep warm".
- [ ] **T70** On-demand activation gateway: hold the socket, start the service, wait for ready, proxy
      the first request.
- [ ] **T71** Metrics history: 1 s sampling while subscribed, 24-hour downsampled retention.
- [ ] **T72** CI budgets: idle footprint < 60 MB RSS, cold path < 1.5 s — failing the build on
      regression.
- [ ] **T73** Dev-tuned defaults pass over every service template (buffer pools, memory limits).

**Milestone M7** — after 30 idle minutes only `mixengined` + the web server are running, and the next
request still succeeds within budget.

---

Previous: [Phase 5 — HTTPS](phase-5-https.md) · Next: [Phase 8 — Differentiators](phase-8-differentiators.md)
