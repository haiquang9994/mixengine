# Build, CI and release

## Local development

```bash
cargo check --workspace --all-targets        # fastest loop
cargo clippy --workspace -- -D warnings
cargo test --workspace                        # unit + component + integration
cargo run -p mixengine-daemon -- --log-level debug   # foreground; --detach backgrounds it
cargo run -p mixengine-cli -- status
```

Rust only — there is no `apps/` and no frontend toolchain
([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)).

Environment knobs: `MIXENGINE_HOME` (isolated sandbox root — always set this when experimenting),
`MIXENGINE_LOG_FORMAT=json`, `MIXENGINE_SYSTEM_TESTS=1`, and the pair `MIXENGINE_INDEX_URL` +
`MIXENGINE_INDEX_KEY` (`--index-url` / `--index-key`), which point `mixengined` at another package
index. Only together: the signature requirement stays, and nobody but us can sign with the key
compiled in — so a URL that moved while the key did not would be a setting that can only ever fail.

**`MIXENGINE_ALLOW_MISSING_PACKAGES=1` is for one script and one situation.**
`.github/scripts/test-no-network.sh` refuses to run at all when a real-server package it expects is
not unpacked, because a suite that quietly does not run is a green tick over nothing. Set this to run
that script by hand without the archives, and the refusals go back to being warnings. Nothing in CI
sets it, and the day something does is the day the Linux leg stops meaning what it says.

### After changing a `sqlx::query!`

`sqlx::query!` checks its SQL against a real database **while compiling**, which is what turns a
misspelled column into a build error instead of a bug found at runtime. Nobody building MixEngine
has such a database, so the answers are committed as `.sqlx/` and every build without a
`DATABASE_URL` reads those instead of connecting. Ordinary builds therefore need nothing.

Editing or adding a query means regenerating them, and committing the result with the code:

```bash
cargo install sqlx-cli --no-default-features --features sqlite,rustls   # once
export DATABASE_URL=sqlite:target/sqlx-dev.db                           # ignored by git, like all of target/
cargo sqlx database create
cargo sqlx migrate run --source crates/mixengine-core/migrations
cargo sqlx prepare --workspace -- --all-targets --all-features
```

Forgetting the last step is invisible on the machine that made the change — `DATABASE_URL` is still
set there — and breaks everyone else's build. That is the one failure `lint` runs
`cargo sqlx prepare --check` for.

**Do not put `DATABASE_URL` in a `.env` file.** sqlx reads one automatically, and a stale database
sitting where every build finds it silently replaces the committed answers with whatever that file
happens to contain.

## CI matrix

CI fires by itself on `master` and on nothing else — a workspace that compiles for three operating
systems is worth a runner when you are asking a question, not on every work-in-progress save. Every
other branch asks for its own answer: push the branch under its own name, then request a run on it.

```bash
git push origin HEAD
gh workflow run ci.yml --ref "$(git branch --show-current)"
gh run list --branch "$(git branch --show-current)" --limit 1
```

The run carries the branch that asked, so two questions in flight stay apart. A second request on
the same branch cancels the first, because by then you have stopped caring about that answer.

| Job | Runner | Runs |
| --- | --- | --- |
| `lint` | ubuntu | `fmt`, `clippy -D warnings`, `cargo deny` (licences + advisories), `sqlx prepare --check` |
| `test` | windows / macos / ubuntu | unit + component + integration, network egress blocked, one real Caddy (below), the connection count against a socket that really is connected, `cargo doc -D warnings` for the runner's own OS |
| `system` | windows / macos / ubuntu, elevated | `#[ignore]`d system tests, and the only place `MIXENGINE_SYSTEM_TESTS=1` is set — on every run of the workflow |
| `bench` | windows / macos / ubuntu | performance budgets from [../standards/testing.md](../standards/testing.md), in a **release** build |
| `bindings` | ubuntu | regenerates ts-rs bindings and fails if the committed output differs |
| `build` | windows, windows arm64, macos, ubuntu, ubuntu arm64 | release binaries + installers for both architectures per OS (macOS ships one universal artifact), uploaded as artifacts |
| `release` | ubuntu | **on a `v*` tag only**: gathers the five legs' artifacts, writes `latest.json`, signs each with the updater key, verifies what it published, and leaves a **draft** GitHub Release a person publishes |

