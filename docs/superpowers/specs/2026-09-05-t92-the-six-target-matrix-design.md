# T92 — The six-target matrix (design)

Roadmap task **T92**, phase 9: *"Public beta: the packaging pipeline running for all runtimes across
six OS/arch targets"*, citing
[runtime-packaging.md](../../../.claude/operations/runtime-packaging.md).

It is the last unticked entry before **milestone M9 — v0.1.0**, and the only entry in the phase whose
subject is not in this repository at all: the pipeline is
[`mixengine-packages`](https://github.com/mixnz/mixengine-packages), which releases on its own clock
and carries its own build plan. So the task reduces to two questions this repository can answer and
nobody else can: **is the sentence true**, and **does the daemon behave correctly where it is not**.

## Goal

A person who installs v0.1.0 on any of the six targets MixEngine ships a build for can install every
runtime and every service this build knows how to run — or is told, in a sentence naming the reason,
why one of them is not on offer for their machine. And a hole that opens later fails a check rather
than being discovered by a user.

## Measured, not assumed

Read on 2026-09-05 out of the published document and out of this tree, rather than reasoned about.

1. **The published index is reachable and current.**
   `https://github.com/mixnz/mixengine-packages/releases/download/index/index.json` —
   `schema: 1`, `generated_at: 2026-08-31T07:40:07Z`, **60 packages**, **318 artifacts**, every
   package on channel `stable`.
2. **Eleven kinds, and they are exactly the eleven this build can install.** `caddy` 5, `mariadb` 5,
   `memcached` 1, `mysql` 5, `nginx` 6, `node` 5, `php` 11, `postgres` 5, `python` 5, `redis` 8,
   `ruby` 4. Against this build: `RuntimeKind::ALL` is `php`, `node`, `python`, `ruby`, and
   `Catalogue::builtin()` is `caddy`, `memcached`, `mariadb`, `mysql`, `nginx`, `php-fpm`,
   `postgres`, `redis` — of which `php-fpm` is not a package, by
   [`recipes.rs`](../../../crates/mixengine-core/src/generate/recipes.rs)' own note
   (*"one of them does not come out of a package"*). Four plus seven is eleven, with nothing on
   either side the other does not have.
3. **Artifacts per target, and this is the finding.**

   | target | artifacts, of 60 |
   | --- | --- |
   | `linux/x86_64` | 60 |
   | `linux/aarch64` | 60 |
   | `macos/x86_64` | 60 |
   | `macos/aarch64` | 60 |
   | `windows/x86_64` | 59 |
   | **`windows/aarch64`** | **19** |

4. **The one missing `windows/x86_64` cell is `redis 7.2.15`**, which has no Windows artifact at
   all — the packaging repository's **P12b**, *"Redis 7.2 builds on Windows and cannot start
   there"*. It is a truthful absence and the only one on that target.
5. **The 41 empty `windows/aarch64` cells are six whole kinds and part of three more.** Nothing at
   all for `memcached`, `mysql`, `nginx`, `php`, `postgres`, `redis`; `node` has 3 of 5 (16 and 18
   predate upstream's first `win-arm64`), `python` 4 of 5 (3.10 has none), `ruby` 2 of 4
   (RubyInstaller's first ARM64 archive is in the 3.4 line). What exists there in full is `caddy`
   and `mariadb`.
6. **Forty of those forty-one have a `windows/x86_64` twin.** The exception is `redis 7.2.15`, from
   finding 4.
7. **Two documents in this repository already say what should happen there, and no code does it.**
   - [runtime-packaging.md:741](../../../.claude/operations/runtime-packaging.md) — *"MixEngine
     itself targets `aarch64-pc-windows-msvc`, so a Windows-on-ARM machine runs the daemon natively
     and **PHP under emulation**."*
   - [index/format.rs:236](../../../crates/mixengine-core/src/index/format.rs) and
     [daemon/runtimes.rs:805](../../../crates/mixengine-daemon/src/runtimes.rs) both say an x86_64
     build under emulation should install x86_64 artifacts — which is the *other* direction, and
     true, and not this one.

   What `Index::artifact` and `offered()` actually do is match `artifact.arch == Arch::host()`
   exactly. So on the `aarch64-pc-windows-msvc` build T85a ships, `mix runtime install php 8.3.33`
   answers **`unsupported_platform: php 8.3.33 is not published for this machine`**, and
   `mix runtime available --kind php` prints **`the package index offers nothing for this machine`**
   — for the one runtime this product exists for.
8. **The packaging repository's own plan says the pipeline is finished but for one cell.** Its
   `docs/roadmap.md` has thirty-one entries and exactly one is open: **P7c**, *"PostgreSQL on
   Windows/ARM64, when 19 makes it possible"*. Everything else — P1–P6b, P7–P7b, P8–P8b, P9, P10–P14
   — is `[x]`, and *"every kind here is now published"*.
9. **`runtime-packaging.md` is stale in three places**, which finding 8 is what shows:
   - its *"Still open"* table lists PostgreSQL, Redis-on-Windows and nginx as cells nobody has
     checked; all three are packed and published (P7/P7a/P7b, P8/P8a/P8b, P9).
   - it has **no row for MySQL** (P14, five versions published 2026-08-20) and **none for
     Memcached** (P8).
   - its Ruby paragraph says *"twenty-five packages and one hundred and thirty-four artifacts"*,
     against sixty and three hundred and eighteen today.
10. **The published index carries a `requires.tzdata` this build cannot read.** Ten artifacts —
    every Linux PostgreSQL cell — state it, and it is prose rather than a version: *"the system
    timezone database at /usr/share/zoneinfo — Debian builds PostgreSQL `--with-system-tzdata`, so
    unlike the Windows and macOS cells this one does not carry its own"*.
    [`Requires`](../../../crates/mixengine-core/src/index/format.rs) has `vcredist`, `macos` and
    `glibc` and nothing else. It parses — the module is deliberately not
    `deny_unknown_fields` — and the sentence is dropped.
11. **Nothing reads `requires` at all.** `grep` finds no consumer of `Requires` outside the format
    module and one `Requires::default()` in an extension fixture. Its doc comment says
    *"Preconditions the daemon checks before installing, and prompts about rather than silently
    satisfying"*, which is not true of any of the three fields;
    [`install.rs`](../../../crates/mixengine-core/src/install.rs)' `SmokeTest` note is where the
    real mechanism is written down — *"every failure the `requires` field describes … is invisible
    until something tries"*.
12. **`test` runs on `ubuntu-latest`, `windows-latest`, `macos-latest` and no ARM runner.** The two
    ARM legs (`windows-11-arm`, `ubuntu-24.04-arm`) exist only in `build`. Anything whose behaviour
    depends on the *host* being ARM64 Windows has no CI that reaches it.
13. **Nothing in this repository has ever parsed the real published index.** Every index test builds
    its own JSON and serves it through `MockRegistry`.

## What the sentence turned out to mean

*"The packaging pipeline running for all runtimes across six OS/arch targets"* is **true on five
targets and 32% true on the sixth**, and this repository cannot make the sixth one true: there is no
upstream ARM64 Windows PHP in any branch, no ARM64 Windows nginx, and P7c waits on a PostgreSQL
release that does not exist yet.

What it *can* do is the thing its own documentation already promised and never built. Every one of
those forty cells has an x86_64 build beside it, and Windows 11 on ARM runs an x86_64 user-mode
process under the operating system's own emulation. So the task is not to fill the cells. **It is to
stop the client pretending the cells are the only answer**, and to say out loud when the answer is
the emulated one.

That inverts nothing the packaging repository decided. Its rule — *"a branch that will not build
natively for an architecture is a cell the index does without, which is a truthful 'not available'
rather than an artifact that silently emulates"* — is about what an **index** may claim, and it
stays exactly as it is: no artifact is relabelled, no `arch` field is a lie. The emulation decision
moves to the only place that can make it honestly, which is the **client that knows what machine it
is on** and can name what it is about to do.

## Design

### D1 — A target is a value, and the host is one case of it

`index::format` gains:

```rust
pub struct Target { pub os: Os, pub arch: Arch }

impl Target {
    pub const fn new(os: Os, arch: Arch) -> Self;
    pub fn host() -> Option<Self>;
    /// Every target whose artifacts this one can execute, most preferred first.
    pub const fn runnable(self) -> &'static [Target];
}
```

`runnable` is a `match` over six statics. Five of them are one element long. The sixth is

```rust
const WINDOWS_ON_ARM: [Target; 2] = [
    Target::new(Os::Windows, Arch::Aarch64),
    Target::new(Os::Windows, Arch::X86_64),
];
```

**Native is first and the order is load-bearing**, which is why the search iterates targets outside
and artifacts inside rather than the other way round: a package with both Windows cells would
otherwise be resolved by whichever artifact the generator happened to write first.

**`Arch::host()` is not touched.** It answers *"what did this build compile for"*, and
[`updates/feed.rs`](../../../crates/mixengine-core/src/updates/feed.rs) is the other caller — a
daemon that self-updates must find the aarch64 MixEngine, not an x86_64 one. Making that function
generous would make `mix self-update` wrong on the same machine this task exists to fix.

### D2 — Selection replaces "find the artifact whose arch equals ours"

```rust
pub struct Selection<'a> { pub artifact: &'a Artifact, pub execution: Execution }

impl Index {
    pub fn select(&self, target: Target, kind: &str, version: &str) -> Option<Selection<'_>>;
    pub fn artifact(&self, kind: &str, version: &str) -> Option<Selection<'_>>;   // Target::host()
    pub fn installable_for(&self, target: Target, kind: &str) -> impl Iterator<Item = &Package>;
    pub fn installable(&self, kind: &str) -> impl Iterator<Item = &Package>;      // Target::host()
}
```

`Execution` is `Native` when the artifact's own `(os, arch)` is the target's, and `Emulated`
otherwise. It is **`mixengine_proto::Execution`** rather than a core type, because it is a fact that
reaches the wire and there is nothing about it a published document and an API could reasonably
disagree on — unlike `Channel`/`PackageChannel`, whose two-enum arrangement exists precisely because
the index's vocabulary is allowed to move independently of the API's. `format.rs` already imports
`mixengine_proto::PackageChannel`, so the direction of the dependency is established.

**Taking the target as an argument is what makes this testable at all** (finding 12): a unit test on
an ubuntu runner can ask what an ARM64 Windows machine would be offered. A host-only API would have
put the entire feature outside CI's reach.

### D3 — `offered()` returns the selection, and its refusal narrows

[`daemon::runtimes::offered`](../../../crates/mixengine-daemon/src/runtimes.rs) keeps its three
told-apart disappointments and returns `(&Package, Selection)`. Its third arm — *"is not published
for this machine"* — now fires only when the version is published for **neither** the native target
nor anything this build can emulate, which on five of six targets is unchanged behaviour and on the
sixth is the difference between forty refusals and none.

### D4 — The wire says which, and the client renders it

`RuntimeRelease` and `PackageRelease` each gain

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub execution: Option<Execution>,
```

`Option<Execution>` and not `bool`, under
[ADR 0019](../../../.claude/decisions/0019-an-added-response-member-is-optional.md): a member added
after protocol 1 was frozen is optional, `None` means *"this peer predates the member"* and never
*"could not determine"*, and a defaulted value would make a client state a fact nobody reported.
`PROTOCOL_VERSION` does not move.

`mix runtime available` and `mix package available` grow a **seventh column, present only when at
least one row is not native**, above which sits one line saying what the word means. On five of six
targets neither appears, which is the point: a column reading `native` sixty times is noise, and the
same column on a Surface is the answer to the question that machine's owner is about to ask.

### D5 — The install says it while it is happening

The install job already reports progress through `install::Watcher`. When the selection is
`Emulated`, the daemon reports one line before the download begins, naming the architecture being
installed and why. That costs no schema, no column and no second place for the truth to live —
and it is where somebody is actually looking.

**Nothing is recorded in `runtime_installs`.** A row remembering the architecture would be a
migration and a `RuntimeSummary` member for a fact that is re-derivable from the index at any time;
it is named under *What this leaves* instead.

### D6 — `cargo run -p mixengine-core --example index-coverage`

Reads the **live published index** through `index::Client::new` — the production URL, the
compiled-in public key, the real minisign verification, the real schema check — into a throwaway
cache directory, then prints two matrices and exits non-zero if the second one has a hole:

1. **Artifacts per target**, which is finding 3: what the pipeline actually produced.
2. **Installable per target**, which is what a MixEngine build on that target can offer once
   `Target::runnable` is applied: every kind × version, resolved through `Index::select` for each of
   the six targets in turn.

The written-down exemptions are a constant in the example, one entry long, each with the packaging
repository's own task named beside it:

```rust
/// Cells this repository knows are empty, and whose reason is written down elsewhere.
const KNOWN_EMPTY: &[(&str, &str, Os, Arch)] = &[
    // P12b — Redis 7.2 builds on Windows and cannot start there, so no Windows cell exists and
    // there is nothing for an ARM64 Windows machine to emulate either.
    ("redis", "7.2.15", Os::Windows, Arch::X86_64),
    ("redis", "7.2.15", Os::Windows, Arch::Aarch64),
];
```

Anything else empty is a failure naming the cell. **It is an example rather than a test** because it
needs the network and `test` has none, and rather than a binary because `mix` may not link
`mixengine-core` — the same two reasons `extensions_json` gives. It is `cargo clippy --all-targets`
material either way, so it cannot rot silently, and it joins the release checklist as the reading
that answers T92's own sentence on the day a release is cut.

**It is also the first code in this repository that parses the real document** (finding 13). That is
worth as much as the matrix: every index test until now judged this build's reader against JSON this
build wrote.

### D7 — ADR 0023, an ARM64 Windows machine runs the x86_64 build

Recording, with the measurement above as its context:

- **Only Windows.** macOS is refused by the packaging document's own rule *and* has no empty cell to
  fill — all four Unix targets are 60 of 60. Linux has no emulator the operating system provides.
- **Automatic, never silent.** The alternative to the emulated artifact on that machine is nothing
  at all, so a flag to opt in would be a refusal with a ceremony attached; what it owes instead is a
  sentence at the listing and a sentence at the install.
- **Native always wins.** A cell that gains a native ARM64 build later is preferred from the next
  index refresh onward, with no code change and no migration. An install already on disk is not
  revisited, which is what pinning a version means.
- **The index is not asked to lie.** No artifact is relabelled; the client chooses among truthfully
  labelled ones.

### D8 — The documentation catches up with the pipeline

- `.claude/operations/runtime-packaging.md`: replace the *"Still open"* table with the three
  answers, add the MySQL and Memcached rows the borrow/build table never had, correct the artifact
  count, and add a short section carrying the measured six-target matrix and the Windows-on-ARM
  consequence.
- `.claude/roadmap/phase-9-ship.md`: tick T92 and write down what it changed about its own sentence.
- `docs/guide/en/runtimes.md` and its Vietnamese twin: one paragraph, because a Surface owner
  installing PHP will see a word no other machine shows. The translation's front-matter SHA-256
  moves with it, per [ADR 0021](../../../.claude/decisions/0021-the-handbook-is-one-corpus-published-three-ways.md).
- `Requires`' doc comment stops claiming the daemon checks these (finding 11) and points at
  `SmokeTest`, which is the mechanism that actually exists.
- `bindings/` is regenerated: `Execution` is a new exported type and two response types changed.

## What was reconsidered, and why the first answer was wrong

**Widening `Arch::host()`.** One line, and it would have broken `mix self-update` on exactly the
machine this task is about — the feed's `artifact(os, arch)` would have accepted an x86_64 MixEngine
for an aarch64 install. Two questions that look like one: *what am I* and *what can I run*.

**A host-only `Index::artifact`.** Simpler signature, and it would have put the whole feature beyond
CI (finding 12), leaving it exercised for the first time by a user on a Surface. The target is an
argument.

**Iterating artifacts and testing "is this one runnable".** Correct-looking, and it hands the choice
between two valid artifacts to the order the generator wrote them in. Targets outside, artifacts
inside.

**`emulated: bool` with `#[serde(default)]`.** Convenient, and refused outright by ADR 0019's last
consequence — a defaulted value makes a client assert a fact no peer reported. `Option<Execution>`
costs a branch and says only what it knows.

**A permanent seventh column.** Honest, and noise on five of six targets. Conditional, with a note
line explaining the word the first time it appears.

**Adding `requires.tzdata` to `Requires`.** Tempting — the publisher states it and we drop it — and
it would put a fourth unread field beside three others while the type's own doc comment already
claims they are checked. Nothing here can act on a prose sentence. It is recorded, and the false
comment is corrected instead; carrying the sentence to a user at install time is a feature with a
design of its own.

**Making the coverage check a `cargo test`.** It would be the strongest possible check and it needs
the network, which `test` deliberately does not have, and it would go red for everyone the day
GitHub has an outage. An example on the release checklist is the reading at the moment it matters.

**A nineteenth `mix doctor` check.** *"Some runtimes on this machine run under emulation"* is not a
fault of the machine and there is no repair; a `ProblemId` for it would fail every `mix doctor` in
every script on every Surface, for ever. The listing is where it belongs.

## What this leaves

- **`windows/aarch64` is 19 of 60 natively and this task does not change that.** Six kinds have no
  ARM64 Windows build anywhere upstream. What changes is that a machine there can install 59 of the
  60 packages rather than 19, and knows which of them are emulated.
- **`redis 7.2.15` is installable on no Windows machine**, native or emulated, and that is correct:
  it does not run there. Named in the exemption list rather than left to be rediscovered.
- **PostgreSQL's ARM64 Windows cell stays P7c's.** The fallback makes it installable, not native;
  when PostgreSQL 19 makes the build possible, the native artifact wins with no change here.
- **Nothing checks `requires` before a download**, including the `vcredist` an emulated x86_64 PHP
  needs — which on an ARM64 Windows machine is the *x64* redistributable, a different package from
  the ARM64 one. The smoke test is what catches it, with the loader's own words rather than ours.
  Unchanged by this task and now written down where the next reader will find it.
- **An installed runtime's row does not remember its architecture** (D5), so `mix runtime list`
  cannot mark an already-installed emulated runtime. Re-derivable from the index; a column is a
  migration and somebody's design.
- **Windows 10 on ARM64 emulates x86-32 and not x86-64.** A machine that old would fail the smoke
  test rather than the selection. MixEngine states no minimum Windows version anywhere today, which
  is a gap this task noticed and does not fill.
- **The example proves the pipeline published, not that the artifacts work.** Sixty checksums and
  three hundred and eighteen URLs are read; nothing is downloaded. What proves an artifact runs is
  `install::SmokeTest`, on the machine that installs it, which is the only place that question can
  honestly be asked.

## Verification

- `cargo test --workspace` — the new unit tests over `Target::runnable` and `Index::select` for all
  six targets, including the two that only exist on Windows-on-ARM, plus the CLI renderer tests for
  both shapes of the table.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --all-features`.
- `bash packaging/bindings.sh` and the CI check that `bindings/` is current.
- `cargo run -p mixengine-core --example index-coverage` against the live index, whose output is the
  matrix quoted in `runtime-packaging.md`.
