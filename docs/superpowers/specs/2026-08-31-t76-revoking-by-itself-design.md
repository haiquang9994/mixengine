# T76 — revoking by itself (design)

Roadmap task **T76**, phase 8. The half of LAN sharing that happens when nobody is typing: T74
shipped `site.unshare` and this ships the three ways a share ends without it — a network change, an
expiry, and the thing a client has to be told so it can show either.

It also answers the one question T75 measured and deliberately did not decide: the firewall rule
MixEngine never made.

## Goal

A share that a person turned on and walked away from ends by itself, for a reason it can state, and
by the same road the person's own `mix site unshare` takes. And the two promises this feature has
been making since T74 — *web ports only*, *no rule left behind* — stop being sentences and become
tests.

## Scope

In: the watcher and what it compares, the `--for` expiry, one event-stream variant, the lazy mDNS
responder and the `mix doctor` note beside it, and the two enforcement tests.

Out: IPv6, which T74's D4 excluded and this task does not reopen. SSID and gateway-MAC detection —
D3 names the case they would catch and why this build does not guess at it. Any repair that removes
a firewall rule MixEngine did not create — D8. Re-sharing a site on its new address after a network
change, which is the behaviour the feature spec forbids in as many words: *sharing does not silently
follow you from home to a café network*.

## Decisions

**D1 — One loop, shaped like `certs::renewal::start`, with a period a test can move.** A new
`sites::revoke` module holds `once()` — one pass, taking the rows and the interface list and
answering what should end — and `start()`, the loop around it: first tick thrown away, cancellation
token, nothing catching up. That is [`certs::renewal`](../../../crates/mixengine-daemon/src/certs/renewal.rs)
and [`services::idle`](../../../crates/mixengine-daemon/src/services/idle.rs) exactly, and the
third instance of a shape is the point at which copying it is the cheap decision.

The period is `[sharing] check_seconds`, defaulting to 30, validated against zero the way
`certs.renew_check_seconds` is. It is a key for that key's stated reason and not for
configurability: **a period no test can move leaves the loop the one part of this task nothing
exercises.**

Thirty seconds and not thirty milliseconds because of what the delay actually costs, which is less
than it looks: a laptop that has moved networks no longer *has* the address its front end is bound
to, so the listener the window would expose is already answering nobody. The window's real cost is
the dead bind named under *Risks*, and that is the strongest argument against a longer default.

**D2 — A change has to survive two consecutive passes, and an error is not a change.** This is the
correction that matters most in this design, and it exists because the naive form is subtly wrong: a
DHCP renewal, a wake from sleep, an adapter resetting — each can make one enumeration report an
interface with no address, or not report it at all. Revoking on that single reading unshares every
site on the machine, reissues every certificate, and leaves somebody to work out by hand what
happened and why. **A false revoke is more expensive than a late one**, which is the whole reason a
30-second period is affordable in the first place.

So `once()` answers *what looks wrong*, and the loop revokes only what looked wrong twice running.
**The debounce covers the network comparison and not the expiry**, which is the distinction that
makes it a correction rather than a delay: a deadline is computed from a row and a clock, with no
flaky reading anywhere in it, so waiting a second period would double the latency of something a
person deliberately asked for and protect nothing. What is being debounced is the reading, not the
decision.

The count lives in the loop and not in the database — it is a property of this process's last
reading, not of the home — so a daemon restarted mid-change simply takes one more period to decide.
And `NetworkInfo::interfaces()` returning `Err` is *no information*: the pass does nothing, keeps
what it believed, and logs. A machine that cannot enumerate its own interfaces has not told us that
the network changed.

**D3 — What is compared is the interface and the address, because that is what this build can say
honestly.** A share ends when the interface it names is no longer up, or when the IPv4 address that
interface holds is not the one that was bound and written into the certificate. Nothing else is
read.

The feature spec says *interface/subnet/SSID*, and this is narrower on purpose. SSID means three new
shell-outs (`netsh wlan show interfaces`, `networksetup -getairportnetwork`, `iwgetid`) that say
nothing about a machine on a cable; the gateway's MAC address means reading a route table and an ARP
cache on three systems. Both are real per-OS work, and what they buy is one case: **two networks
that hand the same interface the same address.** DHCP on an unfamiliar network almost never does,
which is why the cheap comparison catches roaming in practice.

That case is therefore a named limitation of this build rather than a gap somebody rediscovers: a
site shared at home and carried to a café whose router hands out the same `192.168.1.x` address stays
shared. It is written into the feature spec's own text, not only here.

**D4 — Revoking is `Sites::unshare`, called, and not a second copy of it.** The order T74 argued for
— the rule, then the listener, then the certificate — is a security property, and a background caller
with its own spelling of that order is a second place for it to be wrong. What T76 adds around the
call is the reason and the announcement.

