# Local network sharing

**Goal**: test the site you are building on a real phone or tablet on the same Wi-Fi, in two clicks,
without ngrok and without accidentally exposing your database.

## What it does

Enabling sharing for a site:

1. Adds the chosen interface's address to the **shared site's own listeners** — never the front
   end's `bind_addr`, and never `0.0.0.0`.

   **And binds every other site to loopback, which is the half that is easy to miss.** Caddy's own
   default is every interface, so before T74 a home's sites were already listening on the network
   and merely matched no `Host` a stranger would send; opening the port is exactly what this feature
   does, so "opt-in per site" has to be written into every site's rendering rather than only into
   the shared one's. Found by a phone, not by a test — see the T74 design's *What the first real run
   changed*.

   **The shared site also answers to the address by name.** Binding an interface says where a
   connection is accepted; matching `Host` says which site replies, and a phone sends the address it
   was handed. A site bound to the LAN but not named by it answers 200 with an empty body — a blank
   page, which is the slowest failure to diagnose.
2. Adds a firewall rule for the HTTP/HTTPS ports, labelled `MixEngine — shared sites` — one
   elevation prompt, the only one in normal day-to-day use. The rule is **whole state**: one
   `FirewallApply` names every port this machine should have open, so unsharing the last shared site
   is the same operation carrying none (T74, D6). The label is per home and not per site, because
   one plan replaces another and a rule per site could not be superseded that way.
3. Advertises `<slug>-mixengine.local` over mDNS (`mdns-sd`) so phones can use a name instead of an
   IP — this is the one legitimate use of `.local`, and it is *our* hostname, not the site's TLD.
   **One label, and the hyphen is not cosmetic**: this line said `<slug>.mixengine.local` until T75
   measured it. mDNS conventions single-label host names under `.local` (RFC 6762 §3) and Windows'
   resolver enforces the convention — `blog-mixengine.local` resolves, `blog.mixengine.local`
   answers *DNS name does not exist*, same responder, same interface. See the T75 design, D1.
   The name is advertised only on the interface the site is shared on, and it is published as an
   `_http._tcp` service because `mdns-sd` has no hostname-only registration — so a shared site is
   visible in service browsers on that network, carrying its domain and port and nothing else.
4. Adds the LAN IP **and the mDNS name** to the site's certificate SANs and reissues, so HTTPS
   keeps working from the phone (the phone still needs the CA — see below). This overturned
   T50's D4, which said this build issues no IP SAN — see the T74 design's D9, and the reuse check
   that has to compare the address or reissue for ever.
5. Answers the URL and the exact IP/interface being used. `mix site share` draws a **QR code** from
   that URL in the terminal; a graphical client draws its own from the same string. The daemon
   renders neither — T74, D10.

## Hard rules

- **Opt-in per site.** Never global, never on by default. What that means in the rendering is a
  second listener on *this site's* block — Caddy `bind 127.0.0.1 <lan>`, nginx a second `listen` —
  with the front end's own `bind_addr` untouched and every other site rendering exactly what it
  rendered before (T74, D1 and D2).
- **Databases, caches, Mailpit and the daemon API are never exposed.** The API refuses a share
  request for any non-web service; the API does not offer the control, so no client can.
- **Auto-revoke on network change.** The daemon watches for interface/subnet/SSID changes and
  disables sharing, telling the user why. Sharing does not silently follow you from home to a café
  network.
- **Optional time limit** (`--for 2h`), default off.
- Sharing state is on the event stream, so a client can show it at a glance — a tray icon that
  changes whenever anything is shared.

## HTTPS from a phone

Two paths, both offered:

1. **HTTP for the LAN URL** (simplest): the shared URL is `http://…`, no CA needed. Warn if the site
   relies on secure-context APIs.
2. **Install the CA on the device**: the root certificate is served at
   `http://<lan-ip>:<port>/__mixengine/ca.crt` (only while sharing is on, only the public cert), and
   a client shows the URL plus per-OS instructions (iOS also needs *Settings → About → Certificate
   Trust Settings*; Android has separate user/system stores). This is the honest way — a private CA
   simply cannot be trusted by a device that has not installed it.

   **The front end serves it, not the daemon**, because the port is the front end's. What the daemon
   does is render the public PEM into a directory holding exactly one file and point the shared
   site's block at it: `certs/ca/` holds the signing key beside the certificate and is never a
   directory a front end is pointed at. See the T75 design, D9.

## Implementation notes

- Bind changes go through config regeneration + reload, not a restart, so open connections survive.
- `NetworkInfo` in the platform layer supplies interfaces, IPv4/IPv6 addresses and whether an
  interface is Wi-Fi — used to pick a sensible default and to detect changes.
- Firewall handling differs a lot: Windows `netsh advfirewall` rules are precise and removable;
  macOS's application firewall prompts and needs no rule for a listening socket in most setups;
  Linux applies `ufw`/`firewalld` rules only if one of them is active. Where we cannot manage the
  firewall, say so and give the manual command rather than pretending it worked.
- Disabling sharing must remove the firewall rule, stop the mDNS advertisement, rebind to loopback
  and reissue the cert without the LAN SANs.

## Acceptance criteria

- Enable sharing → a phone on the same Wi-Fi loads the site by QR code within seconds.
- With sharing on, a port scan from the phone finds the web port and **nothing else** MixEngine
  manages — this is an explicit integration test.
- Switching Wi-Fi networks disables sharing and notifies the user.
- Disabling sharing leaves no firewall rule behind (verified by enumerating rules by label).
