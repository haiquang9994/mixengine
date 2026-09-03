# T85 — Installers, and a helper in a directory the user cannot write (design)

Roadmap task **T85**, phase 9, the first of it: *"Installers: NSIS per-user + portable zip, `.dmg`,
AppImage/`.deb`/`.rpm`; place `mixengine-elevate` in a root-owned directory."* Two halves that read
as one sentence and are not: the first is a packaging pipeline with no Rust in it, the second is a
change to which file this product runs as root — and the second is the reason the first cannot be
written the obvious way.

## Goal

A `build` job produces, for each of the three operating systems, the artifacts a person downloads;
and a MixEngine installed from any one of them ends up running its privileged helper out of a
directory that nothing running as the user can write to. Both halves are true of the AppImage and
the portable zip as well as of the `.deb` — which is what decides the shape of the second half.

## Measured, not assumed

Every line below was read off this workspace or off the machine this was designed on.

- **`elevation::helper()` looks beside the program and nowhere else.**
  `crates/mixengine-core/src/elevation.rs` joins `mixengine-elevate` onto
  `program.parent()`, and its doc says *"Beside whatever is running, and there is no override — the
  T40b design, D9"*, on the reasoning that *"the directory beside `mixengined` is already exactly as
  trustworthy as `mixengined` itself"*. That reasoning holds for a `cargo build` and stops holding
  the moment an installer puts the two binaries in two directories.
