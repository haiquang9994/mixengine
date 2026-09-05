# T85c — The shim in every artifact (design)

Roadmap task **T85c**, phase 9: *"`mixengine-shim` is in none of the six artifacts."*

`packaging/common.sh` names three binaries, `packaging/stage.sh` builds three crates, and
[`core::shims::source`](../../../crates/mixengine-core/src/shims.rs) looks for a **fourth** beside
the running `mixengined`. When it is not there the daemon answers `Error::ShimMissing`, `<root>/bin`
stays empty, and with it goes **every runtime command the product exists to provide**. A release
installed from any of the six artifacts starts, reports itself healthy, and cannot run `php`.

Found by **T88**, which reads the same list, and left here because it changes what every installer
ships.

## Goal

A person who installs MixEngine from any of the six artifacts, starts the daemon and types `php -v`
in a project gets PHP. Nothing else about any artifact changes.

## Measured, not assumed

Read on 2026-09-05 out of this tree rather than reasoned about.

1. **`shims::source` has no fallback.** `program.parent().join("mixengine-shim<EXE_SUFFIX>")`, and
   `Error::ShimMissing` when that is not a file — [shims.rs:260](../../../crates/mixengine-core/src/shims.rs).
   There is no `PATH` search and deliberately so: a `PATH` search would find the *copy in `bin/`* on
   a machine already set up, and copying a shim from `bin/` into `bin/` would make an upgrade a
   no-op.
2. **Nothing in `packaging/` builds the crate.** `stage.sh` passes `-p mixengine-cli -p
   mixengine-daemon -p mixengine-elevate` in each of its three branches.
3. **`AppRun` carries a *second* hardcoded list** — `for binary in mix mixengined
   mixengine-elevate` — which the AppDir's own `MIX_BINARIES` loop does not reach.
4. **`packaging/macos/probe.sh` carries a third**, as `cli`/`daemon`/`helper`, used by the
   "is this machine occupied" guard, by `cleanup`'s `sudo rm -f`, and by M5.
5. **`packaging/feed.sh` computes the Windows `provides` key with its extension on.**
   `binary="${entry#mixengine/}"` over `unzip -Z1` output yields `mix.exe`, while
   `index::format::Artifact::provides` is documented as *"executable name to its path inside the
   archive"* with `{"php": "php.exe"}` as its own example, `apply::binary_name` appends
   `EXE_SUFFIX` itself, and `apply::stage` looks the smoke-test executable up as `mixengined`.
   See D8.
6. **The shim is 3.7 MB of a 31.8 MB release** on `x86_64-pc-windows-msvc` — `mix` 6.4 MB,
   `mixengined` 24.6 MB, `mixengine-elevate` 0.8 MB. Adding it grows a payload by ~12%.
7. **Two documents already assert the fix.** `.claude/features/updates.md` and
   [ADR 0017](../../../.claude/decisions/0017-smart-app-control-is-an-unsupported-configuration.md)
   both say *"`mix.exe`, `mixengined.exe`, `mixengine-elevate.exe` and `mixengine-shim.exe` are the
   whole of what this project builds. **W1** measures all of them"*. W1 reads the portable zip, so
   that sentence is false today and true the day this lands.

## Scope

**In.** The four binaries in all six artifacts and both update payloads; the second and third
hardcoded lists above; the checks each script ends with; the feed's `provides` key on Windows; a
test that ties `MIX_BINARIES` to the code's own constants; the two documents that say "three".

**Out.** Changing `shims::source` to search anywhere. Changing `apply::swap`'s rule that an update
never *adds* a binary (see *What this leaves*). Anything about `mixengine-elevate`'s placement,
which is [ADR 0015](../../../.claude/decisions/0015-the-helper-installs-itself.md)'s and unchanged.

## Where the fourth binary goes

Per artifact, and the answer is the same everywhere: **beside `mixengined`**, because that is the
only place `shims::source` looks.

