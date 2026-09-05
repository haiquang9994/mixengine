# Packaging

What turns a release build into the files a person downloads. One script per operating system, each
run on that system — there is no cross-packaging here, and CI's `build` job is three legs for that
reason.

Design: [`docs/superpowers/specs/2026-09-04-t85-installers-design.md`](../docs/superpowers/specs/2026-09-04-t85-installers-design.md).
Release process: [`.claude/operations/build-and-release.md`](../.claude/operations/build-and-release.md).

## Running it

```bash
bash packaging/windows/build.sh      # on Windows: a per-user installer and a portable zip
bash packaging/macos/build.sh        # on macOS:   one universal .pkg
bash packaging/linux/build-deb.sh    # on Linux:   .deb
bash packaging/linux/build-rpm.sh    #             .rpm
bash packaging/linux/build-appimage.sh  #          AppImage
bash packaging/linux/build-tarball.sh   #          the update payload
```

Every leg additionally publishes its **`mixengine-elevate` on its own** —
`mixengine-elevate-<version>-<os>-<arch>` — which is the one artifact here that exists for a program
rather than for a person. `mix self-update` never replaces the privileged helper, so a release
cannot deliver it inside a payload; what a machine fetches instead is that file and the `.minisig`
`sign.sh` puts beside it, and the signature's trusted comment is what the elevated process reads to
learn which version and which machine the bytes are for. Roadmap task **T88a**, and
[ADR 0018](../.claude/decisions/0018-a-signed-candidate-is-what-lets-a-path-cross-the-boundary.md).

Everything lands in `target/packaging/dist/`, with a `.sha256` beside each artifact. Each script
opens what it just made and checks the four binaries are in it before it exits — an empty archive
is a perfectly valid archive, and nothing else in the pipeline would notice.

**`mixengine-shim` goes beside `mixengined` in every one of them**, because that is the only place
`core::shims::source` looks. An artifact without it installs cleanly, starts, reports itself healthy,
and leaves `<root>/bin` empty — which is every runtime command the product exists to provide
(roadmap task **T85c**). `packaging/common.sh` names the four binaries and the four crates that
produce them, in one place, and `crates/mixengine-core/tests/packaging.rs` fails the build when that
list and the names the code looks for drift apart.

Two pieces here have checks that need no packaging tools and run on any of the three systems, because
what they get wrong is invisible until a release is in somebody's hands:

```bash
bash packaging/linux/apprun-check.sh  # the AppImage's cache really gets every binary
bash packaging/feed-check.sh          # feed.sh over a fixture distribution — see below
bash packaging/bindings.sh --check    # the committed API contract is what the crate generates
```

| OS | Artifacts |
| --- | --- |
| Windows | `mixengine-<version>-windows-x86_64-setup.exe`, `mixengine-<version>-windows-x86_64.zip` |
| macOS | `mixengine-<version>-macos-universal.pkg`, `mixengine-<version>-macos-universal.tar.gz` |
| Linux | `mixengine-<version>-linux-x86_64.AppImage`, `mixengine_<version>-1_amd64.deb`, `mixengine-<version>-1.x86_64.rpm`, `mixengine-<version>-linux-x86_64.tar.gz` |

## The update payload, and the feed

One artifact per OS is not an installer at all: a plain archive of the release's binaries, which is
what `mix self-update` applies — roadmap task **T88**. **None of the five installers can be applied
by an updater**: the `.deb`, the `.rpm` and the `.pkg` need root, and an AppImage is a file the user
placed rather than a directory of binaries. On Windows this artifact is the portable zip, which
already was one; on the other two it is the `.tar.gz` in the table above.

All three hold **one top-level `mixengine/` directory**, which is what lets one `provides` shape in
the feed describe every artifact this project ships — and what stops a zip extracted into `Downloads`
scattering four binaries there.

```bash
bash packaging/feed.sh --tag v0.2.0 --repo mixnz/mixengine
```

