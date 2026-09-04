# T85a — The second architecture, and the roadmap's own premise corrected (design)

Roadmap task **T85a**, phase 9, split out of T85: *"The second architecture:
`aarch64-pc-windows-msvc` and `aarch64-unknown-linux-gnu`, and an old-glibc Linux build."* Written as
three toolchain questions — "both are cross-compilations of a workspace that builds SQLite, AWS-LC
and libdbus from C, on runners that carry no cross toolchain." That premise is checked below and
found half wrong, in the shape `runtime-packaging.md` already warns about: *"the borrow/build table
is a set of hypotheses until each cell is opened."*

## Goal

Five `build` legs instead of three: the two existing per-OS artifact sets keep building exactly as
T85 left them, Windows and Linux each gain a second, `aarch64` artifact set, and both Linux legs
(not only the new one) start linking against an older glibc than the runner ships, so a `.deb`
built today still runs on an LTS distribution installed two years from now.

## Measured, not assumed

- **This repository is public.** `gh repo view` reports `visibility: PUBLIC`. That is the fact the
  rest of this design turns on.
- **GitHub now publishes free, GA, natively-hosted arm64 runners for public repositories**:
  `windows-11-arm`, `ubuntu-24.04-arm`, `ubuntu-22.04-arm` — Linux arm64 GA August 2025, Windows
  arm64 GA on the same track, both free-tier eligible and (as of the January 2026 change) no longer
  limited to public repositories, though this repository's own eligibility already came from being
  public. **Neither new architecture needs a cross toolchain**: `aarch64-pc-windows-msvc` and
  `aarch64-unknown-linux-gnu` each get a runner built out of that silicon, and `cargo build --release`
  with no `--target` is already a native build there — the same shape T85's Windows and Linux legs
  already use, not a new mechanism. This is the T85 design's own "Out" section turning out to describe
  a runner landscape that existed when it was written and does not exist now, the same way
  `runtime-packaging.md` found the MariaDB row and the PHP-on-macOS row each wrong once actually
  checked.
- **What does not change**: macOS stays universal via `lipo`, unrelated to this task — Apple's own
  toolchain has cross-compiled the other slice since T85, and nothing here touches
  `packaging/macos/build.sh`.
- **`deny.toml` already lists all six release targets.** `targets = [x86_64-pc-windows-msvc,
  aarch64-pc-windows-msvc, x86_64-apple-darwin, aarch64-apple-darwin, x86_64-unknown-linux-gnu,
  aarch64-unknown-linux-gnu]` — written in anticipation of this task; nothing to add there.