**Five of those six exist today**: `lint`, `test`, `bench`, `system` — which arrived with T40, the
first `#[ignore]`d system test — and `build`, which arrived with T85, the task that produced
something to install. `bindings` arrives with the work that gives it something to run, T56, and
`.github/workflows/ci.yml` says so in its opening comment. The consequence is worth naming: until
`bindings` exists, a `ts-rs` type whose committed output has drifted is caught by a person or by
nobody.

**T88 added one step to `release` and one artifact to `build`.** The step is `packaging/feed.sh`,
which writes `latest.json` into the distribution directory **between** gathering the legs and signing
them — it is written there rather than in a leg because no leg can see the other four, and before the
signing rather than after because being in that directory *is* how it gets signed. The artifact is
the update payload: a plain `mixengine-<version>-<os>-<arch>.(zip|tar.gz)` of the release's binaries,
which is the only thing `mix self-update` can apply, since every installer either needs root or is a
file the user placed. `packaging/README.md` has the shape of both.

**The feed's notes come from `git` and not from GitHub**, and the ordering is why: `--generate-notes`
runs when the draft is created, which is after the signing is over, so notes GitHub wrote cannot be
inside a document that was already signed. Re-signing afterwards would put the private key on the
machine of whoever edits the draft, which is the one thing T86 arranged not to need. So `feed.sh`
writes the tag's own commit subjects and a `notes_url` pointing at the page a person may improve
afterwards.

**T86 added `release`, and a second job that is not in the table**: `preflight`, which answers in
thirty seconds the three questions that would otherwise fail an hour into a release — the tag matches
the workspace version, `packaging/updates.pub` matches the key this build pins, and both signing
secrets are set. Neither runs except on a `v*` tag, which is also the run that makes release-checklist
item 1 something CI asserts rather than a person.

**`test` downloads one thing, and it is a server.** `crates/mixengine-cli/tests/caddy.rs` (T31) is
the only suite in the workspace that judges a recipe against the program it configures, which cannot
be faked: whether Caddy accepts a generated Caddyfile — with a Windows path in it — is a question
only Caddy answers. So the job fetches a pinned Caddy from `mixengine-packages`' own release before
the network is taken away, points `MIXENGINE_CADDY_PACKAGE` at it, and runs that suite `--ignored`.
It is a **fixture and not an install**: nothing checks a signature or a hash there, because
`core::index` and `core::install` are what do that and both have suites of their own. Run it by hand
the same way:

```bash
MIXENGINE_CADDY_PACKAGE=/somewhere/caddy cargo test -p mixengine-cli --test caddy -- --ignored
```

It stays a step in `test` rather than becoming a job: it needs the same debug build every other
correctness answer needs, and a job of its own would compile the workspace a second time to run one
test. `#[ignore]` is what keeps it out of a run that has no Caddy — and what makes that visible,
since a skipped test is reported and a test that returned early is not.

`bench` is on all three runners rather than on ubuntu alone, which is what this table used to say.
The budget it gates is the same everywhere; what it stands in front of is not one mechanism, since
the shim `exec`s on Unix and starts a child inside a Job Object on Windows — and the wall clock it
reports beside the gate is the only place that difference is written down as a number. It is a job
of its own rather than a step in `test` because these tests are `#[ignore]`d and need a release
build, which is a second compilation no correctness answer should wait behind. Run one by hand the
way CI does, `--test-threads=1` included:

```bash
cargo build --release -p mixengine-testkit --bin fakeservice
cargo test --release -p mixengine-shim --test overhead -- --ignored --nocapture --test-threads=1
```

Both lines matter. Selecting one test target does not build `fakeservice`, so a release copy from an
earlier build is used as it is; and the two benchmarks each spend their whole time creating
processes, so run in parallel each measures the other.

**Four budgets since T72a**, each its own step so that a red job names what went red without anybody
opening a log: the shim's overhead, the **idle footprint**, the **cold path**, and M3's warm start.
The footprint step runs a daemon and a real Caddy with nothing else, reads them through
`mix metrics` thirty seconds after the last command, gates `mixengined` alone and prints the total
beside it. It needs only the Caddy the fetch step above already pulls, and no `dbus-run-session`
wrapper: nothing in it starts a MariaDB, so nothing in it has a password to store.

