# T69 — Idle detection and `IdlePolicy` shutdown

**Roadmap task:** T69, the second of phase 7.
**Status:** design, agreed 2026-08-26.

## What this is for

Phase 7's goal is that idle costs nothing, and
[`resource-isolation.md`](../../../.claude/features/resource-isolation.md) names the three mechanisms
that carry it: on-demand start, idle shutdown, and hard limits. T68 built the third. This is the
second — the one that decides a service has nothing to do and stops it.

**The vocabulary already exists, and so does the column.** `IdlePolicy { after, probe }` and
`IdleProbe { Connections, HttpCounter }` were written into `mixengine-proto` when `ServiceSpec` was,
and `IdlePolicy`'s own doc comment says "Enforcement is roadmap task T69". `services.idle_minutes`
has been a column since the initial migration; [phase 3](../../../.claude/roadmap/phase-3-services.md)
recorded it as "read from no row into no `IdlePolicy` until T69". So this task adds no type to
describe an idle policy. It adds the four things that were deferred:

1. **A reading.** Nothing in this workspace can count the connections established to a port.
   `PortOwner::listening_on` deliberately does not — its doc says "a socket that is merely
   *connected* to that port does not [count]: what a start collides with is a listener" — and idle
   needs exactly what that sentence excludes.
2. **A clock.** Something has to take the reading repeatedly and decide.
3. **The opt-out.** "Sites can opt out per project (*keep warm*) for the one project being worked on
   all day", which is a column `projects` does not have.
4. **A way to reach all of it**, because a capability no client can reach is not shipped:
   `service.idle`, `service.set_idle`, `keep_warm` on `project.update`, and `mix` over them.

**What it does not do is turn any of it on.** T70 — the on-demand activation gateway — is what makes
a stopped pool start again when a request arrives, and it is not built. Idle-stopping a php-fpm pool
today means a site answers 502 until somebody runs `mix service start`, so every recipe ships no
default and a home that changes nothing behaves exactly as it does now. D2 is what makes turning it
on later a one-line change per recipe rather than a migration.

## D1 — A policy has two halves and they come from two places

`IdlePolicy` is `{ after, probe }`, and the two are not the same kind of fact.

**`probe` belongs to the recipe.** Only the php-fpm recipe knows which port its pool listens on and
whether it renders a status endpoint; only MariaDB's knows the same about MariaDB. A user has no
business typing it, no way to check it, and no reason to want it different — a probe that disagrees
with the program it measures is a bug in our recipe, not a preference.

**`after` belongs to the row.** How long *this* machine's owner is willing to keep a database warm is
the one part of the policy that is theirs, and `services.idle_minutes` is where it already lives.

They are joined where `limits_json` is already joined: `Generator::prepare` in
`mixengine-core/src/generate.rs`, which is the one function that turns a `services` row plus a recipe
into a `ServiceSpec`. One more column in its `SELECT`, one more field on its `Context`, and
`Recipe::idle_probe` beside `Recipe::role` and `Recipe::instancing`.

The alternative — an `idle_json` column holding a whole serialised `IdlePolicy`, matching
`limits_json` — was rejected for the reason that shape is right for limits and wrong here. A
`ResourceLimits` is entirely the user's; an `IdlePolicy` is half ours, and a column holding our half
is a copy of the recipe that goes stale the moment a recipe is corrected. `CLAUDE.md`'s
disposable-generated-config rule, applied to a value rather than to a file.

## D2 — The column has three states, and it has them today rather than when T70 needs them

| `idle_minutes` | Means |
| --- | --- |
| `NULL` | use the recipe's default |
| `0` | never idle-stop, whatever the recipe says |
| `n` | idle-stop after `n` minutes |

Two states would be enough for T69 alone, because in T69 every recipe answers `None` and nothing is
ever stopped. Three are what stop T70 from needing a migration.

The trap is specific. If `NULL` meant "never", then T70 — which turns the feature on by putting `30`
into the php-fpm recipe and `60` into the databases' — would have no way to tell a home that never
touched the setting from one whose owner deliberately switched idle-stopping off. Every existing row
holds `NULL`. Turning defaults on would either ignore the second person or fail to reach the first,
and correcting it afterwards means a migration that has to guess which of the two each `NULL` was.
Distinguishing them costs nothing while both are unreachable, and cannot be added later at any price.

