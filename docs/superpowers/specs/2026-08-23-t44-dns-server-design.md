# T44 — The built-in DNS server: a wildcard, and nothing else

**Roadmap:** T44 (and T46a, closed with it), `.claude/roadmap/phase-4-sites-and-elevation.md`
**Depends on:** T39a (`core::domains` and the TLD table in `proto::domains`), T41 (the hosts block and
`Elevation::require_hosts`), T38 (`PortOwner`), T43 (what a front end actually binds)
**Feeds:** T45 (resolver wiring, which is what turns this on), T46 (`domain.*`), T47 (`mix doctor`)

## What this closes

`site.create` must prompt for nothing. Today it queues a hosts entry per domain and therefore queues
an elevation prompt per site — the exact cost [ADR 0005](../../../.claude/decisions/0005-on-demand-elevation.md)
says the product may not pay repeatedly. A DNS server that answers `*.test` by pattern pays it once
instead, at first-run setup, and never again however many sites exist.

T44 builds that server. It does not make a single name resolve on anybody's machine: nothing routes
a query to it until T45 wires a resolver. What it does build is the thing T45 wires *to*, the mode
that says which of the two mechanisms this home is actually running on, and the report that names
whoever is sitting on the port when the answer is "neither".

It also closes **T46a**. Hosts-only mode is not a second feature to build later — it is the state
this daemon is in on every machine until T45 lands, so it is the branch that has to work first.

## What is asserted rather than measured

T42 measured its three forks on GitHub runners before deciding. This design cannot: the two facts it
turns on need a macOS and a Linux machine, and the machine this was written on is Windows. They are
recorded here as assertions with the consequence of being wrong attached, and the design is arranged
so that being wrong about either is survivable rather than fatal.

**`5353/udp` belongs to mDNS.** macOS runs `mDNSResponder` on it on every machine, and a Linux
desktop runs `avahi-daemon` on it. Both bind the wildcard address with `SO_REUSEPORT`, and a
`tokio::net::UdpSocket` does not set that flag, so `127.0.0.1:5353` is expected to fail with
`EADDRINUSE` on a clean macOS. **If that is right, choosing 5353 would mean the hosts-only branch is
the only branch that ever runs and the central feature of Phase 4 never switches on.** The roadmap
and `features/domains-and-dns.md` both said 5353; both are corrected by this design (D2). If the
assertion turns out to be wrong, nothing breaks — the port is a constant with a config key over it,
and 5353 would merely have been available too.

**53 on Windows is bindable without privilege, unless it is inside an excluded port range.** Windows
has no privileged-port concept, which is the whole reason the table in `features/domains-and-dns.md`
works out: the one OS whose resolver mechanism cannot express a port is the one that does not need
one. Hyper-V and WSL reserve dynamic port ranges (`netsh int ipv4 show excludedportrange`), and a
bind inside one fails with a permission error that is not a permission problem. T47 already owns
detecting that; here it simply becomes a hosts-only mode with an unhelpful `because`, which is a
tolerable failure and not a wrong one.

## What already exists, and is reused unchanged

- **The TLD table** is `mixengine_proto::domains::MANAGED_TLDS`, and the policy over it is
  `mixengine_core::domains`. This server introduces no third opinion about what a domain is; it
  answers for the suffixes that table names and refuses everything else.
- **The hosts block** is `mixengine_core::hosts::desired` and `Elevation::require_hosts`, both from
  T41. Hosts-only mode does not add a producer — `site.create` already calls it on every write
  (`sites.rs`, `wants_the_hosts_file`). What T44 adds is the *other* branch.
- **Who holds a port** is `PortOwner::listening_on`, from T38, including its documented licence to
  answer with a `PortHolder` whose every field is `None`.
- **Shutdown** is the root `CancellationToken` in `main::serve`, which every other spawned task
  already hangs off.

## Decisions

### D1 — Authoritative for managed TLDs, REFUSED for everything else. No forwarding.

`features/domains-and-dns.md` promised that "everything else is forwarded to the system's upstream
resolvers … so putting our server in front is safe", and the roadmap line for T44 says "upstream
forwarding". Both are withdrawn here.

