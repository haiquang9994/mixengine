# 0019. A member added to a response is optional on the wire, and the protocol does not bump for it

**Status**: Accepted
**Date**: 2026-09-05

## Context

[`PROTOCOL_VERSION`](../../crates/mixengine-proto/src/lib.rs) says *"bump it when a change is not
backwards compatible for an older peer"* and stops there. It does not say whether **adding a member
to a response** is such a change, and the answer was decided three times by three different tasks
without anyone writing it down.

Twice the member was made optional — `DaemonShutdown::unordered`, `DnsStatus::because` — and each
time with a test asserting both directions of the skew. Twice it was made **required**:
`DaemonStatus::elevation` (T40b) and `DaemonStatus::dns` (T44), both after protocol 1 was frozen.

The second pair is a bug, and roadmap task **T88c** is where it was found. The handshake compares
`ProtocolVersion` for equality on `daemon.version`; both halves say `1`, so a `mix` from a new build
goes on to ask an older daemon — one that self-update replaced the binaries of but that has not been
restarted yet — for its status, and **fails to deserialise the answer**. `render::status` carries a
note written for exactly that skew, with a test, and the parse fails before it renders. Fixing one of
the two members buys nothing while the other is still required, which is why T44 left it alone.

Nothing has shipped yet, so this costs a decision rather than a migration. What it must not cost is
the same decision again in six months.

## Decision

**Every member added to a response type after a protocol version is frozen is optional on the wire:**
`#[serde(default, skip_serializing_if = "Option::is_none")]` on an `Option<T>`. A peer that predates
the member sends nothing and decodes as `None`; a peer that has it sends the member, so the encoding
is unchanged for everyone current.

**`PROTOCOL_VERSION` bumps for the changes an older peer genuinely cannot survive** — a member
removed, a member's type changed, a member's meaning changed, a method's contract changed — and
adding one is not among them.

**`DaemonVersion` and `Health` are frozen and gain no member at all**, optional or otherwise. They are
what a client reads *before* it has learned whether to trust anything else, so the one answer that
must always decode is the one that never changes shape.

## Consequences

**A new client can always read an old daemon**, which is the state a self-updating product spends
every upgrade in: the binaries are replaced and the daemon has not been restarted. `mix status` —
the command a confused person types — keeps working and explains the skew instead of dying on it.

**`None` is a wire fact, not a domain fact.** It means *"this peer was built before the member
existed"* and it must never come to mean *"could not determine"*: `daemon.status` has been fallible
since T40b precisely so that an unreadable elevation queue fails the call rather than reporting a
number nobody established, and re-spending `None` on it would reintroduce that failure through the
door this rule opened. Every optional member's doc comment says which it is.

**Responses accumulate `Option`s**, and every client pays a branch per member. That is the cost, and
it is the smaller one: the alternative is a client that cannot render the member *or anything else*.

**A default value is not a substitute for the `Option`.** `#[serde(default)]` on the value would keep
call sites unchanged and make a client state a fact nobody reported — `DnsStatus::default()` claims
`api.blog.test` does not resolve, `ElevationSummary::default()` claims a machine missing its hosts
entries is healthy. `mixengine-proto`'s own header already refuses that shape: facts are absent
rather than present-and-empty.

**The rule needs a guard or it decays.** Each response type that has grown members carries a floor
fixture — the frozen member set as hand-written JSON, decoded, asserting `None` for everything added
since. Adding a required member turns it red in `mixengine-proto`, where the rule lives.

## Alternatives considered

**Bump `PROTOCOL_VERSION` whenever a required member appears.** Coherent, and it would turn the parse
failure into the handshake's typed `PreconditionFailed` — a better error, delivered earlier, for
every method rather than one. It lost for two reasons specific to this codebase.

*The number is shared with the `mixengine-elevate` handshake, where it means something else.*
`PROTOCOL_MINIMUM ..= PROTOCOL_VERSION` is the window an installed helper serves, and the helper is
deliberately excluded from auto-update (T88a) — a daemon newer than the helper is the ordinary state
of a machine, not a fault. Bumping the ceiling because a status line gained a member drags that
window along for a reason that has nothing to do with `privileged::*`, until the number tracks
release count rather than incompatibility.

*It makes the remedy unreachable.* The handshake's hint reads *"stop the daemon so the new build
replaces the running one"*, and `mix daemon stop` goes through that same handshake. A bump would have
`mix` refuse the one command it just told somebody to run, leaving them to find a pid and kill it by
hand — the dead end `mix status` exists to prevent.

**Widen the handshake to accept a range of protocol versions.** Serves two protocols instead of
deciding what one may contain, and every method body would then have to know which peer it is talking
to. That is the per-field decision this ADR exists to replace, moved into the daemon.

**Leave it and document the skew.** What the note in `render::status` already did, unreachably.