So `Recipe::idle_default() -> Option<Millis>` exists in this task, is called in this task, and
returns `None` from every recipe in this task. T70 changes four `None`s to `Some`.

## D3 — Counting connections is a new capability, not a wider `PortOwner`

```rust
pub trait ConnectionCount: std::fmt::Debug + Send + Sync {
    /// How many TCP connections are established to `port` on this machine.
    fn established_on(&self, port: u16) -> Result<usize>;
}
```

It sits beside `port_owner` on `Host` rather than inside it. `listening_on` answers "who is in my
way" and returns a `PortHolder` with a pid and a program name in it, which costs a second lookup on
every OS and on macOS costs a second process. Idle asks for a number, every thirty seconds, per
service, forever. Widening the existing trait would mean either building a `PortHolder` nobody reads
or growing a second method on a trait whose whole documented subject is listeners.

**A count and not a list.** Which peer is connected is a question nothing here asks, and the shapes
that could answer it — a `Vec<SocketAddr>`, a per-pid breakdown — are allocations taken every tick to
be thrown away. If a client ever wants "who is using this database", that is a diagnostic and it can
have `listening_on`'s treatment then.

## D4 — Each OS counts the way it already reads its own socket table

| OS | Mechanism |
| --- | --- |
| Linux | `/proc/net/tcp` and `/proc/net/tcp6`, rows whose state is `01` (`TCP_ESTABLISHED`) and whose local port matches |
| Windows | `GetExtendedTcpTable` with `TCP_TABLE_OWNER_PID_ALL`, rows whose `dwState` is `MIB_TCP_STATE_ESTAB` |
| macOS | `/usr/sbin/lsof -i :<port> -sTCP:ESTABLISHED -t`, counted by line |

Each is the file or the call the existing `PortOwner` implementation for that OS already uses, with
one constant changed — `TCP_LISTEN` becomes `TCP_ESTABLISHED`, `_LISTENER` becomes `_ALL` plus a
filter. That is deliberate: the parsing, the ipv6 absence on a machine booted with `ipv6.disable=1`,
the reason macOS shells out rather than guessing at `socket_fdinfo`'s layout — all of it is settled,
tested and commented in those three files, and a second reader of the same table would be a second
place for those findings to be forgotten.

macOS spawns a process per reading, which is the cost that decides D6's period. At one sweep every
thirty seconds, for the services that have a policy at all, it is a process every thirty seconds —
and in this task, with no default policy anywhere, it is no processes at all.

## D5 — `HttpCounter` needs nothing from the platform

Read the URL, pull the named field, compare it with the previous sample; the service was busy if the
number moved. It is an HTTP request to loopback, which `ReadyCheck::Http` already makes, and it is
identical on all three systems.

It is implemented in this task rather than deferred, and the reason is not completeness. A count of
established connections cannot tell a pool serving requests from a pool holding a keep-alive
connection that has been silent for an hour — which is the normal state of a browser tab left open on
a site, and therefore the exact case where wall-clock idling is right and connection counting is
wrong. `Connections` alone would idle-stop nothing that matters and would look like it worked.

## D6 — Idle is counted in consecutive sweeps, never in wall-clock time

The sweeper is a clock in the shape of `certs/renewal.rs`: `tokio::time::interval`, first tick thrown
away, `CancellationToken` to end it, and a `Pass` enum that says whether the sweep ran or was skipped
and why. Its period is thirty seconds.

`after` is honoured as `after / period` consecutive sweeps that saw the service idle. **Not** as a
comparison against a stored `idle_since` timestamp, and the difference is the case that decides it: a
laptop suspended overnight counts none of that time on Linux or macOS, so the tick after the lid
opens arrives eight hours late. A timestamp comparison concludes the service was idle for eight
hours and kills it in the first second of somebody's morning. Counting sweeps concludes it has one
observation, which is the truth — nothing was measured while the machine was asleep.

The counter lives in the sweeper's own memory, not in a column. A daemon restart resets it, which is
correct: a service the daemon has just adopted has been observed zero times.

## D7 — A failed reading is not an idle reading

`lsof` not on the PATH, `/proc/net/tcp` unreadable, the status endpoint refusing the connection: the
counter **resets to zero**, and the sweep logs at debug.

