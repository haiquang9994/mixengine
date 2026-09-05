# 0023. An ARM64 Windows machine installs the x86_64 build, and is told that it did

**Status**: Accepted
**Date**: 2026-09-05

## Context

Roadmap task **T92** asks whether *"the packaging pipeline is running for all runtimes across six
OS/arch targets"*. Measured against the published index of `2026-08-31T07:40:07Z` — sixty packages,
three hundred and eighteen artifacts, eleven kinds, which is exactly the eleven this build can
install — the answer is that it is running on five targets and a third of the way on the sixth:

| target | artifacts, of 60 |
| --- | --- |
| `linux/x86_64`, `linux/aarch64`, `macos/x86_64`, `macos/aarch64` | 60 |
| `windows/x86_64` | 59 |
| **`windows/aarch64`** | **19** |

The forty-one empty ARM64 Windows cells are six whole kinds — `memcached`, `mysql`, `nginx`,
**`php`**, `postgres`, `redis` — and part of three more. Not one of them is a gap in the packaging
pipeline that this project could close: upstream publishes no ARM64 Windows PHP in any branch, no
ARM64 Windows nginx, and PostgreSQL's cell waits on a release that does not exist yet (the packaging
repository's **P7c**). The one missing `windows/x86_64` cell is `redis 7.2.15`, which builds on
Windows and cannot start there (**P12b**), and which is therefore a truthful absence rather than a
hole.

**Forty of the forty-one have an x86_64 twin**, and Windows 11 on ARM runs an x86_64 user-mode
process under the operating system's own emulation. Two documents in this repository already assumed
that this is what happens —
[../operations/runtime-packaging.md](../operations/runtime-packaging.md) says *"a Windows-on-ARM
machine runs the daemon natively and PHP under emulation"*, and `index/format.rs` and
`daemon/runtimes.rs` each carry a comment about emulation — and **no code did it**.
`Index::artifact` and `offered()` matched `artifact.arch == Arch::host()` exactly, so on the
`aarch64-pc-windows-msvc` build that **T85a** ships, `mix runtime install php 8.3.33` answered
*"php 8.3.33 is not published for this machine"* and `mix runtime available --kind php` printed
*"the package index offers nothing for this machine"*. A product whose reason to exist is PHP could
not install PHP on a target it ships an installer for.

This is not a question the packaging repository can answer. Its rule — *"a branch that will not
build natively for an architecture is a cell the index does without, which is a truthful 'not
available' rather than an artifact that silently emulates"* — is about what an **index** may claim,
and it should not change: an artifact labelled `aarch64` that is really `x86_64` is a lie in a
signed document. What was missing is a **client** that knows what its own machine can execute.

## Decision

**A MixEngine build for `aarch64-pc-windows-msvc` installs a `windows/x86_64` artifact when the
package it was asked for has no `windows/aarch64` one, and says so at the listing and at the
install.**

Four parts, and each of them is load-bearing:

- **Only Windows.** macOS is refused emulation by name in the packaging document, and all four Unix
  targets are complete anyway, so there is no cell to fill; Linux has no emulator the operating
  system provides. `index::Target::runnable` is the whole rule and it is six lines: one entry for
  every target, two for ARM64 Windows.
- **Native always wins.** The search walks the preference list on the outside and a package's
  artifacts on the inside, so a package published for both Windows cells resolves to the native one
  — whichever order the generator wrote them in. A cell that gains a native ARM64 build later is
  preferred from the next index refresh, with no code change and no migration.
- **Automatic, and never silent.** The alternative to the emulated artifact on that machine is
  nothing at all, so a flag to opt in would be a refusal with a ceremony attached. What it owes
  instead is a word: `mix runtime available` and `mix package available` grow a seventh column —
  present only when a row needs it — and the install reports one line before the download starts.
  The fact reaches the wire as `Execution`, on `RuntimeRelease` and `PackageRelease`, optional per
  [ADR 0019](0019-an-added-response-member-is-optional.md).
- **The index is not asked to lie.** No artifact is relabelled. The client chooses among truthfully
  labelled ones, which is the only place the choice can be made honestly.

## Consequences

**An ARM64 Windows machine can install fifty-nine of the sixty published packages instead of
nineteen.** The one it cannot is `redis 7.2.15`, which no Windows machine can, and which the
coverage reading names rather than counts.

**The cells are not filled and this decision does not pretend they are.** `windows/aarch64` is 19 of
60 natively today and will stay that way until upstream builds more; PostgreSQL's native cell is
still P7c's. What changed is what a user on that machine can do, not what the pipeline produced —
and the two matrices printed by the coverage reading are deliberately separate so that nobody
mistakes one for the other.

**An emulated runtime is slower, and MixEngine does not measure how much.** Naming it is the whole
of what is promised; a number would need a benchmark on a machine this project does not have.

**Nothing checks `requires` before a download**, this artifact's `vcredist` included — which for an
emulated PHP on an ARM64 machine is the *x64* redistributable, a different package from the ARM64
one. That was already true of every target and is unchanged here; `install::SmokeTest` is the
mechanism that catches a machine that cannot run what it downloaded, with the loader's words rather
than ours.

**Windows 10 on ARM64 emulates x86-32 and not x86-64.** A machine that old would fail the smoke test
rather than the selection, and MixEngine states no minimum Windows version anywhere — a gap T92
noticed and did not fill.

**An installed runtime's row does not record its architecture**, so `mix runtime list` cannot mark
one that is already installed. It is re-derivable from the index at any time; a column would be a
migration and a response member for a fact nothing has asked for yet.

## Alternatives considered

**Publish the x86_64 artifact under an `aarch64` label.** One line in the packaging pipeline, and it
puts a false statement inside a signed document — the exact thing the index's `arch` field exists to
prevent, and the thing that would make every future reader of the matrix wrong about what was built.

**Refuse, and tell the user their machine is unsupported.** Honest, and it makes the ARM64 Windows
installer T85a built a product that starts, reports itself healthy and cannot serve a PHP site.
Removing that artifact from the release would be the consistent version of this alternative, and it
is a worse outcome than an emulated PHP for the same person.

**Ask first, behind `--allow-emulated`.** A consent prompt is worth its cost when there is a second
option. Here there is not: the choice is the emulated build or nothing, so the flag only adds a step
between a person and the only answer. What consent is actually for — knowing what you are getting —
is bought instead by the column and the install line.

**Build the missing runtimes ourselves for ARM64 Windows.** This re-argues *"borrow before you
build"* for six kinds at once, on the platform with the least upstream support, and it is the same
remedy [ADR 0017](0017-smart-app-control-is-an-unsupported-configuration.md) refused for the same
reason: a pipeline maintained for every security release, for ever, is not a cost this project can
carry to close cells the operating system already closes.
