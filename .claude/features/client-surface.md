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
   sharing toggle — **T74**: `site.share` answers the interface, the address and the URL, and
   `site.unshare` takes it back; a machine with more than one network refuses rather than choosing,
   and names the candidates so a client can offer them.
3. **Runtimes** — installed versions per kind with the default marked; available versions;
   install/uninstall as jobs reporting progress; PHP extension toggles per version.
4. **Services** — the settings a service accepts (port, bind, data dir, limits, autostart, idle
   timeout) as data, not as a rendered form; the generated config readable back for display only;
   credentials fetched on demand; validation failures returned per field, not as one string. **And,
   for a database service, where it can be opened — T83**: whether a desktop database client is
   installed to hand the connection to, and the handoff itself, answered per service so a client
   draws the affordance from data instead of probing the filesystem for an application
   ([extensions.md](extensions.md)). Until T83 lands this line is a gap, not a claim.
5. **Logs** — a live tail filterable by service, with the on-disk path so the client can reveal it.
   Log lines arrive on their own stream, never the event stream
   ([ADR 0009](../decisions/0009-logs-travel-on-their-own-stream.md)).
6. **Domains & TLS** — the per-domain diagnostic table of
   [domains-and-dns.md](domains-and-dns.md); CA status with install and uninstall; per-site
   certificate status and reissue. **Two trust answers and not one** — T49b: `cert.ca_status`
   carries `trust` for the system store and `browsers` for the NSS databases Firefox and Chrome read
   instead, one row each with the path and which browser owns it. A client that collapsed them would
   show a green tick beside a browser that shows a red padlock. The repair for the second is
   `daemon.doctor_repair` and raises no prompt, so it is a button and not an elevation flow.
   **And per-site certificate state — T50**: `cert.issue` answers one `SiteCertOutcome` per site with
   the names its certificate covers and how many days it has, so a client renders a table rather than
   asking per site. Reissuing is the same call, is idempotent, and raises no prompt.
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
- **Metrics are sampled at two rates, and the faster one is only while watched.** Opening
  `GET /metrics` streams a reading a second and closing it stops that — the connection *is* the
  subscription, so a client that crashes cannot leave a laptop being polled. With nobody watching the
  daemon still takes one reading a minute, and that is what the 24-hour history is made of: *"what
  was eating my battery last night"* is a question about a night nobody was watching, so a history
  kept only while somebody looked would hold exactly the minutes that needed no recording. One
  reading costs about 10 ms on Windows and about 2 ms on Linux — measured — which is a fiftieth of a
  percent of one core once a minute. This bullet said "sampled only while watched" until **T71**
  built it and found the two halves could not both be true.
- **A missing minute means nobody measured, never that nothing was used.** The service was stopped,
  or the machine was asleep, or the daemon was being replaced. A client draws a gap; joining the
  points across one invents a night of measurements that were never taken. The same rule inside a
  reading: `cpu_percent` is `null` where no figure could be taken, and a client that renders that as
  0% is claiming a service was idle in the second it was most expensive.
- **Per-service CPU and RSS are aggregated across the process group** — a php-fpm master and its
  workers are one row. Shared pages are counted once per process, so the number overstates a pool;
  it is an overestimate on every platform equally, which is the safe direction for a figure this
  project defends in its README. It is **not** the quantity a `memory_mb` limit is judged against
  where a kernel holds it — that is commit charge on Windows and charged pages on Linux — and it
  **is** exactly that quantity where the T71a watchdog holds it instead, which
  `LimitSupport::memory_measure` says by answering `resident`.
- **A memory control is drawn differently for `Advisory` than for `Hard`** — roadmap task **T71a**.
  `Hard` is a wall: at the ceiling the service is killed or its next allocation fails. `Advisory` is
  a watched line: the service may go over it and keep running, and what follows is a warning and —
  where the service's recipe permits — a restart. A client must offer the control in both cases and
  must not present the second as a guarantee. `Advisory { why }` carries a sentence only where the
  machine could be started differently; `null` means an operating system with nothing to fix, and a
  client that printed a placeholder there would be inventing advice.
- **Whether *this* service would be restarted is not on `LimitSupport`.** That type describes the
  machine and is handed no service. `service.limits` answers per service, in `watchdog`:
  `{ after_minutes, restarts }`, or `null` where nothing is watching — which is both a machine that
  enforces the ceiling itself and a service that declared none. A client showing only the restarting
  case would say nothing about the services most worth saying something about, since a database is
  deliberately warned about and left alone.
- **A secret is read at the moment it is handed over, and never rendered on the way.** A client that
  wants to open a database elsewhere asks for the handoff and receives something it can act on; it
  does not receive a password to paste into a command line, because the daemon fetching a credential
  from the keyring at that instant is the only version of this that keeps it out of a shell history,
  an argument list and a log. The same rule is why "reveal password" is a separate deliberate call
  and not a field on a service read.
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
