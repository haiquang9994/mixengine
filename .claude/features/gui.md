# Desktop GUI

**Goal**: everything is doable without the terminal, and the app tells the truth about what is
running and what it costs.

Stack: Tauri v2 + React + TypeScript + Vite. The Rust side of Tauri is a **proxy only** — it opens
the daemon socket, forwards RPC, and relays the SSE event stream to the webview. No domain logic, no
direct process spawning, no filesystem writes outside Tauri's own state.

## Screens

1. **Dashboard** — service tiles (state, uptime, CPU %, RSS, port) with start/stop/restart; global
   "Stop all"; disk usage by category (runtimes, data, logs, certs) with a cleanup action; recent
   events.
2. **Sites** — list with domain, runtime version, HTTPS state, health; create/edit drawer (doc root
   picker, kind, PHP version, linked services, extra domains); "Open in browser", "Open folder",
   "Open terminal here"; per-site LAN sharing toggle.
3. **Runtimes** — installed versions per kind with the default marked; available versions with
   install/uninstall jobs and progress; PHP extension toggles per version.
4. **Services** — per-service settings form (port, bind, data dir, limits, autostart, idle timeout),
   the rendered config read-only, credentials reveal, and the validation error surface.
5. **Logs** — live tail with service filter, level/text search, pause-on-scroll, copy, reveal file.
6. **Domains & TLS** — per-domain diagnostic table ([domains-and-dns.md](domains-and-dns.md)),
   CA status and install/uninstall, per-site cert status and reissue.
7. **Blueprints** — capture current project, list, apply to a new project
   ([blueprints.md](blueprints.md)).
8. **Extensions** — registry browse, install/uninstall, open UI, per-extension settings
   ([extensions.md](extensions.md)).
9. **Settings** — root directory, managed TLDs, default web server, autostart, updates, language
   (English + Vietnamese from day one), reset/repair (`daemon.doctor_repair`).

Plus a **menu-bar / tray item**: overall state, start/stop all, list of sites for quick open, quit.

## Interaction rules

- **Never block the UI on an RPC.** Long operations are jobs with progress; the affected row shows
  its own inline state.
- **Optimistic UI is banned for service state.** Show `Starting…` from the event stream, not a guess.
  A toggle that lies about whether MariaDB is up is worse than a slow one.
- **Elevation is explained before it is requested.** When the daemon emits `ElevationRequired`, show
  every batched operation and what it will literally change (the exact hosts lines, the port, the
  store) — then one prompt. Declining is a supported outcome, not an error dialog.
- **Every error shows the hint** from `Error::hint` plus a "copy diagnostics" button that bundles
  daemon log excerpt + `mix doctor` output.
- **Empty states teach.** No sites yet → the create flow with a short explanation, not a blank list.

## Frontend architecture

- `src/api/` — generated TypeScript types from `mixengine-proto` (`ts-rs`), one thin client module
  per namespace. Nothing else in the app constructs RPC payloads.
- `src/state/` — TanStack Query for RPC reads/mutations; a single SSE subscriber invalidates the
  right query keys on events. No second copy of server state in a store.
- `src/features/<screen>/` — colocated components, hooks, and tests per screen.
- `src/ui/` — design-system primitives only; no feature imports.

Details in [../standards/frontend.md](../standards/frontend.md).

## Metrics

`metrics.subscribe` streams a sample per second while the dashboard is open (and stops when it is
not — polling a sleeping laptop is exactly the behaviour we criticise elsewhere). Per-process CPU/RSS
comes from `sysinfo` in the daemon, aggregated per `ServiceId` across the process group.

## Accessibility & platform fit

- Full keyboard navigation, visible focus, and no colour-only state encoding (icons + text too).
- Respects the OS light/dark setting and the reduced-motion preference.
- Native window chrome per platform; no custom title bar unless it earns its keep.

## Acceptance criteria

- Every mutating RPC in [architecture/daemon-and-ipc.md](../architecture/daemon-and-ipc.md) is
  reachable from the GUI.
- Killing the daemon while the GUI is open shows a clear "daemon not running — start it" state and
  recovers automatically when it returns.
- Cold start of the GUI to a painted dashboard: **< 1.5 s** on a mid-range laptop.