Every resolver mechanism T45 can use is scoped to a TLD: `/etc/resolver/test` is consulted for names
under `.test`, an NRPT rule names the `.test` namespace, `resolvectl domain <link> ~test` is a
routing-only domain, and a NetworkManager dnsmasq drop-in is written `server=/test/…`. If T45 is
correct, **no query outside a managed TLD ever arrives**, and a forwarder is code with no caller.

The one scenario that would produce such a query is a T45 that routes the whole machine here — and
the concrete way that could happen is `resolvectl dns <link> 127.0.0.1:<port>`, which *replaces* a
link's DNS servers rather than adding to them. In precisely that scenario a forwarder does not help:
`/etc/resolv.conf` still points at systemd-resolved's stub on `127.0.0.53`, so we would forward
there, resolved would send the query to the link's DNS server — which is now us — and every lookup
would hang until it timed out. The insurance does not pay out in the accident it was bought for.

REFUSED trades a slow failure for a fast one. A stub resolver that receives REFUSED moves to its
next nameserver immediately rather than waiting out a timeout, so a mis-wiring becomes loud and
instant, which is exactly the signal `domain.dns_status` (T46) and `mix doctor` (T47) exist to
report. A machine whose name resolution has become mysteriously slow is a failure nobody attributes
to MixEngine; one that says REFUSED is diagnosable in a single `dig`.

What leaves the design with it: a cache to invalidate, a second resolver stack inside the daemon,
and the sentence "refuses recursion from non-loopback sources" — which D8 subsumes, because a server
that never recurses has nothing to refuse. It also means MixEngine never opens a resolver on a
user's machine that could answer for a name it did not author.

The cost, stated plainly: if a mechanism ever appears that can only be global, this task grows a
forwarder. That is not expected — all three systems have a scoped mechanism, and a system that did
not would be answered by hosts-only mode (D9) rather than by seizing the machine's DNS.

### D2 — The port is not 5353, and it is a config key

`53` on Windows, because NRPT cannot express a port. **`53535` everywhere else**, because
`/etc/resolver` and `resolvectl` and dnsmasq can express any port at all, and 5353 is the one number
in the high range that is already famously spoken for (see the assertion above). There is nothing to
gain by contending with `mDNSResponder` for a port when every mechanism that will point at us is
happy to be told a different number.

The default is one expression in one place:

```rust
const DEFAULT_PORT: u16 = if cfg!(windows) { 53 } else { 53535 };
```

`cfg!` rather than `#[cfg]` so both arms compile on every OS, which is the same reason the platform
crate compiles every launcher table everywhere.

`[dns] port` overrides it, and exists for two independent and both-real reasons. A machine where
something already holds the default needs a way out that is not "move your home directory". And
**the test suite needs an ephemeral port**: `.claude/standards/testing.md` forbids a test touching
port 53, so without a configurable port there is no legitimate integration test of this server at
all.

### D3 — `AAAA` is NODATA, not `::1`

`features/domains-and-dns.md` says the server answers "`A`/`AAAA` … `127.0.0.1` / `::1`". After T43,
the front end binds `127.0.0.1` only (`generate/recipe.rs` defaults `bind` to it, and T42 D6 already
settled that the macOS pf redirect is `inet` and does not cover IPv6). A name that resolves to an
address nothing is listening on is a browser preferring IPv6 under Happy Eyeballs and waiting before
it falls back — on every connection.

This is the same question T41 answered for the hosts block, with the same answer and for the same
reason, and `core::hosts::LOOPBACK` carries the note. The DNS server matches it: `AAAA` gets NOERROR
with no answer records, which is the honest statement "this name exists and has no IPv6 address",
and a client moves to `A` at once.

Reversing this later is one constant and one test, on the day something binds `::1`.

### D4 — The mode is a function of two terms, and T44 can only produce one of them

```rust
enum ResolverRouting { NotWired }          // T45 adds `Wired`
enum DnsMode { Dns, HostsOnly }
```

Whether this home is running on DNS is not "is the server listening". It is "is the server listening
**and** is something routing a TLD to it". T44 builds the first term. Nothing in this task can make
the second one true.