This is `PortOwner`'s own documented rule — "every caller of this is expected to treat an error as
*no diagnosis* and carry on" — with the stakes raised. There, a failed reading costs a diagnosis.
Here, treating "I could not measure" as "there is nothing to measure" stops a running database
because a tool was missing. An unmeasurable service is never stopped, and stays running forever if
the machine never recovers, which is the failure everybody would rather have.

## D8 — A service with a running dependent is never idle

`ServiceGraph` already knows which services depend on which. Before any probe is taken, a service
with at least one running dependent is idle-exempt.

MariaDB with no connections underneath a running php-fpm pool is not a database nobody wants; it is a
database between two requests. And stopping it would break the dependency the graph exists to
maintain, which the start walker would then have to repair — a mechanism undoing another mechanism's
work, once every sweep.

This is checked before the reading rather than after, because it is cheaper than the reading and
because it cannot be overruled by one.

## D9 — Keep-warm is a column on `projects`, and its reach is `sites`

`projects.keep_warm INTEGER NOT NULL DEFAULT 0`. A service is exempt when it is the `php_service_id`
of a site belonging to a project whose `keep_warm` is set — one join, evaluated per sweep.

**And this is where the task is deliberately partial.** The only path from a project to a service in
today's schema is `sites.php_service_id`, so keep-warm keeps a project's PHP pool warm and does
**not** keep the database that project uses warm. Nothing in the schema says which database a project
uses. Inventing a `project_services` table here would be a second table describing a relationship
`sites` already half-describes, written by nobody, read by one sweeper.

The relationship belongs to **T77** (the blueprint manifest), which is where a project declares what
it needs. When it lands, this join widens; it does not get rewritten. Until then the gap costs
nothing, because with no default policy no database is ever a candidate for stopping.

## D10 — No new event: a reason, on the transition that already exists

`StateReason` gains one arm, and `DaemonEvent` gains nothing:

```rust
Idle { minutes: u32 },
```

`ServiceStateChanged(ServiceTransition)` already carries `reason`, and "why did my PHP stop?" is a
question about a transition, not a separate occurrence. A `ServiceIdled` event would announce the
same moment twice on a stream every client shares — and a client that handled only the new one would
miss idle stops from a daemon that predates it, while one that handled only the transition would show
a stop with no explanation.

`state.rs`'s existing test — every reason must read as a sentence in the clause that follows a state,
lower-case, no full stop — applies to the new arm and is what makes the rendering `stopped — idle for
30 minutes` rather than a variant name a person has to decode.

## D11 — Every recipe ships `None`, and that is the whole of "off"

There is no feature flag, no `enabled` boolean, no setting that switches the sweeper on. The sweeper
runs from the first daemon start after this lands; it finds no service with a policy, and does
nothing. A person who wants it runs `mix service idle mariadb@main --after 60m` and gets it, on their
own head, before T70 exists.

A flag would be a fourth state layered on D2's three, and removing it in T70 would be a change to
something users can already have set.

## The API and CLI surface

```
service.idle       { service }                    -> IdleReport
service.set_idle   { service, after }             -> IdleReport
project.update     { ..., keep_warm: Option<bool> }
```

```rust
pub struct IdleReport {
    /// The policy in force, joined from the row and the recipe. `None` is never idle-stop.
    pub policy: Option<IdlePolicy>,
    /// Which of the three states the row is in, so a client can render "using the default (none)"
    /// differently from "switched off here".
    pub source: IdleSource,
    /// Why this service would not be stopped right now even if its policy said so — a running
    /// dependent, or a project keeping it warm. Empty when nothing exempts it.
    pub exempt: Vec<IdleExemption>,
}

pub enum IdleSource {
    /// `idle_minutes` is NULL and the recipe offered a default. In this task, unreachable.
    Recipe,
    /// `idle_minutes` holds a duration.
    Row,
    /// `idle_minutes` is 0 — switched off here, whatever the recipe says.
    Never,
    /// `idle_minutes` is NULL and the recipe offered nothing. Every service, in this task.
    Unset,
}

pub enum IdleExemption {
    /// This service is depended on by one that is running.
    DependentRunning { service: ServiceId },
    /// A project is keeping it warm.
    ProjectKeptWarm { project: String },
}
```