The cold-path step needs that Caddy and **three PHPs**, fetched into three directories of their own
and named in `MIXENGINE_PHP_RUNTIMES`. Three because a pool is only cold once, so three rounds need
three pools, which need three runtime installs — and three *versions* because two of them predate
`pm.status_listen`, which is what holds T72a's idle probe to working on every PHP this product
offers. The step spends about ninety seconds waiting for the idle sweeper before it measures
anything, and pays that wait once for all three rounds.

**Both run before M3 deliberately.** A failing step ends its job, and M3 starts three servers eight
times over and is bimodal on ubuntu — the first run of the footprint budget was skipped on that
runner for exactly that reason, which is a measurement lost to somebody else's bad minute. Cheap
independent measurements go first, and the cold path needs the rule more than the footprint does:
losing it costs a step that had already stood still for a minute and a half.

```bash
cargo build --release -p mixengine-daemon --bin mixengined
MIXENGINE_CADDY_PACKAGE=/path/to/unpacked/caddy \
  cargo test --release -p mixengine-cli --test idle_footprint -- --ignored --nocapture

MIXENGINE_CADDY_PACKAGE=/path/to/unpacked/caddy \
MIXENGINE_PHP_RUNTIMES=/path/to/php-7.0.33:/path/to/php-7.4.33:/path/to/php-8.3.33 \
  cargo test --release -p mixengine-cli --test cold_path -- --ignored --nocapture
```