- **`.claude/operations/build-and-release.md` already says the installer does it**: *"Places
  `mixengine-elevate` in a **root-owned** directory (`%ProgramFiles%\MixEngine\`,
  `/usr/local/libexec/`) — it must not sit anywhere the user can write"*, one line under *"Places
  `mixengined` and `mix` (per-user location, so updates need no UAC)"*. The two lines are in tension
  and nothing has ever had to resolve it.
- **Four of the six formats cannot write a root-owned file at install time.** A per-user NSIS
  installer runs under the user's own token by construction; a portable zip is unzipped by the user;
  an AppImage is not installed at all; a `.dmg` is a mounted volume somebody drags out of. Only
  `.deb`, `.rpm` and a macOS `.pkg` run as root.
- **There is no `.app` to put in a `.dmg`.** [ADR 0011](../../../.claude/decisions/0011-no-gui-in-this-repository.md)
  withdrew the GUI; what macOS would ship today is three command-line binaries, and the `.dmg` line
  in the roadmap predates that withdrawal.
- **`/usr/local` is not root's on every Mac.** Homebrew on Intel takes ownership of `/usr/local`
  and its subdirectories for the installing user; on Apple Silicon it uses `/opt/homebrew` and
  leaves `/usr/local` alone or absent. A helper installed to `/usr/local/libexec` on the first kind
  of machine sits exactly where the threat model says it must not.
  `/Library/PrivilegedHelperTools` is `root:wheel` and is the directory macOS designates for this.
- **The helper takes a system path out of its own environment, and nothing in this workspace
  establishes what that environment is.** `mixengine_platform::elevated::audit_directory()` reads
  `%ProgramData%`. The three launchers in `windows/prompt.rs`, `macos/prompt.rs` and
  `linux/prompt.rs` scrub nothing and set nothing; whether an elevated child started through
  `ShellExecuteExW`'s `runas` verb receives the caller's environment block is not documented and was
  **not measured here**. That is the whole of the argument: a root process reading a system directory
  out of a variable, where the answer to "who chose that variable" is *we do not know*, is one
  question fewer if it asks the operating system instead.
- **`create_root_owned_directory` already exists** and does exactly what this needs: creates the
  parents, re-asserts the owner and the permissions on every call, and leaves the directory
  world-readable. Written for the audit log; unchanged by this task.
- **`elevated::owner_of` and `others_can_write` already answer the ownership question**, and
  `Owner::is_administrative` is documented as *"the wider question, and the one the audit log's
  directory asks"* — `uid 0` on Unix, `SYSTEM` **or** `BUILTIN\Administrators` on Windows. Both sit
  behind the `elevated` feature, which today means the daemon cannot ask them.
- **`PrivilegedResponse` already carries `elevate_version` and `supported_ops`**, with a doc that
  says the helper *"is installed once and excluded from auto-update, so it drifts behind the daemon
  by design"*. Nothing today can act on that drift, because nothing installs the helper anywhere.
- **CI runs three runners, host architecture only.** `ci.yml`'s matrix is
  `[ubuntu-latest, windows-latest, macos-latest]` and no job passes `--target`. The six-target table
  in `build-and-release.md` describes an intention, not a thing that has ever been built.
- **`ci.yml`'s opening comment names this task**: *"The remaining jobs from that table (`system`,
  `bindings`, `build`) arrive with the work that gives them something to run: … T56 and T85
  respectively."*
- **`PathIntegration` already puts `<root>/bin` on the PATH**, user-writable on all three systems,
  written *"only when `path.install` asks, never on the daemon's own initiative"*.
- **The workspace version is single-sourced** at `[workspace.package] version = "0.1.0"` in the root
  `Cargo.toml`.

## Scope

**In:**

- `packaging/` — one directory per OS, holding the scripts and the inputs that turn a release build
  into artifacts. Bash on all three, because CI already runs `shell: bash` on Windows.
- `.github/workflows/ci.yml` — the `build` job, on the same three runners, uploading the artifacts.
- `mixengine-platform` — `install::helper_path()`, one constant per OS; the `elevated` module's
  reading half becomes available to `host` builds; Windows resolves its known folders through the
  OS rather than through the environment.
- `mixengine-proto` — `PrivilegedOp::HelperInstall {}`, and its three tables.
- `mixengine-elevate` — `helper.rs`, which carries it out by copying its own image.
- `mixengine-core` — `elevation::helper()` prefers the system copy, checks it, and refuses one that
  is not root's; `Error::ElevateUntrusted`.
- `mixengine-daemon` — `Elevation::require_helper`, called at start beside the other three
  `require_*`; the new error mapped to the wire.
- Documentation: [ADR 0015](../../../.claude/decisions/0015-the-helper-installs-itself.md),
  [build-and-release.md](../../../.claude/operations/build-and-release.md),
  [security-model.md](../../../.claude/architecture/security-model.md),
  [platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md),
  [overview.md](../../../.claude/architecture/overview.md), the roadmap.

**Out:**

- **The second architecture of each OS** — `aarch64-pc-windows-msvc` and
  `aarch64-unknown-linux-gnu`. Both are cross-compilations of a workspace that builds SQLite, AWS-LC
  and libdbus from C, on runners that carry no cross toolchain; they are a task of their own and are
  written down as **T85a**. macOS is the exception and is universal here, because Apple's own
  toolchain cross-compiles the other slice with no extra sysroot.
- **Registering the daemon's autostart entry** — item 3 of *"What the installer does"*. It needs
  `ServiceInstaller`, which is in the platform table and has never been built, on all three systems.
  **T85b**.
- **Code signing, notarisation, and the updater's minisign signature** — T86, T86a, T94, and a
  `latest.json` that is T88's. Everything this job produces is unsigned, and says so.
- **Replacing the helper safely across an upgrade.** This design installs and re-installs it behind
  the explicit prompt that already exists; verifying a signature *before* that prompt, and
  negotiating protocol with an older helper, is **T88a**.
- **The uninstall path** — T87, which is the thing that removes what these installers place.

## Decisions

### D1 — The installer is not what places the helper; a privileged operation is

Four of the six formats run entirely as the user. A design in which the root-owned copy exists only
on the two Linux packages is a security property that most users never get and that nothing in the
product can state truthfully — `mix status` would have to say "depends how you installed it".

So the mechanism is uniform and belongs to MixEngine rather than to a packager:
`PrivilegedOp::HelperInstall {}` puts the helper where it belongs, and it is enqueued at daemon start
like the resolver wiring, the CA install and the port grant — which means it is **applied inside the
single first-run prompt those three already cost**, not behind a new one.
`.claude/architecture/security-model.md`'s *"Expected lifetime total: one prompt at first run"*
therefore does not change.

A `.deb`, an `.rpm` or a `.pkg` that has already put the file there is then an optimisation and not a
second mechanism: the operation reads the destination, finds the same bytes, and answers
`AlreadyDone`.

### D2 — The operation carries nothing, and the helper copies its own image

`HelperInstall {}` has no fields. Not "no path yet" — no path ever. The alternative,
`HelperInstall { source: PathBuf }`, hands a compromised daemon a primitive it does not have today:
*copy this file, as root, into a directory only root can write*. That is `Exec { cmd }` with two more
steps, and the closed-enum rule in the security model exists to refuse exactly this shape.

What the helper copies is `std::env::current_exe()` — the image of the process the user has already
approved this run of. Where it copies it to is a constant compiled into this binary. Neither end of
the copy is anything the daemon said.

### D3 — Where "root-owned" is, per OS, and why not `/usr/local` on macOS

| OS | Directory | Owner as the OS ships it |
| --- | --- | --- |
| Windows | `%ProgramFiles%\MixEngine\mixengine-elevate.exe` | `BUILTIN\Administrators`; `Users` get read + execute |
| macOS | `/Library/PrivilegedHelperTools/dev.mixengine.elevate` | `root:wheel`, created by us if absent |
| Linux | `/usr/local/libexec/mixengine/mixengine-elevate` | `root:root`, created by us if absent |

macOS is the one that differs from `build-and-release.md`'s draft, and the reason is measured above:
`/usr/local` belongs to the installing user on any Intel Mac with Homebrew, which makes it the
*worst* of the available directories rather than a neutral one. `/Library/PrivilegedHelperTools` is
where `SMJobBless` helpers go, is `root:wheel`, and is claimed by no package manager. It is flat by
convention, so the file is named `dev.mixengine.elevate` rather than sharing a bare name with
whatever else is in there.

Linux keeps `/usr/local/libexec/mixengine/`, including in the `.deb` and the `.rpm`. A distribution
package writing under `/usr/local` is against Debian policy and is deliberate here: these packages
are published by us and installed by hand, and **one lookup path on a system is worth more than
policy compliance nobody is checking**. The daemon looks in exactly one place, whatever put the file
there.

### D4 — A known folder is asked of the OS, never of the environment

The helper is spawned by the daemon. Whether the process the elevation prompt starts carries the
daemon's environment block is, on Windows, undocumented and unmeasured — see above — and the design
rule for the binary that runs as root is not "prove it is safe", it is *"validates every request
itself rather than trusting the daemon"*. `std::env::var("ProgramFiles")` is a value this binary
cannot show it chose. Left as it is, the worst case is D1's copy landing somewhere an attacker
selected — `C:\Windows\System32\MixEngine\`, or a directory they own outright, which would make the
ownership check in D5 the only thing standing between them and a helper they can rewrite.

Windows therefore resolves both of its directories through `SHGetKnownFolderPath` —
`FOLDERID_ProgramFiles` for this task's destination and `FOLDERID_ProgramData` for the audit log,
which reads a variable today for no better reason and is fixed by the same call. That is roughly
fifteen lines of `unsafe` in a new `windows/known_folder.rs`, and it is worth it here where
`.claude/architecture/platform-abstraction.md` refused Security.framework: that was a certificate
API with a lifetime discipline, this is one call and one `CoTaskMemFree`, and what it buys is a
question the audited binary no longer has to answer.

macOS and Linux read no environment variable at all: their paths are string constants.

### D5 — Absent means fall back; present-and-not-root's means refuse

`elevation::helper()` gets three answers rather than two, and the middle one is the point:

| System copy | What happens |
| --- | --- |
| not there | the copy beside the program is used — a `cargo build`, a first run before the prompt, a machine whose user declined |
| there, and the file **and** its directory belong to an administrative account and are not writable by others | it is used |
| there, and either is not | **`Error::ElevateUntrusted`**, naming the path and which check failed |

Falling back in the third row would be a silent downgrade to the weaker configuration at exactly the
moment somebody has arranged for it. Refusing is loud, is reported by `elevation.status`'s existing
`reason` field, and leaves the machine unable to elevate until a person looks — which is the correct
outcome for "the file this is about to run as root is not root's".

A read that fails is treated as the third row and not the first. The daemon that cannot find out
whether the file it is about to run as root belongs to root has not learned that it does.

The decision is a pure function over four booleans and a pair of paths, so the table above is a unit
test and not an integration test.

### D6 — The daemon asks on every start, and asks again when the bytes differ

`Elevation::require_helper` follows `require_resolver`'s shape exactly: read cheaply, enqueue only
when the machine does not already agree, never prompt by itself.

- no helper beside the program → nothing to install, ask for nothing;
- system copy absent → enqueue;
- system copy present and byte-identical to the shipped one → ask for nothing;
- system copy present and different → enqueue, because a MixEngine upgraded yesterday is otherwise
  driving a helper from the version before it, for ever.
- system copy present and untrusted → ask for nothing and log; the operation would be refused by the
  helper anyway, and `elevation.status` is already saying so through D5.

"Different" is decided by length first and by SHA-256 only when the lengths match, so the ordinary
case — an unchanged install — costs two `stat`s.

**This is a replacement, and it is not auto-update.** `security-model.md`'s boundary is that the
helper is *"replaced only through its own explicit elevation prompt"*, and a queued operation is
exactly that: nothing is copied until a person allows a batch. What T88a adds on top is the
signature check that decides whether the *new* binary deserved to be run at all.

### D7 — The reading half of `elevated` becomes available to the daemon

`owner_of`, `others_can_write` and `Owner` move from `#[cfg(feature = "elevated")]` to
`#[cfg(any(feature = "host", feature = "elevated"))]`, the gate `hosts`, `port_access` and `command`
already use for the same reason: a read half the daemon needs and a write half only the helper does.
`is_elevated`, `audit_directory` and `create_root_owned_directory` stay behind `elevated`.

Nothing is added to `mixengine-elevate`'s dependency closure by this — the direction is the other
way — so `.github/elevate-dependencies.txt` does not move, and CI's diff of it is what proves that.

### D8 — macOS ships a `.pkg`, and the roadmap line changes with it

The roadmap says `.dmg`. A `.dmg` is a mountable volume whose payload is dragged somewhere by the
user, and the thing that used to be dragged was an application bundle ADR 0011 deleted. What is left
to ship on macOS is three command-line binaries, for which a disk image is a tar file with a mount
step in front of it.

A `.pkg`, built with `pkgbuild`, installs them to `/usr/local/bin`, runs as root, and can therefore
place the helper at install time — which makes macOS the third of the three formats where D1's
operation finds its work already done. It is also the artifact whose Gatekeeper behaviour T86a has to
measure, so building it is what makes that task answerable.

`build-and-release.md`'s target table and the roadmap line both change to say `.pkg`.

### D9 — Six artifacts, named one way, checksummed

| OS | Artifacts |
| --- | --- |
| Windows | `mixengine-<version>-windows-x86_64-setup.exe`, `mixengine-<version>-windows-x86_64.zip` |
| macOS | `mixengine-<version>-macos-universal.pkg` |
| Linux | `mixengine-<version>-linux-x86_64.AppImage`, `mixengine_<version>-1_amd64.deb`, `mixengine-<version>-1.x86_64.rpm` |

Each is accompanied by a `.sha256` written by the script that made it. That is not a signature and is
not presented as one — T86 owns the minisign half — it is what lets a person who downloaded twice
tell whether they got the same file.

The version is read out of the workspace `Cargo.toml` by every script, so a release is a version bump
and nothing else. The `.deb` and `.rpm` carry their own numbering conventions (`_amd64`, `-1.x86_64`)
rather than being forced into the common shape, because both are read by tools that parse them.

### D10 — What each installer actually places

All three per-OS scripts stage the same three binaries and differ only in how they wrap them.

- **NSIS**, `RequestExecutionLevel user`: installs into `$LOCALAPPDATA\Programs\MixEngine`, writes
  the uninstall entry under `HKCU`, appends its own directory to `HKCU\Environment\Path`, and asks
  for no UAC at any point. It does **not** write `<root>/bin` to the PATH: that directory is
  `PathIntegration`'s and is written when `path.install` asks
  ([overview.md](../../../.claude/architecture/overview.md)). The two therefore write different
  segments of one value and each removes only its own, which is what makes two authors safe.

  **The PATH edit carries a guard, and it is not decoration.** NSIS's `ReadRegStr` silently
  truncates at `NSIS_MAX_STRLEN`, so writing back what it read can destroy a long PATH. The script
  reads the value, and where its length is at or above that limit it **writes nothing** and says on
  screen that the directory has to be added by hand — a PATH that is not extended is an
  inconvenience, and a PATH that is cut in half is somebody's afternoon.
- **The portable zip** is the same three files under `mixengine/`, with no registry, no PATH and no
  uninstaller. It is the artifact for "try it without installing anything".
- **The `.pkg`** installs `mix` and `mixengined` to `/usr/local/bin` and the helper to
  `/Library/PrivilegedHelperTools/dev.mixengine.elevate`, all `root:wheel`.
- **The `.deb` and the `.rpm`** install `mix` and `mixengined` to `/usr/bin` and the helper to
  `/usr/local/libexec/mixengine/`. Neither carries a maintainer script: a package that only ships
  files has nothing to go wrong at install time, and everything MixEngine needs to do to a machine it
  does through the helper, on first run, with the user watching.
- **The AppImage** carries all three, and its `AppRun` does one thing before `exec`ing `mix`: it
  unpacks the payload into `${XDG_CACHE_HOME:-$HOME/.cache}/mixengine/<version>/` if it is not
  already there, and runs from *that* copy.

  **A daemon cannot live inside an AppImage mount, and this is the whole reason that step exists.**
  The runtime mounts the image on a temporary path and unmounts it when the process exits — so
  `mix`, which starts `mixengined --detach` and then exits, would tear the filesystem out from under
  the daemon it just started, and `mixengine-elevate` beside it would be a path that no longer
  resolves the next time a prompt is raised. Extracting once, keyed by version, costs a few tens of
  megabytes of cache and makes every path the daemon ever hands anybody a real one. Nothing is added
  to `PATH` and nothing is registered: a person who wants `mix` on their PATH symlinks the AppImage
  or installs the `.deb`.

### D11 — The `build` job runs where the other five run, and checks what it made

One job, `strategy.matrix.os` of the same three runners, `--release`, artifacts uploaded with
`actions/upload-artifact`. It fires on `master` and on request, exactly like every other job in this
workflow — the trigger block is shared and does not change.

Every script ends by **opening the artifact it just made and asserting the three binaries are in
it**: `unzip -l`, `dpkg-deb -c`, `rpm -qlp`, `pkgutil --payload-files`, and for the AppImage and the
NSIS installer a run of the extracted payload. A packaging script that silently produced an empty
archive is the failure mode this whole job exists to prevent, and it is not one CI notices by itself.

## Data flow

```
daemon start
  └─ Elevation::require_helper
       ├─ mixengine_platform::install::helper_path()      →  the one constant for this OS
       ├─ compare with the copy beside `mixengined`
       └─ enqueue PrivilegedOp::HelperInstall {}          →  queued, nothing prompted

user runs `mix elevation grant`
  └─ one prompt, one mixengine-elevate, one batch
       └─ HelperInstall
            ├─ destination = install::helper_path()       →  known folder / constant, never $env
            ├─ the directory exists and is not root's?    →  Refused
            ├─ create_root_owned_directory(parent)
            ├─ copy current_exe() → <dest>.new → rename
            └─ Applied { detail }                          →  one line in the audit log

every later start
  └─ elevation::helper(&program)
       ├─ system copy present and root's                  →  run that
       ├─ system copy present and not root's              →  ElevateUntrusted
       └─ system copy absent                              →  the copy beside the program
```

## Testing

**Pure, in `mixengine-core`** — the whole of D5 as a table: absent + beside present, absent + beside
absent, present + trusted, present + untrusted-by-owner, present + untrusted-by-mode, present +
unreadable. No filesystem.

**In `mixengine-platform`** — `install::helper_path()` answers an absolute path on this OS and its
last component is the expected name; on Windows, that the answer does not change when `ProgramFiles`
is set to something else in the environment, which is D4 stated as a test.

**In `mixengine-elevate`** — `HelperInstall` is refused without an administrative token, through the
one gate in `ops::apply`; and a `#[ignore]`d system test in CI's elevated `system` job that installs
it for real, asserts the file is there and is root's, runs the operation a second time and asserts
`AlreadyDone`. The `system` job's existing "what this job left behind" steps gain the helper.

**In `mixengine-daemon`** — `require_helper` against a temporary home: nothing beside the program
enqueues nothing; a helper beside it and no system copy enqueues one; the same operation enqueued
twice deduplicates, which is the queue's own property and is asserted here because this is the first
operation whose dedupe key is a bare name with no data behind it.

**In CI** — the `build` job is its own test, per D11.

## Risks, and where each is answered

- **The first prompt still runs the per-user helper.** On a machine where the system copy does not
  exist yet, the binary UAC elevates is the one in the user's own directory — so malware that
  replaced it before first run gets root once, and with D6 gets *installed* as the permanent helper.
  This is not a regression: today it gets root at every prompt for ever. What it changes is that the
  compromise becomes durable, which is worth stating plainly rather than counting as a win. It is
  inside `security-model.md`'s *"if `mixengine-elevate` is installed somewhere the user can write,
  malware running as the user could replace it"*, and the only thing that closes it is a signature
  the OS checks before the prompt — T94's question, and T88a's check.
- **An upgrade that changes the protocol before the helper is re-installed.** The daemon speaks the
  new protocol, the installed helper speaks the old one, and `PrivilegedResponse::version` already
  makes that a typed refusal rather than a mystery. D6 enqueues the replacement; T88a is what makes
  the replacement safe to trust.
- **`SHGetKnownFolderPath` in the audited binary.** One new `unsafe` block, in the one binary whose
  whole design constraint is being small enough to read in a sitting. Bounded by being a single call
  with an out-pointer and a free, and it replaces a question about the environment rather than adding
  a capability — but it is `unsafe` in the root process, and that is a cost and not a free win.
- **A packaging tool that moves under us.** `appimagetool` is downloaded at a pinned release URL;
  `makensis`, `pkgbuild`, `dpkg-deb` and `rpmbuild` come from the runner image. A version that
  changes its output is caught by D11's checks rather than shipped.
- **`rpmbuild` is not on the ubuntu image by default.** The job installs it. If that install is what
  breaks, it breaks one artifact loudly, not the job's other two.
- **Six artifacts, three of which nobody has installed on a clean machine yet.** That smoke test is
  the release checklist's, and T87's clean-VM run is where "nothing left behind" is proved. This task
  produces the artifacts; it does not claim they have been through a VM.

## What this leaves

- **T85a** — the second architecture on Windows and Linux, with the cross toolchains that needs.
- **T85b** — `ServiceInstaller`, and with it the autostart entry `build-and-release.md` lists as item
  3 of what an installer does.
- **T86** — the minisign signature over everything D9 names, and the keys to make it with.
- **T86a / T94** — what SmartScreen and Gatekeeper do to unsigned artifacts, now that there are
  artifacts to hand them.
- **T87** — uninstall, which is the only thing that removes the file D1 installs. The helper is
  root-owned and outside `MIXENGINE_HOME`, so removing it is a privileged operation of its own —
  the same sentence T87 already carries about the audit log, and now about two files instead of one.
- **T88a** — the signature check in front of D6's replacement.