| Artifact | Path | How |
| --- | --- | --- |
| Windows zip | `mixengine/mixengine-shim.exe` | already automatic — `cp "$stage"/*.exe` |
| Windows NSIS | `$INSTDIR\mixengine-shim.exe` | a `File` line, and a `Delete` line (D2) |
| macOS `.pkg` | `/usr/local/bin/mixengine-shim` | a `lipo -create`, universal like its neighbours |
| macOS `.tar.gz` | `mixengine/mixengine-shim` | already automatic — the `MIX_BINARIES` loop |
| Linux `.deb` | `/usr/bin/mixengine-shim` | an `install` line |
| Linux `.rpm` | `/usr/bin/mixengine-shim` | `%install` and `%files` |
| Linux AppImage | `usr/bin/`, then the version cache | automatic in the AppDir, **not** in `AppRun` (D3) |
| Linux `.tar.gz` | `mixengine/mixengine-shim` | already automatic — the `MIX_BINARIES` loop |

`packaging/feed.sh` needs no list widened: it reads `provides` out of the archive it is describing,
so a payload that gained a binary produces a `latest.json` that lists it.

## Decisions

### D1 — One list of names and one list of crates, both in `common.sh`

`MIX_BINARIES` becomes `(mix mixengined mixengine-shim mixengine-elevate)` — the three that install
into one directory, then the one that does not.

And a new `MIX_CRATES=(mixengine-cli mixengine-daemon mixengine-shim mixengine-elevate)`, which
`stage.sh` uses in all three of its branches instead of spelling `-p` three times over.

**The two lists sitting in one file is the point of this task and not tidying.** T85c happened
because *what is built* lived in `stage.sh` and *what is shipped* lived in `common.sh`, and the code
that needs a fourth name knew about neither. Whoever adds a fifth binary now has one file to edit,
two lines apart.

Order matters to no consumer — checked: the Windows probe prints `${#MIX_BINARIES[@]}`, the two
tarball builders iterate, nothing indexes.

### D2 — The NSIS installer gets a `File` **and** a `Delete`

`Section "Uninstall"` deletes by name and then calls `RMDir`, which only removes an empty directory.
A `File` without a matching `Delete` is an uninstall that silently leaves a 3.7 MB binary and a
directory behind, and `RMDir`'s failure is not reported. Both halves, or neither.

The Windows probe's W3 already exercises this end to end — it installs with `/S`, asserts every
`MIX_BINARIES` entry was written, uninstalls with `_?=`, and fails on anything left behind. So the
fourth name arrives in that assertion for free, which is what makes D2 checked rather than believed.

### D3 — `AppRun` copies whatever is in `usr/bin`, and per file

The AppImage extracts itself into `~/.cache/mixengine/<version>/` before running anything, for the
reason `AppRun` states: the AppImage runtime unmounts when the process exits, and `mix` starts a
daemon that outlives it.

Two things are wrong with widening its hardcoded list to four names.

**It is the second copy of a list this task exists to stop having.** `AppRun` cannot `source
common.sh` — it runs on a user's machine, out of an extracted image. But it does not need a list at
all: the AppDir holds exactly `MIX_BINARIES` and nothing else, so `for source in "$here"/usr/bin/*`
is the same list, read from where it was written.

**And the cache is keyed by version, so a list is not enough.** A machine that ran the 0.1.0
AppImage before this change has a `0.1.0/` cache with three binaries in it, and the existing guard —
`if [ ! -x "$cache/mix" ]` — sees `mix` and copies nothing, for ever. The guard moves inside the
loop: copy any binary the cache does not have. The warm path costs four `test -x` instead of one,
and a stale cache heals itself.

### D4 — The `.deb` and the `.rpm` put it in `/usr/bin`, next to `mixengined`

`shims::source` looks beside the running daemon, and the daemon is `/usr/bin/mixengined` from both
packages. `/usr/local/libexec/mixengine/`, where the helper goes, is not a place the daemon looks.

**`mixengine-shim` therefore becomes a name on the user's `PATH` that they can type.** Accepted:
`shims::dispatch` returns `None` for it — documented and tested — so it exits 127 saying *"this is a
MixEngine shim and is not meant to be run under this name"* and listing what it does answer to,
which is a better outcome than most unknown names on a `PATH`. The alternative is teaching `source`
a second lookup path, which is a change to the
daemon's trust story for a cosmetic gain, and is out of scope by the roadmap's own framing: *"adding
the name is the whole of the fix"*.

### D5 — `packaging/macos/probe.sh` learns the fourth path

