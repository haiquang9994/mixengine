# Build, CI and release

## Local development

```bash
cargo check --workspace --all-targets        # fastest loop
cargo clippy --workspace -- -D warnings
cargo test --workspace                        # unit + component + integration
cargo run -p mixengine-daemon -- --log-level debug   # foreground; --detach backgrounds it
cargo run -p mixengine-cli -- status
npm --prefix apps/desktop install
npm --prefix apps/desktop run tauri dev       # GUI against the running daemon
```

Environment knobs: `MIXENGINE_HOME` (isolated sandbox root — always set this when experimenting),
`MIXENGINE_LOG_FORMAT=json`, `MIXENGINE_SYSTEM_TESTS=1`, and the pair `MIXENGINE_INDEX_URL` +
`MIXENGINE_INDEX_KEY` (`--index-url` / `--index-key`), which point `mixengined` at another package
index. Only together: the signature requirement stays, and nobody but us can sign with the key
compiled in — so a URL that moved while the key did not would be a setting that can only ever fail.

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
| `lint` | ubuntu | `fmt`, `clippy -D warnings`, `cargo deny` (licences + advisories), `sqlx prepare --check`, ESLint, `tsc --noEmit` |
| `test` | windows / macos / ubuntu | unit + component + integration, network egress blocked, `cargo doc -D warnings` for the runner's own OS |
| `system` | windows / macos / ubuntu, elevated | `#[ignore]`d system tests — on `master`, and on a requested run whose branch touches `platform`/`elevate` |
| `bench` | windows / macos / ubuntu | performance budgets from [../standards/testing.md](../standards/testing.md), in a **release** build |
| `bindings` | ubuntu | regenerates ts-rs bindings and fails if the committed output differs |
| `build` | all three | release binaries + installers, uploaded as artifacts |

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

## Targets

| OS | Targets | Installer |
| --- | --- | --- |
| Windows | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | MSI (WiX via Tauri) + a portable zip |
| macOS | `x86_64-apple-darwin`, `aarch64-apple-darwin` → universal binary | `.dmg`, notarised |
| Linux | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | AppImage + `.deb` + `.rpm` |

Linux builds link against an old glibc (build in a manylinux-style container) so binaries run on
LTS distros.

## What the installer does

1. Places `mixengined`, `mix` and the GUI (per-user location, so updates need no UAC).
2. Places `mixengine-elevate` in a **root-owned** directory (`%ProgramFiles%\MixEngine\`,
   `/usr/local/libexec/`) — it must not sit anywhere the user can write.
3. Registers daemon autostart (logon task / LaunchAgent / systemd **user** unit).
4. Adds `<root>/bin` to PATH.
5. **Does not** install the CA, resolver config, port grant, or any runtime — those happen on first
   use, batched into a single elevation prompt, so a fresh install changes as little as possible.

Uninstall reverses all of it: stop services, remove the hosts block, resolver/NRPT rule, firewall
rules, port grant, CA from every store, autostart entries, PATH entry. It asks before deleting
`data/` and prints exactly what it kept.

## Signing

**MixEngine ships without OS code signing.** Two different signatures are involved and only one is
in use — see [../features/updates.md](../features/updates.md) for the full table and consequences.

- **Updater signature (minisign / Ed25519)** — free, **mandatory in Tauri v2**, and the thing that
  actually protects users from a tampered update. Private key in CI secrets, public key compiled in.
- **Authenticode / Apple Developer ID** — not purchased. Accepted costs: SmartScreen warnings on
  Windows that reset with every release, and a Gatekeeper block on the first macOS launch that since
  macOS 15 requires System Settings → Privacy & Security → "Open Anyway".
- Linux: detached minisign signatures published with the release.

Recommended sequencing: Linux and Windows first, macOS once a Developer ID is available. Revisit
[ADR 0005](../decisions/0005-on-demand-elevation.md) if that changes — signing and the elevation
design are linked decisions.

## Versioning and updates

- SemVer, single version across the workspace, tagged `v0.1.0`. Pre-1.0 the API may break between
  minors; each break is listed in the changelog.
- Auto-update via the Tauri updater against a `latest.json` published on GitHub Releases. Updates are
  **opt-in prompts**, not silent, because an update restarts the user's running services.
- **`mixengine-elevate` is excluded from auto-update** and is replaced only through its own explicit
  elevation prompt. This is a security boundary, not a convenience choice.
- The daemon and clients negotiate a protocol version on connect; so do the daemon and
  `mixengine-elevate`. An old elevate keeps serving the operations it knows while the app asks the
  user to upgrade it.

## Release checklist

1. `cargo deny` clean, all CI green on all three OSes.
2. Bump version, update `CHANGELOG.md`, verify the migration path from the previous release with a
   real upgrade test (old `mixengine.db` → new binary).
3. Build, sign, notarise; smoke-test each installer on a clean VM: install → create site → HTTPS →
   uninstall → verify nothing left behind.
4. Publish the release, then the updated package index if runtimes changed
   ([runtime-packaging.md](runtime-packaging.md)).
