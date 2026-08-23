# Local domains and DNS

**Goal**: a project becomes `blog.test` (and `*.blog.test`) with no manual hosts editing, and
wildcard subdomains work.

## TLD policy

- **Default managed TLD: `.test`** — reserved by RFC 6761 for exactly this, never resolvable
  publicly, no conflicts.
- **`.internal` is managed too**, and is the one to reach for when `.test` reads wrong. ICANN
  reserved it in July 2024 as the private-use TLD — RFC 1918 for names — so it will never be
  delegated, and `blog.internal` states an intention where `blog.test` states an experiment. It
  arrived with T45 rather than later on purpose: `mixengine-elevate` is excluded from auto-update, so
  a TLD added after a release is refused by every installed helper until the user reinstalls it.
- **`.local` is supported but warned about**: it is mDNS/Bonjour territory (RFC 6762). Using it
  breaks or is broken by Bonjour/Avahi on the same machine. A client shows a warning with a "use
  .test instead" action; the CLI requires `--i-know` for `.local`.
- **And `.local` is never wired to the DNS server** (T45). A site may be declared on it and gets one
  exact hosts entry like any other name. Pointing a *resolver* at it is a different act: the server
  answers `A 127.0.0.1` for **every** name under a managed TLD at any depth, so an
  `/etc/resolver/local` would send `printer.local` and every other Bonjour name on the user's
  network to loopback, machine-wide. Two tables express this: `MANAGED_TLDS` is what a site may be
  named on, `WIRED_TLDS` is what a resolver may be pointed at, and `.local` is in the first only.
- **`.localhost`** is offered as a zero-config alternative: many resolvers already map `*.localhost`
  to loopback, so it needs no hosts or DNS change at all.
- `.dev`, `.app` and other real TLDs are **refused** — they are public and HSTS-preloaded.

## Two mechanisms — DNS is primary, hosts is the fallback

This ordering is a consequence of
[ADR 0005](../decisions/0005-on-demand-elevation.md): editing the hosts file needs an elevation
prompt *every time a site is created*, while the DNS server answers wildcards by pattern after a
**single** one-time setup. Creating a site must prompt for nothing, so DNS leads.

### 1. Built-in DNS server (primary)

`hickory-server` inside the daemon:

- Listens on **`127.0.0.1:53535` on macOS/Linux** (unprivileged — no root needed) and on
  **`127.0.0.1:53` on Windows** (which has no privileged-port concept, so no elevation either).
  `[dns] port` in `config.toml` moves it. **Deliberately not 5353**, which an earlier draft named:
  that port belongs to mDNS, and `mDNSResponder` and `avahi-daemon` hold it on every ordinary macOS
  and Linux desktop — choosing it would have meant the hosts-only fallback was the only branch that
  ever ran (T44 design, D2).
- Answers `A` for `*.<managed-tld>` → `127.0.0.1`, at any depth, whether or not a site has been
  declared for the name. **`AAAA` is answered `NOERROR` with no records**, not `::1`: after T43 the
  front end binds IPv4 only, and a name that resolves to an address nothing is listening on is a
  browser preferring IPv6 and waiting before it falls back. This is the same correction T41 made to
  the hosts block below, for the same reason (T44 design, D3).
- Everything outside a managed TLD is **`REFUSED`**. An earlier draft of this page said such queries
  were forwarded to the system's upstream resolvers with a small cache; that was withdrawn by T44
  (design, D1) and there is no forwarder, no cache and no recursion in the daemon. Every wiring
  mechanism below is scoped to a TLD, so a query outside one never arrives — and the one way it
  could, a Linux wiring that replaced a link's DNS servers with ours, is exactly the case where
  forwarding loops back through `systemd-resolved` and hangs instead of helping. `REFUSED` sends a
  stub resolver to its next nameserver at once, which makes a mis-wiring loud rather than slow.
- The sockets bind loopback and nothing else, which is the whole of the access control: a query from
  off the machine cannot arrive, and with no recursion there would be nothing to abuse if one did.

OS wiring, via `ResolverConfig` in the platform layer — **one elevated operation, once**:

