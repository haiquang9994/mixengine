# T45 — `ResolverConfig`: making one TLD arrive at our own server

**Roadmap:** T45, `.claude/roadmap/phase-4-sites-and-elevation.md`
**Depends on:** T44 (the server and the mode), T40 (the helper and its file protocol), T40b (the
queue), T64 (`mix elevation grant`), T41 (the marker-block engine, the atomic replace, the TLD
table), T42 (the read-capability-plus-operation shape this copies)

## What this closes

T44 built half of a name mechanism. A `hickory-server` answers `A 127.0.0.1` for every name under a
managed TLD, at any depth — and nothing on any machine sends it a single query, so
[`Dns::start`](../../../crates/mixengine-daemon/src/dns/mod.rs) hard-codes `ResolverRouting::NotWired`,
every home reports `hosts_only`, and `site.create` goes on queueing a hosts entry per domain and
spending an elevation prompt per batch.

T45 is the other half: the elevated, per-OS operation that makes a managed TLD arrive at that
server, and the read-only capability that answers whether it already does. It is the producer of
`ResolverRouting::Wired`, and **both halves switch on together** — the branch T44 wrote and could
only reach from a test is the branch this task first runs for real.

What it buys is the acceptance criterion Phase 4 is named after: **creating a site after first-run
setup triggers zero elevation prompts**, because a wildcard needs no record per domain.

## What was measured, rather than remembered

Six rounds on GitHub runners from a throwaway workflow on a branch that no longer exists, each one a
fake DNS server that answered `A 127.0.0.1` for everything and **logged every question it was
asked** — because the measurement has to tell three states apart that a resolver reports
identically: the query never arrived, it arrived and the answer was thrown away, or it worked.

Runs: [32645470164][r1], [32645728349][r2], [32645964245][r3], [32646103337][r4],
[32646195665][r5], [32646301311][r6].

[r1]: https://github.com/mixnz/mixengine/actions/runs/32645470164
[r2]: https://github.com/mixnz/mixengine/actions/runs/32645728349
[r3]: https://github.com/mixnz/mixengine/actions/runs/32645964245
[r4]: https://github.com/mixnz/mixengine/actions/runs/32646103337
[r5]: https://github.com/mixnz/mixengine/actions/runs/32646195665
[r6]: https://github.com/mixnz/mixengine/actions/runs/32646301311

**A Windows developer machine could answer none of it, and the two documents this task was written
from were wrong about the one system this project develops on least.**

### macOS — the feature spec was right, and it is better than it claimed

`/etc/resolver/test` holding `nameserver 127.0.0.1` and `port 53535`:

- **It needs no flush.** `before-flush.test` resolved to `127.0.0.1` through `gethostbyname` the
  moment the file existed — before `dscacheutil -flushcache` and before
  `killall -HUP mDNSResponder`. A different name asked after the flush worked too, which is what
  separates "took effect at once" from "the cache happened to be empty".
- **It is scoped.** `example.com` kept its real address throughout and the fake server was never
  asked about it. `api.deep.blog.test` resolved, so wildcards work at depth.
- **An unprivileged process can read it back**, two ways: `scutil --dns` prints the `domain`,
  `nameserver[0]` and `port`, and the file itself is world-readable.
- **macOS also asks our server for `_dns.resolver.arpa` type 64 (SVCB)** — Discovery of Designated
  Resolvers. That name is outside every managed TLD, so T44 answers `REFUSED`, no encrypted
  transport is discovered, and nothing else happens. Correct, and worth writing down so the first
  person to see it in a log does not think it is a bug.

### Linux — every mechanism the feature spec and the roadmap named is unusable

ubuntu-24.04, systemd 255, `systemd-resolved` active, `systemd-networkd` active, **NetworkManager
not installed at all** (`nmcli: command not found`).