So throughout T44 the mode is `HostsOnly` on every machine, and `site.create` goes on queueing hosts
entries exactly as it does today. That is not the task failing to do anything — it is the task not
breaking the product. A mode that read only "the server is listening" would, the moment this merged,
stop `require_hosts` from queueing anything while no resolver pointed anywhere, and no name would
resolve at all.

Typing the second term rather than passing a `bool` is what makes it read as the separate question
it is. **`ResolverRouting::Wired` is declared here rather than left to T45**, and that is a change
from this design's first draft: with only `NotWired` in the enum, the branch that decides whether a
home writes hosts entries at all would have had no test that could reach it, and a branch first run
on the day T45 lands is a branch that breaks then. The variant exists; nothing but a test
constructs it; `Dns::start` hard-codes `NotWired` and T45 replaces that constant with a probe.

The `Dns` branch is written and tested in full here even though nothing outside a test can construct
its input. Its content is that **the desired hosts block is empty** — so `require_hosts` queues a
`HostsApply` that *clears* the managed block rather than skipping the queue. Clearing is right: a
home that has moved to DNS should not leave stale names behind, and T41's operation carries whole
state, so an empty block is a well-formed thing to ask for and is reversible.

### D5 — This crate writes its own `RequestHandler`

`hickory-server` offers `Catalog` over `InMemoryAuthority`, and an `InMemoryAuthority` holding a
`*.test A 127.0.0.1` wildcard would nearly do it (RFC 4592 wildcards do match at any depth below the
closest encloser, so `api.blog.test` would be covered).

It is declined. This zone holds no records — it is a *function of a name*, not a set of data — and
routing it through an authority turns "REFUSED outside the managed TLDs" and "NODATA for `AAAA`"
into configuration of a state machine somebody else wrote, verified by asserting on that state
machine's behaviour. Implementing `RequestHandler` directly is roughly 150 lines, puts the whole
policy in one pure function, and lets the answering table below be a table of unit tests over
`Message` in, `Message` out, with no socket anywhere.

`hickory-proto` is still what parses and builds the messages, and `hickory-server` is still what owns
the UDP and TCP framing, EDNS handling and truncation. What is not delegated is the policy.

### D6 — Bind first, diagnose after; a failed bind never fails the daemon

The order is `bind` → on error, ask `PortOwner`. Not `PortOwner` → `bind`: between a probe and a bind
is a race, and `PortOwner`'s own documentation says every caller must treat its failure as "no
diagnosis" and carry on, which only holds if it is on the error path. A diagnosis that fails must not
become the failure being diagnosed.

A bind that fails is a `warn!` and a mode, never a refusal to start. This is T40b D10's argument
transplanted: refusing to start leaves the user with no daemon at all, which is strictly worse than a
daemon that says which of its two mechanisms it is running on.

`PortOwner` reads TCP only, and DNS is mostly UDP, so a UDP-only holder of the port will be reported
as "some other program" with no name. Extending the trait to UDP means three new implementations
(`GetExtendedUdpTable`, `lsof -i UDP`, `/proc/net/udp[6]`) to improve a sentence, and both branches
lead to the same next move — hosts-only mode — so the diagnosis is a courtesy, not a decision.
`PortHolder` was designed with every field optional for exactly this. Revisiting belongs with T47,
which already carries the neighbouring problem of Windows excluded port ranges.

### D7 — The server answers for every managed TLD; which ones are wired is T45's question

`MANAGED_TLDS` is `test`, `localhost` and `local`. All three are answered.

`.local` will almost certainly not be wired by T45, because routing it here would cut Bonjour off at
the knees, and `.localhost` may not need wiring at all since many resolvers map it already. Neither
is a reason for the server to be silent about them. Answering a name nobody sends is harmless; being
silent about a name T45 later decides to send is a bug that has to be fixed in two places.

### D8 — Loopback-only bind, which is the whole of the access control

The sockets bind `127.0.0.1` and nothing else, so a query from off the machine cannot arrive. That is
a stronger statement than checking the source address of a query that already reached us, it is one
line, and combined with D1 it means there is no recursion to abuse even from the loopback interface.

### D9 — Hosts-only mode is reported as a mode, with the promise it costs named

