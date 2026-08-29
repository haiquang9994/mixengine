# T70 and T70a — On-demand activation

Roadmap: [.claude/roadmap/phase-7-efficiency.md](../../../.claude/roadmap/phase-7-efficiency.md).
Feature: [.claude/features/resource-isolation.md](../../../.claude/features/resource-isolation.md),
"1. On-demand start (the big win)".

**One design, two tasks.** T70 is the web path and T70a is the database path, and the split is by
*where the activator binds* — D2 and D4 — because that is the only decision the two do not share.
Everything else on this page is common: one activator, blind to the protocol, starting a plan rather
than a process. Splitting the document as well would have split the argument that makes them one
mechanism, so D4 is written here and is T70a's alone.

## What this is for

T69 built idle detection and shipped it switched off, and said why in one sentence: *stopping a pool
is only safe once something starts it again on the next request.* This is that something. Until it
exists every recipe answers `None` to `Recipe::idle_default`, nothing is ever idle-stopped unless a
person asks per service, and the whole of phase 7's pitch — idle costs nothing — is a setting nobody
is advised to turn on.

M7 is what fixes the scope: *"30 idle minutes leaves only the daemon and the web server."* Only those
two — which means the pools stop and **the databases stop too**. So activating php-fpm and leaving
MariaDB unreachable after its own idle policy stops it does not reach the milestone; it moves the
broken case from one service to another. That is why T70a is a separate task and not an optional one,
and why each task turns on only the `idle_default` it can itself start again (D9).

## D1 — The activator never speaks the protocol it activates

It accepts a connection, makes sure the service is running, dials the service, and copies bytes both
ways until one side closes.

That is the whole mechanism, and refusing to parse anything is what makes it one mechanism instead of
four. FastCGI, the MySQL protocol, the PostgreSQL protocol and RESP are four wire formats with
nothing in common except that each begins with a client connecting and saying something. An activator
that parsed FastCGI would be a FastCGI implementation in `mixengined` — new code on the request path,
with a parser's bug surface, that exists only to throw the parse away and forward the bytes anyway.

The client's first bytes are held in a buffer while the service starts and written to it once the
dial succeeds. Nothing interprets them, and nothing needs to: a client that sends nothing until the
server greets it (MySQL does exactly this) is served by the same code as one that speaks first.

**What this costs is honest to name.** The activator cannot tell a real client from a port scanner,
so anything that connects starts the service. On a loopback address, on a developer's machine, the
things that connect are that developer's. It also cannot report a protocol-level error — a failed
activation closes the connection, and what the client says about that is the client's own message.
D6 is why that is not the only thing it does.

## D2 — The web path is a second upstream, not a held address

The front end keeps pointing at the pool's own address. The activator binds **a separate, permanent
address of its own**, and the site file names it as a fallback:

```caddy
php_fastcgi unix//…/run/php-fpm-8.3.sock unix//…/run/php-fpm-8.3.activate.sock {
	lb_policy first
	lb_try_duration 5s
	lb_try_interval 50ms
	fail_duration 10s
}
```

The block is not decoration — the measurement below is that the bare two-address form is *wrong*, not
merely unhelpful.

Not: the daemon binds `php-fpm-8.3.sock` while the pool is stopped and hands it over when it starts.

Three reasons, and the first one is fatal to the alternative.

**There is no way to hand an address over without a window in which it is bound by nobody.** To let
php-fpm bind `php-fpm-8.3.sock`, the daemon must close its listener and unlink the file first — and
php-fpm binds it several hundred milliseconds later, at the end of its own startup. Every connection
arriving in between is refused by the kernel. The first request is served, which is the promise; the
second one, from the same page's second asset, is a 502. `SO_REUSEPORT` does not rescue it: two
listeners on one address make the kernel load-balance between them, which sends requests to the
daemon after the pool is up and serving.

