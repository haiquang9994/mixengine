# T88c — one rule for a growing `DaemonStatus`

**Date**: 2026-09-05
**Roadmap**: [T88c](../../../.claude/roadmap/phase-9-ship.md), Phase 9
**Status**: design

## The bug

A `mix` from a new build, talking to an older daemon that has not been restarted yet, **fails to
deserialise `daemon.status`**.

The handshake does not catch it, and cannot: `crates/mixengine-cli/src/client.rs` asks
`daemon.version` first and compares `ProtocolVersion` for equality. Both halves say `1`, so the
handshake passes and the client goes on to believe the answer. But `DaemonStatus` has grown two
**required** members since protocol 1 was frozen —

- `elevation: ElevationSummary` (T40b), and
- `dns: DnsStatus` (T44) —

and a daemon built before either of them sends neither. `serde` refuses the payload, and the command
that exists to explain a confusing machine is the one that dies on it.

`render::status` carries a note written for exactly this skew:

> `mix is 0.2.0 and this daemon is 0.1.0 — they speak the same protocol, so this is a daemon that
> has not been restarted since the upgrade`

It has a unit test and it is **unreachable in production**: the parse fails before anything is
rendered. Found while reviewing T44; T44 left it alone because fixing one field buys nothing while
the other is still required.

`update` (T88) is already optional and skew-tolerant, and its doc comment says out loud that it
deliberately does not add to this debt.

## The choice

Two rules were available, and the roadmap asks for **one rule for the whole struct** rather than a
decision per field.

### Rejected: bump `PROTOCOL_VERSION` whenever a required member appears

Coherent, and it would turn the parse failure into the handshake's typed `PreconditionFailed` — a
better error, delivered earlier, for every method rather than for this one. It is refused for two
reasons that are specific to this codebase.

**1. The number is shared with the `mixengine-elevate` handshake, where it means something else.**
`PROTOCOL_MINIMUM ..= PROTOCOL_VERSION` is the window an installed helper serves, and the helper is
**deliberately excluded from auto-update** — a daemon newer than the helper is the ordinary state of
a machine, not a fault (T88a). Bumping the ceiling because a status line gained a member drags that
window along for a reason that has nothing to do with `privileged::*`. The version number would come
to track *release count* rather than *incompatibility*, and a handshake whose number means "how many
releases ago" is a handshake nobody can reason about.

**2. It makes the remedy unreachable.** The handshake's hint reads *"stop the daemon so the new build
replaces the running one"*. `mix daemon stop` goes through the same handshake. A protocol bump would
have `mix` refuse the one command it has just told the user to run, leaving them to find the pid and
kill it by hand — which is precisely the class of dead end `mix status` exists to prevent.

### Chosen: every member added after a version is frozen is optional on the wire

`#[serde(default, skip_serializing_if = "Option::is_none")]` on an `Option<T>`, for `elevation` and
`dns` at once, and for every member added to any response type from here on. `PROTOCOL_VERSION` is
bumped for changes an older peer genuinely cannot survive — a member **removed**, a member's **type**
changed, a member's **meaning** changed, a method's contract changed — and adding one is not among
them.

This is what the crate already does three times over (`DaemonStatus::update`,
`DaemonShutdown::unordered`, `DnsStatus::because`), each with a test asserting both directions of the
skew. T88c stops the pattern being a habit and makes it the rule.

Because the decision is cross-cutting — it binds every wire type and every out-of-repo client — it
is recorded as **ADR 0019**, not as an edit to a doc comment.

## `Option`, and not `#[serde(default)]` on the value

`#[serde(default)]` on a non-`Option` member with a `Default` impl would keep every call site
unchanged. It is refused because **both defaults are lies**:

- `DnsStatus::default()` would claim hosts-only with no wildcards. That is a statement about whether
  `api.blog.test` resolves, made by a client, about a daemon that said nothing.
- `ElevationSummary::default()` would claim nothing is waiting for permission. *Degraded* is
  `pending != 0` and nothing else (the T40b design, D6), so a fabricated zero renders a **healthy
  machine that is missing its hosts entries** — the exact failure D6 exists to prevent.

`crates/mixengine-proto/src/daemon.rs`'s own header already settles this: facts are *absent* rather
than *present-and-empty*, because "a client that renders '0 services' before the concept exists is
showing a fact nobody established". A default that invents a fact is worse than the parse error it
replaces.

## What `None` means, and what it must never come to mean