- **A `/etc/systemd/resolved.conf.d/` drop-in with `DNS=127.0.0.1:53535` and `Domains=~test` hijacks
  the entire machine.** After the restart, `getent hosts github.com` answered `127.0.0.1`, so did
  `example.com`, and the fake server was asked about both. A global routing domain does not scope
  the global DNS servers — the global scope still matches everything. This is precisely the
  "never change the machine's global DNS server" that
  [domains-and-dns.md](../../../.claude/features/domains-and-dns.md) forbids, and it is the reason
  the measurement existed.
- **`resolvectl dns lo …` is refused outright**: `Failed to set DNS configuration: Link lo is
  loopback device.` So is `resolvectl domain lo` and `resolvectl revert lo`. The roadmap's own
  suggestion cannot be carried out at all.
- **A link of our own with no address is accepted and does nothing.** `resolvectl status mixengine0`
  reported `DNS Servers: 127.0.0.1:53535` and `DNS Domain: ~test` and `Current Scopes: none`.
  systemd-resolved builds no DNS scope for a link it does not consider up, so no query is ever sent
  — the configuration reads as applied and resolves nothing, which is the worst failure shape there
  is.

What works, and is what D10 chooses:

- **A dummy link with a link-local address.** Adding `169.254.53.53/32` flipped the link to
  `routable` and `Current Scopes: DNS`, and the name resolved immediately. Adding a routable
  `10.53.53.53/32` on top changed nothing further, so **the link-local address is sufficient** and no
  RFC 1918 range has to be claimed on the user's machine.
- **And it works declared in files**, which is the only shape that survives a reboot without a
  standing process of ours: `/etc/systemd/network/10-mixengine.netdev` and
  `…/10-mixengine.network`, then a `systemd-networkd` restart. `networkctl list` reported the link
  `routable configured` and `from-files.test` answered `127.0.0.1`.
- **Scoped**, in every working configuration: `example.com` and `github.com` kept their real
  addresses and the fake server was never asked about either.
- **Readable back with no privilege**: `resolvectl status mixengine0` prints the servers and the
  routing domain to an ordinary user.

### Windows — it works, and four rounds of evidence that it did not were my own bug

- **Port 53 binds with no privilege.** `bind(('127.0.0.1', 53))` succeeded. Windows reserves nothing
  below 1024, which is the whole reason T44 put the server on 53 there.
- **The rule routes, to `127.0.0.1`.** With an NRPT rule for namespace `.test`,
  `GetHostAddresses("nrpt-a.test")` returned `127.0.0.1` and the fake server logged the question.
  `api.deep.nrpt.test` did too, so wildcards work at depth.
- **It is scoped**: `example.com` kept its real address and was never asked of our server.
- **The registry home is exactly reachable.** `Add-DnsClientNrptRule -Namespace ".test"
  -NameServers "127.0.0.1"` writes
  `HKLM\SYSTEM\CurrentControlSet\services\Dnscache\Parameters\DnsPolicyConfig\{GUID}` holding
  `Name` REG_MULTI_SZ `.test`, `GenericDNSServers` REG_SZ `127.0.0.1`, `ConfigOptions` REG_DWORD
  `0x8`, `Version` REG_DWORD `2`, plus `Comment`, `DisplayName` and `IPSECCARestriction`. **A rule
  written straight to those values is read back by `Get-DnsClientNrptRule`**, which is what lets the
  helper write it without running PowerShell — see D11.
- **Windows asks for `A` and nothing else.** Every query NRPT sent was type 1. macOS and Linux both
  asked `A` and `AAAA`. T44 already answers `AAAA` as NODATA with an `SOA`, so nothing changes; it
  is recorded because a reader comparing the three logs will notice.
- **`nslookup` does not honour NRPT.** `nslookup nrpt-b.test` went straight to the machine's
  configured server and returned NXDOMAIN, while the same name resolved through `getaddrinfo` at the
  same moment. **T46's `domain.dns_status` and T47's `mix doctor` must not use it**, or they will
  report a correctly wired machine as broken.