**D5 — An automatic revoke's firewall plan waits in the queue, and the degradation is stated rather
than discovered.** `elevation.enqueue` queues; it does not raise a prompt. That is what stops a
laptop opened in a café from throwing a UAC dialog at somebody who asked for nothing — and it means
the empty `FirewallApply` that T74's unshare relies on is not applied until a person grants it. For
that interval the machine has a rule open on a port nothing is listening on, which inverts T74's
ordering rule.

It is the right way round to be wrong. The listener is gone before the rule is, so the open port
leads nowhere; the alternative — a dialog nobody asked for, on a network the user has just joined —
is worse in kind and not merely in degree. Two things narrow it further: the rule is private-profile
only, and a machine that has just joined an unfamiliar network is usually placed by Windows in a
different profile, so the rule left waiting frequently does not even apply. The queued operation is
announced as `ElevationRequired` already, and `mix status` counts it.

**D6 — `--for` is a property of the share, and a deadline already in the past is refused.** The
column is `shared_until`, and it is measured from `shared_since` — which is the value T74 goes out of
its way to preserve when a site is shared twice, precisely so that typing the command again extends
nothing.

Taken alone that produces a trap: `mix site share blog --for 1h` on a site shared three hours ago
would print a URL and a QR code for something the next pass unshares. **A URL that is dead when it
is printed is worse than a refusal**, so `site.share` refuses it — `InvalidArgument`, naming how long
the site has been shared and pointing at `mix site unshare` followed by a fresh `mix site share
--for`. Extending a share stays something a person says out loud rather than something a repeated
command does quietly.

**D7 — One event variant, carrying the answer `site.share` gives.** `DaemonEvent::SiteSharingChanged
{ domain, sharing: Option<SiteSharing>, because }`, emitted by `site.share`, by `site.unshare` and by
the watcher — one variant for the question *is anything shared, and why did that change?*, which is
the tray icon the feature spec asks for.

`because` is `Requested | Expired | NetworkChanged { was, now }`, where `now` is `None` for an
interface that is gone. A client that only wants the icon reads `sharing`; one that wants to say why
the phone stopped working reads `because`.

**D8 — The rule MixEngine never made: bind late, then say plainly that it exists.** T75 measured it —
binding UDP 5353 makes Windows raise its own dialog, and Allow writes an every-port TCP-and-UDP rule
for `mixengined.exe` on the Private *and* Public profiles. Wider than this feature's whole promise,
created outside `mixengine-elevate`, and not removed by `site.unshare` because MixEngine never made
it. The roadmap offers three answers; this takes the first and third and refuses the second.

*Bind late.* [`Mdns::start`](../../../crates/mixengine-daemon/src/mdns.rs) currently builds its
`ServiceDaemon` at daemon start, so the dialog arrives on a machine where nothing is shared and
nobody has asked for anything — the worst possible moment to be asked a firewall question. The
responder is instead built on the first advertisement and shut down when the set empties, which is
the same whole-state reconciliation `advertises` already performs. The dialog then lands in the
second after somebody typed `mix site share`, where the question has an obvious answer.

*Refuse to pre-empt it.* Writing a narrow UDP 5353 rule ourselves would stop the dialog, and it would
cost T75's D8 — that `mixengine-elevate` accepts web TCP ports and nothing else is a security
property of an audited binary, not an omission — and it would need an elevation prompt at daemon
start, which is the very thing being avoided.

*Say it.* A new `mix doctor` check reports the rule where it exists, as `Outcome::Note` and never as
`Problem`. The variant is the decision: a `Problem` carries a `ProblemId`, and a `ProblemId` is what
`daemon.doctor_repair` matches on. **Automatically deleting a firewall rule that MixEngine did not
create and the user personally clicked Allow on is not a repair**, so the condition is given no
identity a repair could key off. What the note carries is the sentence and the `netsh` command to
remove it by hand. On macOS and Linux the check is `Skipped` with its reason, as every per-OS check
in that report already is. Reading rules needs no elevation.

**D9 — One lock, because a background caller is what makes the missing one real.** `Sites` holds no
mutex today, and T74 was safe without one for a reason that expires here: share and unshare arrived
only from a person. The watcher makes *read the rows → compute the whole-state plan → write* run
beside an API request doing the same thing. Whole-state recomputation absorbs most of that race and
not all of it, so a `tokio::sync::Mutex` covers the span from the row write to the end of
reconciliation. It is small, and it is what makes "whole state" true rather than nearly true.

## Data model

Migration `0013_site_sharing_until.sql`, one column on `sites`:

| Column | Type | Meaning |
| --- | --- | --- |
| `shared_until` | `INTEGER NULL` | Milliseconds since the epoch. `NULL` is a share with no expiry, which is the default. |

