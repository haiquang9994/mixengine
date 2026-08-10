# Build, CI and release

## Local development

```bash
cargo check --workspace --all-targets        # fastest loop
cargo clippy --workspace -- -D warnings
cargo test --workspace                        # unit + component + integration
cargo run -p mixengine-daemon -- --foreground --log-level debug
cargo run -p mixengine-cli -- status
npm --prefix apps/desktop install
npm --prefix apps/desktop run tauri dev       # GUI against the running daemon
```

Environment knobs: `MIXENGINE_HOME` (isolated sandbox root — always set this when experimenting),
`MIXENGINE_LOG_FORMAT=json`, `MIXENGINE_SYSTEM_TESTS=1`.

## CI matrix

| Job | Runner | Runs |
| --- | --- | --- |
| `lint` | ubuntu | `fmt`, `clippy -D warnings`, `cargo deny` (licences + advisories), ESLint, `tsc --noEmit` |
| `test` | windows / macos / ubuntu | unit + component + integration, network egress blocked |
| `system` | windows / macos / ubuntu, elevated | `#[ignore]`d system tests — on `main` and on PRs touching `platform`/`helper` |
| `bench` | ubuntu | performance budgets from [../standards/testing.md](../standards/testing.md) |
| `bindings` | ubuntu | regenerates ts-rs bindings and fails if the committed output differs |
| `build` | all three | release binaries + installers, uploaded as artifacts |

## Targets

| OS | Targets | Installer |
| --- | --- | --- |
| Windows | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | MSI (WiX via Tauri) + a portable zip |
| macOS | `x86_64-apple-darwin`, `aarch64-apple-darwin` → universal binary | `.dmg`, notarised |
| Linux | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | AppImage + `.deb` + `.rpm` |

Linux builds link against an old glibc (build in a manylinux-style container) so binaries run on
LTS distros.

## What the installer does

1. Places `mixengined`, `mix`, `mixengine-helper` and the GUI.
2. Registers daemon autostart (logon task / LaunchAgent / systemd user unit).
3. Adds `<root>/bin` to PATH.
4. **Does not** install the helper, the CA, or any runtime — those happen on first use, with
   consent, so a fresh install changes as little as possible.

Uninstall reverses all of it: stop services, remove the hosts block, resolver/NRPT rule, firewall
rules, CA from every store, autostart entries, PATH entry. It asks before deleting `data/` and
prints exactly what it kept.

## Signing

- Windows: Authenticode on every binary and the MSI.
- macOS: Developer ID signing + notarisation + stapling; the helper is signed with the same team ID
  and the daemon verifies that before trusting it.
- Linux: detached minisign signatures published with the release.

## Versioning and updates

- SemVer, single version across the workspace, tagged `v0.1.0`. Pre-1.0 the API may break between
  minors; each break is listed in the changelog.
- Auto-update via the Tauri updater, checking a signed manifest. Updates are **opt-in prompts**, not
  silent, because an update restarts the user's running services. Never update while a supervised
  service is under load without asking.
- The daemon and clients negotiate a protocol version on connect; a client older than the daemon's
  minimum tells the user to update instead of failing cryptically.

## Release checklist

1. `cargo deny` clean, all CI green on all three OSes.
2. Bump version, update `CHANGELOG.md`, verify the migration path from the previous release with a
   real upgrade test (old `mixengine.db` → new binary).
3. Build, sign, notarise; smoke-test each installer on a clean VM: install → create site → HTTPS →
   uninstall → verify nothing left behind.
4. Publish the release, then the updated package index if runtimes changed
   ([runtime-packaging.md](runtime-packaging.md)).
