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

## Two mechanisms, used together

### 1. Hosts file (always)

Exact hostnames are written into the managed block:

```
# BEGIN MixEngine
127.0.0.1  blog.test
127.0.0.1  api.blog.test
::1        blog.test
# END MixEngine
```

Reliable everywhere, works with every resolver, survives our DNS server being down. This alone
covers the common case, so **DNS is optional** — a user who declines the elevation prompt still gets
working sites, just no wildcards.

### 2. Built-in DNS server (for wildcards)

`hickory-dns` inside the daemon, listening on `127.0.0.1:53` (UDP+TCP; the port is bound by the
helper on Unix):

- Answers `A`/`AAAA` for `*.<managed-tld>` → `127.0.0.1` / `::1`.
- Everything else is forwarded to the system's upstream resolvers (read from the OS, refreshed on
  network change) with a small cache — so putting our server in front is safe.
- Refuses recursion from non-loopback sources.

OS wiring, via `ResolverConfig` in the platform layer:

| OS | Mechanism | Scope |
| --- | --- | --- |
| macOS | `/etc/resolver/test` containing `nameserver 127.0.0.1` | only that TLD, no global change |
| Linux | `systemd-resolved`: `resolvectl dns <link>` + `resolvectl domain <link> ~test`; fallback: NetworkManager dnsmasq drop-in `server=/test/127.0.0.1` | only that TLD |
| Windows | NRPT rule: `Add-DnsClientNrptRule -Namespace ".test" -NameServers "127.0.0.1"` | only that TLD |

**Never** change the machine's global DNS server. If the only available mechanism would be global,
report `unsupported_platform` and fall back to hosts-only mode with wildcards disabled.

Port 53 is frequently occupied (Docker Desktop, systemd-resolved stub, Internet Sharing). Detect
this at startup, report which process holds it, and offer: hosts-only mode, or a custom port (usable
on macOS/Linux resolver files, not with NRPT).

## Domain lifecycle

- `site.create` allocates `<slug>.test` and writes both the hosts entry and (if enabled) nothing
  extra for DNS — wildcards are pattern-matched, not per-domain records.
- Extra domains via `domain.add { site_id, domain }`; aliases share the site's certificate SANs
  ([tls.md](tls.md)) which triggers a cert reissue.
- Removing a site removes its hosts entries in the same transaction; orphan entries are cleaned by
  `mix doctor`.
- `domain.dns_status` reports, per domain: hosts entry present?, DNS server answering?, resolver
  wired?, what the OS actually resolves it to (a real lookup, not our own cache) — this is the
  diagnostic users need when "it doesn't work".

## Acceptance criteria

- After creating a site, `ping blog.test` and `curl http://blog.test` work in a *new* shell without
  a reboot on all three OSes.
- With DNS enabled, `curl http://anything.blog.test` reaches the site.
- Disabling MixEngine's DNS leaves the machine's normal name resolution untouched (test: resolve a
  public domain before/after).
- Uninstall removes the managed hosts block and the resolver/NRPT rule completely.