**The hot path must not go through the daemon.** The feature spec's promise is "first hit is slow
(~1 s); the rest are normal" — the rest are normal because the front end talks to the pool directly,
as it does today. A permanent proxy hop for every request of every site would be a throughput and a
latency cost paid forever to save a service that is running anyway.

**Both front ends have the vocabulary — and Caddy's needs three directives, not one.** This was the
design's one unproven claim, so it was measured before the rest was written: a real Caddy 2.10.0 and
a real nginx 1.24.0 on Ubuntu 24.04, a dead address at `127.0.0.1:9001` standing in for the stopped
pool and a live one at `:9002` standing in for the activator, twenty requests each.

| Rendering | Result |
| --- | --- |
| nginx: `upstream` group, `server …:9002 backup`, `proxy_next_upstream error timeout` | **200 on the first request**, 7.9 ms |
| Caddy: `reverse_proxy …:9001 …:9002` | **8 of 20** — a coin flip |
| Caddy: `+ lb_policy first`, `lb_try_duration 5s` | **0 of 20**, every one taking the full 5 s |
| Caddy: `+ fail_duration 10s` | **20 of 20** — first 55.8 ms, the rest ~1.5 ms |

**Any two of Caddy's three directives fail, and they fail in opposite directions**, which is the
whole reason this was worth an afternoon rather than a paragraph. With none of them Caddy treats the
two addresses as equal peers and load-balances between them at random — so the naive rendering is not
merely a missing retry, it sends *half of a healthy site's traffic through the activator*. With
`lb_policy first` and a retry budget but no passive health checking, nothing ever marks the pool
unavailable, so `first` keeps choosing the address that is already refusing and spends the entire
budget on it before answering 502 — a rendering that is slower than having no fallback at all.
`fail_duration` is what lets `first` move on.

**And `fail_duration` is the knob with the real cost, so pick it deliberately.** Measured directly:
after the pool came back up, traffic kept going through the activator for exactly the ten seconds
`fail_duration` names, and returned to the pool on the eleventh. That interval is not a failure — the
activator finds the service running and proxies straight through (D8) — but it is a hop on every
request in it. Too short and Caddy re-probes a pool that is still starting; too long and a warm pool
is reached the slow way. It is a number to justify in the implementation, not to default into.

What is still unmeasured is narrow and should be said: both readings above used `proxy_pass` and
`reverse_proxy` rather than `fastcgi_pass` and `php_fastcgi`, because neither machine had a php-fpm
to answer. The retry lives in the shared upstream framework in both servers and `php_fastcgi` is
sugar over `reverse_proxy`, so the mechanism is the same one — but "the same one" is an argument, and
the suites that run a real pool (`crates/mixengine-cli/tests/caddy.rs`, `nginx.rs`) are where it
stops being one.

## D3 — One address per service, **stable**, and bound for as long as the daemon runs

Which service a connection is for has to be decided from the connection alone, because D1 forbids
reading what travels on it. The only thing a bare byte stream carries is the address it arrived on —
so there is one activator address per activatable service, and one activator behind all of them.

**The requirement on that address is that it never changes, and "bound permanently" is the same
requirement again.** The rendered site file must be the same bytes whether the pool is up or down: a
file that changed when a pool stopped would make every idle stop rewrite `etc/` and reload the front
end — a reload storm driven by the thing that exists to save work — and an address that is only bound
sometimes has a race against the front end dialling it.

**Stable is not the same as derived, and the two shapes need different answers.** The first draft of
this decision said "derived by a fixed suffix" for both, which is right for one of them and unsafe
for the other:

- **A Unix socket** is derived: `run/php-fpm-8.3.sock` → `run/php-fpm-8.3.activate.sock`. Free, and
  stable by construction. The derivation is **fallible** and must say so — `sun_path` is 104 bytes on
  macOS and 108 on Linux, `Endpoint::in_run_dir` already refuses a home too deeply nested for one,
  and nine more characters is enough to cross that line for a home that was just inside it.
