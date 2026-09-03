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
2. **Sites** — list carrying domain, runtime version, HTTPS state and health, and its owner
   (**T81b**: `SiteOwner` is a project by name or an extension by id — an extension's site is shown,
   started and stopped, and every other edit is refused with the uninstall command that removes
   it); create and edit with
   doc root, kind, PHP version, linked services and extra domains; reveal the doc root path and a
   browsable URL so the client can open a browser, a file manager or a terminal itself; per-site LAN
   sharing toggle — **T74**: `site.share` answers the interface, the address and the URL, and
   `site.unshare` takes it back; a machine with more than one network refuses rather than choosing,
   and names the candidates so a client can offer them.

   **And the share that ends without anybody ending it — T76.** `site.share` takes an optional
   length (`for_seconds`), `SiteSharing` carries the deadline back, and
   `DaemonEvent::SiteSharingChanged` announces every change in either direction with a
   `SharingChange` saying why: somebody asked, the length ran out, or this machine left the network
   the site was shared on — that last one carrying both addresses, because the pair is the
   explanation.

   **This is the one place where `mix` is genuinely the weaker client, and it is written down rather
   than left to be discovered.** A share that ends while nobody is looking is exactly the change
   somebody needs to be told about, and a terminal is not where they are looking: with only the CLI,
   the reason is in `daemon.log` and on a stream nothing is reading. A graphical client is where
   `SiteSharingChanged` becomes a notification — the whole reason the variant carries its reason
   rather than only its state. No API is missing; the affordance is.
3. **Runtimes** — installed versions per kind with the default marked; available versions;
   install/uninstall as jobs reporting progress; PHP extension toggles per version.
   **And what an extension's plan names before anybody agrees to it — T82**: a `web-app` freezes two
   things at install, the php-fpm pool it runs on and the database it administers, and
   `ExtensionPlan.site` carries both. Which server an administrative interface opens onto is not a
   detail somebody should discover afterwards, so a client that shows the permissions shows this
   beside them; `mix extension plan` prints it on its own line.
   **And which account it would be signed in as — T82a**: a `web-app` declaring
   `[web-app.database].signs_in` is handed that server's superuser password in a php-fpm pool of its
   own, and `ExtensionPlan.site.signs_in` names the account. It is the most consequential thing an
   extension can be granted, so a client renders it *among* the permissions rather than beside the
   site's domain — and says the same three things `mix` says: which account, that the password comes
   from the OS keyring when the pool starts, and that nothing writes it to disk.
4. **Services** — the settings a service accepts (port, bind, data dir, limits, autostart, idle
   timeout) as data, not as a rendered form; the generated config readable back for display only;
   credentials fetched on demand; validation failures returned per field, not as one string. **And,
   for a database service, where it can be opened — T83**: whether a desktop database client is
   installed to hand the connection to, and the handoff itself, answered per service so a client
   draws the affordance from data instead of probing the filesystem for an application
   ([extensions.md](extensions.md)). `database.client` answers
   `DatabaseClientReport { protocol, secret, client }` — `installed` with the executable,
   `not_installed` with where this system looked and the homepage, or `no_client` — and
   `protocol: null` for a service no client opens, all of them states. `database.open` answers
   `DatabaseHandoff` with `launched: running | handed_on` and `secret`, the keyring address the
   password was read from, never the password: it went into the started process's environment and
   nowhere else. **And making one — T77a**: a client creating a project on a database stack needs
   `database.create`, which answers the database, the account, and the keyring address the credential
   sits at. It never receives the password itself; a client that wants to *show* one is asking for
   T83's handoff.
   **The address is both halves, and `client` carries one too — T84**: `secret` is
   `{ service, key }` rather than the key alone, so a client renders *"stored in your credential
   store as …"* without hardcoding MixEngine's namespace — which is the business logic `CLAUDE.md`
   keeps out of clients. `database.client` composes it from the recipe and the service id, starting
   nothing and reading nothing, so the affordance can be drawn before anything is opened.
   **And whether a `desktop-app` is on the machine — T84**: `ExtensionPlan.client` is
   `installed { program }` or `not_installed { searched }` for that kind and absent for every other,
   beside `ExtensionPlan.homepage`. MixEngine finds such an application rather than installing it,
   so the entry's version is not the machine's answer, and a client that draws an install button
   draws it from these two rather than from the version.
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
7. **Blueprints** — capture the current project, list what is captured, import one somebody else
   wrote, apply one to a new project ([blueprints.md](blueprints.md)). **Two obligations a client
   cannot decline** — T78a: every listing that names a blueprint shows whether anything vouched for
   it, because `blueprint.import` decides that once and nothing ever raises it; and before an apply
   whose plan holds a `RunScaffold` step, the client shows that exact command with the trust state
   and sends a `ScaffoldConsent` naming both. The daemon refuses a consent that disagrees with
   either, so a client that skipped the marking would be a client whose applies fail. The plan
   carries `source` and `trusted` for exactly this, so no second call is needed. Job output — what
   the command prints — is `GET /logs/job/{id}`, on the log stream and never the event stream.
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
  and not a field on a service read. **Built by T83**: `database.open` reads the credential at that
  instant, starts the located client itself with it in that process's environment, and answers
  with the keyring address it came from — a client is told *where*, and is never handed *what*.
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