`latest.json` lists, per operating system and architecture, the payload's URL, its SHA-256 and its
size, and where each binary sits inside it. **It is written into the distribution directory before
`sign.sh` runs**, so it is signed with everything else and `latest.json.minisig` lands beside it under
the name `mixengine_core::index::Client` appends. That signature is the whole chain of trust: an
installed MixEngine verifies the document before parsing it, and then checks the payload against the
SHA-256 the verified document carries.

`provides` maps each executable's **name** to its path inside the payload — `mixengined`, never
`mixengined.exe`, on every operating system. That is `index::format::Artifact`'s own shape, and it is
what `updates::apply` reads: it appends this platform's executable suffix itself. `feed-check.sh`
runs the script over a fixture distribution and asserts exactly that, because the only sign of
getting it wrong is a `mix self-update` that refuses the release it was offered.

macOS is universal, so its one archive is listed under **both** architectures. The notes are the
tag's own commit subjects, read from `git` — the feed is signed before the draft release exists, so
GitHub's generated notes cannot reach it — with `notes_url` pointing at the page somebody may have
edited afterwards.

The version comes from `[workspace.package]` in the root `Cargo.toml`, so cutting a release is a
version bump and nothing else. Host architecture only: the second architecture on Windows and Linux
is roadmap task **T85a**, and macOS is universal here because Apple's toolchain builds the other
slice with no extra sysroot.

## The API contract

```bash
bash packaging/bindings.sh            # regenerate bindings/ in place
bash packaging/bindings.sh --check    # regenerate into a temp dir and diff; writes nothing
bash packaging/bindings.sh --pack     # archive the committed tree into dist; runs no cargo
```

`bindings/` at the repository root is the MixEngine API as TypeScript: every request, response,
event and error, generated from `mixengine-proto` with `ts-rs` and committed — roadmap task **T56**,
[design](../docs/superpowers/specs/2026-09-05-t56-the-published-api-contract-design.md). MixEngine
ships no graphical client ([ADR 0011](../.claude/decisions/0011-no-gui-in-this-repository.md)), so
that directory *is* the surface such a client is written against.

**Every file in it is generated**, the barrel and its README included, which is what lets `--check`
be a plain `diff -r` with nothing to exclude and what makes a deleted type take its file with it.
Where the files go and how a `u64` is spelled live in `.cargo/config.toml` rather than in the script,
so the obvious command — `cargo test -p mixengine-proto --features ts` — produces exactly the
committed answer.

`--pack` writes `mixengine-api-<version>-typescript.tar.gz`, an installable npm tarball with a single
top-level `package/` and no runtime code in it at all. The version is stamped **there** and is not in
the committed tree, so cutting a release stays a version bump and nothing else. `sign.sh` signs it
beside the binaries, and `feed.sh` leaves it alone: a payload is matched by the
`mixengine-<version>-<os>-…` shape and this is not one.

What the contract states is what the daemon **writes** — a few requests accept more than that, and
[ADR 0020](../.claude/decisions/0020-the-published-contract-is-the-shape-the-daemon-writes.md) is
why those alternatives are not described.

## The user handbook

```bash
bash packaging/docs.sh              # build the site into target/site/
bash packaging/docs.sh --reference  # regenerate docs/guide/en/cli.md from `mix` itself
bash packaging/docs.sh --restamp    # rewrite every Vietnamese page's source_sha256
bash packaging/docs.sh --check      # build into a temp dir, validate it, diff the reference
```

`docs/guide/{en,vi}/` is the handbook: sixteen Markdown pages per language, published at
`https://mixnz.github.io/mixengine/` as HTML **and** as plain Markdown at a predictable address, and
compiled into `mix` so that `mix docs <topic>` answers the same bytes with no network and no running
daemon — roadmap task **T90**,
[design](../docs/superpowers/specs/2026-09-05-t90-the-documentation-site-design.md).

**Unlike `bindings/`, the generated site is not committed**, and the difference is what each is for:
`bindings/` is source code another repository compiles, and this is what a browser receives at a URL.
So `--check` builds into a temporary directory and asserts the shape of what came out; the one thing
it diffs is `docs/guide/en/cli.md`, which is generated by `mix docs --reference` and *is* committed,
because it is a page of the corpus like any other.