| OS | Mechanism | Custom port? |
| --- | --- | --- |
| macOS | one marked file per TLD: `/etc/resolver/<tld>` with `nameserver 127.0.0.1` + `port 53535` | yes (`man 5 resolver`) |
| Linux | a `systemd-networkd` **dummy link of MixEngine's own** — `mixengine0`, carrying `169.254.53.53/32`, declared in `/etc/systemd/network/10-mixengine.{netdev,network}` with `DNS=127.0.0.1:53535` and `Domains=~test ~localhost ~internal` | yes |
| Windows | one NRPT rule for every namespace, written as registry values under a fixed GUID in `HKLM\SYSTEM\CurrentControlSet\services\Dnscache\Parameters\DnsPolicyConfig` | **no** — hence port 53 on Windows |

The one platform whose resolver mechanism cannot express a port is the one that lets an unprivileged
process bind 53. It works out exactly.

**Never** change the machine's global DNS server. If the only available mechanism would be global,
report `unsupported_platform` and fall back to hosts-only mode with wildcards disabled.

**The Linux row above replaces two mechanisms this page used to name, and T45 measured both
unusable.** `resolvectl dns <link> …` *replaces* that link's servers rather than adding to them, and
the obvious way out — using the loopback link, which the machine does not resolve through — is
refused by systemd-resolved by name: *"Link lo is loopback device"*. The other, a
`/etc/systemd/resolved.conf.d/` drop-in with `DNS=127.0.0.1:53535` and `Domains=~test`, **redirects
the entire machine**: after it, `getent hosts github.com` answered `127.0.0.1`. A global routing
domain does not scope the global DNS servers. NetworkManager, whose dnsmasq drop-in was named as the
fallback, is not installed on a stock Ubuntu server at all.

So Linux gets a link of its own, and it must carry an address: a dummy link with none is accepted,
reports its servers back through `resolvectl status`, and never gets a DNS scope — so nothing is
ever sent to it, while everything about it reads as applied. A link-local `/32` is enough, so no
RFC 1918 range is claimed on the user's machine.

**And on Linux the mechanism is a question about the machine rather than about the platform.** A
Linux running neither `systemd-resolved` nor `systemd-networkd` has no scoped mechanism at all; that
home stays on the hosts file and `daemon.status` says why.

Port 53 on Windows can still be occupied (Docker Desktop, Internet Sharing, a local AD DNS). The
daemon binds first and asks who holds the port only after the bind fails — a probe beforehand is a
race — and reports the holder by name where the OS will give one. `PortOwner` reads TCP only, so a
UDP-only holder is reported as "another program on this machine"; T47 owns whether that is worth
three more per-OS implementations.

### Three things a diagnostic must know

Measured with T45, and each one turns a working machine into a false alarm if a tool gets it wrong:

- **`nslookup` does not honour the NRPT on Windows.** It talks to the configured server directly, so
  it answers NXDOMAIN for a name `getaddrinfo` resolves at the same moment. `domain.dns_status` and
  `mix doctor` must use the resolver the operating system gives programs — `getaddrinfo` — and never
  `nslookup`. T46 built it that way, and **includes the operating system's cache deliberately**: T45
  had to defeat the cache because it was asking whether a mechanism works, while this asks what the
  user's browser sees right now, and the cached answer is that answer.
- **Windows asks for `A` and nothing else.** macOS and Linux both ask `A` and `AAAA`. Nothing depends
  on this; a reader comparing three logs will otherwise think one is broken.
- **macOS also asks for `_dns.resolver.arpa` type 64 (SVCB)** — Discovery of Designated Resolvers.
  That name is outside every managed TLD, so the server answers `REFUSED`, no encrypted transport is
  discovered, and nothing else happens. It is correct, and it is in the log.

### Which mechanism is running, and how a client knows

`daemon.status` carries a `dns` object: the mode (`dns` or `hosts_only`), where the server is
listening if it is, whether **wildcards** work, and a sentence saying why when they do not. The two
are separate questions and both are reported: a server can be listening perfectly while nothing on
the machine routes a name to it, which is every machine until the resolver wiring of T45 lands.

`wildcards` is stated rather than left to be derived from the mode, because it is the specific thing
a hosts-only home loses: `blog.test` works and `api.blog.test` does not.

**And it is a list of TLDs rather than a flag**, from T45 on. Every mechanism above scopes to one
TLD and `.local` is never wired, so a home can perfectly well answer `*.blog.test` by pattern while
still needing a hosts line for `shop.local`. A boolean would have to say `true` and leave a client to
work out which half of its sites it applied to.

