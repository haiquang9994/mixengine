# T56 — The published API contract (design)

Roadmap task **T56**, phase 9: *"Publish the API contract: `ts-rs` bindings generated from
`mixengine-proto`, committed, checked current by CI, and released as an artifact beside the
binaries."*

MixEngine ships no graphical client
([ADR 0011](../../../.claude/decisions/0011-no-gui-in-this-repository.md)), and that ADR's first
argument is that *"the API is the product surface"*: the JSON-RPC API and the TypeScript bindings
generated from `mixengine-proto` are **a released artifact, versioned like any other**. Today they
are neither generated nor released. A client in another repository can derive them for itself — the
crate is `serde`-only for exactly that reason — but what it cannot do is point at a file and say
*this is the contract version 0.1.0 answered to*.

The task waited until shipping deliberately, and its own sentence says why: nothing in this
workspace consumes the bindings, so maintaining them against a still-moving API is the speculative
work ADR 0011 withdrew a whole phase for. What changed is that the API has stopped moving — phases
0–8 are closed and phase 9 is a release — so the copy is now worth freezing.

`.github/workflows/ci.yml` has said the rest since T85, in its opening comment: the sixth job of
[build-and-release.md](../../../.claude/operations/build-and-release.md)'s table, `bindings`,
*"arrives with the work that gives it something to run — T56 — and until it does, a `ts-rs` type
whose committed output has drifted is caught by a person or by nobody."*

## Goal

A client author in another repository can:

1. read `bindings/` in this repository and see every request, response, event and error as a
   TypeScript type, with the crate's own prose carried across as TSDoc;
2. download one archive from a MixEngine release, verify it with `packaging/updates.pub`, and
   `npm install` it; and
3. trust that both are current, because a change to `mixengine-proto` that did not regenerate them
   is a red CI job rather than a discovery.

Nothing about the wire changes. This task adds no method, no field and no behaviour to the daemon.

## Measured, not assumed

Read on 2026-09-05 out of this tree and out of `ts-rs` 12.0.1, rather than reasoned about. Every
number below decided something further down.

1. **`mixengine-proto` declares 295 public types and no generic ones.** `grep '^pub struct\|^pub
   enum'`. The absence of generics removes the whole of `ts-rs`' hardest surface from this task.
2. **Exactly one type name occurs twice**: `doctor_api::Outcome` and `rpc::Outcome`. `ts-rs` names
   a file after the type, so those two are one file. See D9.
3. **Ten types have a hand-written `Serialize` or `Deserialize`** — `ErrorCode`, `ExtensionId`,
   `JobKind`, `MetricsSubject`, `Millis`, `PackageVersion`, `ServiceId`, `EnvValue`,
   `VersionConstraint`, `rpc::Version`. Seven of them are `#[serde(transparent)]` newtypes whose
   *serialising* half is derived, which `ts-rs` gets right on its own. Only three need a decision:
   D8.
4. **`u64` becomes `bigint`.** Measured, not read: `export type Millis = bigint;`. `JSON.parse`
   never produces a `bigint`, so the default mapping is a false statement about this wire. There is
   an environment variable for it and no cargo feature: `TS_RS_LARGE_INT` — D5.
5. **`serde(transparent)` and `serde(deny_unknown_fields)` are unsupported, and each prints a
   compiler note.** This crate carries 12 and 18 of them. The notes are **not** lint warnings:
   `cargo clippy --all-targets -- -D warnings` over a probe carrying both exited **0**. So this is
   signal hygiene and not a build failure — D6.
6. **Windows writes `/` in import paths.** The probe ran on this machine and emitted
   `import type { JsonValue } from "../serde_json/JsonValue";`. Generation is therefore
   OS-independent in the one place it could plausibly not have been.
7. **`ts-rs` serialises its writes and de-duplicates them.** `export::export_and_merge` takes a
   process-global `Mutex`, `File::create`s a path the process has not written yet, and **merges**
   into one it has. Two consequences: the generated `#[test]`s cannot tear a shared file, and a
   file two types both claim would carry both declarations in test-execution order — which is D9's
   second reason.
