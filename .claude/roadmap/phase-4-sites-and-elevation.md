# Phase 4 — Sites, domains and on-demand elevation

*Goal: `http://blog.test` works, and creating a site prompts for nothing.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

Design: [ADR 0005](../decisions/0005-on-demand-elevation.md). Nothing here installs a persistent
root process.

---

- [x] **T39** Project model: `project.create|list|show|update|delete|export`, `mixengine.toml`
      read and write, and the `runtime.uninstall` refusal a project pin earns.
      Design: [T39 spec](../../docs/superpowers/specs/2026-08-22-t39-project-model-design.md).
      **`create` is also the import**: with no `--name` and no `--pin`, both come from the manifest
      lying at the root, so a second method would have been a second code path for one outcome.
- [x] **T39a** Site model: `sites`, `site_domains`, `site_service_links`, the four site kinds
      (`php-fpm`, `static`, `reverse-proxy`, `node-app`), doc roots, and the `[site]` and
      `[[services]]` halves of `mixengine.toml`.
      Design: [T39a spec](../../docs/superpowers/specs/2026-08-22-t39a-site-model-design.md).
      T39 left those sections opaque: `core::manifest` reads the file whole and its writer preserves
      them byte for byte, so this task gives them types rather than teaching a second reader about
      them. T43 renders what this declares.
      Three things the task grew that this line did not say. **No new table** — the three have stood
      since the initial migration; `0006_site_state.sql` only closes `sites.state` as
      `enabled`/`disabled`, which is the CHECK `0001_initial.sql` deferred to "a later phase". **The
      fourth refusal is T39a's**, because T39a creates the debt: `service.delete` refuses a service a
      site declares, with a `--force` that crosses the declaration and never the running process
      (T39/D8's line). And **`core::domains`** arrives here rather than with T46, because a site
      cannot be created without deciding what a domain may be.
      **A known gap, recorded rather than left to be discovered:** nothing in this roadmap supervises
      a node process. `node-app` is a declaration; if T43 renders it identically to `reverse-proxy`,
      that is the honest outcome and belongs written down there.
- [x] **T40** **`mixengine-elevate`**: one-shot binary, typed request/response over files, self
      validation, atomic writes under lock, root-owned audit log, distinct "user declined" exit code. **(P)**
      Design: [T40 spec](../../docs/superpowers/specs/2026-08-22-t40-elevate-design.md).
      **The frame plus exactly one operation, `Probe`**, which applies nothing: an empty frame cannot
      be run, and the first time the request/response lifecycle ran for real would otherwise be inside
      a task simultaneously learning what a hosts-file marker block is. `Probe` is also the version
      negotiation the auto-update exclusion makes necessary, and it hands T41a a real binary to put in
      front of Smart App Control.
      Four things this line did not say. **The exit code is the fallback and the response file is the
      protocol** — exit 0 means "there is a report", not "it worked", which inverts what the stub's
      own comment asked for and answers the same danger better. **Elevation is per operation, not a
      gate on the process**, because the operation that reports whether the token is elevated must be
      able to report `false`. **The audit log lives outside `MIXENGINE_HOME`**, since a root-owned
      file in a user-owned directory can be unlinked by that user whatever its mode says. And
      **`mixengine-platform` grew features**, so a binary that runs as root does not carry tokio, the
      keyring backend and its vendored libdbus; CI diffs its whole dependency closure against
      `.github/elevate-dependencies.txt`.
      **This task created the `system` CI job**, which `build-and-release.md` said would arrive with
      the first `#[ignore]`d system test.
      **Three things the code said that the design had not.** `serde`'s `deny_unknown_fields` never
      fires on a *unit* variant of an internally tagged enum — it is read through `deserialize_any`,
      which drops every key but the tag — so `Probe` is `Probe {}`, an empty struct variant, and the
      rule holds for the operation carrying no fields as well as the ones that will. Cargo refuses a
      member's `default-features = false` on an inherited dependency whose workspace entry leaves the
      defaults on, so the default is off at the root and the six crates that want the whole platform
      crate say `features = ["default"]`. And `mixengine-platform` had never been built on its own:
      `tokio/rt` and `tokio/time` were reaching it through the daemon's feature unification.
      **A debt it created:** the audit log is the first thing MixEngine leaves outside
      `MIXENGINE_HOME`, and removing it is itself a privileged operation — `mix uninstall` owes it
      one (T47, T92).
      **A question it recorded rather than answered:** whether `mixengined` should refuse to start
      under an elevated token. That is a change to the daemon, not to the helper, and belongs with
      T40b.
- [x] **T40a** `Elevation` trait: `ShellExecuteEx`/`runas`, osascript `with administrator privileges`,
      `pkexec` — **including polkit-agent detection and the manual-command fallback on Linux**. **(P)**
      Design: [T40a spec](../../docs/superpowers/specs/2026-08-22-t40a-elevation-design.md).
      **The capability stops at the prompt.** It raises one, waits, and answers `Completed`,
      `Declined` or `Unavailable`; it never opens `response.json`, which is `serde_json` over
      types with no operating system in them and is T40b's. `Completed` therefore means the
      helper *ran*, not that it left a report — a crash is not a per-OS event and every
      caller handles it anyway.
      **The half of each launcher that is a decision is compiled on all three systems.**
      `src/prompt.rs` holds the tables — which exit code means the person said no, which
      means there was nobody to ask, how a path is quoted — so each system's table is tested
      on every one of them; only the call that can be made nowhere else stays in `sys::prompt`.
      That is what a `#[path]`-mapped OS directory otherwise costs: a test beside
      `linux/prompt.rs` runs on Linux alone.
      Measured, not reasoned about: on a macOS runner already running as root,
      `do shell script — with administrator privileges` **runs straight through without
      authenticating** — the whole round trip took 0.19 s, so the row stayed a round trip
      rather than being reduced to `probe()`. The Linux runner asserts the opposite branch for
      real: no graphical session, so `Unavailable` carrying the `pkexec` command to run by hand,
      and nothing written beside a request no elevated process ever opened.
      Not proved by any CI run: nobody clicks Cancel — 1223, `-128` and 126 are held by unit
      tests and confirmed only by a person at a machine. T41a is the natural place for the
      Windows leg.
- [x] **T40b** Elevation queue in the daemon: batch pending ops into one invocation,
      `ElevationRequired` event, decline → degraded mode with a pending list. Test: no code path
      elevates in a loop.
      Design: [T40b spec](../../docs/superpowers/specs/2026-08-23-t40b-elevation-queue-design.md).
      The queue is a table whose unique key is the operation itself, so "no code path elevates in a
      loop" is a property of the schema rather than of anybody's discipline; the runtime half is one
      grant slot, and a second is `conflict`. Answered the question T40 recorded and left open: an
      elevated daemon is warned about and reported in `daemon.status`, **not** refused — CI's whole
      Windows third runs the daemon suites under a full token (T2b), and a hard refusal would turn
      one platform red for a reason unrelated to the code under test.
      **No producer ships with it.** T41's `HostsApply` is the first, on T22's and T19's precedent:
      the alternative is writing the queue twice, once inside the first producer and once properly.
- [x] **T64** The CLI half of elevation UX: `mix` prints every operation an `ElevationRequired`
      batches and what each will literally change — the exact hosts lines, the port, the store —
      *before* raising the prompt, and after a decline `mix status` keeps showing the pending list
      until it is granted or dropped. Moved here from the withdrawn Phase 6
      ([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)); the CLI is the only client now,
      so this is the whole of the elevation UX rather than half of it.
      **The screen is rendered from `elevation.status`, not from the event.** `mix` subscribes to no
      event stream at all — it follows a job by polling `job.wait` — and T40b's D8 made
      `ElevationRequired` carry the same list `elevation.status` answers with, so what looked like a
      listener to build was a call to make. The ordering the task asks for is T40b's D1 and not this
      client's: the daemon raises nothing on its own initiative, so there *is* a moment between
      knowing the batch and asking for it.
      **The gate is what can be read, not `IsTerminal`.** A rule written around a terminal would be
      one no test could reach: `Command` hands its child a pipe and never a console, so every
      assertion about the question would have had to be made against a mock of the question. End of
      file is the condition that actually matters anyway — it is what a cron job, a CI step and a
      service manager look like — and it is refused rather than assumed in either direction: yes
      raises a dialog nobody is sitting in front of, no is a decline the caller cannot tell from a
      grant that happened. `--yes` is how a script answers in advance, and `--json` requires it.
      **What it deliberately did not do.** An empty queue is still `elevation.grant`'s refusal to
      make and is forwarded rather than anticipated — a client that composed "nothing is waiting"
      would be a second opinion on a precondition the daemon holds. `mix status` still carries the
      count and not the list, which is T40b's D6 and was not revisited: the list is a screen, and
      `mix elevation status` is where a screen goes.
      **Nothing in the suite grants, on T40a's precedent.** A successful grant is a real dialog on
      the machine running `cargo test`, so `crates/mixengine-cli/tests/elevation.rs` proves the
      screen and the three answers that stop before the prompt, and the prompt itself stays held by
      unit tests and by a person at a machine.
      **One piece of scaffolding, with T41's name on it.** There was no producer when this landed
      and no `mix elevation enqueue`, ever, so the suite wrote its own row through
      `mixengine_testkit::privileged`. T41 made a test that creates a site and *then* finds an
      operation waiting possible, which proves what that could not — and took the module with it.
- [x] **T41** `PrivilegedOp::HostsApply` — marker-block editing with atomic write, locking, and the
      "unrelated lines survive" regression test. **(P)**
      **The whole state, not a delta.** A block that has drifted — a user edited it, a crash left it
      half written — cannot be pulled back by "add this line", so the operation carries what the
      block should hold when the helper is finished. That makes it idempotent, makes it its own
      repair, and makes `AlreadyDone` a byte comparison rather than a judgement.
      **Its dedupe key is its kind, and a newer state supersedes an older one.** T40b keyed on the
      serialisation, which is right for `Probe` and wrong here: two sites created before anybody
      clicks Allow would be two valid rows disagreeing on the one screen whose job is to say what is
      about to happen. The insert became a guarded upsert; `requested_at` is deliberately not
      refreshed, and the `WHERE` clause is what keeps "nothing changed" meaning "announce nothing".
      No migration — `Probe`'s key did not move.
      **Mechanism in `mixengine-platform`, policy in `mixengine-elevate`.** The engine ends in
      `ReplaceFileW` on one system and `chown` on the others, so it lives where OS calls live and is
      tested exhaustively against files a test owns. What may be *written* is forty lines in the
      audited binary, which calls the shared syntax predicate itself: the managed TLD table moved to
      `mixengine-proto` so the helper can read it without being handed one in a request.
      **`HostsFile` is read-only, and the architecture table was wrong.** Add and remove need a
      token, and a capability the daemon can call is by definition one it holds no token for. The
      trait is `path()` and `managed()`; the write is the privileged operation.
      **The producer reads the disk before it spends a prompt.** `Elevation::require_hosts` compares
      the machine's block against what the database says and enqueues only on a difference. A read
      that fails is logged and the operation is queued anyway — the helper is the authority on that
      file, and "your hosts file has two BEGIN markers" belongs on T64's screen rather than in a site
      creation's error. Disabled sites keep their lines: a name that resolves and is refused by the
      web server is diagnosable, and excluding them would put a password dialog on `site.disable`.
      **Known limitation: a multi-account machine.** Two homes share one `# BEGIN MixEngine` block.
      The new machine-wide `hosts.lock` stops them interleaving a write; it does not stop the second
      one's desired state replacing the first one's. Per-account markers would fix it and would make
      the block unreadable and `mix doctor` a great deal harder, so it is recorded here rather than
      solved.
      **The scaffolding T64 named is gone.** `mixengine_testkit::privileged` is deleted, and both
      elevation suites now create a site and find an operation waiting — the queue is filled by the
      product. Nothing in any suite grants: a successful grant is a real dialog on the machine
      running `cargo test`, and under this task it would also edit that machine's hosts file.
- [ ] **T41a** Does an unsigned build run at all, and does this edit survive a machine that has never
      heard of us? **(P)**
      Two questions, and **the first one is already half answered — badly.** Smart App Control refuses
      to *load* an unsigned binary that has no reputation: no warning, no "Run anyway", no path
      exclusion, and Defender's own exclusion list does not apply to it. That is measured rather than
      feared — two of this workspace's own test binaries were refused on a developer machine on
      2026-08-13, inside a directory Defender had been told to ignore, with the Code Integrity events
      recorded in [../features/updates.md](../features/updates.md). Every binary in this product is
      unsigned by design ([ADR 0005](../decisions/0005-on-demand-elevation.md)), so under an enforcing
      SAC there is nothing to elevate, nothing to supervise and nothing to prompt with.
      **Measure the remedy before the population.** "How many users have SAC enforced" is a number
      nobody can act on — 30% and 60% lead to the same next move — and it was the first thing this
      task asked for, wrongly. The question with an action attached is whether **a certificate this
      project can actually buy** makes SAC accept the binary: if it does, the whole thing is a line
      item in T86 and a few hundred dollars a year, ADR 0005 survives with one clause struck, and the
      population stops mattering. SAC admits a file on its signature *or* on ISG reputation, and a
      freshly issued OV certificate has no reputation; whether an EV one is honoured immediately the
      way SmartScreen honours it is precisely the thing to settle by buying the cheapest usable
      certificate and trying it on the VM, not by reading about it.
      Only if that answer is **no** is the population worth counting — SAC enabled on a clean Windows
      11 install, off after an in-place upgrade, switching itself out of evaluation when it sees a
      developer at work — and then it decides between accepting the loss and changing how this is
      distributed.
      The second question is the one this task was originally written for. Defender ships a
      `HostsFileHijack` heuristic aimed at writes to `%SystemRoot%\System32\drivers\etc\hosts`, and an
      unsigned binary doing it is far likelier to trip it. So: an unsigned build, a clean Windows VM
      with full protection on, elevation through the real prompt, the marker block written — and a
      record of what actually happened, including SmartScreen on the first run of the elevated binary
      and the Gatekeeper equivalent on macOS.
      **The SAC half does not need T41 and should not wait for it.** It needs a binary and a clean
      machine, both of which exist today; only the hosts half needs the code this phase builds. Run
      it as soon as there is a VM to run it on.
      **Here rather than with T86a because of what a bad answer costs.** T86a asks the same question
      of the *installer and the updater*, where a bad answer changes a release process. This one can
      invalidate ADR 0005 itself — and T42, T43, T44, T45 and the whole of Phase 5 are built on top
      of it, so learning at phase 9 that the elevated write is quarantined means five phases resting
      on a design that never reaches a user's machine. It is a day's work against a VM, which is the
      entire argument for doing it now: cheap to run, and cheap to be wrong about only while it is
      early.
      Findings go in [../features/updates.md](../features/updates.md) beside T86a's, not into this
      file.
      **Carried to the first release, decided 2026-08-23.** The argument above for running it now is
      not answered — it is overruled by the fact that neither half can be run at all today. Both need
      a clean Windows 11 VM with SAC enforced, and the half that decides everything else needs a code
      signing certificate to be bought first. So this becomes a debt against **v0.1.0** rather than
      against phase 4: **nothing ships to a user until it is answered**, and T42 through T47 and the
      whole of Phase 5 are knowingly built on an assumption that a VM could have checked. What the
      deferral accepts is the cost the paragraph above names: if the answer is no, it invalidates
      [ADR 0005](../decisions/0005-on-demand-elevation.md) and five phases rest on a design that never
      reaches a user's machine. Answer it the moment a VM exists — the release gate is the deadline,
      not the schedule.
- [x] **T42** `PortAccess`: no-op on Windows, `cap_net_bind_service` on Linux, a pf anchor redirect
      plus a boot-time job on macOS ([ADR 0012](../decisions/0012-a-boot-time-job-enables-the-packet-filter-on-macos.md)).
      The re-probe is the producer: every daemon start asks, which covers "after every app update"
      and the losses that were not updates — **and closes T88b**. `nftables` was not needed and is
      not there: `setcap` was measured to work and to be readable back without privilege. Design in
      [../../docs/superpowers/specs/2026-08-23-t42-port-access-design.md](../../docs/superpowers/specs/2026-08-23-t42-port-access-design.md).
- [x] **T43** Site → config → reload end-to-end; `site.start|stop`, idempotent re-runs. A site file
      belongs to the front end's **own** document set, appended by `Recipe::sites` and selected by
      `Role::FrontEnd`, so T30's staging, the server's own checker and T31's reload arc are the ones
      that already exist. `sites/` is a directory the recipe declares **swept** — it holds exactly
      what was rendered into it, and a removal counts as a change, or a deleted site goes on being
      served. A pool's address is one expression (`Recipe::upstream`), and a php-fpm site whose pool
      is gone is left out rather than failing the render: `service.delete --force` crosses a site's
      declaration, and a render that failed over it would leave a daemon that cannot render anything.
      The bind mapping reaches `mixengine-core` as data, from a pure `PortAccess::bindings`.
      `Degraded` is **deferred** and `features/services.md`'s promise corrected — one set, one
      judgement. Design in
      [../../docs/superpowers/specs/2026-08-23-t43-site-to-config-design.md](../../docs/superpowers/specs/2026-08-23-t43-site-to-config-design.md).
- [x] **T44** Built-in DNS server (`hickory-server`): **53535** on macOS/Linux and **53** on Windows,
      over `[dns] port`, wildcard `A` for every managed TLD at any depth, loopback-only sockets,
      port-in-use detection naming the holder. **Closes T46a with it**, because hosts-only is not a
      later feature but the state every machine is in until T45 wires a resolver.
      **Not 5353**: that port is mDNS's and is held by `mDNSResponder` and `avahi-daemon` on every
      ordinary desktop, so it would have made the fallback the only branch that ever runs. The
      number in [ADR 0005](../decisions/0005-on-demand-elevation.md)'s illustration is corrected
      with it; its argument is untouched.
      **No upstream forwarding, and the roadmap line that asked for it is withdrawn.** Every wiring
      mechanism T45 can use is TLD-scoped, so a query outside a managed TLD never arrives — and the
      one wiring that would deliver one, a Linux link whose DNS servers were replaced with ours, is
      exactly where forwarding loops back through `systemd-resolved` and hangs. `REFUSED` sends a
      stub resolver to its next nameserver at once, which is what makes a mis-wiring diagnosable by
      T46 and T47 instead of a machine that has become slow. `AAAA` is NODATA with an `SOA` rather
      than `::1`, which is T41's correction applied a second time: the front end binds IPv4 only.
      **The mode has two terms and this task can only produce one**, so it reports `hosts_only`
      everywhere and `site.create` keeps queueing hosts entries exactly as it did — T45 is what
      switches both halves on together. Design in
      [../../docs/superpowers/specs/2026-08-23-t44-dns-server-design.md](../../docs/superpowers/specs/2026-08-23-t44-dns-server-design.md).
- [x] **T45** Resolver wiring per OS: a marked `/etc/resolver/<tld>` file on macOS, a
      `systemd-networkd` dummy link of our own on Linux, one NRPT rule written as registry values on
      Windows — TLD-scoped only, never global. **The producer of the mode**, so T44's server is now
      the thing a home actually resolves through, and both halves switched on together as that task
      said they would. **(P)**
      **Every Linux mechanism this roadmap and the feature spec named turned out to be unusable**,
      and only measuring found it: a `resolved.conf.d` drop-in with a global routing domain
      redirects the **whole machine** (`getent hosts github.com` answered `127.0.0.1`),
      `resolvectl dns lo` is refused by systemd-resolved by name, a real link has its servers
      *replaced*, and NetworkManager — whose dnsmasq drop-in was the named fallback — is not
      installed on a stock Ubuntu server at all. What works is a dummy link **carrying an address**:
      without one it is configured, reports its servers back, and never gets a DNS scope, which is
      the worst failure of the four because it reads as applied.
      **The helper is never told where to point.** `127.0.0.1`, the link's name and address, and the
      Windows registry GUID are compiled into `mixengine-elevate`; the operation carries which TLDs
      and which port and nothing else. That narrows the shape
      [platform-abstraction.md](../architecture/platform-abstraction.md) had sanctioned — it read
      `ResolverInstall { tld, addr }` — and needs no ADR, because it grants strictly less.
      **`.internal` joined `MANAGED_TLDS` with this task**, and `.local` is managed and never wired:
      the server answers every name under a wired TLD, so an `/etc/resolver/local` would send
      `printer.local` to loopback machine-wide.
      **What it deliberately did not do**, and who owns each: a real-lookup diagnostic is **T46**'s,
      and it inherits two measured facts — `nslookup` bypasses the NRPT on Windows and would report a
      working machine as broken, and macOS asks the server for `_dns.resolver.arpa`. Reconciling a
      wiring that drifted is **T47**'s. `ResolverRevoke` ships built, validated and tested with **no
      producer**, on T42's precedent; uninstall (**T87**) is it. Whether a user may ever nominate
      their own TLD is parked and needs an ADR of its own: the helper's table is compiled in
      precisely so that a request cannot extend it.
      **Two debts recorded.** `169.254.53.53/32` is fixed and nothing negotiates it, so a machine
      that already uses that link-local address collides; the whole-state shape makes the fix
      additive if it ever bites. And two homes on one machine still share every artifact — the same
      debt T41 recorded for the hosts block and T42 for the macOS anchor — which on Windows cannot
      even be told apart by port, NRPT having no field for one.
      Design in
      [../../docs/superpowers/specs/2026-08-23-t45-resolver-wiring-design.md](../../docs/superpowers/specs/2026-08-23-t45-resolver-wiring-design.md).
- [x] **T46** `domain.*` RPC + `domain.dns_status` real-lookup diagnostics.
      Three methods: `domain.add { site, domain }`, `domain.remove { domain }` and
      `domain.dns_status { domain? }`.
      **The two verbs add no capability and exist anyway.** `site.update` already replaces a site's
      whole domain list, so a client *could* compose them — by reading the list, appending to it and
      sending it back, which is business logic in a client and a read-modify-write that drops
      whatever another client added in between. They are thin over the same `sites::update`, so the
      TLD check, the hosts queueing and the front-end re-render keep exactly one implementation.
      **`remove` carries no site**, because `site_domains_domain` is `UNIQUE` — the index its own
      migration calls "the one that decides ownership".
      **Two refusals, and the second is the interesting one.** A site's last domain is refused
      because "at least one" is the invariant `0001_initial.sql` records as this layer's to uphold.
      A site's *primary* is refused because promoting another name in its place would change what
      the site is — its canonical URL, and from phase 5 the name on its certificate — under a verb
      that says "remove a domain". `site.update` reorders, and the head of the list it is given is
      the primary; that is where changing a primary belongs.
      **The diagnostic reports four facts and no verdict**, because they fail independently: a hosts
      line with no server, a server with no resolver, and a resolver wired to a TLD this name is not
      on are three faults with three fixes. Collapsing them is what `DnsStatus::wildcards` had to
      stop doing in T45. The sentence it adds names the first thing that is wrong and never what to
      do about it — repair is **T47**'s, and a diagnostic that suggests a fix it cannot perform will
      drift from the thing that performs it. **T47 should render this report rather than recompute
      it.**
      **A name nothing declares is answered rather than refused**: somebody asking why `foo.test`
      does not work when they never declared it is owed exactly that, and the other three facts still
      hold an answer.
      **The lookup is `getaddrinfo` and never `nslookup`**, on T45's measurement that `nslookup`
      bypasses the NRPT and would report a correctly wired Windows machine as broken — and it
      **includes the operating system's cache on purpose**, which is the opposite of what T45's test
      needed: that one asked whether a mechanism works, this asks what a browser sees now.
      **One recorded cost.** `spawn_blocking` cannot be cancelled, so the bound on a lookup stops the
      daemon *waiting* and does not stop the lookup: a hung resolver holds one blocked thread per
      domain asked about until it gives up. Written into the function rather than hidden, because a
      timeout that reads like a cancellation is how a thread leak becomes invisible.
      Design in
      [../../docs/superpowers/specs/2026-08-24-t46-domain-rpc-design.md](../../docs/superpowers/specs/2026-08-24-t46-domain-rpc-design.md).
- [x] **T46a** Hosts-only fallback mode — **closed by T44**: wildcards disabled and reported as a
      field of their own, the mode and the reason for it on `daemon.status` and on the first line of
      `mix status`, batched hosts prompts unchanged from T41.
- [x] **T47a** `mix doctor`: nine checks, reported. **Writes nothing** — no row, no file, nothing
      enqueued, and no elevation prompt can result from a call, which is what makes it safe on a
      timer and inside T93's bundle.
      **T47 was one line covering four subsystems, and this is the read half of it.** The split is by
      what the code does to the machine rather than by subsystem, because that is the boundary a
      reviewer can accept on one side of and reject on the other.
      The checks: the managed hosts block (T41's own comparison), the resolver (T45's probe), the DNS
      server (T44), answering on 80 and 443 (T42), operations waiting for permission (T40b/T64),
      every declared domain (**T46's report, rendered rather than recomputed**, which T46 asked for
      by name), the home's permissions (T3a), what this system promises about a service's
      descendants, and the port ranges this system has reserved.
      **`Note` is a separate outcome from `Problem`, and [ADR
      0007](../decisions/0007-supervised-child-owns-a-process-group.md) is why.** What MixEngine can
      promise about a killed daemon's descendants is total on Windows, the immediate child only on
      Linux, and nothing on macOS. Reporting the macOS answer as a *fault* would report the operating
      system as broken and leave a user with nothing to do; reporting it as nothing at all is the
      exact failure that ADR exists to prevent. The same distinction saves `hosts_only`, which T46a
      closed as a **supported mode**: calling it a problem would put a permanent fault on every
      machine that never wired a resolver.
      **`Skipped` is an outcome and not silence.** Every check appears in every report in a fixed
      order, so a shorter list on one system cannot read as a clean bill of health.
      **A `Problem` carries an id and never advice** — `hosts_block_differs`, `resolver_not_wired`,
      and six more, closed on the wire. T46 argued that a diagnostic must not suggest a fix it cannot
      perform, because the advice drifts from the thing that performs it; an id is a *name for a
      condition*, and being closed is what stops T47b's repairs and this build's findings drifting
      apart at all.
      **Windows' reserved port ranges are the one check that saves a person from a wrong search**
      rather than telling them something they could have found: a bind into a reserved range fails
      with an access error, so it reads as a permission problem, and elevation, UAC and the firewall
      are all the wrong place to look. `netsh int ipv4 show excludedportrange`, parsed in
      `mixengine-platform`; macOS and Linux answer `Skipped` with a reason. Only an overlap with a
      port this home needs is a `Problem`.
      **The `icacls` question T3a left open is settled: keep it.** T3a deferred the decision because
      the *apply* path was verified working and the check had no caller; this task is the caller, and
      the whole of what it needs is "is inheritance still severed, yes or no" — which `icacls`
      answers. The ~150 lines of `unsafe` FFI (`GetNamedSecurityInfoW` +
      `GetSecurityDescriptorControl` for `SE_DACL_PROTECTED`, `GetAce` + `EqualSid` for the three
      trustees) buy a trustee comparison nothing asks for. **What would reopen it** is a caller that
      needs to know *who* has access rather than whether inheritance was severed — a shared-machine
      audit, or a repair that must restore a specific ACE.
      **One thing CI measured that nobody had asked:** the GitHub Windows runner has **port 80 inside
      a reserved range**, so `mix doctor` reports a problem on a home with nothing in it there — and
      it is right, because a front end on that machine genuinely could not bind 80. The finding was
      correct and the test's premise was not: a suite may assert that *its* condition is absent and
      then present, never that the machine running it is well.
      Design in
      [../../docs/superpowers/specs/2026-08-24-t47a-doctor-design.md](../../docs/superpowers/specs/2026-08-24-t47a-doctor-design.md).
- [ ] **T47b** `daemon.doctor_repair`: act on what T47a found, keyed off `ProblemId`, and **flush the
      deferred privileged operations** — which is the repair for `permission_pending` and is the one
      thing here that raises a prompt.
      **Stale generated configuration is this task's**, and was moved here from T47 deliberately:
      deciding whether a file under `etc/` still matches the state means rendering the whole of it
      again and comparing, which is precisely what the repair path does before it installs anything.
      Building that in the read-only half would mean either building it twice or building the repair
      early and calling it a diagnostic. There is a second reason it matters less than it sounds:
      generated configuration is disposable and never parsed back, so a drifted file is corrected by
      the next write that touches it — the fault it represents is "the front end is serving a stale
      rendering *right now*", which is a thing a repair fixes and a report cannot.
      **Reconciling orphans** is here too, on the same line: T13 and `Registry::recover` already
      adopt what survived and stop what nothing declares at every start, so what is left for a repair
      is the case a running daemon notices later.
- [ ] **T93** `mix doctor --bundle`: one diagnostics archive — daemon log excerpt, `mix doctor`
      output, versions and platform facts, credentials redacted — so that "copy diagnostics"
      costs a client nothing to assemble
      ([../features/client-surface.md](../features/client-surface.md)). Carried over from the
      withdrawn Phase 6's T66, which owned the requirement
      ([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)).

**Milestone M4** — create a site and open `http://blog.test` in a fresh shell on all three OSes with
**zero elevation prompts after first-run setup**.

**The second clause this milestone used to carry is gone**, and deliberately rather than because it
was hard: it read "`mix uninstall --dry-run` shows a complete cleanup", and **no task in this phase
builds `mix uninstall`**. The complete uninstall path is **T87**, in
[phase 9](phase-9-ship.md) — so M4 as written could not be reached at the end of phase 4 however much
of phase 4 was done, and [todo.md](todo.md) had already been stating the milestone without it.

What is true, and is what the clause was reaching for: **this is the phase where MixEngine first
touches the machine outside its own home** — the hosts block (T41), the port grant (T42), the resolver
wiring (T45), and the helper's audit log (T40). Enumerating that is **T47**'s, which reconciles
exactly those; removing it is T87's, which is where a dry run of the removal belongs, next to the
removal it is a run of.

---

Previous: [Phase 3 — Services](phase-3-services.md) · Next: [Phase 5 — HTTPS](phase-5-https.md)
