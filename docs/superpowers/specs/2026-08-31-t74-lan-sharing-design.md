# T74 — LAN sharing, first half (design)

Feature spec: [`.claude/features/lan-sharing.md`](../../../.claude/features/lan-sharing.md).
Roadmap: [T74](../../../.claude/roadmap/phase-8-differentiators.md), Phase 8.

## Goal

Turn one site — never all of them — into something a phone on the same Wi-Fi can open, and be able
to turn it off again leaving nothing behind. Everything that makes that automatic (watching for a
network change, an expiry, the event stream, the two enforcement tests) is T76; everything that
gives the phone a *name* rather than an address is T75.

## Scope

**In.** Per-site opt-in and its reverse, both driven by hand. Choosing an interface. Rendering the
site's listener on that address as well as on loopback. One firewall operation through
`mixengine-elevate`. Reissuing the site certificate with the LAN IP as a SAN, and reissuing it
without one when sharing stops. The LAN URL, and a QR code printed by `mix`.

**Out.** mDNS and the CA download endpoint (T75). Auto-revoke on network change, `--for`, sharing on
the event stream, the port-scan test and the no-rule-left-behind test (T76). IPv6 — D4.

**The reverse is in T74 and not T76**, which is a deliberate departure from how the roadmap line for
T76 currently reads. A share that cannot be turned off by the person who turned it on is a firewall
rule left open on somebody's laptop, and shipping the opening without the closing is the one order
these two must not land in. T76 keeps every *automatic* revocation; the roadmap line moves when this
spec is approved.

## Decisions

**D1 — The listener is per site; `services.bind_addr` is not touched.** A site that is shared
renders its own listener on the chosen address in addition to loopback; every other site renders
exactly what it renders today. Rebinding the front end as a whole would put every site on the LAN
and is what the spec's "opt-in per site, never global" forbids. Bind changes go through config
regeneration and a reload, not a restart, so open connections survive.

**D2 — Both listeners are written, loopback first.** Caddy's `bind` *replaces* the default, so a
site block that says `bind 192.168.1.10` stops answering on `127.0.0.1` — the site would come up on
the phone and go down on the developer's own browser. The rendering is therefore
`bind 127.0.0.1 192.168.1.10` for Caddy and two `listen` lines for nginx.

**D3 — The address is the interface's own, not `0.0.0.0`.** It is narrower, it is the address that
goes into the URL and into the certificate, and it makes "which interface is this on" a fact the row
already holds rather than something to re-derive. The spec allows either; a DHCP lease moving is
T76's problem, and T76 is where it is detected.

**D4 — IPv4 only in T74.** `NetworkInfo` will report both, and the row can hold either, but a second
address doubles the SAN, URL and QR surface for a case no acceptance criterion in the feature spec
mentions. Where an interface has no IPv4 address, sharing on it is a typed refusal.

**D5 — Choosing the interface is explicit when it is ambiguous.** One interface that is up, is not
loopback and has a private IPv4 address: use it. More than one: refuse with the list, and
`mix site share <site> --interface <name>` picks. This is `Error::hint` doing what it exists for.

**D6 — The firewall is `PrivilegedOp::FirewallApply { plan }`, whole-state.** It follows
`HostsApply`, `PortAccessGrant` and `ResolverApply` exactly: the plan names every port this machine
should have open for MixEngine, so a second request supersedes the first, "already done" is a
comparison rather than a judgement, and a rule set that drifted is repaired by the operation that
created it. Unsharing the last shared site is an apply with an empty list, which is also what T76's
auto-revoke will send. `FirewallPlan` carries one variant per OS mechanism, the way `ResolverPlan`
does.

**D7 — `mixengine-elevate` validates the plan itself, with rules it can check alone.** It cannot
consult the database, so "is this a web port" is not a question it can answer — what it can do is
refuse everything that is provably not one. TCP only. Every rule named with the fixed prefix
`MixEngine — `. A port below 1024 refused unless it is 80 or 443. A port on the standing deny list
of ports MixEngine's own non-web services use — 3306, 5432, 6379, 11211, 1025, 8025 — refused
outright, which is the "databases are never exposed" rule enforced at the last gate rather than
trusted to the caller. A bounded number of ports per plan. It never takes a command string from the
daemon.

**D8 — Where the firewall cannot be managed, say so.** macOS's application firewall needs no rule
for a listening socket in most setups; Linux gets a rule only where `ufw` or `firewalld` is active.
Both answer `Unmanaged { reason, manual_command }`, which the CLI prints. Reporting success there
would be the one lie that costs a user an afternoon.