8. **The shapes this crate actually uses all survive.** Measured on a probe carrying one of each:
   internally tagged enums with `rename_all` and per-variant `rename`, `untagged`, `flatten` (merged
   inline), `serde_json::Map`, `BTreeMap`, newtypes over newtypes, and
   `#[serde(default, skip_serializing_if = "Option::is_none")]` → `note?: string | null`. Every
   `skip_serializing_if` field in this crate carries `default` beside it, which `ts-rs` requires
   before it will honour the first.
9. **Doc comments come across as TSDoc.** `/** … */` above every declaration, verbatim. No `///`
   line in this crate contains `*/`, so none of them can close a comment early.
10. **The `lint` job compiles every feature.** `cargo clippy --workspace --all-targets --all-features`
    and `cargo sqlx prepare --workspace --check -- --all-targets --all-features`. So a feature added
    here is linted whether or not a job asks for it — and `cargo deny` runs `--all-features` too,
    which is what puts `ts-rs` in the licence and duplicate-version graph.
11. **The `test` job runs `cargo test --workspace --all-targets --all-features` on all three
    operating systems.** Which means the generator *runs* on all three, every CI run, once this
    lands. D3 turns that from a side effect into the thing that makes it safe.

## Scope

**In.** A `ts` feature on `mixengine-proto`; the derive on every wire type; a generated, committed
`bindings/`; `packaging/bindings.sh` (generate, check, pack); a `bindings` CI job; the archive in
`release`; a test that ties the derive set to the type set; the one rename D9 needs; and the
documents that describe the six-job table, the packaging directory and the client surface.

**Out.** Any change to the wire. Any client. A second target language. Publishing to a registry —
the artifact is a file on a release page, and who pushes it to npm is a decision for whoever owns
that namespace, not for this task. Making `mix` consume the bindings, which would be business logic
in a client.

## The shape of the answer

```
mixengine-proto (every wire type: #[cfg_attr(feature = "ts", derive(TS), ts(export))])
        │
        │  cargo test -p mixengine-proto --features ts --lib
        │  TS_RS_EXPORT_DIR + TS_RS_LARGE_INT from .cargo/config.toml
        ▼
bindings/                     ← committed, entirely generated
  index.ts                    ← the barrel, written by packaging/bindings.sh
  README.md                   ← what this is, written by packaging/bindings.sh
  DaemonStatus.ts  …one per type ← written by ts-rs
  serde_json/JsonValue.ts     ← written by ts-rs, for `params` and `result`
        │
        ├─ CI job `bindings`: bindings.sh --check   (regenerate into a temp dir, diff -r)
        │
        └─ CI job `release`:  bindings.sh --pack    (+ package.json, + licences)
                                    ▼
              target/packaging/dist/mixengine-api-<version>-typescript.tar.gz
                                    ▼
                              packaging/sign.sh → .minisig, beside the binaries
```

## Decisions

### D1 — `ts-rs` is optional, behind a `ts` feature, off by default

`crates/mixengine-proto/src/lib.rs` opens with *"it is `serde`-only on purpose — no I/O, no platform
code, no domain logic — so that a client can depend on it without pulling in the daemon's world"*.
`mixengine-proto` is in **every** binary this project ships, `mixengine-elevate` included, and that
one's dependency closure is a security decision the `lint` job counts
(`.github/elevate-dependencies.txt`).

So: `ts-rs = { workspace = true, optional = true }` and `[features] ts = ["dep:ts-rs"]`. Nothing a
user runs compiles it. The sentence in `lib.rs` stops being an aspiration and starts being a
manifest, which is the better version of it.

Features taken: **`serde-compat`** (the default, and the whole reason this works — the wire shape is
written in `#[serde(…)]` attributes and nowhere else), **`serde-json-impl`** (D7), and
**`no-serde-warnings`** (D6). Deliberately not `format`: it is a `dprint` dependency for
whitespace, and `ts-rs`' own output is already one declaration per line.

### D2 — Every wire type carries the derive, and a test says which types those are

One line above each, inside a `cfg_attr` so the attribute vanishes with the feature:

```rust
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
```