**Rounds one to five said Windows routed nothing, and all five were void.** The fake server was
started with `Start-Process` in one workflow step and asked from the next, and the runner kills a
step's process tree when the step ends — so the server was dead before any rule was ever added. On
macOS and Linux `nohup … &` survived across steps, which is why only Windows was affected. The
control added in round five is what caught it: the first `nslookup` reached the server, the one in
the following step got "No response from server", and the log had gained nothing between them.

That mistake is the reason for D14. **A negative result with no control is a statement about the
instrument, not about the system**, and this design owes its test suite the control it took six
rounds to add here.

### What implementing it measured that the probe could not

Two facts the six rounds never asked for, both found by the system suite on its first CI run.

**Windows: writing the registry values is only half of applying them.** Every probe round ran
`ipconfig /flushdns` between adding the rule and querying, so "does a rule written straight to the
registry take effect on its own?" was never put. It does not: `Add-DnsClientNrptRule` reaches the DNS
Client through its WMI provider, which notifies the service, and a plain registry write does not — so
the rule sat there routing nothing. The helper now sends `SERVICE_CONTROL_PARAMCHANGE` to `Dnscache`,
which is the service control manager's own verb for "your parameters changed". **Not a restart**:
stopping the DNS Client takes every name on the machine with it for as long as it is down, to deliver
a message there is a documented control for. And it is best effort rather than an error — the rule is
written by then, and a machine whose service declined the notice picks it up at its next start.

**Linux: applying the wiring and the machine routing through it are not the same instant.**
`systemctl restart systemd-networkd` returns before the link is up and `systemd-resolved` has a scope
on it. The manual measurement hid this behind a `sleep 3` and nobody noticed.

Both together are why [`ResolverConfig::probe`] documents which question it answers — *is the
configuration in place*, not *does a name resolve now* — and why the system suite waits for a bound
rather than asserting at once. `PortAccess`' macOS probe draws the same line for the same reason, and
the honest end-to-end check is a real lookup, which is T46's.

## What already exists, and is reused unchanged

- **The queue and its guarded upsert** (T40b, T41 D2): `Elevation::enqueue`, the `dedupe_key` unique
  index, the `ElevationRequired` event, the one grant slot, and the degraded mode a decline leaves
  behind. T45 adds a producer and an operation, not a mechanism.
- **`mix elevation grant`** (T64) prints every pending operation and what it will literally change
  before it raises anything. T45 owes it a `describe()` per operation and nothing else.
- **The helper's request checks** (T40): not a symlink, a regular file, not owned by a superuser, not
  writable by anyone but its owner.
- **The atomic replace** (T41 D7): temp file beside the target, `sync_all`, `ReplaceFileW` on
  Windows and `rename` plus a directory fsync on Unix, carrying the old file's ownership and mode.
  macOS's three resolver files and Linux's two networkd files are all written through it.
- **The machine-wide lock** (T41, T42): a lock file in the root-owned audit directory, taken by each
  system's own `apply` *after* it has decided the plan is its mechanism — never before, or "this
  system does not do that" becomes a permission error on the two machines the plan was not written
  for.
- **`MANAGED_TLDS`** in `mixengine-proto`, compiled into both the daemon and the helper, which D3
  and D5 lean on entirely.
- **`Dns::mode()` and its two-term rule** (T44 D4): listening *and* routed, or this home is on the
  hosts file. T45 supplies the second term and changes nothing about the rule.

## Decisions

### D1 — The capability reads; the write is an operation

`ResolverConfig` answers which mechanism this machine has and whether the wiring is already in
place. It never prompts and never writes, exactly as
[`PortAccess`](../../../crates/mixengine-platform/src/traits/port_access.rs) does and for the same
reason: the write needs a token the daemon does not have, so a capability the daemon can call is by
definition one it holds no token for.

Reading needs no privilege on any of the three systems — measured: `/etc/resolver/test` is
world-readable, `resolvectl status mixengine0` answers an ordinary user, and the NRPT registry key
is readable under `HKLM\SYSTEM\CurrentControlSet`. That is what makes it affordable to ask on every
start, which D7 depends on.