- **A TCP port cannot be derived**, and arithmetic is the trap: with pools on 9000 and 9001, a
  "port + 1" rule gives the first pool an activator on the second pool's own port. A collision like
  that is silent — one service binds first and the other reports a conflict about a port nobody
  chose. So the port is **allocated once and persisted on the `services` row**, in a second column
  beside the one that is there, by the allocator that already exists: `core::services::ports`, whose
  rule is exactly the one needed here — *free means free on the machine, not free in the table*,
  asked by binding, bounded, and taken in the same critical section as the insert. Stability comes
  from the row, as it does for the port the pool itself listens on.

  T34c's closing note applies to this column too and should not have to be rediscovered: an
  allocated port belongs to its row for as long as the row lives. The activator's is not in anybody's
  `.env`, but it *is* in a rendered site file, and moving it silently is the reload storm this
  decision opened by refusing.

A connection on that address therefore means one thing: *the front end could not reach the primary.*
What the activator does about it is D8's.

## D4 — The database path holds the service's own address, and the window is stated

**This decision is T70a's, and the only one here that is.** It is written down with the rest because
it shares every other decision on this page — one activator, protocol-blind, starting a plan and not
a process — and separating the paper would separate the reasoning that makes both paths one mechanism.
What T70a adds to a finished T70 is a second caller and this binding rule.

There is no front end in front of a database. A client dials `127.0.0.1:3306` and nothing else will
do, so for these services the activator does bind the service's own address while it is stopped and
releases it when it starts — the arrangement D2 rejected for the web path.

It is rejected there and accepted here because the alternatives are worse, not because the objection
stopped applying:

- **Always proxying** — the database listens somewhere private and 3306 is permanently the daemon's —
  removes the window and adds two failures. Every query's bytes cross the daemon for the connection's
  whole life, and a daemon that dies takes a *running* database's reachability with it. Today a
  crashed daemon leaves a working database; that is not a property to trade for a startup window.
- **Not activating databases at all** leaves M7 unreachable, per the opening.

So: the window exists, it is the service's own start time, and it is entered only by a *second*
client arriving while a *first* client's connection is already starting the service. The first client
is always served. Write it down in `resource-isolation.md` where a user reads it, not only here.

**This is the one place D3's "bound for as long as the daemon runs" does not hold**, and it cannot:
the address belongs to the service, so the daemon binds it on the idle stop and releases it on the
start. Which also settles D8 for these services by construction — a service a person stopped is never
bound for, so their stop has nothing to undo it.

## D5 — Activation starts the plan, not the process

The activator calls the same `Services::start` a person's `mix service start` calls, with the plan
built from the graph. A pool with a dependency gets its dependency started, in order, and a service
already mid-start for another reason is joined rather than started twice — `Registry::begin` already
decides both, under the lock that makes the second one safe.

Nothing about activation is a second way to start a service. It is a second *caller*.

## D6 — An activation that cannot finish must answer, not hang

Three outcomes, and each is bounded:

| Outcome | What the client sees | What is recorded |
| --- | --- | --- |
| Service became ready | its bytes, proxied | the ordinary `starting → running` with `StateReason::Requested` |
| Start failed, or ready timed out | the connection closed | the failure the walk reported, as it is today |
| Another client is already activating | its bytes, proxied, after the same wait | one start, not two |

The budget is the service's own `ReadyCheck` timeout, which is already per-recipe and already the
number a person raises when their machine is slow. Inventing a second timeout here would give a slow
MariaDB two different opinions about how long it is allowed to take.

**A connection is never held longer than that budget.** A client blocked forever on a service that
will never start is worse than a refusal: a refusal is a message, and a hang is a page that spins.

## D7 — The listener lives in `mixengine-platform`

A listener that binds either a Unix socket path or a TCP address is `#[cfg(unix)]` code, and
`mixengine-daemon` may not hold any — the gate in `crates/mixengine-proto/tests/workspace_layering.rs`
fails the build on it, by name. `ipc.rs` is the shape to follow: one type in the crate root, two
implementations under `unix/` and `windows/`, a byte stream handed back with no opinion about what
travels on it.

