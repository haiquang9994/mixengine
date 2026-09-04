# 0015. The privileged helper installs itself, and the installer does not

**Status**: Accepted
**Date**: 2026-09-04

## Context

[ADR 0005](0005-on-demand-elevation.md) puts every privileged operation in one short-lived binary,
`mixengine-elevate`, and [security-model.md](../architecture/security-model.md) states the condition
that makes that worth anything: *"if `mixengine-elevate` is installed somewhere the user can write,
malware running as the user could replace it and gain root the next time the user approves a
prompt."* Until T85 nothing installed it anywhere — `mixengine_core::elevation::helper` looked
beside the running program and nowhere else, which was true and sufficient for as long as a release
was a `cargo build`.

[build-and-release.md](../operations/build-and-release.md) already said what should happen: *"Places
`mixengine-elevate` in a **root-owned** directory … it must not sit anywhere the user can write."*
It said the **installer** does it, one line under *"Places `mixengined` and `mix` (per-user location,
so updates need no UAC)"*. Three facts, measured while writing T85, are what that line does not
survive:

1. **Four of the six shipped formats install entirely as the user.** A per-user NSIS installer runs
   under the user's own token by construction; a portable zip is unzipped by the user; an AppImage is
   not installed at all; and a `cargo build` is what every developer has. Only the `.deb`, the `.rpm`
   and the macOS `.pkg` run as root. A security property that most users never get is one nothing in
   the product can state truthfully — `mix status` would have to say *"it depends how you
   installed it"*.
2. **`/usr/local` is not root's on every Mac.** Homebrew on Intel takes ownership of it for the
   installing user, which makes the directory that line names the *worst* available choice on that
   machine rather than a neutral one.
3. **The helper reads its own destination**, and the process that hands it an environment is the one
   it is written not to trust.

## Decision

**`PrivilegedOp::HelperInstall {}` is the mechanism, and no installer is.**

- The operation **carries no fields**. What is copied is `std::env::current_exe()` — the image of the
  process the user has already approved this run of — and where it goes is a constant compiled into
  that binary. A `HelperInstall { source }` would hand a compromised daemon a primitive it does not
  have: *copy this file, as root, into a directory only root can write*.
- It is enqueued at every daemon start, beside the resolver wiring, the CA install and the port
  grant, so it is applied **inside the prompt first-run setup already costs**. The lifetime budget in
  [security-model.md](../architecture/security-model.md) does not change.
- The destination is per OS: `%ProgramFiles%\MixEngine\mixengine-elevate.exe`,
  `/Library/PrivilegedHelperTools/dev.mixengine.elevate`,
  `/usr/local/libexec/mixengine/mixengine-elevate`. macOS uses the directory the system designates
  for a privileged helper rather than `/usr/local`, for the reason above. Windows asks
  `SHGetKnownFolderPath` rather than reading `%ProgramFiles%`.
- **Resolution prefers the installed copy, falls back to the copy beside the program when nothing is
  installed, and refuses outright when something *is* installed and it is not an administrator's.**
  Falling back in that last case would be running the weaker configuration at exactly the moment
  somebody arranged for it.
- A `.deb`, an `.rpm` or a `.pkg` ships the helper at the same path anyway, because it runs as root
  and can. The operation then answers `AlreadyDone`. That makes a system package an **optimisation
  of one mechanism** rather than a second mechanism.

## Consequences

- One story on six formats: what a machine ends up running as root does not depend on how MixEngine
  arrived on it.
- **The first prompt still runs the copy beside the daemon**, because on a machine where nothing is
  installed yet that is the only candidate there is. Malware that replaced it before first run gets
  root once — which is what happens today at *every* prompt — and, with this decision, gets installed
  as the permanent helper. That is a durable compromise where the status quo was a repeated one, and
  it is stated rather than counted as a win. The only thing that closes it is a signature the
  operating system checks before the prompt: **T94**'s question, and **T88a**'s check.
- Replacing the helper across an upgrade becomes possible, and is a queued operation applied behind
  the same explicit prompt — which is what
  [security-model.md](../architecture/security-model.md)'s auto-update boundary asks for. What
  **T88a** adds is the minisign verification that decides whether the new binary deserved that
  prompt at all.
- `mixengine-elevate` gains one `unsafe` block, for `SHGetKnownFolderPath`. That is a cost in the one
  binary whose design constraint is being readable in a sitting, taken because it removes a question
  the binary would otherwise have to answer about who chose its environment.
- Uninstall (**T87**) gains a second root-owned file outside `MIXENGINE_HOME` to remove, beside the
  audit log it already owed one to.

## Alternatives considered

- **An installer that elevates once.** Loses the UAC-free update the per-user Windows installer
  exists for, and is simply impossible for the portable zip, the AppImage and a `cargo build` — so it
  would have to coexist with this mechanism rather than replace it.
- **`HelperInstall { source: PathBuf }`.** Rejected in one line: it is `Exec { cmd }` with two more
  steps, and the closed-enum rule in the security model exists to refuse that shape.
- **Leaving the helper beside the program.** The status quo, and the thing T85 exists to change.
  Every prompt would go on running a file anything running as the user can rewrite.
- **`/usr/local/libexec` on macOS too**, for symmetry with Linux. Refused on fact 2 above: symmetry
  that puts the file in a Homebrew-owned directory is symmetry with no security in it.