`#[ts(export)]` makes `ts-rs` generate a `#[test]` per type that writes that type's file. **The
alternative was `TS::export_all_to` from an example**, called on a set of root types and reaching
the rest transitively — and it was refused for the reason this task exists: a root list is a second
list of what the contract contains, maintained by hand, drifting silently, which is
[T85c](2026-09-05-t85c-the-shim-in-every-artifact-design.md) in another file. `#[ts(export)]` has no
list. What replaces the list is a test that reads the source
(`crates/mixengine-proto/tests/bindings.rs`, see *Testing*): every `pub struct` and `pub enum` in
`src/` has a file in `bindings/`, or is named in a short allow-list with a reason.

**The consequence is worth stating rather than discovering**: `cargo test --workspace --all-features`
regenerates the committed bindings as a side effect, on whichever operating system runs it. That is
the mechanism and not an accident — it is what keeps a developer's tree honest between the moment
they add a field and the moment CI tells them, and measurement 7 says the writes cannot tear.

### D3 — `bindings/` sits at the repository root and is *entirely* generated

At the root rather than under `crates/mixengine-proto/`, because it is a product artifact and not a
crate internal: the reader it exists for does not have this workspace open, and the archive of it is
published beside the binaries.

**Entirely generated** — the barrel and the README included — because that is what makes the check a
plain `diff -r` between the committed tree and a fresh one, with no file to exclude and no rule to
remember. Regeneration begins with `rm -rf bindings`, so a type that was deleted takes its file with
it; `ts-rs` alone would leave it there for ever.

One thing is deliberately **not** in the committed tree: `package.json`. It carries the version, and
[build-and-release.md](../../../.claude/operations/build-and-release.md) says *"cutting a release is
a version bump and nothing else"*. A committed, versioned `package.json` would make that false — a
bump would leave the `bindings` job red until somebody regenerated. So the version is stamped at
**pack** time, into the archive, where a version belongs. D12.

### D4 — The generator's configuration lives in `.cargo/config.toml`, not in the script

```toml
[env]
TS_RS_EXPORT_DIR = { value = "bindings", relative = true }
TS_RS_LARGE_INT = "number"
```

`ts-rs` reads both at run time, so a developer who types the obvious command —
`cargo test -p mixengine-proto --features ts` — gets *the committed answer* rather than a stray
`crates/mixengine-proto/bindings/` full of `bigint`. A script that exported the variables itself
would be a second way of being right and a first way of being wrong.

Cargo's `[env]` does not override a variable already set unless `force = true`, which is what lets
`bindings.sh --check` point one run at a temporary directory. `relative = true` resolves against the
directory holding `.cargo/`, so it is the repository root whatever the working directory is.

This is the first `.cargo/config.toml` in this repository. It sets two variables and nothing else —
no flags, no target, no registry — so it cannot quietly change how anything is built.

### D5 — `TS_RS_LARGE_INT = "number"`

`ts-rs` maps `i64`, `u64`, `i128` and `u128` to `bigint` by default, which is right for a codec that
can produce one and wrong for this one. A client receives these values from `JSON.parse`, which
produces a `number` for every JSON number there has ever been; a binding that said `bigint` would
fail to type-check against the value it actually describes.

**It is lossy above 2⁵³ and that is checked rather than waved at**: the `u64`s on this wire are
milliseconds (`Millis`, `Uptime`), a Unix millisecond timestamp, byte counts of downloads and of
resident memory, and counts of things on one machine. The largest of them reaches 2⁵³ in about
285,000 years. The daemon writes a JSON number in every one of those places regardless of what a
binding claims, so `number` is the truthful reading and `bigint` was the fiction.

### D6 — `no-serde-warnings`, because 30 notes per compile is a lint signal spent on nothing

`ts-rs` prints a note for every serde attribute it does not model. This crate has 18
`deny_unknown_fields` and 12 `transparent`, and measurement 10 says three CI steps compile it with
`--all-features`. Neither attribute has a TypeScript meaning — `deny_unknown_fields` is a
deserialiser's strictness and `transparent` is what `ts-rs` does to newtypes anyway (measured:
`export type Name = string`), so nothing is lost by silencing them.

`deny.toml` explains this repository's rule for exactly this case, one level up: *"a warning in a CI
log is a warning nobody opens"*. Thirty notes on every `lint` run is how the one that mattered would
be missed.

**Measured before it was silenced**, because it changes the argument: with `-D warnings` these do
not fail a build (measurement 5). Had they, this decision would have been forced rather than chosen,
and the right answer might have been to widen `ts-rs` instead.