The first line is not optional and the reason is the same shape as `fakeservice`'s: `cargo test -p
mixengine-cli` builds `mix` and **not** `mixengined`, so the suite drives whichever daemon was built
last — which, while this budget was being written, was one still carrying the bug it had been written
to find. **It cost the cold path an hour too**: a 502 that had already been fixed went on being
measured, because the suite was still starting the daemon from before the fix.

## Targets

| OS | Targets | Installer |
| --- | --- | --- |
| Windows | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | NSIS per-user installer + a portable zip |
| macOS | `x86_64-apple-darwin`, `aarch64-apple-darwin` → universal binary | `.pkg` |
| Linux | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, both against glibc 2.28 | AppImage + `.deb` + `.rpm` |

**What T85 built is the host architecture of each row, natively; T85a built the rest, also
natively.** GitHub's own arm64-hosted runners (`windows-11-arm`, `ubuntu-24.04-arm`) — free and GA for
this repository, which is public — turned the second `aarch64` row into another native leg rather than
a cross-compilation of the first, the same way macOS has always built both of its slices on Apple's
own toolchain. The one real toolchain question left was the glibc floor: both Linux legs compile
inside a pinned `manylinux_2_28` container, matching the floor `runtime-packaging.md` already measured
for PHP 7.0–8.0, so a `.deb` built today keeps running on the LTS distributions it is aimed at rather
than on whatever glibc the `build` job's runner happens to ship this month.

**macOS ships a `.pkg` and not the `.dmg` this table used to name.** A disk image is a carrier for
something you drag out of it, and the thing that used to be dragged was an application bundle
[ADR 0011](../decisions/0011-no-gui-in-this-repository.md) deleted; what is left to ship there is
three command-line binaries. A `.pkg` also runs as root, which is what lets it place the privileged
helper at install time — see [ADR 0015](../decisions/0015-the-helper-installs-itself.md).

## What the installer does

1. Places `mixengined` and `mix` (per-user location on Windows, so updates need no UAC;
   `/usr/local/bin` from the `.pkg` and `/usr/bin` from the `.deb` and the `.rpm`).
2. **Does not place `mixengine-elevate`. MixEngine does that itself** — the operation
   `PrivilegedOp::HelperInstall`, applied inside the elevation prompt first-run setup already costs
   ([ADR 0015](../decisions/0015-the-helper-installs-itself.md)). It goes to
   `%ProgramFiles%\MixEngine\`, `/Library/PrivilegedHelperTools/` or
   `/usr/local/libexec/mixengine/`, and it must not sit anywhere the user can write. The `.deb`, the
   `.rpm` and the `.pkg` ship it at that same path anyway, because they run as root and can — and
   the operation then answers `AlreadyDone`. **The four ways of installing that run entirely as the
   user** — the per-user Windows installer, the portable zip, the AppImage, and a `cargo build` —
   are why this cannot be a packager's job.
3. **Does not register daemon autostart either. MixEngine does that too** — `autostart.enable`,
   reachable from `mix autostart enable`, and by nothing an installer runs
   ([ADR 0016](../decisions/0016-autostart-is-registered-by-mixengine.md)). Item 2's argument
   reversed: a logon task lives under one account's SID, a LaunchAgent in one user's
   `~/Library/LaunchAgents`, a systemd *user* unit in one user's `~/.config` — so the three formats
   that run as root are precisely the three that cannot know which account will use MixEngine, and
   the three that run as the user are not where a "may I start something at every login" question
   belongs. Nothing about it is elevated on any of the three systems, and `mix autostart disable`
   takes it away without stopping a running daemon.
4. Puts its own directory on this user's PATH, so `mix` is runnable. **Not `<root>/bin`**, which is
   the directory of runtime shims and belongs to `path.install`: the two therefore write different
   segments of one value and each removes only its own, which is what makes two authors safe. On
   Windows the edit carries a guard — NSIS's `ReadRegStr` truncates at `NSIS_MAX_STRLEN`, so a PATH
   at or above that length is left alone with a line on screen saying so.
5. **Does not** install the CA, resolver config, port grant, or any runtime — those happen on first
   use, batched into a single elevation prompt, so a fresh install changes as little as possible.

## Packaging

The scripts live in [`packaging/`](../../packaging/), one directory per OS, each run on that system;
there is no cross-packaging, which is why the `build` job is three legs.

```bash
bash packaging/windows/build.sh         # a per-user installer and a portable zip
bash packaging/macos/build.sh           # one universal .pkg
bash packaging/linux/build-deb.sh       # .deb
bash packaging/linux/build-rpm.sh       # .rpm
bash packaging/linux/build-appimage.sh  # AppImage
```

Everything lands in `target/packaging/dist/` with a `.sha256` beside it. **A checksum is not a
signature** and is not offered as one: it is what lets a person who downloaded twice tell whether
they got the same file. The signature is `packaging/sign.sh`, which T86 added and which the `release`
job runs over that same directory — `.sha256` files are not signed, because a signature over a
checksum is a weaker way of saying what the signature over the artifact already says.

Each script ends by opening the artifact it just made and asserting the three binaries are in it —
`unzip -l`, `7z l`, `pkgutil --payload-files`, `dpkg-deb -c`, `rpm -qlp`, and for the AppImage a run
of the thing itself. A packaging script that silently produced an empty archive is the failure this
whole job exists to prevent, and it is not one CI notices by itself.

The version comes from `[workspace.package]` in the root `Cargo.toml`, read by every script, so
cutting a release is a version bump and nothing else.

The elevated helper creates its own audit log on first run — `%ProgramData%\MixEngine\elevate.log`,
`/Library/Logs/MixEngine/elevate.log`, `/var/log/mixengine/elevate.log` — which is the first thing
MixEngine leaves outside `MIXENGINE_HOME`. Removing it is itself a privileged operation, so
`mix uninstall` owes it one (**T87**, the complete uninstall path). T47's `mix doctor` reports it and
does not remove it — a diagnostic that deleted a root-owned audit trail would be deleting the record
of what it was diagnosing.

**Since T85 there are two such files, not one.** The helper the operation in item 2 installs is
root-owned and outside `MIXENGINE_HOME` for exactly the audit log's reason, and removing it needs the
same privileged operation of its own. T87 owes both.

Uninstall reverses all of it: stop services, remove the hosts block, resolver/NRPT rule, firewall
rules, port grant, CA from every store, autostart entries, PATH entry. It asks before deleting
`data/` and prints exactly what it kept.

## Signing

**MixEngine ships without OS code signing.** Two different signatures are involved and only one is
in use — see [../features/updates.md](../features/updates.md) for the full table and consequences.

- **Updater signature (minisign / Ed25519)** — free, and the thing that actually protects users from
  a tampered update. It was mandatory while the updater was Tauri's; now it is ours by choice, and
  the choice does not change ([ADR 0011](../decisions/0011-no-gui-in-this-repository.md)).
  **Built by T86**: the private half is this repository's `UPDATE_SECRET_KEY` and `UPDATE_PASSWORD`
  secrets, the public half is committed as `packaging/updates.pub` and pinned as
  `core::updates::PUBLIC_KEY`, and `packaging/sign.sh` verifies every signature it makes back against
  that pinned key — so a run signed by a secret that is not its pair fails before anything is
  uploaded. Rotating the key strands every installed copy; read
  [../features/updates.md](../features/updates.md) before doing it.
- **Authenticode / Apple Developer ID** — not purchased. Accepted costs: SmartScreen warnings on
  Windows that reset with every release, and a Gatekeeper rejection of the `.pkg` that since macOS 15
  requires System Settings → Privacy & Security → "Open Anyway" if it is opened in Finder.
  **T86a measured both, and both are narrower than that sentence.** On Windows the installer is the
  only file judged and nothing it writes is judged again; on macOS
  `sudo installer -pkg <file> -target /` installs the quarantined package from a terminal without any
  of that, which for a command-line product is the instruction to document first. See
  [../features/updates.md](../features/updates.md).
- Linux: detached minisign signatures published with the release.

Recommended sequencing: Linux and Windows first, macOS once a Developer ID is available. Revisit
[ADR 0005](../decisions/0005-on-demand-elevation.md) if that changes — signing and the elevation
design are linked decisions.

## Versioning and updates

- SemVer, single version across the workspace, tagged `v0.1.0`. Pre-1.0 the API may break between
  minors; each break is listed in the changelog.
- Auto-update via `mix self-update` against a `latest.json` published on GitHub Releases. Updates
  are **opt-in**, never silent, because an update restarts the user's running services. **Built by
  T88** — [design](../../docs/superpowers/specs/2026-09-04-t88-self-update-design.md): the daemon
  checks at start and on a daily clock, both silent on failure; a release is downloaded, hashed
  against the signed feed, unpacked and *run once* before anything is replaced; and a copy of
  MixEngine that a `.deb`, an `.rpm`, a `.pkg` or an AppImage installed is refused in words rather
  than updated in place.
- **`mixengine-elevate` is excluded from auto-update** and is replaced only through its own explicit
  elevation prompt. This is a security boundary, not a convenience choice.
- The daemon and clients negotiate a protocol version on connect; so do the daemon and
  `mixengine-elevate`. An old elevate keeps serving the operations it knows while the app asks the
  user to upgrade it.

## Release checklist

1. `cargo deny` clean, all CI green on all three OSes.
2. Bump version, update `CHANGELOG.md`, verify the migration path from the previous release with a
   real upgrade test (old `mixengine.db` → new binary).
3. Push the tag `v<version>`. CI runs everything, signs every artifact, and leaves a **draft** release
   carrying each artifact with a `.sha256` and a `.minisig` beside it. Nothing is notarised — that is
   the right-hand column of the signing table, and it is not purchased.
4. Smoke-test each installer *from that draft* on a clean VM: install → create site → HTTPS →
   uninstall → verify nothing left behind. Then edit the notes and publish the draft by hand.

   **And take the two readings no machine can take** — roadmap task **T86a**, whose other half is
   measured by `packaging/*/probe.sh` on every run of the `build` job. Both need a *browser* download
   of the *published* asset, because that is what applies the mark the operating system reads; a file
   copied onto the VM by any other route is not the file a user gets. Record what happened in
   [../features/updates.md](../features/updates.md), beside the measured readings:

   1. **Windows.** Download `…-setup.exe` in Edge from the release page and double-click it in
      Explorer. Did "Windows protected your PC" appear? What did the publisher line say? Was "More
      info → Run anyway" needed to get past it? Then do the same with the portable `.zip`: extract it
      *in Explorer* and run `mix.exe` — the probe's W4 measures that Explorer marks all three
      binaries, so this is where the count of warnings a user sees is confirmed.
   2. **macOS 15+.** Download the `.pkg` in Safari and double-click it in Finder. Record the dialog,
      then the System Settings → Privacy & Security → "Open Anyway" path and how many steps it took.
      The probe's M4 already answers `installer(8)` from a terminal; what this adds is the flow a
      person who did not read the instructions will actually meet.

   **The SmartScreen half is inherently a two-release reading and is therefore taken twice**: once
   here, and again on the release after this one, to confirm the warning returned. It is expected to
   — reputation with no publisher identity accrues to a file hash, which is the probe's W1 — and the
   point of taking it is that the prediction is checked rather than assumed.
5. Publish the updated package index if runtimes changed
   ([runtime-packaging.md](runtime-packaging.md)).