Not cosmetic. The probe's `cleanup` runs `sudo rm -f "$cli" "$daemon" "$helper"` after installing
the real `.pkg` at its real paths; a fourth file the package writes is a file the probe **leaves on
the machine**. Worse, the next run's *"this machine already has MixEngine at:"* guard reads the same
three paths, so it would not see the leftover, would install over it, and would then delete it as
its own — the exact scenario that guard exists to refuse.

So `shim=/usr/local/bin/mixengine-shim` joins the occupied check, the cleanup, and M5's
"which installed files carry the package's quarantine attribute" loop. M5's `none of 3` becomes
`none of ${#…}`-style counting so the number cannot go stale again.

M6 keeps running `mix` alone: it measures whether an ad-hoc-signed Mach-O executes on this machine,
which is a property of the linker and not of which binary is asked.

### D6 — Each script's "open what was just made" check reads `MIX_BINARIES`

`packaging/windows/build.sh` ends with `for name in mix.exe mixengined.exe mixengine-elevate.exe`.
That is a fourth copy of the list, and one that would have gone on passing after every other change
in this task — a check that asserts three of four things is a check that would not have caught
T85c either. It reads `MIX_BINARIES` with the platform suffix appended.

`build-deb.sh`, `build-rpm.sh` and `macos/build.sh` check *paths* rather than names, and paths differ
per binary — `/usr/bin/…` against `/usr/local/libexec/mixengine/…`. Those lists stay written out, one
line longer, because the thing being asserted there is the layout and not the count.

### D7 — A test ties `MIX_BINARIES` to the code's own constants

T85c was possible because `MIX_BINARIES` is a list nothing forces to agree with the code that needs
its entries. `packaging/updates.pub` had the same shape and is pinned by a test in
[updates.rs](../../../crates/mixengine-core/src/updates.rs) that `include_str!`s the committed file
— *"read at compile time on purpose: a file that is deleted or moved is then a build error, rather
than a test that reads nothing and passes"*. The same trick applies here.

A new `crates/mixengine-core/tests/packaging.rs` reads `packaging/common.sh`, parses the
`MIX_BINARIES=( … )` line, and asserts the set equals the four names — with three of them supplied
by the constants that actually need them rather than retyped:

- `shims::BINARY`, a new `pub const` that `shims::source` builds its file name from. The whole of
  T85c in one assertion: *this name must be in the release, or `bin/` is empty.*
- `updates::apply::KEPT` — the helper. In the release because the three root-running formats place
  it, never replaced by an update.
- `updates::apply::SMOKE_EXECUTABLE`, which becomes `pub`. A payload that does not publish it is one
  `apply::stage` refuses with `MissingFromArtifact`.

`mix` is the fourth and has no constant to borrow; it is spelled once, in the test, with a comment
saying so.

**Deliberately a *set* equality and not a subset.** A subset check passes on a `MIX_BINARIES` that
someone shortened, which is the failure being prevented.

### D8 — The feed's `provides` key loses its `.exe`, on Windows only

Not in T85c's sentence, and in T85c's argument: the roadmap justifies leaving this fix here by
saying *"the swap set is the payload's own `provides` intersected with what is installed, so an
installed 0.2.0 takes a 0.3.0 payload that has a shim with no further change"*. On Windows that
intersection is empty today, and adding the shim to the payload does not change it.

`feed.sh` writes `"mix.exe": "mixengine/mix.exe"` for the zip. Every reader wants the bare name:

- `apply::swap` joins `binary_name(name)` — `name` + `EXE_SUFFIX` — so it would look for
  `mix.exe.exe`, find nothing installed, and report the whole update as `kept`.
- `apply::stage` asks `provides` for `mixengined`, and gets `None` → `Error::MissingFromArtifact`.
  So the Windows self-update fails at staging rather than silently, which is why nobody has seen it.
- `index::format`'s own documentation and example say name-to-path, and
  `crates/mixengine-cli/tests/self_update.rs` builds its `provides` that way — the test that would
  have caught this is written correct and never runs against `feed.sh`.

One line: strip a trailing `.exe` from the key, not from the path. The path keeps its extension,
because that is what is inside the archive.

A `.exe` in the *middle* of a name cannot occur — these are our own four binaries — so the strip is
a suffix removal and not a regex.

## Data flow