Nullable independently of T74's three, which are set together or not at all: a share without an
expiry is the ordinary case, and folding this into that trigger would make `--for` mandatory.

**No column for the last revocation and its reason.** T74 decided sharing has no history worth
keeping, and a `last_revoked_because` would be exactly the derived value that outlives what it was
derived from. What follows from that is written down in *Testing* and in the feature spec: a
CLI-only user learns why from the event stream or from `daemon.log`, and the client-surface note
records what a graphical client does with it.

## API

- `site.share { site, interface?, for_seconds? } -> SiteSharing` — the extra field is optional, and
  D6's refusal is raised here.
- `SiteSharing` gains `until: Option<Timestamp>`.
- `DaemonEvent::SiteSharingChanged` — D7.
- A `mix doctor` check, no new method.

`mix site share --for 2h`, parsed by the CLI into seconds, and `mix site show` prints the deadline.
No new mutating method, so the no-client-only-capability rule needs nothing new to hold.

## Elevation

**T76 adds no `PrivilegedOp` and widens none.** The revoke queues the same whole-state
`FirewallApply` T74 already queues; D5 is about *when* it is applied, not about what it is. D8's
refusal is the same statement from the other direction: the helper's TCP-web-ports-only vocabulary
is unchanged.

## Testing

Unit, beside the module, with no daemon and no disk:

- `once()` against a hand-built row list and a hand-built interface list: an interface gone, an
  address changed, an address unchanged, an expiry passed, an expiry not yet passed, and a shared
  site whose interface is fine — six cases, one function.
- D2's debounce as its own test: one bad reading revokes nothing, the same reading twice revokes,
  and an expiry revokes on the first pass because it is not a reading. And an `Err` from the
  enumeration revokes nothing however many times it repeats.
- D6's refusal: a deadline computed to fall before `now` is `InvalidArgument` and names the site.
- The event serialises flat and round-trips, as every variant in `event.rs` is asserted to.

Integration, against a real `mixengined` in a sandbox home, in the shape
[`tests/renewal.rs`](../../../crates/mixengine-daemon/tests/renewal.rs) established — the loop is
driven by setting `check_seconds` low rather than by waiting:

- A share with a one-second expiry ends by itself, the row goes `NULL`, and the event arrives.
- A daemon restarted with an already-expired row revokes on its first pass.

And the two the roadmap names:

**The port scan.** A shared site, the front end up and one non-web service beside it; connect to
every port this home manages on the shared address and assert that the web port answers and nothing
else does.

**What this test proves is narrower than its name, and the docstring has to say so.** The
connection comes from the machine being scanned, to its own address, so it never crosses the
firewall: it proves *what is listening*, not *what is allowed through*. That is still the half worth
having — T74's real defect was a wildcard bind found with `netstat`, not a firewall rule — but a
later reader who believes this test guards the "web ports only" promise entire will be wrong. The
firewall half is the test below and the real run.

**No rule left behind.** Windows only, and CI's answer rather than this machine's: the daemon suites
run under a full token there, and a dev machine's `cargo test` does not. The test invokes
`mixengine-elevate` directly, the way
[`core/tests/elevation.rs`](../../../crates/mixengine-core/tests/elevation.rs) does — apply a plan
carrying ports, apply the empty plan, then enumerate by label and assert nothing matches. Gated on
`is_elevated()` and skipped with its reason otherwise, because a test that quietly passes when it
did not run is worse than one that says it was skipped. `ufw` has no comment field to name a rule
with, which is why this is a Windows test and not three.

**And a real run on this machine before T76 is called done**, as T74 and T75 both needed and both
were changed by: share a site, move the machine to another network, and watch the share end and say
why. The mDNS dialog is observed at the moment of the share rather than at daemon start, and the
rule it writes is found by the doctor note.

## Dependencies

None. `if-addrs` is already a dependency, `mdns-sd` is already a dependency, and the loop, the
config key and the event variant are all shapes this workspace already has three of.

## What the first real run changed

Run on this machine on 2026-08-31 against a sandbox home, `check_seconds = 5`, Wi-Fi at
`192.168.50.36`. Recorded the way T74's and T75's are, because the same rule held for both of them:
what a test asserts is what a spec said, and a spec can be wrong in the same place.

**D8 held in all three of its parts, and the first two were measured rather than argued.** Before any
share, `netstat -ano -p UDP` showed no socket on 5353 owned by the daemon — Edge and `svchost` held
theirs, and MixEngine held none. After `mix site share`, `UDP 0.0.0.0:5353` was owned by the daemon's
pid. After the share ended **by itself**, the socket was gone again and
`Resolve-DnsName blog-mixengine.local` answered *DNS name does not exist*. So the responder binds at
the share, not at the start, and lets go when the last share does.