`source` and `exempt` exist for `runtime.resolve`'s reason: the question is asked precisely when the
answer is surprising. "Why is this thing still running?" has four answers that look identical from
outside — no policy, policy switched off, a dependent, a keep-warm project — and a report that
collapsed them into `Option<IdlePolicy>` would send a person to change a setting that was never the
cause. This is `DnsStatus`'s rule from T46, applied to a smaller question.

`keep_warm` is a field on the existing `project.update` rather than a `project.keep_warm` method.
The method exists, it takes a partial update, and a whole RPC for one boolean is a surface nobody
asked for.

CLI:

```
mix service idle <id>                        # the report, both renderings
mix service idle <id> --after 30m            # a duration
mix service idle <id> --never                # 0
mix service idle <id> --default              # NULL
mix project keep-warm <name> [--off]
```

## How this is proved

Three tiers, because three different things are in doubt.

**The socket table, on fixtures and then for real.** Unit tests parse a captured `/proc/net/tcp` the
way `linux/ports.rs` already tests its own reader against two real rows. That proves the parsing and
proves nothing about the constant: whether `01` is the state a real established connection reports is
a question only a real connection answers. So one `#[ignore]`d system test — the same test, run by each of the three runners — opens a
listener, connects to it, asserts the count is one, drops the connection and asserts it is not one,
in the `system` job. This is the only part of the task CI is the first place to learn
about, and it is why it is written as a system test rather than trusted.

**The decision, against `mock::Host`.** `mock::Host` grows a settable connection count, and with it
the whole of the sweeper's judgement is testable with no socket, no process and no disk: `after` is
honoured in consecutive sweeps and not in elapsed time, a failed reading resets the counter, a
running dependent exempts, a keep-warm project exempts, a service with no policy is never a
candidate, an `HttpCounter` whose number moved is busy and one whose number did not is idle. This is
the largest tier and it runs in milliseconds.

**One real round.** `fakeservice`, an `idle_minutes` of one and a sweeper period shortened through
config: hold a connection and it stays running; drop it and it reaches `stopped` with
`StateReason::Idle` on the transition. One test, end to end, over a real socket — the shape every
phase here has closed with.

No benchmark. The idle-footprint budget is T72's, and this sweeper cannot be measured by it.

## What this task deliberately does not do

- **It does not start anything back up.** That is T70, and until it lands the feature is off by
  default for that reason and no other (D11).
- **It does not keep a project's databases warm** — only its PHP pool. The relationship is T77's
  (D9).
- **It does not sample CPU or RSS.** Idle here is measured in connections and counters, which is what
  the feature doc asks for. Per-process resource sampling and its 24-hour history are T71, and the
  macOS memory watchdog that needs that sampler is T71a.
- **It does not gate the build on an idle footprint.** T72.
- **It does not read a database's query counter, and `IdleProbe` grows no arm that could.** The
  roadmap line for this task says "connections, request counters, query counters", and the third of
  those is the one thing here that cannot be built as written. MariaDB publishes `Queries` through
  `SHOW GLOBAL STATUS` and nowhere else; PostgreSQL through `pg_stat_database`; both are read by
  speaking the database's own protocol as an authenticated user. A probe that could do it would have
  to carry a username and a password, and it lives on `ServiceSpec` —
  [ADR 0006](../../../.claude/decisions/0006-servicespec-in-proto-and-secret-free.md) is titled
  "`ServiceSpec` lives in `mixengine-proto` and never carries a secret" and says of that type "there
  is no field a password fits into". So `IdleProbe` stays at two arms. A database is measured by its
  established connections, which is a weaker signal in exactly one way — a client connected and
  idle looks busy — and that error is in the safe direction: it keeps a database running that could
  have been stopped, and never stops one somebody is holding open. Reaching the counter would mean
  the daemon reading the keyring and probing out-of-band, which is a mechanism, not a variant, and
  it is not worth building for a signal that only ever says *more* idle than we already conclude.
- **It does not add a `Connections` variant that reads a service's own status endpoint.** The feature
  doc says connections are counted "from the service's own status endpoint where available, otherwise
  the OS socket table"; `HttpCounter` is the first half of that sentence and a recipe that has a
  status endpoint uses it. A second probe that tries one and falls back to the other would hide which
  of the two answered, in the one report whose whole job is saying why.