```
packaging/common.sh
  MIX_CRATES   ──► stage.sh: cargo build --release -p … (×4)
  MIX_BINARIES ──► stage.sh: copy from target/…/release into the stage, and assert each is there
                     │
                     ├─ windows/build.sh   ─► zip (mixengine/*.exe)      ─► payload
                     │                     ─► makensis → setup.exe        ─► File ×4, Delete ×4
                     ├─ macos/build.sh     ─► lipo ×4 → pkgroot → .pkg
                     │                     ─► lipo ×4 → mixengine/ → .tar.gz ─► payload
                     └─ linux/build-*.sh   ─► .deb /usr/bin + /usr/local/libexec
                                           ─► .rpm  same layout
                                           ─► AppDir usr/bin ─► AppRun ─► ~/.cache/mixengine/<v>/
                                           ─► mixengine/ → .tar.gz      ─► payload

packaging/feed.sh
  opens each payload ─► provides{name → path}  (name without .exe — D8)
                          │
                          └─► latest.json ─► apply::stage (smoke: provides["mixengined"])
                                          └─► apply::swap  (target = installed/name + EXE_SUFFIX)

first daemon start on the installed machine
  shims::source(mixengined) ─► mixengine-shim beside it ─► shims::refresh(<root>/bin, shim)
                                                            └─► php, node, npm, python, ruby, …
```

## Testing

**Unit.** `crates/mixengine-core/tests/packaging.rs` — D7. Two cases: the parsed `MIX_BINARIES`
equals the four names, and the line is actually found (a `common.sh` that stopped declaring the
array must fail rather than parse to an empty set and compare equal to nothing).

**Existing tests that cover this and needed no change** — worth naming, because they are why this is
a packaging task and not a core one: `apply.rs`'s `a_binary_this_install_does_not_have_is_left_alone`
already describes *"how `mixengine-shim` behaves the day T85c is done"*, and
`core/tests/shims.rs` already asserts `source` finds a shim beside `mixengined` and fails without
one.

**In CI.** The `build` job is the test, on all five legs, and every script fails on a binary missing
from what it just made: `unzip -l` and `7z l` on Windows, `pkgutil --payload-files` and `lipo
-archs` on macOS, `dpkg-deb -c`, `rpm -qlp`, a real run of the AppImage, and `tar -tzf` on both
payloads. The Windows probe's W1/W3/W4 additionally read all four binaries out of the zip and the
installed directory, and W3 asserts the uninstaller left none behind.

**Not tested here.** That a released machine actually populates `bin/` — that is a clean-VM run,
which is the release checklist's and T87's, and this task does not claim to have done it.

## Risks, and where each is answered

- **The `.rpm` spec has the name in two places.** `%install` writes it and `%files` lists it; an
  `%files` missing an installed file makes `rpmbuild` fail, and a `%files` naming a file that was not
  installed does too. Both edited, and `rpm -qlp` checks the result.
- **`RMDir` after the uninstall.** D2 — the `Delete` is added with the `File`, and the probe's W3
  asserts nothing is left.
- **A leftover `/usr/local/bin/mixengine-shim` on the macOS runner.** D5, and it is the reason that
  decision exists rather than a note under it.
- **A stale AppImage cache.** D3 — the guard moved inside the loop.
- **+12% on every artifact.** Accepted, and it is the binary the product exists to provide. The two
  update payloads grow by the same 3.7 MB, which is one download per release per machine.
- **`mixengine-shim` typed on a PATH.** D4 — exit 127 with the message `dispatch` already produces.
- **D8 changes T88's behaviour in a task named T85c.** Stated in the roadmap entry and in the commit,
  and it is one line with three readers named above. Leaving it would ship a Windows self-update that
  cannot stage, which is worse than a slightly wide task.

## What this leaves

- **An install predating this release still has no shim after a self-update.** `apply::swap` rule 2
  keeps — never adds — a binary the install does not have, on purpose: adding files is an install's
  business and not an update's. Nothing has been released from this repository, so today the set of
  affected machines is empty; the moment a release exists it stops being empty, and the answer then
  is a reinstall or a rule in `swap` that is somebody's design and not a line slipped into this one.
  Recorded in the roadmap under this task.
- **`packaging/README.md`'s "What is not here" still says there is no `latest.json`.** T88 wrote
  `feed.sh` and did not revisit that section. Out of scope here; noted so the next reader of that
  file knows it is stale rather than true.
