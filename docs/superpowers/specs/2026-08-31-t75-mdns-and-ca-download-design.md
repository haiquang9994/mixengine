# T75 — a name for the phone, and the authority it has to install (design)

Feature spec: [`.claude/features/lan-sharing.md`](../../../.claude/features/lan-sharing.md).
Roadmap: [T75](../../../.claude/roadmap/phase-8-differentiators.md), Phase 8.
Predecessor: [T74](2026-08-31-t74-lan-sharing-design.md), whose D9 and D11 this task extends.

## Goal

T74 hands a phone an address. This task hands it a **name**, puts that name where a browser will
match it, and serves the one file without which HTTPS from a phone cannot work at all — this home's
certificate authority. The three are one task because they fail together: a name nothing certifies
is an HTTPS warning, and a certificate nothing trusts is the same warning with extra steps.

## Scope

**In.** The mDNS advertisement and its lifecycle. That name in the site's rendering and in the
certificate's SANs. The CA download endpoint, served only while sharing is on and only the public
certificate. `mix site share` printing the name, the CA URL and a short per-OS hint.

**Out.** Auto-revoke on network change, `--for`, sharing on the event stream, the port-scan and
no-rule-left-behind tests — all T76. IPv6, which T74's D4 excluded and this task does not reopen. A
guided per-device trust flow, which is a screen a graphical client owns.

## What the spike measured

Run on this machine on 2026-08-31, Windows 11, Wi-Fi at `192.168.50.36`, against `mdns-sd` 0.21.1.
It sits before the decisions because **D1 exists only because of it**, and because the thing it
overturned was written down in two places and believed by everyone.

| Hostname registered | Windows `Resolve-DnsName` |
| --- | --- |
| `mixenginespike.local` | 192.168.50.36 |
| `blog-mixengine.local` | 192.168.50.36 |
| `blog.mixengine.local` | *DNS name does not exist* |

Same responder, same interface, same minute, one variable: **the number of labels**. The third row
is the name the roadmap and the feature spec both promise.

Three more answers from the same run, each of which a decision below rests on:

