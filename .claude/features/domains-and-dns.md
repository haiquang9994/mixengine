# Local domains and DNS

**Goal**: a project becomes `blog.test` (and `*.blog.test`) with no manual hosts editing, and
wildcard subdomains work.

## TLD policy

- **Default managed TLD: `.test`** — reserved by RFC 6761 for exactly this, never resolvable
  publicly, no conflicts.
- **`.local` is supported but warned about**: it is mDNS/Bonjour territory (RFC 6762). Using it
  breaks or is broken by Bonjour/Avahi on the same machine. A client shows a warning with a "use
  .test instead" action; the CLI requires `--i-know` for `.local`.
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
| macOS | `/etc/resolver/test` with `nameserver 127.0.0.1` + `port 53535` | yes (`man 5 resolver`) |
| Linux | `resolvectl dns <link> 127.0.0.1:53535` + `resolvectl domain <link> ~test`; fallback NetworkManager dnsmasq drop-in `server=/test/127.0.0.1#53535` | yes |
| Windows | NRPT rule: `Add-DnsClientNrptRule -Namespace ".test" -NameServers "127.0.0.1"` | **no** — hence port 53 on Windows |

The one platform whose resolver mechanism cannot express a port is the one that lets an unprivileged
process bind 53. It works out exactly.

**Never** change the machine's global DNS server. If the only available mechanism would be global,
report `unsupported_platform` and fall back to hosts-only mode with wildcards disabled. Note that
`resolvectl dns <link> …` *replaces* that link's servers rather than adding to them, so the Linux
path has to reach a scoped mechanism rather than a link the machine actually resolves through.

Port 53 on Windows can still be occupied (Docker Desktop, Internet Sharing, a local AD DNS). The
daemon binds first and asks who holds the port only after the bind fails — a probe beforehand is a
race — and reports the holder by name where the OS will give one. `PortOwner` reads TCP only, so a
UDP-only holder is reported as "another program on this machine"; T47 owns whether that is worth
three more per-OS implementations.

### Which mechanism is running, and how a client knows

`daemon.status` carries a `dns` object: the mode (`dns` or `hosts_only`), where the server is
listening if it is, whether **wildcards** work, and a sentence saying why when they do not. The two
are separate questions and both are reported: a server can be listening perfectly while nothing on
the machine routes a name to it, which is every machine until the resolver wiring of T45 lands.

`wildcards` is stated rather than left to be derived from the mode, because it is the specific thing
a hosts-only home loses: `blog.test` works and `api.blog.test` does not.

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
- Extra domains via `domain.add { site_id, domain }`; aliases share the site's certificate SANs
  ([tls.md](tls.md)) which triggers a cert reissue.
- Removing a site queues removal of its hosts entries; orphans are reconciled by `mix doctor`, which
  is also where deferred hosts changes get flushed if the user declined earlier.
- `domain.dns_status` reports, per domain: hosts entry present?, DNS server answering?, resolver
  wired?, what the OS actually resolves it to (a real lookup, not our own cache) — this is the
  diagnostic users need when "it doesn't work".

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
