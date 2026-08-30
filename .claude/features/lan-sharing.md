# Local network sharing

**Goal**: test the site you are building on a real phone or tablet on the same Wi-Fi, in two clicks,
without ngrok and without accidentally exposing your database.

## What it does

Enabling sharing for a site:

1. Rebinds the **web server only** from `127.0.0.1` to `0.0.0.0` (or to the chosen interface).
2. Adds a firewall rule for the HTTP/HTTPS ports, labelled `MixEngine — shared sites` — one
   elevation prompt, the only one in normal day-to-day use. The rule is **whole state**: one
   `FirewallApply` names every port this machine should have open, so unsharing the last shared site
   is the same operation carrying none (T74, D6). The label is per home and not per site, because
   one plan replaces another and a rule per site could not be superseded that way.
3. Advertises `<slug>.mixengine.local` over mDNS (`mdns-sd`) so phones can use a name instead of an
   IP — this is the one legitimate use of `.local`, and it is *our* hostname, not the site's TLD.
   **T75**, not T74: until it lands the URL is the address.
4. Adds the LAN IP to the site's certificate SANs and reissues, so HTTPS keeps working from the
   phone (the phone still needs the CA — see below); the mDNS name joins it in T75. This overturned
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
2. **Install the CA on the device**: the daemon serves the root certificate at
   `http://<lan-ip>:<port>/__mixengine/ca.crt` (only while sharing is on, only the public cert), and
   a client shows a QR code plus per-OS instructions (iOS also needs *Settings → About → Certificate
   Trust Settings*; Android has separate user/system stores). This is the honest way — a private CA
   simply cannot be trusted by a device that has not installed it.

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