### D2 — One mechanism per OS, but on Linux it is a runtime question

macOS is always `ResolverDirectory` and Windows is always `Nrpt`; those are properties of the
operating system. **Linux is not.** `SystemdLink` needs `systemd-resolved` to be running and
`systemd-networkd` to be managing links, and a machine with neither has no scoped mechanism at all —
NetworkManager was not even installed on the runner, so the dnsmasq drop-in the feature spec named
as a fallback is not a fallback, it is a different machine.

So `ResolverMethod` has a fourth variant, `None`, and it is a **valid answer rather than an error**:
this home stays in `hosts_only`, `DnsStatus::because` says why in words, and nothing fails. That is
the feature spec's own instruction — "if the only available mechanism would be global, report
`unsupported_platform` and fall back to hosts-only mode" — with the correction that it is not the
platform that is unsupported but this particular machine's configuration.

`PortAccessMethod` is chosen by `#[cfg]`; `ResolverMethod` is chosen by looking at the machine. That
difference is the whole of D2 and is why `method()` returns a `Result` here and not a constant.

### D3 — The plan carries TLDs and a port, and never a nameserver address

**This is the security decision of the task.** The obvious shape for the operation is "point these
names at this address", and it is wrong. `mixengine-elevate` exists because a compromised daemon *is*
the attacker (`.claude/architecture/security-model.md`), so an operation that accepts a nameserver
address from the request is an operation that lets whoever owns the daemon redirect the machine's
name resolution anywhere — with a valid signature, through the audited binary, with the user's own
Allow click.

So `127.0.0.1` is **compiled into the helper**, exactly as `PERMITTED` is in
[`hosts.rs`](../../../crates/mixengine-elevate/src/hosts.rs). So is the Linux link name
`mixengine0`, its link-local address, and the Windows registry GUID. The request carries only two
things the helper cannot know: **which of the managed TLDs to wire**, and **which port the server is
listening on**.

The port has to travel, because `[dns] port` is a real setting and a test daemon binds an
ephemeral one. It is bounded rather than trusted: non-zero, and on Windows a plan carrying a port at
all is refused, because NRPT has no field for one.

### D4 — Whole state, like `HostsApply` and `PortAccessGrant`

The plan says what this machine should end up routing, not what to add. A second request supersedes
the first rather than queueing behind it, "already done" is a comparison rather than a judgement, and
a wiring that has drifted is repaired by the same operation that created it. `dedupe_key` is the bare
string `"resolver"` for both directions, so a revoke enqueued behind a pending apply replaces it —
T42's D12 shape, for T42's reason: they are two answers to one question.

### D5 — What the helper checks, per variant

One module, `crates/mixengine-elevate/src/resolver.rs`, with nothing else in it, meant to be read in
one sitting — `hosts.rs`' pattern.

Common to every variant:

- every TLD is in `MANAGED_TLDS`, the constant this binary compiles in;
- the list is non-empty and no longer than `MANAGED_TLDS`, and holds no duplicates;
- **`local` is refused outright**, see D9;
- the plan's variant is this system's mechanism, or `Unsupported`.

`ResolverDirectory` (macOS): the port is non-zero. Each file written is `/etc/resolver/<tld>` and no
other path is constructible, because the TLD has already been checked against a table of single
labels. **A file that exists without MixEngine's marker line is not overwritten** — it is somebody
else's resolver configuration for that TLD, and silently replacing it is the failure T41's
marker-block engine exists to prevent. The marker is a leading comment line, and the whole-state
apply removes marked files for TLDs no longer in the plan.

`SystemdLink` (Linux): the port is non-zero. Two files under `/etc/systemd/network/`, both named by
the helper. The reload is `systemd-networkd` and nothing else.

`Nrpt` (Windows): registry values under one fixed GUID, written directly — D11.