It is not `ipc.rs` itself. That endpoint is the daemon's own, singular, and carries a peer check that
answers "who is this" — three properties an activator's listener has none of.

## D8 — Only a service the daemon idled is started by a connection

A person who runs `mix service stop mariadb@main` and watches it start again on the next connection
has been overruled by their own tool. The address stays bound either way (D3); what changes is the
answer:

| The service is | The activator does |
| --- | --- |
| stopped, last reason `StateReason::Idle` | start it, wait, proxy |
| stopped, last reason anything else | close the connection |
| running | proxy straight through — the primary was refused for some other reason, and this is not the place to diagnose it |

That distinction already exists and is already persisted — T69 put the reason on the transition rather
than inventing an event for it, and this is the first thing to read it back rather than display it. A
daemon restart therefore keeps it: what was idled is still idled, and what a person stopped stays
stopped.

## D9 — A recipe's default is turned on by the task that can start it again, and by no earlier one

T69 wrote the cost down as "four `None`s": php-fpm, the databases, the caches, and the front ends
staying at `None` forever. The split divides them by which task makes each safe.

| Recipe | `idle_default` | Turned on by |
| --- | --- | --- |
| php-fpm | 30 min | **T70** |
| MariaDB, MySQL, PostgreSQL | 60 min | **T70a** |
| Redis, Memcached | 60 min | **T70a** |
| Caddy, nginx | `None`, permanently | nothing — the thing that starts everything else back up cannot be the thing that gets stopped |

The numbers are the ones `resource-isolation.md` already publishes. In each task it is the **last**
commit and not the first: until the activator is proved against that service's own real client, a
default that idles it is a default that breaks a home which changed nothing. T70 landing with the
database defaults still `None` is therefore not an oversight to tidy up later — it is the only state
in which a half-finished mechanism is safe to ship.

## The API and CLI surface

Nothing new, and that is deliberate. Activation is not something a person asks for — it is what makes
a setting they already have safe to turn on. `service.list` already reports the state and the reason;
a service stopped by idle already says so.

One addition to `mix doctor`, which is where "why is my site 502" is answered: a site whose pool is
idle-stopped and whose front end has no fallback upstream rendered is a home whose site files predate
this task, and re-rendering fixes it. That is a check, not a new command.

## How this is proved

- **The front-end retry — done, and it changed the design** (D2): measured before the rest was
  written, against a real Caddy and a real nginx. What it settled is in the table there; what it
  leaves is the same reading through `php_fastcgi` and `fastcgi_pass` against a real pool, which
  belongs in `caddy.rs` and `nginx.rs` beside the site renderings they already assert.
- **The splice knows nothing** (D1): a client that speaks first and one that waits to be greeted, in
  the same test, through the same code. Written under T70 with a synthetic pair rather than waiting
  for T70a's real MySQL — the property is about the activator, and a test that needed a database to
  state it would be a test of the database.
- **A person's stop is not undone** (D8): `mix service stop`, then a connection, then the service is
  still stopped.
- **A failed start closes the connection** (D6) rather than holding it to the heat death of the page.
- **Two clients, one start** (D4/D6): both served, one `starting → running`. T70a's, where the
  refusal window makes it the case that decides the design; T70 gets it for free from `Registry::begin`
  and asserts it anyway.
- Cross-platform, as everything here: the Unix socket path on Linux and macOS, TCP on Windows, from
  one test that names neither.

## What these tasks deliberately do not do

- **No activation for a service nothing can address.** A php-fpm pool on a Unix socket has one; a
  service with neither a port nor a socket has nothing to bind and is left alone, the same way T69
  left it unmeasurable rather than measuring it wrongly.
- **No `mix` command that starts a service by connecting to it.** The CLI has `mix service start`,
  which is clearer and already there.
- **No sharing of one activator address between two homes.** The address is derived from the home's
  `run/`, like everything else in it.