### D7 — `serde-json-impl`, and the `serde_json/` directory it brings

Five field positions in this crate hold a `serde_json::Value` or a `serde_json::Map`:
`rpc::Request::params`, `rpc::ResponseOutcome::Success::result`, `JobOutcome::…::result`,
`PrivilegedRequest::ops` and `ServiceCreate::overrides`. The feature gives them
`JsonValue = number | string | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null`,
which is a precise and *usable* recursive type.

The alternative was `#[ts(type = "unknown")]` on those five fields, which needs no feature and no
extra file. It was refused: `unknown` puts a cast at every client call site, and a JSON-RPC `params`
member genuinely is a JSON value rather than a mystery.

The cost is that `ts-rs` fixes that type's path, so the published tree carries
`bindings/serde_json/JsonValue.ts` — one directory named after a Rust crate inside a TypeScript
package. Accepted and left alone: the name says where the type came from, and the barrel re-exports
it flat so no client has to know.

### D8 — Three of the ten hand-written serde impls need a line; seven need nothing

`ts-rs` reads attributes, not `impl` blocks. Seven of the ten are `#[serde(transparent)]` newtypes
whose *serialising* half is derived, so the attributes tell the truth and the derive is enough:
`ExtensionId → ServiceId`, `JobKind → string`, `Millis → number`, `ServiceId → string`,
`PackageVersion → string`, `VersionConstraint → string`, and `EnvValue`, whose
`#[serde(tag = "from")]` is a derived `Serialize` this crate already writes.

The three with no serde attribute at all:

| Type | Attribute | Yields |
| --- | --- | --- |
| `ErrorCode` | `#[cfg_attr(feature = "ts", ts(rename_all = "snake_case"))]` | `"not_found" \| "elevation_required" \| …` |
| `MetricsSubject` | `#[cfg_attr(feature = "ts", ts(as = "String"))]` | `string` |
| `rpc::Version` | `#[cfg_attr(feature = "ts", ts(type = "\"2.0\""))]` | `"2.0"` |

`ErrorCode` gets a **literal union rather than `string`**, because a closed set of codes is the most
useful thing this contract can hand a client and the crate already treats it as closed. That makes
`ts-rs`' own `rename_all` responsible for reproducing `ErrorCode::as_str()`, which is a second
spelling of the same list — so a test asserts every `ErrorCode::ALL` wire string appears as a
literal in the generated file, and that the file holds no others. `MetricsSubject` is
`"daemon" | "service:<id>"`, which a TypeScript template literal could express and a `String` says
honestly; the parsing rule is `MetricsSubject::parse`'s and stays there.

### D9 — `rpc::Outcome` becomes `rpc::ResponseOutcome`

Measurement 2: two types named `Outcome`. Measurement 7: `ts-rs` would write both into `Outcome.ts`,
the second *merging* into the first, in whatever order the test harness ran them — so the published
contract would carry two `export type Outcome` declarations in a file that is not valid TypeScript,
and would carry them differently on different runs.

The fix could have been `#[ts(rename = "RpcOutcome")]`, and is not, for two reasons. A TypeScript
name that no Rust name matches is a contract a client cannot grep back into this repository. And a
rename attribute leaves the *next* collision to be discovered the same way, whereas an invariant —
**type names are unique across `mixengine-proto`** — can be asserted once by a test and hold for
every type added afterwards.

`ResponseOutcome` is what it is: *"the half of a `Response` that is either a result or an error,
never both"*. Four call sites outside `rpc.rs`, all in this workspace, none on the wire — the name
is a Rust identifier and the JSON is `#[serde(untagged)]`, so nothing a client sees changes.

### D10 — The contract describes what the daemon **writes**

A binding is one shape. Several types on this wire accept more than they emit: `Millis`
deserialises `"10s"` as well as `10000`, `EnvValue` accepts a bare string as well as its tagged
form, `ErrorCode` accepts a code this build never heard of and answers `Internal`. Every one of
those leniencies is deliberate and documented where it lives.

The published contract states the **serialising** shape, which is what a client reads from the
daemon and the strict form of what it may send. The lenient alternatives are not in it. This is a
rule for the whole crate rather than a note about three types, so it is
[ADR 0020](../../../.claude/decisions/0020-the-published-contract-is-the-shape-the-daemon-writes.md)
— and the test in *Testing* enumerates the hand-written `Deserialize` impls so a fourth cannot be
added without somebody reading that ADR.