### D6 — The wiring is per TLD, so the hosts block is per TLD too

T44 gave this home one `DnsMode` and
[`require_hosts`](../../../crates/mixengine-daemon/src/elevation.rs) branches on it: `HostsOnly`
means a line per declared domain, `Dns` means an empty block. That was right while nothing was
wired, because both terms were false for every TLD at once.

It stops being right here. Every mechanism measured is **scoped to one TLD** — one file per TLD on
macOS, one namespace per rule on Windows, one entry in `Domains=` on Linux — and `local` is
deliberately never wired. A home holding both `blog.test` and `shop.local` needs a hosts block
containing exactly one line, `shop.local`.

So `mixengine_core::hosts::desired` filters by whether *that domain's* TLD is wired, rather than
reading one home-wide flag. `DnsMode` survives as the summary a person reads on `mix status`; it
stops being the thing the hosts block is computed from.

This is a correction to T44 rather than a cost of T45: the mechanism was always per TLD, and a
home-wide mode was the simplification available to a task where nothing could be wired at all.

### D7 — The producer is the daemon's start-up probe, and that is what makes M4 true

`Elevation::require_resolver()` runs at every daemon start, beside `require_port_access`. It is both
the producer and the re-probe, which is T42's D7 one capability along.

The ordering this buys is the whole point:

1. A fresh home starts. Nothing is wired, so `ResolverApply` goes into the queue **before any site
   exists**.
2. The user runs `mix elevation grant` once — first-run setup.
3. The mode flips (D8), and from then on `site.create` computes a hosts block that is already what
   is on disk, enqueues nothing, and **prompts for nothing**.

Asking after the first site is created gets this wrong in a way that is invisible until it is
counted: the block would already hold that site's line, emptying it is a second operation, and the
second operation is a second prompt — which is exactly the acceptance criterion Phase 4 is measured
against.

A probe that fails asks for nothing, like `require_port_access` and unlike `require_hosts`: a probe
that could not read the machine has said nothing about what to ask for.

### D8 — The mode flips inside the running daemon, not at the next start

`Dns.routing` becomes interior-mutable and `Dns::reprobe()` runs after a grant finishes flushing.
Without it, a user grants permission, the machine is wired, and the daemon goes on writing hosts
entries until somebody restarts it — the grant would appear to have done nothing, which is the
worst possible outcome for the one screen whose job is to be believed.

`require_hosts` is then called once more after the re-probe, so the block that a hosts-only home
accumulated is cleared by the same grant that made it unnecessary.

### D9 — `.internal` joins the table, and `.local` is never wired

`MANAGED_TLDS` becomes `["test", "localhost", "internal", "local"]`, and a second constant
`WIRED_TLDS` names the three of them that may ever be wired — `local` is not one.

`.internal` is added now rather than later because ICANN reserved it in July 2024 for exactly this
purpose — it is RFC 1918 for names, it will never be delegated, and `blog.internal` states an
intention where `blog.test` states an experiment. Adding it *now* is much cheaper than adding it
after a release: `mixengine-elevate` is excluded from auto-update, so its table is allowed to be
older than the daemon's, and a TLD introduced later is refused by every installed helper until the
user reinstalls it. That is the correct failure and it is still a failure worth not scheduling.

**`.local` stays in `MANAGED_TLDS` and is refused by the wiring.** A site may be declared on it —
the CLI already demands `--i-know` — and that site gets a hosts entry, one exact name, like any
other. Wiring it is a different act: T44 answers `A 127.0.0.1` for *every* name under a managed TLD
at any depth, so an `/etc/resolver/local` file would send `printer.local` and every other Bonjour
name on the user's network to loopback, machine-wide. The refusal lives in the helper (D5) as well
as in the planner, because it is a rule about what may be done to a machine and not a preference
about what MixEngine does.