- **`aws-lc-sys` needs nothing beyond a C/C++ compiler for either new target.** Its own documentation
  states pre-generated bindings ship for every platform it supports, including both
  `aarch64-unknown-linux-gnu` and `aarch64-pc-windows-msvc`; for a non-FIPS build (which is what this
  workspace's `rustls`/`aws-lc-rs` edge uses) bindgen, CMake and Go are never invoked. The "builds
  SQLite, AWS-LC and libdbus from C" line in T85's design is true and is not, on its own, a reason a
  toolchain is missing — a C compiler is all three of those need, and every runner and container named
  below carries one.
- **`libdbus-sys`'s non-vendored path goes through `pkg-config`**, which needs `libdbus-1-dev`'s
  headers and `.pc` file present for the *target* — satisfied today on `ubuntu-latest` and
  `windows-11-arm`/`ubuntu-24.04-arm` alike by nothing more than "the same package the host build
  already resolves", because in every case the build is native. It is **not** satisfied by an
  `almalinux`-based container unless that container installs the equivalent (`dbus-devel`) itself —
  D6 below.
- **`mixengine-packages` already answered "what glibc floor, and what container" for this exact
  product.** `runtime-packaging.md`'s PHP 7.0–8.0 section: AlmaLinux 8, glibc 2.28, chosen because
  its OpenSSL/ICU/autoconf are old enough to be the era these builds need and because
  `manylinux_2_28` is the name upstream tooling already uses for that floor. This task reuses the
  same floor for MixEngine's own binaries — one glibc number for the whole product rather than one
  per component — rather than opening a fresh evaluation.
- **`packaging/stage.sh` already has a `--target` argument**, used today only by
  `packaging/macos/build.sh`'s two slices; its own comment says *"the second architecture on Windows
  and Linux is roadmap task T85a"*. This task is what the comment is waiting for, not a redesign of
  the flag.
- **`ci.yml`'s `build` job matrix is `[ubuntu-latest, windows-latest, macos-latest]`, one artifact set
  per OS, no `--target` passed.** Every conditional in that job (and in `test`/`system`/`bench`) keys
  off `runner.os`, which GitHub reports as `Linux`/`Windows`/`macOS` identically for the arm64 runners
  and their x64 counterparts — confirmed by reading GitHub's own runner-images documentation for
  what `runner.os` resolves to. Nothing in `test`, `system`, `bench` or `lint` needs to change; only
  `build`'s matrix and the packaging scripts it calls do.
- **NSIS ships an x86 `makensis.exe`, and Windows 11 (not Windows Server) carries x86-on-arm64
  emulation.** `windows-11-arm` is the desktop image, not Server, so `makensis` running under
  emulation to *write* a zip and an installer — not to run anything performance-sensitive — is
  expected to work. This is a claim about the runner image the way the T40a design called its own
  macOS authentication question a *measurement*: it is checked in CI in this task's first PR, not
  assumed true here.
- **`appimagetool` publishes an `aarch64` asset per release**, alongside `x86_64` and `armv7`, at the
  same GitHub Releases page `build-appimage.sh` already pins a tag from. The existing script already
  parameterises `ARCH=` as an environment variable passed to the tool; what it hardcodes is the
  *download URL's* architecture segment, which this task makes conditional on the host.

## Scope

**In:**

- `.github/workflows/ci.yml` — the `build` job's matrix grows from three rows to five; each row now
  states its own `target` triple and (Linux only) `container` image, rather than the job assuming
  the runner's ambient toolchain.
- `packaging/common.sh` — `mix_host_target()` (the `host:` line of `rustc -vV`, the single source of
  truth every script below reads instead of re-deriving an architecture from `uname -m`, which is
  exactly the value emulation can lie about) and `mix_arch_label()` (the per-package-format spelling
  of that triple's architecture: `amd64`/`arm64` for `.deb`, `x86_64`/`aarch64` everywhere else).
- `packaging/stage.sh` — a `--container <image>` mode that runs the release build inside `docker run`
  rather than on the host, for the two Linux legs that need an older glibc than the runner ships.
- `packaging/windows/build.sh`, `packaging/linux/build-deb.sh`, `packaging/linux/build-rpm.sh`,
  `packaging/linux/build-appimage.sh` — artifact names and, for `.deb`/`.rpm`, the architecture field
  inside the package itself, all derived from `mix_arch_label()` instead of hardcoded.
- `.claude/operations/build-and-release.md` — the targets table and the `build` job description,
  updated to state five rows and the glibc floor.
- Documentation: this spec, the roadmap line, `runtime-packaging.md` is **not** touched — it is about
  bundled runtimes (PHP, Node, …), not about MixEngine's own three binaries, and the floor this task
  picks is a citation of that document, not an edit to it.

**Out**, unchanged from T85's own "Out" section plus one addition:

- **T85b** — `ServiceInstaller` / autostart registration.
- **T86 / T86a / T94** — signing, SmartScreen/Gatekeeper behaviour, a certificate.
- **T87** — uninstall.
- **The package index's `requires.glibc` field.** That mechanism belongs to
  `mixengine-packages`' signed index for *bundled runtimes*; MixEngine's own binaries have no such
  index entry anywhere today, and inventing one is a bigger task than picking a floor and building
  against it. What this task owes instead is a documented number in `build-and-release.md`, which a
  future task can wire into whatever surfaces it to a user.

## Decisions

### D1 — Native, not cross: the runner supplies the architecture

`aarch64-pc-windows-msvc` is built on `windows-11-arm`; `aarch64-unknown-linux-gnu` is built on
`ubuntu-24.04-arm`. Neither leg passes `--target` to reach a *different* architecture than the
runner's own — the flag exists (D5) so every leg states its target explicitly and none relies on
`cargo build`'s ambient default, not because any leg is cross-compiling. This is the whole reason the
task is smaller than its own roadmap line: "cross toolchain" was the framing that made T85 split it
out, and the framing was the part that was wrong.

### D2 — The old-glibc floor applies to both Linux architectures, inside a manylinux_2_28 container

Both Linux legs compile inside `quay.io/pypa/manylinux_2_28_x86_64` / `manylinux_2_28_aarch64` —
pulled natively (no QEMU: `ubuntu-latest` is x86_64 hardware, `ubuntu-24.04-arm` is aarch64 hardware,
and Docker resolves the matching manifest slice on each) — pinned to a dated tag rather than
`:latest`, resolved and recorded when this is implemented rather than guessed here, for the reason
`runtime-packaging.md` gives for pinning every other toolchain in this product: *"a build is only
reproducible if its toolchain is pinned, and 'whatever the runner has' is not pinned."*

`manylinux_2_28` is glibc 2.28 — the same floor `runtime-packaging.md` measured for PHP 7.0–8.0 on
Linux, chosen there for the same reason it is reused here: it is a floor low enough to run on distros
still in LTS support and it is a name upstream tooling already agrees on, rather than a number this
project would otherwise have to defend alone.

`ubuntu-latest`'s existing Linux leg **moves into the container** rather than gaining a sibling: today
it links against whatever `ubuntu-latest` ships (glibc 2.35+ depending on the image), which is exactly
the thing `build-and-release.md` already flags — *"the `build` job below uses the runner's own"* — as
the gap this task closes. The artifact this leg produces keeps its existing file name
(`mixengine-<version>-linux-x86_64.*`); what changes is the glibc it links against, not what it is
called. Worth stating plainly rather than leaving implicit: **same name, different binary
provenance** — a compatibility improvement, not a rename, and the reason it belongs in this task's own
"what changed and why" rather than being silently folded into "add arm64".

### D3 — `stage.sh --container`: bind-mount, don't copy

```bash
docker run --rm \
  --user "$(id -u):$(id -g)" \
  -v "$MIX_ROOT:/work" -w /work \
  -e CARGO_HOME=/work/target/cargo-home \
  "$container" \
  bash -c 'rustup-init -y --profile minimal --default-toolchain "$(sed -n "s/^channel = \"\\(.*\\)\"/\\1/p" rust-toolchain.toml)" \
    && source "$CARGO_HOME/env" \
    && cargo build --release --locked --target '"$target"' -p mixengine-cli -p mixengine-daemon -p mixengine-elevate'
```

Three things this buys over the alternative of copying the repo in and the binaries back out:

- **`target/` lands in the same place either way.** The container writes to
  `/work/target/$target/release`, which through the bind mount *is*
  `$MIX_ROOT/target/$target/release` — exactly where `stage.sh`'s non-container path already looks,
  so nothing downstream (the `.deb`/`.rpm`/AppImage scripts, which run on the host afterwards with
  the host's `dpkg-deb`/`rpmbuild`/`appimagetool`) needs to know a container was involved.
- **`--user "$(id -u):$(id -g)"` is not optional.** Docker runs as root by default, and a root-owned
  `target/` is a directory the next step — running as the CI user — cannot clean up, and on a
  developer's own machine cannot be `rm -rf`'d without `sudo`. This is the same class of failure
  `build-and-release.md` already records for MariaDB's bootstrap script wanting to `chown` to a user
  that does not exist here: a container's default identity is a fact about the container, not about
  the thing being built, and it is stated explicitly rather than inherited.
- **The toolchain is pinned inside the container rather than borrowed from it.** `manylinux_2_28`
  ships its own Rust in some variants and not in others, and whichever it is would not be *this*
  workspace's `rust-toolchain.toml` version. `rustup-init` reading that file's `channel` is the same
  single source of truth `ci.yml`'s "Install pinned toolchain" step already uses everywhere else,
  applied here instead of trusting an image.

The `--container` build has no warm `Swatinem/rust-cache`: the cache action addresses `~/.cargo` and
`target/` on the *runner*, and this build's cache directory is inside the bind-mounted tree at a path
the action does not know to key. Every run of these two legs is a cold `cargo build --release` of the
whole workspace. Accepted rather than solved here: these are not the legs a developer watches turn
green during iteration, and `timeout-minutes: 60` already has headroom — see Risks.

### D4 — One triple, read once: `mix_host_target()` and `mix_arch_label()`

```bash
mix_host_target() { rustc -vV | sed -n 's/^host: //p'; }
mix_arch_label() {
  case "$1" in
    x86_64-*) echo x86_64 ;;
    aarch64-*) echo aarch64 ;;
    *) echo "unrecognised target: $1" >&2; return 1 ;;
  esac
}
```

Every packaging script derives its architecture word from `rustc -vV`'s own `host:` line — asked of
the toolchain that is about to build, not of `uname -m`, which is precisely the value an emulated
shell can misreport. That risk is not hypothetical here: Git for Windows may or may not ship a native
`aarch64` build yet, and a `git-bash` running under x64 emulation on `windows-11-arm` would have
`uname -m` answer `x86_64` while `rustc` — installed by `rustup`, which GitHub's own runner image
provisions natively per architecture — answers correctly. One function, called by every script that
needs the answer, is what keeps that distinction from being re-decided per script and re-broken in
whichever one nobody re-checked.

`.deb`'s `Architecture:` field wants `amd64`/`arm64`; everything else in this product (`rpm` file
names, the zip/AppImage/pkg names, `Cargo`'s own target triples) says `x86_64`/`aarch64`. Debian's
vocabulary is the outlier and it is isolated to the one script that needs it
(`build-deb.sh`) rather than pushed into `mix_arch_label()` as a second return value.

### D5 — `stage.sh` always takes `--target`, from every caller

Today only `packaging/macos/build.sh` passes `--target`; the other three per-OS scripts call
`stage.sh` bare and rely on the ambient host build. Every caller now passes
`--target "$(mix_host_target)"` explicitly — a native build passes its own host triple straight
through cargo, so nothing about the *build* changes for the two existing legs; what changes is that
no script is silently trusting cargo's default any more, which is the property that makes adding a
fifth leg a matter of the CI matrix and not of guessing which script still assumes one architecture.

### D6 — The container installs what a native leg gets for free

A native Linux leg (`ubuntu-latest` and `ubuntu-24.04-arm`, before this task; neither exists as a
*build*-job leg with its own package install after it) needs no extra step because the runner image
already carries `libdbus-1-dev` and OpenSSL headers — measured indirectly, since `lint`/`test`/`system`
already build this workspace on `ubuntu-latest` today and pass. `manylinux_2_28` does not carry
either by default, so the container invocation installs them explicitly:

```bash
dnf install -y dbus-devel openssl-devel perl-core make
```

— the same package family `mixengine-packages`' own AlmaLinux 8 recipe already installs for the PHP
7.x/Ruby builds, named here rather than reached for blind. `perl-core`/`make` are `openssl-sys`'s
build requirements if the crate ever falls back to compiling rather than linking the container's
OpenSSL; named defensively and checked, not assumed, by the first container run.

**Deliberately not** a `dbus-secret-service`/`vendored` Cargo feature flip. That would remove the
`dnf install` line but would change every Linux leg's binary — including `test`, `lint` and `system`,
none of which this task otherwise touches — to statically link libdbus rather than load the system
one, which is a bigger behavioural change than "the release binary now also runs on an older glibc"
and belongs to a task that weighs it on its own, not to a side effect of fixing two CI legs.

### D7 — Artifact matrix

| OS | Runner | Target | Container |
| --- | --- | --- | --- |
| Windows x86_64 | `windows-latest` | `x86_64-pc-windows-msvc` | — |
| Windows aarch64 | `windows-11-arm` | `aarch64-pc-windows-msvc` | — |
| macOS universal | `macos-latest` | (both, via `lipo`, unchanged) | — |
| Linux x86_64 | `ubuntu-latest` | `x86_64-unknown-linux-gnu` | `manylinux_2_28_x86_64` |
| Linux aarch64 | `ubuntu-24.04-arm` | `aarch64-unknown-linux-gnu` | `manylinux_2_28_aarch64` |

New file names, following D9 of the T85 design exactly:
`mixengine-<version>-windows-aarch64-setup.exe`, `mixengine-<version>-windows-aarch64.zip`,
`mixengine-<version>-linux-aarch64.AppImage`, `mixengine_<version>-1_arm64.deb`,
`mixengine-<version>-1.aarch64.rpm`. Every existing x86_64/universal name is unchanged.

### D8 — `ci.yml`'s `build` job: five matrix entries, install steps keyed on more than `runner.os`

```yaml
strategy:
  fail-fast: false
  matrix:
    include:
      - os: windows-latest
        target: x86_64-pc-windows-msvc
      - os: windows-11-arm
        target: aarch64-pc-windows-msvc
      - os: macos-latest
        target: ""
      - os: ubuntu-latest
        target: x86_64-unknown-linux-gnu
        container: quay.io/pypa/manylinux_2_28_x86_64:<pinned digest or tag>
      - os: ubuntu-24.04-arm
        target: aarch64-unknown-linux-gnu
        container: quay.io/pypa/manylinux_2_28_aarch64:<pinned digest or tag>
```

`runner.os`-keyed steps (installing `rpm`/`desktop-file-utils`, installing NSIS via choco, calling the
right per-OS script) are unchanged, because GitHub reports the same `runner.os` for an arm64 runner as
its x64 counterpart. The per-OS script invocation gains `MIX_TARGET="${{ matrix.target }}"` and, for
Linux, `MIX_CONTAINER="${{ matrix.container }}"` in the step's `env:`, which the scripts read instead
of a new CLI flag threaded through every `run:` line.

**A smoke step runs first on each of the two new legs, ahead of the full packaging pipeline**:
`cargo build --release --locked --target <target> -p mixengine-cli` alone (natively for Windows,
inside the container for Linux), asserting the binary's own `rustc -vV`-derived triple prints back
what was asked for. A leg where the toolchain, the emulation or the container is wrong fails in
under a minute with a plain compiler error, instead of forty minutes into a full workspace release
build with three binaries and three packaging tools. This is the plan's first task (see below) and
stays in the workflow afterward — it costs one more `cargo build -p` invocation, mostly served from
the same incremental cache the full build then reuses, and it is what turns "the arm64 runner doesn't
exist the way the search results said" into a five-line CI log instead of a cancelled sixty-minute job.

### D9 — `build-and-release.md`'s targets table gains the floor and the fifth/sixth rows

```
| OS | Targets | Installer |
| Windows | x86_64-pc-windows-msvc, aarch64-pc-windows-msvc | NSIS per-user installer + a portable zip |
| macOS | x86_64-apple-darwin, aarch64-apple-darwin → universal binary | .pkg |
| Linux | x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu (glibc 2.28 floor, `manylinux_2_28`) | AppImage + .deb + .rpm |
```

The paragraph explaining T85/T85a's split is replaced with one stating what actually shipped: five
`build` legs, native on every one, and the reason "cross toolchain" no longer describes any of them.

## Data flow

```
build job, per matrix row
  ├─ Windows/macOS row: packaging/{windows,macos}/build.sh
  │    └─ stage.sh --target <own host triple>       →  cargo build --release --target …  (native)
  │
  └─ Linux row: packaging/linux/build-{deb,rpm,appimage}.sh
       └─ stage.sh --target <own host triple> --container <manylinux_2_28 image>
            └─ docker run --user "$(id -u):$(id -g)" -v repo:/work <image>
                 ├─ rustup-init --default-toolchain <rust-toolchain.toml's channel>
                 └─ cargo build --release --target …               →  target/<target>/release/*
            (host, afterward) dpkg-deb / rpmbuild / appimagetool over the same target/ tree
```

## Testing

**In CI, and only in CI** — this task adds no unit test, the same way T85's own D11 named `build`
itself as the test. Two additions to what T85 already checks:

- The smoke step (D8) on the two new legs, checked before the full pipeline runs.
- Every existing "open what was just built and check the three binaries are in it" assertion in the
  four per-OS scripts runs unchanged on the new legs, because they read `MIX_BINARIES` and the
  staged directory rather than anything architecture-specific.

**Not tested here, and named rather than left implicit**: nothing runs the new `aarch64` binaries.
CI has no arm64 target to execute *against* beyond the build itself (`windows-11-arm` and
`ubuntu-24.04-arm` could run what they just built, being the right architecture, but `test`/`system`
do not build a release artifact and this task does not extend them to). A clean-VM install-and-run
smoke test is T87's, on all six artifacts, not four.

## Risks, and where each is answered

- **The arm64 runner claims above come from documentation and search results, not from this
  repository's own Actions history.** D8's smoke step is the mitigation: the first real answer is a
  cheap, early CI step, not an assumption this design leaves unchecked. If `windows-11-arm` or
  `ubuntu-24.04-arm` turn out unavailable to this repository for a reason none of the sources surfaced
  (queue policy, a plan restriction), the smoke step's job fails at "no runner matched the label"
  before anything else runs, and the fallback is the cross-compilation path the roadmap originally
  asked for — not designed here, because it should not be designed twice on spec.
- **`git-bash` on `windows-11-arm` may run under x86 emulation.** D4 avoids depending on the answer;
  the smoke step's printed triple is where this is actually observed.
- **`manylinux_2_28` may not carry every package D6 asks for**, or may name it differently than
  AlmaLinux's own `dbus-devel`/`openssl-devel` (the image is AlmaLinux-derived but not identical).
  Checked by the container leg's own build failing loudly on a missing header, in the same way every
  `mixengine-packages` recipe found its package list by a failed compile rather than by reading a
  manifest.
- **A cold `cargo build --release` inside a container, twice, is the slowest part of this job.**
  Accepted in D3. If it pushes the job past its 60-minute timeout, the fix is a container-local cache
  volume keyed by `Cargo.lock`'s hash, which is worth adding only once it is known to be needed.
- **`quay.io/pypa/manylinux_2_28_aarch64` pulled on `ubuntu-24.04-arm` is still a download of a large
  image**, on a runner class that is newer and less proven than `ubuntu-latest`. If image pulls are
  slow or flaky there, that is a fact about the arm64 runner fleet this task's first CI run will
  surface, not something to pre-solve.
- **Two glibc floors, if the two Linux legs ever drift** — `manylinux_2_28_x86_64` and
  `manylinux_2_28_aarch64` are published together and versioned together upstream, so drift would be
  this task pinning them to different tags by mistake, not an upstream property. The pinned tag is
  recorded in one place (the CI matrix) rather than per-script, so there is one line to keep in step.

## What this leaves

- **T85b** — `ServiceInstaller`, unaffected by which architectures `build` produces.
- **T86 / T86a / T94** — signing and SmartScreen/Gatekeeper now have five more artifact identities to
  eventually cover, none of it designed here.
- **T87** — the clean-VM uninstall proof, still owed on all six artifacts and not exercised by this
  task's CI.
- **A cache for the container-based Linux legs**, if the cold build noted in Risks turns out to cost
  more than the timeout allows.