`DnsStatus` carries `wildcards: bool`, and in hosts-only mode it is `false`. That field exists
because the loss is specific and a user will hit it as a surprise otherwise: `blog.test` works and
`api.blog.test` does not, because a hosts file has one line per name and no patterns. The API says so
rather than leaving a client to infer it from the mode — the "no business logic in clients" rule
applied to a fact, not just to a computation.

`because` is a sentence the daemon writes, not a code a client translates, on the precedent of
`Error::UnsupportedPlatform { reason }`. It is where the port holder's name surfaces:

- `port 53 is held by Docker Desktop Backend.exe`
- `port 53 is held by another program on this machine` (`PortOwner` gave nothing)
- `[dns] enabled = false in mixengine.toml`
- `no resolver routes a managed TLD to this server yet` — every machine, for the whole of T44

### D10 — TTL 60, and negative answers carry an SOA

The positive answer is a constant, so a long TTL would be safe against staleness of the *address* and
unsafe against staleness of the *mechanism*: a home that switches to hosts-only should not have
resolvers holding our answers for an hour. 60 seconds costs a query a minute per name and bounds the
window.

NODATA is only correctly cacheable with an `SOA` in the authority section (RFC 2308), so the server
synthesises one per TLD apex, with `minimum` set to the same 60. Without it, a client re-asks for
`AAAA` on every single connection, which is the cost D3 was trying to avoid, moved one layer down.

## The interface

```rust
// crates/mixengine-daemon/src/dns/answer.rs — pure, no I/O
pub(super) struct Reply {
    code: ResponseCode,
    authoritative: bool,
    answers: Vec<Record>,
    authority: Vec<Record>,   // the SOA that makes a negative answer cacheable
}

pub(super) fn reply(op_code: OpCode, queries: &[LowerQuery]) -> Reply;
```

A slice rather than one question, because "exactly one question" is itself part of the policy: a
message carrying none or several is refused here rather than unwrapped by the caller.

```rust
// crates/mixengine-daemon/src/dns/server.rs
struct Handler;                       // impl RequestHandler, holds nothing
async fn bind(port: u16) -> io::Result<(UdpSocket, TcpListener)>;

/// Bind, start answering, and stop when `shutdown` is cancelled. The address is returned because
/// `port = 0` lets the OS choose and the caller has to be told which it chose.
pub(super) async fn start(port: u16, shutdown: CancellationToken) -> io::Result<SocketAddr>;
```

```rust
// crates/mixengine-daemon/src/dns/mod.rs
pub(crate) enum ResolverRouting { NotWired, Wired }   // only `NotWired` is produced before T45

pub(crate) struct Dns { /* state, routing */ }

impl Dns {
    /// Never fails: a bind that did not work is a mode and a sentence (D6).
    pub(crate) async fn start(config: &config::Dns, host: &dyn Host, shutdown: CancellationToken) -> Self;
    pub(crate) fn mode(&self) -> DnsMode;
    pub(crate) fn status(&self) -> DnsStatus;
}
```

```rust
// crates/mixengine-proto/src/daemon.rs
pub struct DnsStatus {
    pub mode: DnsMode,
    pub listening: Option<String>,
    pub wildcards: bool,
    pub because: Option<String>,
}

pub enum DnsMode { Dns, HostsOnly }

pub struct DaemonStatus { /* … */ pub dns: DnsStatus }
```

```rust
// crates/mixengine-core/src/config.rs
pub struct Dns {
    pub enabled: bool,          // default true
    pub port: Option<u16>,      // None = this OS's default
}
```

`Elevation::require_hosts` gains the branch:

```rust
let desired = match self.dns.mode() {
    DnsMode::HostsOnly => mixengine_core::hosts::desired(&self.store).await?,
    DnsMode::Dns => Vec::new(),
};
```

## The answering table

| Query | Answer |
| --- | --- |
| `A` for `<tld>.` or any name under it, at any depth | `A 127.0.0.1`, TTL 60, `AA` set |
| `AAAA` for the same | NOERROR, no answers, `SOA` in authority (D3, D10) |
| Any other qtype under a managed TLD (`MX`, `TXT`, `SRV`, `CNAME`, …) | NODATA, same shape |
| `SOA` or `NS` for a TLD apex | answered; this server is authoritative for it |
| Any name outside `MANAGED_TLDS` | `REFUSED` (D1) |
| Class other than `IN`, or opcode other than `Query` | `REFUSED` |