**The rule MixEngine never made is on this machine, and the doctor note found it.** Windows holds two
inbound rules named `mixengined.exe` for the daemon's own path — enumerated independently with
PowerShell — and `mix doctor` reported exactly two, as a `Note`, with the `netsh delete` command in
it. The count-the-path approach survived contact with real `netsh` output.

**The expiry ended a share nobody ended.** `--for 2m` printed `ends in 1m 59s`, `mix site show`
counted down, and thirty seconds after the deadline the row was empty with
`because=the length it was shared for has run out` in `daemon.log`.

**One defect, and it was in a sentence rather than in the mechanism.** D6's refusal fired correctly —
`--for 1s` on a share running for 43 seconds was refused, naming both lengths and pointing at
`unshare` — but the message read *"for — a share&nbsp;&nbsp;&nbsp;&nbsp;measured from when it began"*,
with a run of spaces in it. A Rust line continuation had been lost while the file was being edited, so
`cargo fmt` joined the two lines keeping their indentation. No test could have caught it: every
assertion about that refusal asks for its code and its hint, and asserting on the exact wording of a
sentence is how a message becomes unchangeable. **A person reading the output caught it in one
second, which is the argument for the run rather than a footnote to it.**

**And what this run could not measure, stated rather than left to be assumed: the network change
itself.** Changing an interface's address needs an elevated token this session did not have, and the
one adapter under this session's own control — WSL's `vEthernet` — keeps its address across
`wsl --shutdown`, so there was no honest way to make an interface move. What *is* covered: the
reading, by the unit tests beside `sites::revoke` in both directions and with the debounce pinned
either way; and the road, end to end and on real hardware, because an expiry and a network change
take the same `pass()` → `Sites::unshare` path and the expiry was watched taking it. What is not
covered is the enumeration changing under a running daemon. It is the one claim in this design still
resting on tests alone.

## What the first CI run changed

The manual run above was made on one machine, and CI is three. It found two defects, neither of them
in the feature and both in what its tests assumed about the machine underneath.

**A suite may not assume this machine has a network.** `tests/revoke.rs` read the first non-loopback
interface and *asserted* one existed. On Linux that assertion is false by design: CI runs the whole
test job inside a network namespace holding nothing but loopback, deliberately, to prove that nothing
in the suite reaches the network. A machine with only loopback cannot share at all — `choose` refuses
loopback, which is T74's D3 — so the test was asserting a property of the machine rather than of
MixEngine. It is the same mistake as a test that assumes a port is free, and it now skips with a
printed reason, on `tests/firewall.rs`' rule that a skip has to be visible.

**And the CLI's own grammar, which only a real invocation could check.** `mix site share` takes its
domain positionally while `mix site create` takes `--domain`; the port-scan suite was written with
`--domain` on both and failed on Windows and macOS with *unexpected argument*. The manual run had
already hit this at the terminal and the command line was corrected there — the *test* was not, which
is a small lesson worth keeping: fixing what you typed is not fixing what you wrote down.

**What CI could prove that this machine could not.** `tests/firewall.rs` ran its body on the Windows
leg rather than skipping: a real rule was written through `mixengine-elevate`, found by label, and
gone after the empty plan. That is the *"disabling sharing leaves no firewall rule behind"* line of
the feature spec, verified against a real firewall for the first time — and it is the half the port
scan explicitly cannot make.

## Risks

- **D2 is the one that fails quietly if it is wrong.** A debounce that is too eager unshares
  somebody's demo; one that is too slow is invisible. The unit tests pin both directions, and the
  real run is what says whether a wake-from-sleep produces the reading this decision assumes.
- **A dead bind outlives the window, and its blast radius is every site.** Between the address
  vanishing and the revoke, the front end holds a `bind` to an address the machine no longer has —
  so any re-render in that interval, for a certificate renewal or an unrelated new site, may fail to
  reload and take down sites that were never shared. The 30-second period bounds it; it does not
  remove it, and it is the strongest argument against a longer default.
- **The lazy responder is a refactor of code T75 measured working.** `ServiceDaemon::shutdown` is
  one-way, so the field moves under the lock and the restart path is new. The reconciliation tests
  T75 wrote are what keep this honest, and they run register/unregister/restart already.
- **The port scan needs a real non-loopback address on the CI runner**, and `choose_interface`
  refuses to guess where several are up — which a runner with virtual adapters will be. The test
  names its interface from what `NetworkInfo` reports rather than relying on the default.

## Text that this task makes wrong

- The feature spec's *"the daemon watches for interface/subnet/SSID changes"* — D3 narrows it, and
  the case that narrowing gives up belongs in that document rather than only in this one.
- T74's data-model note, *"T76 adds `shared_until` here"*, which stops being a promise and becomes a
  description.
- `.claude/features/client-surface.md`, which gains the revocation reason: a CLI user reads it from
  the log, and a graphical client is where it becomes a notification.
