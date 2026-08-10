# Local domains and DNS

**Goal**: a project becomes `blog.test` (and `*.blog.test`) with no manual hosts editing, and
wildcard subdomains work.

## TLD policy

- **Default managed TLD: `.test`** — reserved by RFC 6761 for exactly this, never resolvable
  publicly, no conflicts.
- **`.local` is supported but warned about**: it is mDNS/Bonjour territory (RFC 6762). Using it
  breaks or is broken by Bonjour/Avahi on the same machine. The GUI shows a warning with a "use
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

`hickory-dns` inside the daemon:

- Listens on **`127.0.0.1:5353` on macOS/Linux** (unprivileged — no root needed) and on
  **`127.0.0.1:53` on Windows** (which has no privileged-port concept, so no elevation either).
- Answers `A`/`AAAA` for `*.<managed-tld>` → `127.0.0.1` / `::1`.
- Everything else is forwarded to the system's upstream resolvers (read from the OS, refreshed on
  network change) with a small cache — so putting our server in front is safe.
- Refuses recursion from non-loopback sources.

OS wiring, via `ResolverConfig` in the platform layer — **one elevated operation, once**:

| OS | Mechanism | Custom port? |
| --- | --- | --- |
| macOS | `/etc/resolver/test` with `nameserver 127.0.0.1` + `port 5353` | yes (`man 5 resolver`) |
| Linux | `resolvectl dns <link> 127.0.0.1:5353` + `resolvectl domain <link> ~test`; fallback NetworkManager dnsmasq drop-in `server=/test/127.0.0.1#5353` | yes |
| Windows | NRPT rule: `Add-DnsClientNrptRule -Namespace ".test" -NameServers "127.0.0.1"` | **no** — hence port 53 on Windows |

The one platform whose resolver mechanism cannot express a port is the one that lets an unprivileged
process bind 53. It works out exactly.

**Never** change the machine's global DNS server. If the only available mechanism would be global,
report `unsupported_platform` and fall back to hosts-only mode with wildcards disabled.

Port 53 on Windows can still be occupied (Docker Desktop, Internet Sharing, a local AD DNS). Detect
this at startup, report which process holds it, and offer hosts-only mode.

### 2. Hosts file (fallback, and for exact names)

Used when the user declines the resolver setup, when the platform cannot scope DNS to a TLD, or for a
one-off exact hostname. Entries go into the managed block:

```
# BEGIN MixEngine
127.0.0.1  blog.test
127.0.0.1  api.blog.test
::1        blog.test
# END MixEngine
```

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
  marked unavailable in the GUI.
- Disabling MixEngine's DNS leaves the machine's normal name resolution untouched (test: resolve a
  public domain before/after).
- Uninstall removes the managed hosts block and the resolver/NRPT rule completely.