`RA` is never set: this server does not recurse and says so.

## Crate changes

- **`mixengine-daemon`** — new `dns/` module (`answer.rs`, `server.rs`, `mod.rs`); `main::serve`
  starts it and holds the handle; `api/rpc.rs` fills `DaemonStatus::dns`; `elevation.rs` grows the D4
  branch.
- **`mixengine-proto`** — `DnsStatus`, `DnsMode`, the `DaemonStatus` field.
- **`mixengine-core`** — `config::Dns` and its `template.toml` block.
- **`mixengine-cli`** — one DNS line in `mix status`.
- **`Cargo.toml`** — `hickory-server` and `hickory-proto`, both `default-features = false`, with the
  note explaining that the default set is empty on the server and that no `resolver`, `__tls`,
  `__dnssec` or `sqlite` feature is taken.
- **No change to `mixengine-platform`.** The `SystemResolvers` capability this design started out
  needing died with the forwarder (D1); build it when T46 has a caller for it.

## Testing

1. **The policy table** — every row of the table above, over `answer()`, with no socket. Plus what
   the table does not say and the protocol does: `BLOG.TEST` matches (DNS is case-insensitive), a
   trailing dot is the same name, `blog.test.evil.com` does **not** match (the suffix check is by
   label, never by string), a multi-question message, and an EDNS0 OPT record surviving the round
   trip.
2. **The mode table** — `(enabled, listening, routing) → (DnsMode, because)`, pure.
3. **Integration on an ephemeral port** — a real server, real UDP *and* real TCP queries against
   loopback, messages built and parsed with `hickory-proto` rather than by adding `hickory-client`.
   TCP is tested explicitly: a truncated answer sends a client to TCP, and a server that only speaks
   UDP fails there silently.
4. **The seam with hosts** — `HostsOnly` queues the block today's code queues; `Dns` queues an empty
   one. This test is what protects D4 from being tidied away later.

## Out of scope, and where each goes

- Resolver / NRPT / `/etc/resolver` wiring, and the elevated operation behind it — **T45**, which is
  what produces `ResolverRouting::Wired`.
- `domain.*` RPC and `dns_status` with a real lookup — **T46**.
- Reading the OS's upstream resolvers — build it when T46 needs it, with a caller.
- UDP in `PortOwner`, and Windows excluded port ranges — **T47**.
- Answering on a LAN address so a phone can reach a site — **T75**, and it is mDNS, not this.

## Documents corrected by this change

- `.claude/features/domains-and-dns.md` — the port (D2), `AAAA → ::1` (D3), and the forwarding
  paragraph (D1).
- `.claude/roadmap/phase-4-sites-and-elevation.md` — T44 ticked and rewritten, **T46a ticked and
  folded into it**, and the port in T45's line corrected.

## Found while reviewing this change, and left alone

**`daemon.status` is not backwards compatible within protocol 1, and this adds to it.** `dns` is a
required field, as `elevation` has been since T40b, so a `mix` from a new build asking an older
daemon that has not been restarted fails to deserialise the answer — including the note
`render::status` writes for exactly that skew, which is now unreachable. This change does not
introduce the problem and cannot fix it alone: making `dns` optional buys nothing while `elevation`
stays required. It is written down as **T88c** in [phase 9](../../../.claude/roadmap/phase-9-ship.md),
where one rule can be chosen for the whole struct.

**Every suite that starts a real daemon was about to bind the default DNS port**, which is 53 on
Windows — rule 1 of `.claude/standards/testing.md` forbids exactly that, and two suites in parallel
would have raced for it with the loser silently in hosts-only mode. `mixengine_testkit::Home` now
writes `[dns] port = 0` into every home it makes: the daemon still binds, still registers both
transports and still reports where, on a port the operating system hands out. This is what D2's
second reason for the key was for.

## Known limitation

Until T45 lands, this server answers nothing, because nothing asks it. Its whole observable effect in
T44 is a line in `mix status` saying which mode this home is in and why — which is the honest report
of a machine that is still running on the hosts file.
