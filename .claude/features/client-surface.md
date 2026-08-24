# Client surface

**Goal**: prove, on paper, that the API is sufficient for a full graphical client — before a client
finds out by hitting a wall.

MixEngine ships no GUI ([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)). A graphical
client lives in its own repository and reaches the daemon through the same JSON-RPC API and event
stream the CLI uses, with TypeScript types generated from `mixengine-proto`. This page is not that
client's design; it is the list of things such a client must be able to *ask for*. Every line below
is a claim about the API, and each one is either satisfied by a method in
[architecture/daemon-and-ipc.md](../architecture/daemon-and-ipc.md) or it is a gap.

## Screens, and what each one demands of the API

1. **Dashboard** — per-service state, uptime, CPU %, RSS and port in one read; start/stop/restart per
   service and a global stop-all; disk usage broken down by category (runtimes, data, logs, certs)
   with a cleanup action; the recent slice of the event stream.
2. **Sites** — list carrying domain, runtime version, HTTPS state and health; create and edit with
   doc root, kind, PHP version, linked services and extra domains; reveal the doc root path and a
   browsable URL so the client can open a browser, a file manager or a terminal itself; per-site LAN
   sharing toggle.
3. **Runtimes** — installed versions per kind with the default marked; available versions;
   install/uninstall as jobs reporting progress; PHP extension toggles per version.
4. **Services** — the settings a service accepts (port, bind, data dir, limits, autostart, idle
   timeout) as data, not as a rendered form; the generated config readable back for display only;
   credentials fetched on demand; validation failures returned per field, not as one string.
5. **Logs** — a live tail filterable by service, with the on-disk path so the client can reveal it.
   Log lines arrive on their own stream, never the event stream
   ([ADR 0009](../decisions/0009-logs-travel-on-their-own-stream.md)).
6. **Domains & TLS** — the per-domain diagnostic table of
   [domains-and-dns.md](domains-and-dns.md); CA status with install and uninstall; per-site
   certificate status and reissue.
7. **Blueprints** — capture the current project, list what is captured, apply one to a new project
   ([blueprints.md](blueprints.md)).
8. **Extensions** — browse the registry, install and uninstall, per-extension settings, and whatever
   an extension needs to be opened ([extensions.md](extensions.md)).
9. **Settings** — root directory, managed TLDs, default web server, autostart, updates, and
   `daemon.doctor_repair`.

A tray or menu-bar item needs no more than the dashboard does: overall state, stop-all, and the site
list.

## Rules the API is responsible for

These are not client style guidance. Each one is a constraint on what the daemon must send, and the
reason a client can behave well without inventing anything.

- **State is announced, never inferred.** A client shows `Starting…` because the event stream said
  so. The API must make optimistic rendering unnecessary — a toggle that lies about whether MariaDB
  is up is worse than a slow one, and no client should have to guess.
- **Nothing blocks.** Any operation that can outlast a request is a job with an id and progress, so
  a client can render inline per-row state instead of a spinner over everything.
- **Elevation is explainable before it is requested.** `ElevationRequired` carries every batched
  operation and what it will literally change — the exact hosts lines, the port, the store — so a
  client can show that and then raise one prompt. A decline is a supported outcome the API models,
  not an error ([ADR 0005](../decisions/0005-on-demand-elevation.md)).
- **Every error carries its own remedy.** `Error::hint` is what a client renders as the suggested
  action, and the diagnostics bundle is `daemon.bundle` (T93): one call assembles the archive and
  answers with its path, so "copy diagnostics" is a file to open rather than five readings to
  gather. What it refuses to carry it names, so a client can show that too rather than presenting
  the archive as complete.
- **Metrics are sampled only while watched.** `metrics.subscribe` streams a sample per second while
  something is listening and stops when nothing is — polling a sleeping laptop is exactly the
  behaviour criticised elsewhere in these docs. Per-process CPU and RSS come from `sysinfo` in the
  daemon, aggregated per `ServiceId` across the process group.
- **A dead daemon is a legible state.** A client that loses the socket can tell the difference
  between "not running" and "not answering", and reconnects without being restarted.

## Left to the client

Layout, theming, accessibility, keyboard navigation, empty states, localisation and window chrome
belong to whoever builds the client. Nothing in this repository specifies them, and nothing in the
API should assume them.

## Acceptance criteria

- Every mutating RPC in [architecture/daemon-and-ipc.md](../architecture/daemon-and-ipc.md) is
  reachable from the CLI — which, since the CLI is the only client here, is the whole guarantee that
  no capability is trapped behind a screen that does not exist.
- Every screen above can be assembled from documented methods and events, with no method existing
  solely to serve one of them.