**A server on an operating-system-chosen port is answering and is still not something anything may
be wired to.** `[dns] port = 0` is what every test home in this repository uses so that no suite
takes port 53 off the machine running it; a resolver pointed at a number this process will not have
again is one elevation prompt spent to break name resolution until the next restart. The daemon asks
for nothing on such a home.

### 2. Hosts file (fallback, and for exact names)

Used when the user declines the resolver setup, when the platform cannot scope DNS to a TLD, or for a
one-off exact hostname. Entries go into the managed block:

```
# BEGIN MixEngine
127.0.0.1  api.blog.test
127.0.0.1  blog.test
# END MixEngine
```

**One line per name, `127.0.0.1` only, sorted by name.** An earlier draft of this page drew a
matching `::1` line and that was wrong for the build it describes: nothing decides that the web
server binds `::1` until T43, and a name that resolves to an address nothing is listening on is a
browser timing out before it retries. `mixengine-elevate` permits `::1` so T43 can start emitting it
without touching the audited binary.

Reliable everywhere, works with every resolver, survives our DNS server being down — but **each
change costs an elevation prompt**, which is why it is no longer the default path. Hosts edits are
batched: several pending entries are applied in one elevated invocation.

## Domain lifecycle

- `site.create` allocates `<slug>.test`. With DNS wired up this writes **nothing** and prompts for
  **nothing** — wildcards are pattern-matched, not per-domain records. In hosts-only mode it queues a
  hosts entry and the user is prompted (once, for the whole batch).
- **The wiring is asked for at daemon start, before any site exists** (T45). That ordering is what
  makes the promise above true: on a fresh home the operation is in the queue in time for first-run
  setup's single grant, and `site.create` afterwards computes a hosts block that already matches the
  disk and enqueues nothing. Asking after the first site was created would mean emptying a block that
  already had a line in it — a second operation, and therefore a second prompt.
- **The hosts block holds the names no resolver routes**, per TLD rather than per home. A home with
  both `blog.test` and `shop.local` needs a block with exactly one line in it.
- Extra domains via `domain.add { site, domain }` and `domain.remove { domain }` (T46). `site` is the
  `SiteRef` every `site.*` method takes; **`remove` carries no site**, because
  `site_domains_domain` is `UNIQUE` and a name therefore belongs to exactly one site. A new name is
  always an alias — **the primary is never changed by these two**, and removing the primary or a
  site's last domain is refused by name. Reordering, and so choosing a primary, is `site.update`'s:
  the head of the list it is given becomes the primary.
- Aliases will share the site's certificate SANs ([tls.md](tls.md)), which triggers a reissue.
  **Nothing reissues anything yet**: certificates are phase 5, so as of T46 adding a domain writes a
  row, re-renders the front end and queues a hosts entry when the TLD is unwired, and that is all.
- Removing a site queues removal of its hosts entries; orphans are reconciled by `mix doctor`, which
  is also where deferred hosts changes get flushed if the user declined earlier.
- `domain.dns_status` reports, per domain, **four facts and no verdict** (T46): which site declares
  it, if any; whether the managed hosts block holds a line for it, read off disk; whether its TLD is
  wired, so names under it are answered by pattern; what this daemon's own server answers, asked over
  its socket; and what the operating system actually resolves it to. Plus one sentence naming the
  first thing that is wrong.
- **Four rather than one, because they fail independently.** A hosts line with no server, a server
  with no resolver, and a resolver wired to a TLD this name is not on are three faults with three
  fixes; one boolean would leave every client working out which it had — the derivation
  `DnsStatus::wildcards` had to stop making in T45.
- **A name nothing declares is answered rather than refused.** Somebody asking why `foo.test` does
  not work when they never declared it is owed exactly that, and the other three facts still hold an
  answer.
- **The sentence says what is wrong and never what to do.** Repair is T47's, and `mix doctor` should
  render this report rather than compute its own: a second implementation of the four facts is a
  second answer to one question.

## Acceptance criteria

- After creating a site, `ping blog.test` and `curl http://blog.test` work in a *new* shell without
  a reboot on all three OSes.
- **Creating a site after first-run setup triggers zero elevation prompts** on all three OSes.
- With DNS enabled, `curl http://anything.blog.test` reaches the site.
- Declining the resolver setup still yields working sites via hosts-only mode, with wildcards clearly
  reported as unavailable.
- Disabling MixEngine's DNS leaves the machine's normal name resolution untouched (test: resolve a
  public domain before/after).
- Uninstall removes the managed hosts block and the resolver/NRPT rule completely.