**`--restamp` is run after translating a page, never instead of it.** Every Vietnamese page carries
the SHA-256 of the English page it was made from, so editing the English one without revisiting the
Vietnamese one is a failing test rather than a discovery six months later. All the stamp records is
that somebody looked; no machine here can check that a translation is right.

Publishing is `.github/workflows/pages.yml` and not the `release` job: the site follows `master`
rather than a tag, because a handbook that only updated when a version was cut would describe the
previous release for as long as the next one took.

## Signing

```bash
bash packaging/sign.sh          # signs everything in target/packaging/dist
```

Every artifact gets a detached `.minisig` beside it, made with the updater key and **verified back
against the key compiled into `mixengine-core`** before the script returns — so a signature this
product would not accept fails the run rather than reaching a release. `.sha256` files are not signed:
a checksum is for a person who downloaded twice, and a signature over it would be a weaker way of
saying what the signature over the artifact already says. The script also counts, because a release
with one unsigned artifact in it is the failure it exists to prevent.

The private half is not in this repository and never will be. In CI it arrives as
`MIX_SIGN_SECRET_KEY` / `MIX_SIGN_PASSWORD` and is used by one job on one runner; by hand it is read
from `~/.config/mixengine/updates.key` and the password is typed. Roadmap task **T86**,
[design](../docs/superpowers/specs/2026-09-04-t86-updater-signing-design.md).

## Probing

```bash
bash packaging/windows/probe.sh      # on Windows, after windows/build.sh
bash packaging/macos/probe.sh        # on macOS,   after macos/build.sh
```

What an unsigned release looks like to the machines that judge it — roadmap task **T86a**,
[design](../docs/superpowers/specs/2026-09-04-t86a-unsigned-distribution-design.md), findings in
[`.claude/features/updates.md`](../.claude/features/updates.md). Each takes a fixed list of readings
against the artifacts beside it, prints a report, and writes it to `target/packaging/probe/` — which
is **not** `dist/`, because the release job signs and publishes everything it finds in there.

What they measure is the **mark**, not the verdict: SmartScreen is reached through
Mark-of-the-Web and Gatekeeper through `com.apple.quarantine`, so which files ever carry one is a
property of our own artifacts, while the dialog itself needs a browser and a person. That half is
release-checklist item 4 in
[build-and-release.md](../.claude/operations/build-and-release.md).

A reading that came back wrong about a MixEngine artifact **fails**; anything the machine could not
answer is printed as a **void reading** under its own heading, so a green run that measured nothing
cannot be read as a green run that measured and found nothing.

Some readings install for real — the NSIS installer into a temporary directory and this account's
`PATH`, the `.pkg` into `/usr/local/bin` and `/Library/PrivilegedHelperTools` as root. Those are
behind `MIX_PROBE_INSTALL=1`, set by CI's `build` job and nowhere else, and are skipped without it.
Both probes put the machine back as they found it, and the macOS one **refuses to run at all** when
there is already a MixEngine installed: it writes the real paths, and removing them afterwards would
take a real installation with it.

Neither probe ever turns a protection off to obtain a reading. A number measured on a machine we
disarmed is about the tampering rather than about the product.

## What is not here

**No OS code signing.** Authenticode and an Apple Developer ID are not purchased
([ADR 0005](../.claude/decisions/0005-on-demand-elevation.md)). The minisign signature above is the
other column of that table and is not a substitute for it: it says the file is ours, not that the
operating system will run it without a warning.

**No installer places `mixengine-elevate`.** MixEngine installs it itself, inside the elevation
prompt first-run setup already costs — [ADR 0015](../.claude/decisions/0015-the-helper-installs-itself.md).
The `.deb`, the `.rpm` and the `.pkg` ship it at that same path anyway, because they run as root and
can; the operation then finds its work already done. The per-user Windows installer, the portable zip
and the AppImage cannot, which is why the mechanism is not a packager's.

**No autostart entry.** `ServiceInstaller` is roadmap task **T85b**.