**D9 — The certificate gains an IP SAN, and SAN comparison stops being a string list. This
overturns T50's D4.** `certs/leaf.rs` states in prose that nothing here issues an IP SAN, and treats
anything but a DNS name in that extension as having come from somewhere it did not write. That was
right while every name a site answered to was a hostname; a LAN address is the first name that is
not. The rule it replaces — *report only what a browser will match a hostname against* — survives,
because a browser does match an IP SAN when the URL is an IP.
`certs/leaf.rs` today decides a certificate is current with `cert.sans == domains`, both plain DNS
names. An IP SAN is a different `SanType`, so the comparison becomes a set of typed names; get this
wrong and every reissue looks like a change, which loops. Turning sharing off reissues without the
IP, by the same path.

**D10 — The daemon answers a URL; `mix` draws the QR.** The daemon has no business rendering
terminal graphics, and a graphical client will draw its own from the same string.

## Data model

Migration `0012_site_sharing.sql`, columns on `sites`:

| Column | Type | Meaning |
| --- | --- | --- |
| `shared_interface` | `TEXT NULL` | The OS name of the interface. `NULL` means not shared. |
| `shared_address` | `TEXT NULL` | The IPv4 address bound and certified, as text. |
| `shared_since` | `TEXT NULL` | RFC 3339, for display. |

Not a separate table: sharing is at most one row per site, has no history worth keeping, and every
reader wants it in the same read as the site. T76 adds `shared_until` here.

## API

- `site.share { site, interface? } -> SiteSharing` — validates, picks the address, writes the row,
  regenerates and reloads config, reissues the certificate, and enqueues the firewall apply. Returns
  the URL, the address, the interface, and the firewall outcome (`Applied` / `Unmanaged`).
- `site.unshare { site } -> SiteSharing` — the same path backwards, ending in an apply that no longer
  carries this site's ports.
- `SiteDetail` gains `sharing: Option<SiteSharing>` so `site.show` answers it in one read.

Both are mutating, so both reach the CLI: `mix site share`, `mix site unshare`.

## Elevation

One prompt. `site.share` produces at most one `PrivilegedOp`, and it is batched through the existing
`ElevationRequired` machinery, so a share that also needs nothing else raises exactly one dialog —
the spec's "the only one in normal day-to-day use".

## Testing

- Render tests for both front ends: a shared site carries loopback *and* the LAN address; an
  unshared site beside it carries only loopback.
- The nginx consequence of D2 and D3, asserted rather than discovered: nginx groups servers by
  listen address before it consults `server_name`, and the LAN address has exactly one server block
  in its group, so a LAN request carrying another site's `Host` is answered by the shared site as
  that group's default rather than by the site named. That is the intended outcome — no unshared
  site is served over the LAN — and it is a test so that a later refactor cannot quietly turn it
  into the other one.
- Certificate: a share adds the IP SAN, a second share is idempotent, an unshare removes it.
- The firewall trait against its mock on all three OSes; `Unmanaged` rendered by the CLI as the
  manual command.
- `mix site share` prints a URL and a QR block; `mix site unshare` leaves the row `NULL`.

**And the real thing, on Windows, before T74 is called done.** Smart App Control was turned off on
the development machine on 2026-08-31 and the block it used to impose is measured gone — a Caddy
installed by `mix package install` runs, having been refused outright the week before. That removes
the reason this spec would otherwise have deferred every real-world check to T76: a front end now
starts here, so a share can open a real `netsh advfirewall` rule through `mixengine-elevate` and a
phone on the same Wi-Fi can be pointed at the LAN URL. One manual run over HTTP closes the loop the
automated tests cannot: Caddy agrees with D2's reading of `bind`, the rule appears and disappears
with the share, and the site answers on the phone.

macOS and Linux stay with CI, as every **(P)** task does. T76 still owns the port scan and the rule
enumeration as *tests*; what moves here is the one-off human check that the mechanism works at all.

## Dependencies

Two new workspace crates: one to enumerate interfaces (`if-addrs`), one to render a QR code to the
terminal. Both are small, pure Rust, and cross-platform.

## Risks

- **D9 is the fragile one.** SAN comparison sits under certificate renewal, which runs on a timer
  for every site — a mistake there reissues in a loop rather than failing loudly.
- **Caddy's `bind` semantics are asserted from the documentation**, not yet measured. The render
  test proves what we generate; only running Caddy proves Caddy agrees — and that run is now
  possible on this machine, so D2 is checked rather than trusted before the task closes.
- **HTTPS from the phone is not solved by T74.** The IP SAN makes the certificate honest; the phone
  still does not trust this home's CA until T75 serves it. The manual check therefore goes over
  HTTP, which the feature spec offers as the simplest of its two paths.