### D11 — One script, three verbs

`packaging/bindings.sh`, beside the other things a release is made of:

```bash
bash packaging/bindings.sh            # regenerate bindings/ in place
bash packaging/bindings.sh --check    # regenerate into a temp dir and diff; writes nothing
bash packaging/bindings.sh --pack     # archive the committed bindings/ into dist; runs no cargo
```

`--check` regenerates **into a temporary directory** and `diff -r`s it against `bindings/`, rather
than running the generator in place and asking `git diff --exit-code`. Two reasons: it leaves the
checkout untouched, so a red job is a message and not also a dirty tree; and it does not depend on
the checkout being a git repository at all, which is what lets the same command answer on a machine
that unpacked a tarball.

`--pack` runs no cargo and reads only the committed tree. What guarantees that tree is current is
that `release` needs `bindings` — D13 — rather than a second regeneration inside the release job,
which would be a *third* place the answer could be computed.

### D12 — `mixengine-api-<version>-typescript.tar.gz`, packed from the committed tree

One archive, named so that nothing else in `packaging/` claims it: `feed.sh` matches payloads as
`mixengine-<version>-<os>-…` and helpers as `mixengine-elevate-<version>-…`, and this is neither, so
it is not offered to `mix self-update` and not listed as a helper. `sign.sh` signs everything in the
distribution directory that is not a `.sha256` or a `.minisig`, so it is signed with the binaries
and verified back against the key this build pins, for free.

Inside, alongside the committed `.ts` files and the barrel:

- **`package.json`** — `@mixengine/api`, the workspace version, `"types": "./index.ts"`, no `main`.
  The package is **type-only**: there is not one line of runtime code in it, so an entry point that
  ran would be a lie about what it is.
- **`LICENSE-MIT`** and **`LICENSE-APACHE`**, copied from the root. A published package with no
  licence in it is one nobody in an organisation is allowed to install.

And it ends by **opening what it just made** — the rule every other script in `packaging/` follows,
for the reason `README.md` gives: *"an empty archive is a perfectly valid archive, and nothing else
in the pipeline would notice."* It asserts `package.json`, `index.ts`, both licences, and the same
count of `.ts` files as `bindings/` holds.

### D13 — A `bindings` job on ubuntu, and `release` needs it

The sixth row of the table in
[build-and-release.md](../../../.claude/operations/build-and-release.md), written down since T85 and
empty until now. Ubuntu alone: generation is OS-independent (measurement 6) and the `test` job
already runs the generator on all three (measurement 11), so a second and third runner here would
re-measure one behaviour at twice the cost — the reasoning T86a's `windows-latest` probe uses one
job along.

Three steps, in this order:

1. `bash packaging/bindings.sh --check` — the gate the row describes.
2. `cargo clippy -p mixengine-proto --features ts --locked -- -D warnings`. Redundant with `lint`'s
   `--all-features` today, and kept anyway: this job is where somebody looks when the contract is
   the thing that broke, and it costs nothing on a crate this job has already compiled.
3. `bash packaging/bindings.sh --pack` — so the packing path runs on **every** CI run and not for
   the first time during a release. That is the rule `lint` already applies to `sign.sh` and
   `feed.sh`: *"the only other thing that would ever run it is a release."*

`release` gains `bindings` in its `needs`. Without it a tag whose committed contract had drifted
would publish the drift, signed, which is worse than publishing nothing.

## Testing

**`crates/mixengine-proto/tests/bindings.rs`** — no feature, every operating system, reading the
committed tree and the crate's own source. This is [T85c's D7](2026-09-05-t85c-the-shim-in-every-artifact-design.md)
applied to a second list-that-nothing-forces-to-agree.

1. *Every public type is in the contract.* Every `pub struct` / `pub enum` in `src/` has
   `bindings/<Name>.ts`, or is in `NOT_ON_THE_WIRE` with a one-line reason —
   `ServiceSpecBuilder`, `SpecError`, `VersionError` and their like, which are Rust ergonomics
   rather than wire types. **A set comparison and not a subset**, in both directions, so a type
   removed from the crate and left in `bindings/` fails too.
