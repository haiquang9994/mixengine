# 0020. The published contract is the shape the daemon writes, not everything it accepts

**Status**: Accepted
**Date**: 2026-09-05

## Context

Since roadmap task **T56**, `mixengine-proto` is not only the single source of truth for the wire —
it is also the source of a **published** TypeScript contract: `bindings/` at the repository root,
committed, checked by CI's `bindings` job, and released as
`mixengine-api-<version>-typescript.tar.gz`
([design](../../docs/superpowers/specs/2026-09-05-t56-the-published-api-contract-design.md)).

A TypeScript type is **one** shape. Several types on this wire deliberately accept more than they
emit, and each leniency is argued where it lives:

- [`Millis`](../../crates/mixengine-proto/src/time.rs) deserialises `"10s"` as well as `10000`,
  because a duration in a hand-edited request is a thing a person writes.
- [`EnvValue`](../../crates/mixengine-proto/src/service.rs) accepts a bare string as well as its
  `#[serde(tag = "from")]` form, so a recipe's common case is not a two-key object.
- [`ErrorCode`](../../crates/mixengine-proto/src/error.rs) accepts a code this build has never heard
  of and answers `Internal`, because a client older than its daemon should still be able to read the
  message a person needs.

`ts-rs` derives from the attributes, so what it produces is the **serialising** shape. That was
going to be true whether or not anybody decided it, which is the reason to decide it.

## Decision

**The published contract states what the daemon writes, and the strict form of what it reads.** The
lenient alternatives are not in it.

Two guarantees follow, and they are the ones a client author needs:

- A client that **sends** the published shape is always understood.
- A client that **reads** the published shape always parses what arrives.

`bindings/README.md` says this to the client author in the same words, because a rule only this
repository knows is a rule that surprises somebody.

## Consequences

**Adding a hand-written `Deserialize` to `mixengine-proto` is now a decision about the contract as
well as about the wire.**
`crates/mixengine-proto/tests/bindings.rs::the_hand_written_deserialisers_are_the_ones_that_were_thought_about`
holds the list of the ten that exist and fails when an eleventh appears — pointing here, so that the
person who wrote it decides what the binding should say before the list is updated.

**Three types needed a shape written by hand**, because their serde carries no attribute for `ts-rs`
to read: `ErrorCode` is a literal union produced by `#[ts(rename_all = "snake_case")]` and tied to
`ErrorCode::as_str` by a test; `MetricsSubject` is `string`, its `"daemon"`/`"service:<id>"` grammar
staying in `MetricsSubject::parse`; `rpc::Version` is the literal `"2.0"`. The other seven are
`#[serde(transparent)]` newtypes whose derived `Serialize` already tells the truth.

**A leniency can still be added.** Nothing here forbids one — what it forbids is adding one and
leaving the published contract to describe it by accident.

## Alternatives considered

**Describe both shapes.** A union per lenient type — `MillisWire = number | string`, an `EnvValue`
that is also a bare `string` — doubles the size of the contract, and makes *every* client handle a
form no daemon ever sends, in exchange for an affordance only `mix` and a hand-edited JSON file use.
The cost lands on the readers, and the benefit lands on the writers, of whom there are two.

**Publish the deserialising shape instead.** Then reading a response would require narrowing a union
at every call site, which is the same cost in the direction that is used far more often: a client
reads the daemon's answers constantly and constructs a request occasionally.

**Say nothing and let `ts-rs` decide.** That is the status quo this record replaces, and it produces
the same files — the difference is that nobody would have known the rule, so the first person to add
a lenient `Deserialize` would have widened the wire and narrowed the contract in one commit without
noticing either.