`.dev`, `.lc` and every other delegated TLD stay refused where they already are, in
`core::domains::normalised`. Whether a user may ever nominate their own TLD is a real question with
a real answer — the helper's table is compiled in precisely so that a request cannot extend it — and
it is out of scope here; see the last section.

### D10 — Linux gets a link of its own, and every alternative was measured out

MixEngine creates a dummy interface named `mixengine0` carrying `169.254.53.53/32`, declared in two
files under `/etc/systemd/network/` and brought up by `systemd-networkd`.

This is a larger footprint than a single file and it is the only thing that works. The global
drop-in redirects the whole machine, the loopback link is refused by systemd-resolved by name, a
real link would have its own servers replaced, and a link with no address is configured and inert.
A dummy link is the smallest object systemd-resolved will attach a scoped DNS server to.

The address is link-local and `/32`: it makes the link `routable`, which is the only property being
bought, and it adds no route anything else can reach. A routable RFC 1918 address was measured and
bought nothing further, so none is claimed.

### D11 — Windows writes registry values, not PowerShell

The helper writes `HKLM\SYSTEM\CurrentControlSet\services\Dnscache\Parameters\DnsPolicyConfig\{GUID}`
directly, with the four values measured, under **one GUID compiled into the binary**. It never runs
`Add-DnsClientNrptRule`.

`mixengine-elevate` never runs arbitrary commands, and a fixed cmdlet with validated arguments is
still a shell-out to a scripting host from a process holding an administrative token. The registry
write is the same effect with none of that, and the measurement is what makes it available: a rule
written to those values is read back by `Get-DnsClientNrptRule`, so what MixEngine writes is what
Windows' own tooling sees and can remove.

A fixed GUID rather than a generated one is what makes D4 cheap: "already done" is a read of one
key, revoke is a delete of one key, and two homes on one machine converge on one rule rather than
accumulating a rule each.

### D12 — `DnsStatus.wildcards` becomes the list of TLDs that have them

The field is a `bool` today and D6 makes it a lie: the honest answer is "yes for `test`,
`localhost` and `internal`, no for `local`". A client that has to derive per-TLD behaviour from one
boolean derives it wrongly, and this field exists (T44 D9) precisely so that nothing has to derive
it. It becomes the list, `DnsMode` stays the one-word summary, and per-domain detail remains T46's
`domain.dns_status`.

### D13 — Revoke ships built, validated and tested, with no producer

`ResolverRevoke` is written and refused correctly and enqueued by nothing, exactly as
`PortAccessRevoke` was in T42 D12. Uninstall (T87) is its producer. The reason to ship it now is
that reversing a wiring written five phases earlier is a worse task than writing both halves while
the mechanism is in front of us.

### D16 — A server on an operating-system-chosen port is answering, and nothing may be wired to it

`[dns] port = 0` asks the OS for a port. Every test home in this workspace uses it, so that no suite
takes 53 off the machine running it — and `config::Dns::port` already says why it is useless to
anybody else: *"a port that changes on every start is a port no resolver can be wired to."*

The producer has to act on that, and finding out that it did not was what running the existing suites
caught: a fresh test home queued a `resolver-apply` naming an ephemeral port. Applying one would be
an elevation prompt spent to point the machine's resolver at a number this process will not have
again — breaking name resolution for those TLDs until the next restart, on a home whose whole purpose
was to avoid touching the machine.

So `Dns` carries `wirable` beside its state, and `wirable_port()` is a different question from
`port()`: the server is answering either way, and only one of the two answers may be routed to. Every
suite that was written before T45 keeps exactly the queue it had, which is how the decision announced
itself as right rather than as convenient.

### D15 — The operation is already sanctioned; its shape is narrowed, and that needs no ADR

`.claude/architecture/platform-abstraction.md` keeps the closed list of things that cross into
`mixengine-elevate`, and says adding an operation **with effects** requires an ADR. Resolver wiring
is already on that list, so T45 adds no capability — but it is on the list in a shape this design
refuses:

```rust
ResolverInstall{ tld: String, addr: SocketAddr },   // addr may carry a non-53 port
ResolverRemove { tld: String },
```

That signature hands the helper a nameserver address, which is exactly what D3 rules out, and it is
one TLD per operation, which D4 rules out. They become `ResolverApply { plan }` and
`ResolverRevoke { target }`.

**No ADR.** The rule as written exists to stop a new capability being granted quietly, and this
change grants strictly less: an operation that could have pointed the machine at any address may now
point it only at loopback, and one that carried a name may now carry only a member of a compiled-in
table. Removing reach needs no ADR for the same reason `PathIntegrationApply` needed none when T26
took it off the list entirely. The precedent is one row up as well — T42 replaced whatever
`PortAccessGrant` was going to be with `plan: PortAccessPlan` and wrote ADR 0012 about the boot job,
not about the shape.

What it does need is the document updated in the same commit, in two places: the `PrivilegedOp`
listing, and the `ResolverConfig` row of the trait table — whose Linux cell reads "systemd-resolved
per-link domain, else NM/dnsmasq drop-in" and whose Windows cell names `Add-DnsClientNrptRule`. Both
were measured wrong today.

### D14 — Every negative assertion in the test suite carries a control

The system suite starts a real DNS server, wires the machine, asks, and unwires. A test that asserts
a name did **not** resolve, or that a query did **not** reach the server, must first assert — with
the same instrument, at the same moment — that the server answers when asked point blank. Six rounds
of measurement for this design produced four rounds of confident, wrong, negative results because
nothing checked whether the instrument was still alive.

On Windows the instrument is `getaddrinfo`, never `nslookup`, which was measured to bypass NRPT
entirely.

## The interface

```rust
// mixengine-proto :: privileged

/// How this machine is asked to route a managed TLD to MixEngine's own server.
///
/// **Carries no nameserver address** — D3. The helper compiles in `127.0.0.1`, the Linux link name
/// and its address, and the Windows registry GUID; a request that could name any of them is a
/// request that could point this machine's name resolution anywhere.
pub enum ResolverPlan {
    /// macOS: one `/etc/resolver/<tld>` per TLD, each naming a port.
    ResolverDirectory { tlds: Vec<String>, port: u16 },

    /// Linux: a dummy link of our own, declared to `systemd-networkd`.
    SystemdLink { tlds: Vec<String>, port: u16 },

    /// Windows: **one** NRPT rule naming every TLD. Its `Name` value is a `REG_MULTI_SZ`, so all
    /// three namespaces live under the one compiled-in GUID rather than one rule each — which is
    /// what makes D4's "already done" a read of a single key. No port: NRPT cannot express one,
    /// which is why T44 puts the server on 53 there.
    Nrpt { tlds: Vec<String> },
}

pub enum ResolverTarget { ResolverDirectory {}, SystemdLink {}, Nrpt {} }

pub enum PrivilegedOp {
    // …
    ResolverApply { plan: ResolverPlan },
    ResolverRevoke { target: ResolverTarget },
}
```

```rust
// mixengine-platform :: traits::resolver

pub enum ResolverMethod { ResolverDirectory, SystemdLink, Nrpt, None }

pub struct ResolverState {
    pub method: ResolverMethod,
    /// The TLDs this machine already routes here, on the port asked about.
    pub wired: Vec<String>,
    /// Why the rest are not, in words, for `mix doctor`.
    pub missing: Option<String>,
}

impl ResolverState {
    pub fn plan(&self, tlds: &[&str], port: u16) -> Option<ResolverPlan>;
    pub fn target(&self) -> Option<ResolverTarget>;
}

/// Whether anything on this machine routes a managed TLD to MixEngine's server — roadmap task T45.
///
/// Reads only, and never prompts: the write is `PrivilegedOp::ResolverApply`, applied by
/// `mixengine-elevate`.
pub trait ResolverConfig: std::fmt::Debug + Send + Sync {
    fn method(&self) -> Result<ResolverMethod>;
    fn probe(&self, tlds: &[&str], port: u16) -> Result<ResolverState>;
}
```

