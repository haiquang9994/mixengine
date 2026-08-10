# Phase 6 — Desktop GUI

*Goal: the terminal becomes optional.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

- [ ] **T55** Tauri v2 shell + Rust proxy to the daemon socket + SSE relay to the webview.
- [ ] **T56** `ts-rs` binding generation + the CI check that committed bindings are current.
- [ ] **T57** Frontend foundation: Vite, strict TS, TanStack Query, event→invalidation mapping,
      `ui/` primitives, i18n (English + Vietnamese), light/dark.
- [ ] **T58** Dashboard: service tiles, metrics (`metrics.subscribe`, sampling only while subscribed),
      disk usage, recent events.
- [ ] **T59** Sites screen: list, create/edit drawer, open in browser/folder/terminal.
- [ ] **T60** Runtimes screen: installed/available, install jobs with progress, PHP extension toggles.
- [ ] **T61** Services screen: settings forms, rendered config read-only, validation errors,
      credential reveal.
- [ ] **T62** Logs viewer: live tail, filter, search, pause-on-scroll.
- [ ] **T63** Domains & TLS screen: the diagnostic table, CA install/uninstall, per-site reissue.
- [ ] **T64** Elevation UX: first-run setup screen requesting one batched prompt; per-op dialogs
      showing the literal change (the exact hosts lines, the port, the store); a persistent "pending
      permissions" surface after a decline.
- [ ] **T65** Tray/menu-bar item: state, start/stop all, quick-open sites, sharing indicator.
- [ ] **T66** Settings screen + `doctor_repair` surface; "copy diagnostics" on every error.
- [ ] **T67** GUI cold-start benchmark (< 1.5 s) and Playwright E2E for create-site → open.

**Milestone M6** — a user installs, creates a Laravel-shaped site with HTTPS, and never opens a
terminal.

---

Previous: [Phase 5 — HTTPS](phase-5-https.md) · Next: [Phase 7 — Efficiency](phase-7-efficiency.md)