**`None` means one thing: this daemon was built before the member existed.** It is a property of the
wire and not of the domain.

A current daemon always writes `Some`. `elevation` is read from a fallible source — `daemon.status`
has been fallible since T40b precisely so that a queue it cannot read fails the call rather than
reporting a number nobody established — so "could not determine" is already an `Error` and never a
`None`. `dns` is read from state this daemon owns and cannot fail.

That invariant is worth stating in the doc comments and worth a test, because the next person to
touch this file will otherwise find a convenient `Option` sitting there and reuse it for "unknown",
which reintroduces D6's failure through the door this change opened.

## Rendering

`render::status` is the only reader of either member in this repository (`status_json` passes
`DaemonStatus` through verbatim). Both elevation lines are already conditional, so absence is
naturally silent there; the `names` line is unconditional and would vanish without a word.

**Absent members are not printed, and the note names them.** No fabricated value, no placeholder line
that says nothing. The skew note — reachable again, which is the point of the whole task — grows a
second clause listing what the daemon did not report, in the order the missing lines would have
appeared:

```
mixengined 0.0.9 — running (pid 4123, up 13m 32s)
  home      /home/dev/.local/share/mixengine
  endpoint  /home/dev/.local/share/mixengine/run/mixengined.sock
  database  /home/dev/.local/share/mixengine/data/mixengine.db
  protocol  1
  note      mix is 0.1.0 and this daemon is 0.0.9 — they speak the same protocol, so this is a
            daemon that has not been restarted since the upgrade; it did not report how names
            resolve, or what is waiting for permission
```

The two clauses are independent conditions joined into one note rather than two `note` lines: a
status somebody reads daily earns at most one, and the second clause is the explanation of the first
in the only case both occur.

`mix status --json` needs no change and gets none — the daemon half is `DaemonStatus` verbatim, so an
absent member is absent from the JSON as well, which is the honest encoding of a fact nobody
reported.

## The wire does not change

A current daemon populates both members, and `skip_serializing_if` only elides `None`, so what
`mixengined` puts on the wire is **byte for byte what it puts there today**. Every integration test
reading `daemon["elevation"]["pending"]` stays green, and no client is asked to re-learn a shape.

That property is the reason this change is safe to make at ship time rather than one release later.

## Keeping the rule

A rule with no enforcement decays. Two tests carry it.

**A protocol-1 floor fixture.** A hand-written JSON object holding exactly the members `DaemonStatus`
was frozen with at protocol 1 — `version`, `protocol`, `pid`, `home`, `endpoint`, `database`,
`started_at`, `uptime` — decoded, and asserted to yield `None` for all three of `elevation`, `dns`
and `update`. Anyone adding a required member later turns this red in `mixengine-proto`, where the
rule lives, rather than in a CLI suite that would blame the command.

**A no-fabrication test in the renderer.** A status with both members absent prints neither line,
prints the note, and names both absences.

## What is deliberately not done

**Other response types are not retrofitted.** The rule binds them from here on; the existing ones are
left as they are, because the consequence is not the same. A method whose response gained a required
member fails one command — and an older daemon almost certainly answers `not_found` to that method
anyway, since the member and the method usually arrive together. `daemon.status` is the exception
worth spending a task on: it is the diagnostic every client makes on every connection, and it fails
exactly when a person is trying to find out why their machine is confusing them.

**`daemon.version` and `Health` are frozen and stay frozen.** They are the handshake — the one answer
that must decode for a client to learn it should not trust the rest — so they may never gain a
member, required or optional. `Health`'s test already pins its exact serialised bytes; the doc
comments say why that is deliberate rather than incidental.

**The handshake is not loosened.** Exact-equality on `ProtocolVersion` stays: this change is about
what one protocol version is allowed to contain, not about serving two of them.

## Files

| File | Change |
| --- | --- |
| `.claude/decisions/0019-*.md` | New ADR: an added member is optional; the protocol bumps for the rest |
| `crates/mixengine-proto/src/daemon.rs` | `elevation` and `dns` become `Option`; docs; floor fixture test |
| `crates/mixengine-proto/src/lib.rs` | `PROTOCOL_VERSION` doc states what does and does not bump it |
| `crates/mixengine-cli/src/render.rs` | `status` handles absence; the note names it; tests |
| `.claude/architecture/daemon-and-ipc.md` | The rule, under **Protocol**, pointing at the ADR |
| `.claude/roadmap/phase-9-ship.md` | Tick T88c |