`Host` gains `fn resolver(&self) -> &dyn ResolverConfig`, the accessor its own documentation has
been promising since T40a.

## Crate changes

- **`mixengine-proto`** — `ResolverPlan`, `ResolverTarget`, two `PrivilegedOp` variants and their
  `name`/`dedupe_key`/`requires_elevation`/`describe` arms, two new entries in `PrivilegedOp::ALL`.
  `MANAGED_TLDS` gains `internal`, and a `WIRED_TLDS` constant names the subset D9 permits.
  `DnsStatus::wildcards` changes from `bool` to `Vec<String>` (D12).
- **`mixengine-platform`** — `traits/resolver.rs`, `resolver/` compiled under both `host` and
  `elevated`, `macos/resolver.rs`, `linux/resolver.rs`, `windows/resolver.rs`, `mock/resolver.rs`,
  the `Host::resolver` accessor, and a `resolver.lock` beside the existing two.
- **`mixengine-elevate`** — `resolver.rs` (validation only) and two arms in `ops.rs`.
- **`mixengine-core`** — `hosts::desired` filters by wired TLD (D6).
- **`mixengine-daemon`** — `Dns.routing` becomes interior-mutable, `Dns::start` probes,
  `Dns::reprobe`, `Elevation::require_resolver`, and the call after `flush`.
- **`.github/workflows/ci.yml`** — a `resolver` leg in the `system` job on all three OSes.
- **`.claude/architecture/platform-abstraction.md`** — the `PrivilegedOp` listing and the
  `ResolverConfig` row (D15), in the commit that changes the types.

## Testing

**Unit.** Plan and target construction per method; `describe()` for both directions; `dedupe_key`
shared between them; the helper's refusals one per rule, including `local` and including a Windows
plan carrying a port; `hosts::desired` with a mixture of wired and unwired TLDs.

**Daemon.** `require_resolver` enqueues on an unwired machine and enqueues nothing on a wired one;
`Dns::mode` flips on `reprobe`; the hosts block is emptied by the grant that wires the machine. All
against `mock::Host`, no sockets.

**System**, `crates/mixengine-platform/tests/resolver.rs`, `#[ignore]`d, elevated, one leg per OS in
the `system` job — Windows included, unlike port access, because Windows has a real mechanism here.
Each test takes a copy of whatever it touches and asserts the machine came back. Each carries its
control (D14).

## Out of scope, and where each goes

- **`domain.*` and `domain.dns_status`** — T46. The real-lookup diagnostics belong there; T45's probe
  answers "is the rule present", not "does this machine actually resolve the name". The measurement
  hands T46 two facts it will need: use `getaddrinfo` rather than `nslookup` on Windows, and expect
  `_dns.resolver.arpa` in the server's log on macOS.
- **`mix doctor` and repair** — T47, which is also where a wiring that drifted gets reconciled.
- **Uninstall** — T87, the producer `ResolverRevoke` ships without.
- **A user-chosen TLD.** The helper's table is compiled in so that a request cannot extend it, so
  "let the user add `.dev`" is not a setting but a change to the trust model: it needs an answer to
  "where does the helper get a table it is allowed to believe" — its own root-owned file? a wider
  compiled-in list? — and that is an ADR, not a field. Parked.
- **Two homes on one machine.** All three artifacts are machine-wide and generated from per-home
  state, so the second home's wiring replaces the first's — the same debt T41 recorded for the hosts
  block and T42 for the macOS anchor. The lock stops them interleaving a write and nothing stops
  that.

## Known limitation

`169.254.53.53/32` is a fixed link-local address on a link nothing routes through, so a collision is
bounded to a machine that has already chosen the same address for something else. It is not
negotiated and nothing detects it. If it ever bites, the fix is to read the address back in `probe`
and refuse to claim one already present, which the whole-state shape makes additive.