2. *Type names are unique.* D9's invariant, asserted rather than remembered.
3. *One declaration per file.* Every `bindings/**/*.ts` but the barrel holds exactly one
   `export type` — which is what a merged collision would break, and the second half of the reason
   D9 renames rather than aliases.
4. *`ErrorCode` is a complete literal union.* Every `ErrorCode::ALL` wire string appears in
   `ErrorCode.ts`, and the file names no code that is not one — D8's second spelling, tied to the
   first.
5. *The hand-written deserialisers are the ones ADR 0020 knows about.* The source is scanned for
   `impl<'de> Deserialize<'de> for …` and the set compared with a written list. A new one is a test
   failure whose message points at the ADR, because it is the point where somebody decides what the
   contract will and will not say.

**`packaging/bindings.sh --check`**, in the `bindings` job: the content itself, byte for byte.
Tests 1–5 above answer *"is every type present and unambiguous"*; only regeneration answers
*"does the file say what the type says"*.

**`packaging/bindings.sh --pack`**, in the same job on every run and in `release`: the archive is
opened and its contents asserted, per D12.

**Nothing type-checks the TypeScript.** There is no frontend toolchain in this repository and
[ADR 0011](../../../.claude/decisions/0011-no-gui-in-this-repository.md) is why; installing one to
run `tsc --noEmit` over generated output would be a toolchain, a lock file and a version to maintain
for a check on somebody else's code generator. What stands in its place is that the output is
`ts-rs`' own and that this crate uses none of the shapes where `ts-rs` is known to need help
(measurement 8, and no generics at all). **It is a real gap and it is named here rather than
implied**: the first thing a client repository should do is compile these, and the first bug it
finds belongs back in this file.

## Risks, and where each is answered

- **`cargo deny` gains `ts-rs` in an `--all-features` graph.** MIT, and its only non-optional
  dependencies are `thiserror` 2 and `ts-rs-macros`, both of whose subtrees this workspace already
  carries. A duplicate would be a red `lint`, which is the check working.
- **`.github/elevate-dependencies.txt` moves.** It must not: `ts` is off by default and
  `mixengine-elevate` takes `mixengine-proto` plainly. The `lint` job diffs that file on every run,
  so this is asserted rather than assumed.
- **The `test` job dirties the checkout.** D2. Byte-identical to what is committed whenever
  `bindings` is green, and nothing in `test` reads the tree's cleanliness.
- **A doc comment containing `*/` would close a TSDoc block early.** None exists today
  (measurement 9); the day one does, the generated file stops being valid TypeScript and no test in
  this repository notices. Left open on purpose — see *What this leaves*.
- **274 files in every diff that touches proto.** The cost of the word *committed* in the task's own
  sentence. It is also the benefit: a reviewer sees the contract change beside the field that
  changed it.
- **`Millis` says `number` and accepts `"10s"`.** D10, ADR 0020, and test 5.
- **A version bump reddening the `bindings` job.** D3 and D12 — the version is not in the committed
  tree.

## What this leaves

- **Nothing type-checks the generated TypeScript in this repository.** Argued in *Testing* rather
  than skipped quietly. The cheapest honest fix is a client repository that compiles them and says
  so; the second cheapest is a `bindings` job step that installs a Node toolchain, which is a
  standing maintenance cost this repository has never paid and should not start paying for a check
  with no consumer.
- **The lenient deserialisers are not described.** ADR 0020 makes that a rule instead of an
  omission, and test 5 makes adding a fourth a deliberate act. Describing both shapes would mean two
  types per request — `MillisWire = number | string` and friends — which is a contract twice the
  size for a leniency only `mix` and a hand-written JSON file use.
- **`PROTOCOL_VERSION` is not exported.** It is a constant, and a type-only package cannot carry a
  runtime value without becoming something that has to be built. A client learns the protocol from
  the handshake, which is where it has to learn it anyway — a number frozen into a binding would be
  the version the bindings were *generated* from, and a client that trusted it would be trusting the
  wrong end of the connection.
- **Nothing publishes to npm.** D12's archive is installable from a URL, which is what a release page
  is for. A registry name is an account, an owner and a rotation policy, and none of those are this
  task's to decide.