- **Coexisting on UDP 5353 works.** Before the spike started, this machine already had two processes
  bound to it — `svchost` (Windows' own dnscache) and `msedge`. The spike bound alongside them,
  announced, and logged `Respond("Wi-Fi")` when asked.
- **The advertisement can be pinned to one interface.** `disable_interface(IfKind::All)` then
  `enable_interface("Wi-Fi")`, and the answer comes back scoped:
  `AddressesFound(..., interface_ids: [Wi-Fi])`.
- **There is no hostname-only registration.** `ServiceDaemon::register` takes a full `ServiceInfo`,
  so a hostname is always published as part of a service. The log says so:
  `Announce("blog._http._tcp.local.", "blog-mixengine.local.:Wi-Fi")`.

## Decisions

**D1 — The name is `<slug>-mixengine.local`, one label, and this overturns the text in two
documents.** The roadmap's T75 line and item 3 of the feature spec both say
`<slug>.mixengine.local`. mDNS conventions single-label host names under `.local` (RFC 6762 §3), and
Windows' resolver enforces that convention: the measurement above is a control pair differing only
in label count. Both documents move when this spec is approved, the way T74's spec moved T76's line.

**Not `<slug>.local`**, which is shorter and would also resolve. The feature spec's own words for
why: *"this is the one legitimate use of `.local`, and it is **our** hostname, not the site's TLD."*
The flat `.local` namespace is shared with every printer, Mac and IoT device on the Wi-Fi; a name
that carries `-mixengine` keeps the namespacing that sentence asks for while staying inside what the
protocol conventions.

**iOS was not measured** — there is no phone in this loop. It does not change the decision: Windows
is one of three supported systems and is the machine the developer types the name on, so the
multi-label form was already unusable before any phone had an opinion.

**D2 — `<slug>` is the first label of the primary domain, and a collision is a typed refusal.**
`blog.test` gives `blog-mixengine.local`. A site has no slug column — it has an ordered domain list —
and deriving the name rather than storing it means the name cannot drift from the site it names.

- A label that is not a legal mDNS label is normalised by the same
  [`domains::slug`](../../../crates/mixengine-core/src/domains.rs) this crate already uses for
  project names, so there is one definition of what a slug is in this repository.
- A primary domain with no dot uses the whole thing; an empty result is a typed refusal rather than
  a name like `-mixengine.local`.
- **Two shared sites whose first labels agree** (`blog.test` and `blog.dev`) is refused with the
  list and a hint, exactly as T74's D5 refuses an ambiguous interface. It is only ever a refusal on
  the *second* share, and unsharing the first makes the name available again.

**And the check lives in one function called from two places.** `site.update` can change the domains
of a site that is already shared, which re-derives its name — so the same helper that `site.share`
asks runs there too, and the update path re-renders, reissues and re-advertises through the
machinery it already runs. A rule enforced at one entrance is a rule with a back door.

**D3 — The name has to appear in four places, and T74 already paid for learning this.** The lesson
recorded in T74's *What the first real run changed* was that binding an interface decides where a
connection is *accepted* while the site block's address list decides which site *replies*. A name
repeats it exactly:

1. The mDNS `A` record, answering with the shared address.
2. The site block's address list (Caddy) and `server_name` (nginx) — without this the phone
   resolves the name, connects, and gets 200 with an empty body.
3. The certificate's SANs, as a DNS name.
4. `SiteSharing` on the wire, and the CLI's output.

Miss the second and the failure is a blank page, which is the slowest failure this feature has.

**D4 — The responder is whole-state, reconciled from the rows.** A module
`mixengine-daemon/src/mdns.rs` holds the `ServiceDaemon` and the set of names currently advertised,
and exposes one method — `advertises_what_it_declares()` — that reads every shared row and
reconciles: register what is missing, unregister what is no longer there. Called from `share`, from
`unshare`, from the `site.update` path of D2, and at daemon start.

This is [T74's D6](2026-08-31-t74-lan-sharing-design.md) applied to a second mechanism, for the same
two reasons: a daemon restarted while sites are shared must advertise them again without anybody
asking, and *"is this right?"* becomes a comparison rather than a judgement.

**D5 — A responder that will not start never fails a share, and never touches the certificate.**
Where 5353 cannot be bound or an interface cannot be enabled, the site is still shared by address —
the outcome T74 already ships. `SiteSharing` gains `advertised: bool` and the CLI says why the name
is missing, in the shape of [T74's D8](2026-08-31-t74-lan-sharing-design.md) `Unmanaged`.

**The decoupling is the load-bearing half.** The configuration and the certificate carry the name
whenever the site is shared, whatever the responder is doing. Deriving them from responder health
instead would mean a responder dying triggers a certificate reissue — and the renewal timer runs
that comparison for every site on a schedule, which is precisely the loop
[T74's D9](2026-08-31-t74-lan-sharing-design.md) named as the fragile thing in this area.

**D6 — The advertisement is pinned to the shared interface.** `mdns-sd` announces on every
interface by default. This machine has eight addresses across Wi-Fi, Ethernet, Bluetooth, Tailscale
and two Hyper-V switches; announcing `blog-mixengine.local` on all of them publishes a name that
resolves to an address the site is **not** bound to and the firewall rule does **not** cover — a
URL that fails for whoever is handed it, and a small statement about this machine made on networks
nobody asked it to speak on. The API measured in the spike is what pins it.

**D7 — Advertising a name means advertising an `_http._tcp` service, and this spec chooses what it
says.** There is no hostname-only registration in `mdns-sd`, so this is not an option being
weighed — it is a consequence being written down instead of being discovered. While a site is
shared it appears in every Bonjour service browser on the Wi-Fi. Therefore:

- The instance name is the site's primary domain, which is what a person browsing would want to see.
- The port is the home's web port, so a browser that follows the service reaches the site.
- **No TXT properties.** Nothing about the project, its root, or its runtime goes on the wire: a
  document root is an absolute path on somebody's laptop, and the same rule the blueprint spec
  states — never data, credentials or absolute paths — applies to a multicast packet at least as
  strongly.

**D8 — UDP 5353 does not enter the firewall plan, and `mixengine-elevate` stays TCP-only.** T74's D7
refuses everything that is provably not a web port, TCP included in that word — a security property
of an audited binary, not an omission. Widening it so that mDNS works would trade a real guarantee
for a convenience.

What makes that affordable is the measurement: 5353 was already bound by Windows' own dnscache
before anything of ours ran, so the host stack participates in mDNS on this machine by default.
Where a firewall does block it, the name silently fails to resolve and everything else still works —
so the daemon says so and gives the manual command — T74's D8 again, applied to the second mechanism
that a machine can refuse us.

**D9 — The CA is served from a directory that contains only the CA.**
[`ca::key_path`](../../../crates/mixengine-core/src/certs/ca.rs) puts `certs/ca/root.key` beside
`certs/ca/root.crt`. Pointing a front end's file server at that directory — the obvious rendering,
and one `rewrite` away from correct — would publish this home's certificate authority **private
key** on the local network. That is the most dangerous single line this task could write.

So the public PEM is rendered as a **generated document** into the `etc/` tree, into a directory
holding exactly one file, and the front end is pointed there. The private key is not in any
directory a front end serves, and that is true by construction rather than by two directives
agreeing. Rotation (T54) comes free: a new authority renders different bytes, the installer sees a
change, the server reloads.

- **The file is rendered unconditionally; only the route is conditional.** They are public bytes
  that nothing serves until a site is shared, and a file whose existence tracks sharing state is one
  more thing `swept()` has to get right.
- **`Content-Type: application/x-x509-ca-cert`.** Without it iOS does not offer to install a
  profile and Android does not recognise the file. The endpoint is useless without the header.
- Caddy needs the existing block body wrapped in a second `handle` so the two are mutually
  exclusive, with `bind` staying at block level; nginx needs one `location =` with an `alias`.
- Rendered into the HTTP **and** HTTPS blocks. The HTTP one is what a phone can actually use, since
  a device that has not installed the authority cannot trust the other; the HTTPS one is there so
  the path does not 404 on the scheme somebody happens to try.

**D10 — SAN comparison happens in two places, and the second one is already wrong on `master`.**
[`covered()`](../../../crates/mixengine-core/src/certs/leaf.rs) composes the list that
`leaf::reusable` compares against, and that path is correct: it includes what sharing added, so
reissue is idempotent.

The second place is [`certs.rs`](../../../crates/mixengine-daemon/src/certs.rs), where `problem()`
is handed `&record.domains` — the bare domain list, with nothing sharing added — and answers
`CertProblem::NamesDiffer` whenever `cert.sans` differs from it. **Every shared site therefore
reports `NamesDiffer` today**, because T74 put an IP SAN in the certificate and left this comparison
reading the list from before. It is a defect shipped with T74 rather than one this task introduces,
and this task is where it is fixed: adding a second SAN to the same certificate would otherwise make
a wrong answer wronger.

The fix is one function composing the covered set, asked by both — `mix cert status` and the
reissue path answering the same question the same way, by construction. The order is **domains,
then the mDNS name, then the address**: T74's rule that the head of the list is the common name
survives, the address stays last, and the name is a DNS SAN that belongs with the other names.

**D11 — The QR code stays on the IP URL.** Android's resolver does not answer `.local` for a
browser, so a QR carrying the name would be a broken URL for a large share of phones. The name and
the CA URL are printed as text beside it, with one line telling an iOS user that installing the
profile is not the last step — *Settings → About → Certificate Trust Settings* is. `mix` prints
this; a graphical client draws its own from the same fields.

## Data model

**No migration.** The name is derived from the domains and the sharing row, both of which the site
record already carries, and `advertised` is a property of a running responder rather than of the
home. A derived value stored is a value that can disagree with what it was derived from.

## API

`SiteSharing` gains three fields:

| Field | Meaning |
| --- | --- |
| `name` | `<slug>-mixengine.local`, always present while shared — it is in the configuration and the certificate whether or not the responder is up (D5). |
| `advertised` | Whether the responder is currently answering for it. |
| `ca_url` | `http://<address>:<port>/__mixengine/ca.crt`. |

No new method, and both existing mutating methods already reach the CLI.

## Elevation

**T75 raises no elevation prompt and adds no `PrivilegedOp`.** Stated rather than assumed: the
firewall plan is unchanged (D8), the mDNS socket is unprivileged, and the CA file is written inside
the home. The one prompt in this feature is still T74's.

## Testing

- Render, both front ends: a shared site carries the mDNS name in its address list / `server_name`
  and carries the CA route; an unshared site beside it carries neither.
- Certificate: sharing adds exactly two SANs in the order of D10; sharing again reissues nothing;
  unsharing removes both. Asserted against **both** comparison sites of D10.
- Reconciliation: register, unregister, and a daemon starting with a shared row already present.
- Name derivation: normalisation, a single-label primary domain, and the collision refusal — asked
  through `site.share` and through `site.update`, because D2's whole point is that it is one rule.
- **The private key is not serveable**: the directory the front end is pointed at contains exactly
  one file and it is not `root.key`.
- Two sites shared on one address, which T74's nginx note assumed away: assert which one answers
  `Host: <ip>`, so the grouping behaviour is a decision on record rather than something a phone
  discovers.
- CLI: the name, the CA URL and the hint are printed; `advertised: false` renders as a reason and
  not as silence.

**And a real run on this machine before T75 is called done**, for the same reason T74 needed one:
every defect that survived T74's tests was found in the first minute against real hardware. The
name resolves, the site answers to it, and the CA downloads and installs. macOS and Linux stay with
CI, as every **(P)** task does.

## Dependencies

`mdns-sd` 0.21.1 — pure Rust, cross-platform, and the crate the feature spec already names. It
lives in `mixengine-daemon` beside `hickory-server`, not in `mixengine-platform`: it makes no
OS-specific call, and the platform rule exists to keep `#[cfg]` out of logic, not to route every
socket through a trait.

## What the first real run changed

Run on this machine on 2026-08-31 against a sandbox home, Caddy 2.10.2, Wi-Fi at `192.168.50.36`.
Recorded the way T74's is, because the same rule held: what a test asserts is what a spec said, and
a spec can be wrong in the same place.

**Everything the design claimed, held.** `Resolve-DnsName blog-mixengine.local` answered
`192.168.50.36`. The site returned its real body over HTTP *and* over HTTPS when addressed by name,
rather than the empty 200 that D3 exists to prevent. The certificate carried exactly
`blog.test`, `blog-mixengine.local` (DNS) and `192.168.50.36` (IP), in that order, with the subject
still `CN=blog.test`. The authority downloaded as 618 bytes of PEM with
`Content-Type: application/x-x509-ca-cert`, parsed as a CA, and contained no private key.
`mix site unshare` withdrew the advertisement — the name stopped resolving — and reverted both the
rendering and the certificate.

**`caddy validate` accepted the `handle` restructure**, answering *Valid configuration* against the
generated file. That retires the risk this spec named as the largest of the rendering changes.

**D9's danger was measured rather than argued.** With the site shared, the directory the front end
is pointed at held exactly `ca.crt`; `certs/ca/` held `root.crt` **and** `root.key` and appeared in
no directive.

**One defect was found before the run, by reading.** The first version of the responder registered a
service under the site's domain and withdrew it by rebuilding the instance name from the *mDNS*
name. `mdns-sd` escapes the instance label, and the two strings never matched — so unsharing would
have left the name resolving, which is precisely what the feature spec says unsharing must stop. The
module now keeps the name it registered under, and a test pins the escaping that caused it.

**And one thing the design did not anticipate, which belongs to T76.** Binding UDP 5353 makes
Windows raise its own *Windows Security* dialog asking whether to allow `mixengined.exe` through the
firewall. D8 is still right that MixEngine adds no rule for mDNS — but the operating system asks on
our behalf, and the rule it writes when somebody clicks Allow is **every port, TCP and UDP, on the
Private and Public profiles**. That is far wider than the "web ports only" promise this feature is
built around, it is not created through `mixengine-elevate`, and `site.unshare` does not remove it
because MixEngine never made it. T76 owns the enforcement tests for exactly this promise, so the
question — refuse the prompt, scope it, or say plainly that it exists — is recorded on T76's line
rather than answered here.

## Risks

- **Wrapping the Caddy block body in `handle` is the riskiest rendering change here**, because it
  touches the template every site is rendered from rather than only a shared one's. The net is real
  and already in place: the recipe runs `caddy validate` against the staged file, so a mistake fails
  at render rather than at the next request.
- **D10 is the one that is already failing.** The reissue path is safe — `reusable` compares against
  `covered` — but `mix cert status` has been calling every shared site's certificate wrong since
  T74. A fix that composes the covered set in one place and asks it from both is the only shape that
  cannot drift again; two call sites kept in step by hand is what produced this.
- **Coexistence on 5353 is measured on Windows only.** macOS's `mDNSResponder` and Linux's
  `avahi-daemon` are the responders that actually own that port on their systems, and neither has
  been observed sharing it with us. CI answers this, and a failure to bind is D5's degradation
  rather than a broken share.
- **The `_http._tcp` announcement is a new thing this product says about itself on a network.** D7
  bounds what it says; it does not remove the fact that it speaks.

## Text that this task makes wrong

Three places assert something that stops being true the moment this lands, and each is a line
somebody will otherwise trust:

- [`SiteSharing::url`](../../../crates/mixengine-proto/src/site_api.rs) — *"HTTP, and http alone
  until T75"*.
- Item 3 of the feature spec — *"`<slug>.mixengine.local`"*, and *"T75, not T74: until it lands the
  URL is the address"*.
- The roadmap's T75 line, for the same name.
