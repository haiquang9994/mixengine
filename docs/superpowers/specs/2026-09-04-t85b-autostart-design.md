# T85b — `ServiceInstaller`, and the console window nobody asked for (design)

Roadmap task **T85b**, phase 9: *"`ServiceInstaller`: register the daemon's autostart entry — Task
Scheduler logon task, LaunchAgent, systemd **user** unit."* Item 3 of *"What the installer does"* in
[build-and-release.md](../../../.claude/operations/build-and-release.md), and the one item of that
list that has never been built — the trait is a row in
[platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md)'s table with no
implementation behind it on any of the three systems.

Two things this task changes about the sentence it was written from, both argued below: **no
installer registers the entry** (D1, and a new [ADR 0016](../../../.claude/decisions/0016-autostart-is-registered-by-mixengine.md)),
and **the Windows leg needs a change inside `mixengined` itself** (D4), because a logon task that
runs a console program puts a terminal window on the user's desktop at every login — measured on a
real Windows 11 machine rather than reasoned about.

## Goal

A person who installs MixEngine, asks for it to come back at login, and reboots, finds the daemon
running and no window on their screen. A person who never asks gets nothing written outside
`MIXENGINE_HOME`. Either one can undo it with one command, and `mix autostart status` says which of
the two they are.

## Measured, not assumed

All four measurements were taken on 2026-09-04 on Windows 11 Pro 26200 and in WSL, with scratch
entries created and deleted; nothing was left on either machine.

1. **A console-subsystem program started by Task Scheduler gets a visible console window.** A
   `LogonTrigger` task with `<LogonType>InteractiveToken</LogonType>`, run through
   `schtasks /Run`, reported `GetConsoleWindow() != 0`, `IsWindowVisible() == true`, and session 1 —
   the user's own interactive session. This is the same mechanism
   [`windows/command.rs`](../../../crates/mixengine-platform/src/windows/command.rs) already measured
   from the other direction, for the eight `icacls` calls a daemon makes at start-up: *"one
   `mixengined --detach` produced nine of them"* — a console-subsystem child of a parent with no
   console is handed a new console, and on Windows 11 that is a terminal window.
2. **`<Hidden>true</Hidden>` does not hide it.** The identical task with that setting reported
   `visible=true` again. The element hides the *task* in the Task Scheduler UI and says nothing
   about the process's windows. This is the setting every "how do I hide the console" answer names,
   and it is the wrong one.
3. **`GetConsoleProcessList` separates the two cases cleanly.** Under Task Scheduler the console had
   **1** process attached — the program itself. Started from a shell (`cmd` → `powershell`) the same
   program found **4**. A console this process is the only member of is one Windows created for it
   and nobody is reading; a console it shares is a terminal somebody is looking at. That is the
   discriminator D4 turns on. Note that `GetConsoleWindow()` alone is *not* a discriminator: in the
   shell case it returned `0`, because a ConPTY console has no window of its own.
4. **`schtasks /Query /XML ONE` hands the registered `<Command>` and `<Arguments>` straight back**,
   which is what makes `state` able to say what the entry will actually run. Redirected to a file it
   is single-byte text in the console codepage, not UTF-16 — D6 is where that matters.
5. **WSL has a working systemd user session** (`systemctl --user is-system-running` → `running`,
   `XDG_RUNTIME_DIR=/run/user/1000`), so the Linux leg is developable here. GitHub's `ubuntu-latest`
   historically has none, which is why D7 makes "no user manager" a first-class answer rather than a
   failure.

## Scope

**In:**

- `ServiceInstaller` in `mixengine-platform`: the trait, `AutostartPlan`, `AutostartState`,
  `AutostartMechanism`, a `Host` accessor, a recording mock, and three implementations.
- Windows: a Task Scheduler logon task named `MixEngine`, registered through `schtasks.exe` from a
  generated task XML.
- macOS: `~/Library/LaunchAgents/dev.mixengine.daemon.plist`, which `loginwindow` loads by itself.
- Linux: `$XDG_CONFIG_HOME/systemd/user/mixengined.service`, enabled with `systemctl --user enable`.
- `mixengined` releasing a console it is the sole owner of, so the Windows entry starts a daemon and
  not a terminal window (D4).
- API: `autostart.status`, `autostart.enable`, `autostart.disable`, answering `AutostartReport`.
- CLI: `mix autostart status | enable | disable`, and its rendering.
- Documentation: [ADR 0016](../../../.claude/decisions/0016-autostart-is-registered-by-mixengine.md),
  `platform-abstraction.md`, `daemon-and-ipc.md`, `overview.md`, `build-and-release.md`, the roadmap.

**Out:**

- **`mix doctor` reporting a stale entry.** The capability answers the question; whether a finding is
  raised for "registered, but naming a `mixengined` that is no longer there" belongs to T47's finding
  set and to the updater's own task, not to the capability that makes it askable.
- **Removing the entry at uninstall.** T87 owns the complete uninstall path and now has one more
  thing to remove; this task gives it `autostart.disable` to remove it with.
- **Re-pointing the entry after an update.** An update that moves `mixengined` leaves an entry naming
  the old path. `autostart.enable` run again fixes it, and `autostart.status` says so — doing it
  automatically is T88's, which is the task that knows an update happened.
- **A fallback for a Linux machine with no systemd user session.** An XDG `autostart/*.desktop` file
  would be one, and it starts nothing on a headless box, which is the machine that needs it. The
  answer here is `AutostartMechanism::None` and a reason naming the manual command.
- **Anything elevated.** Every mechanism in this task is per-user and user-writable on all three
  systems. Nothing here becomes a `PrivilegedOp`, and the closed list does not grow.

## The types

In `mixengine-platform`, `traits/autostart.rs`:

```rust
pub enum AutostartMechanism { LogonTask, LaunchAgent, SystemdUser, None }

/// What an entry would be asked to start. Fields, not a rendered command line — D5.
pub struct AutostartPlan { pub program: PathBuf, pub home: PathBuf }

pub struct AutostartState {
    pub mechanism: AutostartMechanism,
    /// Where a person would go and look: the task name, the plist path, the unit path.
    pub location: String,
    pub enabled: bool,
    /// Whether *this call* wrote — D6. Always false from `state`.
    pub changed: bool,
    /// What the registered entry will run, read back off the machine. Empty when nothing is
    /// registered, and best-effort where a system cannot hand it back faithfully — D6.
    pub command: Vec<String>,
}

pub trait ServiceInstaller: std::fmt::Debug + Send + Sync {
    fn enable(&self, plan: &AutostartPlan) -> Result<AutostartState>;
    fn disable(&self) -> Result<AutostartState>;
    fn state(&self) -> Result<AutostartState>;
}
```

In `mixengine-proto`, `autostart_api.rs`: `AutostartReport` carries the same five fields plus
**`for_this_home: bool`**, which the daemon composes by comparing the `--home` inside `command` with
its own root (D3). That comparison is the daemon's because
[CLAUDE.md](../../../CLAUDE.md) forbids business logic in a client, and because two clients
disagreeing about whether an entry is "yours" is exactly the class of bug this product exists to
prevent.

## Decisions

### D1 — MixEngine registers the entry, and no installer does

`build-and-release.md` item 3 says *"Registers daemon autostart"* under *"What the installer does"*.
That line does not survive the same three facts that killed its neighbour at T85, plus one of its
own:

1. **Four of the six shipped formats install entirely as the user** — the per-user NSIS installer,
   the portable zip, the AppImage, and a `cargo build`. Only the `.deb`, the `.rpm` and the `.pkg`
   run as root.
2. **And the three that do run as root are the three that cannot do this.** A LaunchAgent, a systemd
   *user* unit and a logon task are all per-user: they live in one account's home or under one
   account's SID. A package installed by root does not know which account will use MixEngine, and on
   a shared machine there may be several. This is the reverse of the helper's case — there, only root
   could do it; here, only the user can.
3. **An install that silently arranges a background process at every login is a consent question.**
   `shims.rs` already carries the argument for the neighbouring capability: *"A daemon that edited
   `~/.zprofile` because it happened to start at login would be a program that changed the shell of
   somebody who had only installed it."* An autostart entry is the same category, one step louder.
4. **One behaviour on all three systems is worth more than a Windows-only convenience.** The NSIS
   installer *could* do it, being per-user. A product where autostart is on after installing on
   Windows and off after installing on macOS is one nothing can state truthfully.

So: `autostart.enable` is the mechanism, asked for by a person, and no installer is. Written down as
[ADR 0016](../../../.claude/decisions/0016-autostart-is-registered-by-mixengine.md) in ADR 0015's
shape, and `build-and-release.md` item 3 is rewritten to say so.

### D2 — The trait keeps the name `ServiceInstaller`, and nothing else does

`ServiceInstaller` is an unfortunate name in a codebase where *service* means MariaDB and php-fpm —
`ServiceSpec`, `ServiceId`, `service.*`. The obvious move is to rename it to `Autostart`.

**It is not renamed, and the reason is the ADRs.** `ServiceInstaller` is named in
[ADR 0002](../../../.claude/decisions/0002-cross-platform-from-day-one.md)'s day-one capability list
and in [ADR 0007](../../../.claude/decisions/0007-supervised-child-owns-a-process-group.md); renaming
would mean editing two accepted decision records, which `CLAUDE.md` forbids outright — *"Changing a
cross-cutting decision requires a new ADR, not an edit to an accepted one"* — and an ADR whose whole
content is a rename is a bad trade for a name that appears in one trait definition.

Everything a reader meets more often is named for what it does: the module is `traits/autostart.rs`
(the file names in `traits/` are conceptual already — `access.rs` holds `DirectoryAccess`, `path.rs`
holds `PathIntegration`), the value types are `Autostart*`, the API is `autostart.*` and the command
is `mix autostart`. The trait's own header opens by saying which "service" it means.

### D3 — One entry per user, naming one home

The mechanisms are per-user, and MixEngine is per-home. A person with two `MIXENGINE_HOME`s could
have one entry each, keyed by a hash of the root.

**One fixed name, and it names one home.** A hashed label produces entries nobody can find by
looking: a `MixEngine (a3f9c1)` task left behind by a home that was deleted is undiscoverable, and
T87 cannot remove what it cannot enumerate. One home starting at login is also the honest product:
`mix` starts a daemon for any other home the moment it is asked
([daemon-and-ipc.md](../../../.claude/architecture/daemon-and-ipc.md), *client autostart*), so the
second home costs a person nothing but the first command.

Enabling from a second home **replaces** the entry and reports `changed: true`. `autostart.status`
carries the command the entry actually holds, so a daemon can say *"registered — for another home"*
rather than *"registered"*, which is the confusing half-state this field exists to name. That
comparison is the daemon's, not a client's: `AutostartReport::for_this_home` is composed before the
answer goes on the wire.

### D4 — `mixengined` releases a console it is the only process in

Measurements 1 and 2: a logon task running `mixengined.exe` puts a visible terminal window on the
desktop at every login, and `<Hidden>` does not stop it. Four ways out were considered:

- **Run `mixengined --detach` from the task.** Refused. The launcher is a console program too, and it
  waits up to `DETACH_TIMEOUT` (30 s) for the daemon to answer — so the window is *worse*, not
  better, and Task Scheduler then supervises nothing.
- **`<LogonType>S4U</LogonType>`**, which runs the process non-interactively and shows no window.
  Refused: it needs the batch-logon right, which is not granted to an ordinary account on every
  client SKU, and it puts the daemon on a window station from which T83's `DesktopApps` could never
  start MixDB.
- **Ship a windows-subsystem launcher binary.** Refused: a fourth binary in every artifact, for one
  system, to work around one flag.
- **Make `mixengined` a windows-subsystem binary.** Refused outright: it would print nothing when run
  from a terminal, which is how every developer and every `mix` diagnostic uses it.

What is done instead: at start-up, on Windows only, the daemon asks whether it is the **only process
attached to its console**. Measurement 3 says that is `1` under Task Scheduler and `4` from a shell.
If it is alone, the console was created for it by Windows and nobody is reading it, so the daemon
lets it go — and the console host, having no attached process left, exits and takes the window with
it.

Two details that are the whole of the care this needs:

- **The standard handles are redirected to `NUL` first.** `FreeConsole` invalidates them, and a
  daemon whose every `write` to stderr fails afterwards is a worse bug than the window. `NUL` is
  opened, `SetStdHandle` points all three at it, *then* the console is freed.
- **It happens after arguments are parsed and only on the path that runs a daemon.** `--version` and
  a `clap` usage error must still reach the console that started them; a person who ran
  `mixengined --help` by double-clicking is not who this is for.

It lives in `mixengine-platform` — `process::release_unattended_console()`, a no-op on both Unixes —
because it is `#[cfg(windows)]` by nature and
[platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md) allows no such
thing in the daemon. It is in `process` and not behind `Host` for the reason that module's four
neighbours are: it is not a question about the machine that a mock could answer, it is a concrete
handle being closed.

The residue is a **flash**: the window exists from process creation until the daemon reaches that
call, tens of milliseconds. That is stated rather than hidden, and it is the price of `mixengined`
remaining a program a terminal can run.

### D5 — `schtasks.exe` with an XML file, not COM and not `/TR`

[platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md) rule 5 prefers a
Windows API to a shell-out. The Task Scheduler API is COM (`ITaskService`, `ITaskDefinition`,
`IRegisteredTask`), and this workspace depends on `windows-sys`, which is raw FFI with no COM
support: reaching it would mean hand-written vtable calls and `IUnknown` reference counting for an
operation that runs when somebody types a command. `DirectoryAccess`'s `icacls` is the standing
exception for exactly this shape of trade, and this is the second instance of it.

**The tool is called with an argument vector and the task is described in a file**, which is what
keeps it inside the rule that matters (the T40 design, D9: *the program is named, never a command
line*). `schtasks /Create /TN MixEngine /XML <file> /F` puts no user-controlled text on a command
line at all — the program path and the home go inside the XML as `<Command>` and `<Arguments>`,
which are separate elements with no quoting rules between them. The alternative, `/SC ONLOGON /TR
"<command line>"`, would put a path with a space in it through two levels of quoting, which is the
bug class this project already refuses everywhere else.

The XML is written **UTF-16LE with a BOM**, which is what Microsoft documents. A UTF-8 file was
accepted by `schtasks` on the machine this was measured on, and that is not relied on: a file whose
declaration says `encoding="UTF-16"` and whose bytes are not is a coincidence, not a contract.

Fixed contents, each for a reason:

| Element | Value | Why |
| --- | --- | --- |
| `<LogonTrigger><UserId>` | this account's SID | `sid::current_user()` already exists. A display name is localised and can be changed; a SID cannot. |
| `<Principal><LogonType>` | `InteractiveToken` | The daemon runs in the user's own session — D4's window station, T83's reason. |
| `<RunLevel>` | `LeastPrivilege` | Nothing here is elevated, and a task that asked for elevation would prompt at every login. |
| `<MultipleInstancesPolicy>` | `IgnoreNew` | The single-instance lock already refuses a second daemon; this stops the task from producing one to be refused. |
| `<DisallowStartIfOnBatteries>` | `false` | **Defaults to `true`.** Left alone, the daemon never starts on a laptop that logged in on battery — the single most likely way this feature would be reported as "does not work". |
| `<StopIfGoingOnBatteries>` | `false` | Its pair: a daemon killed when a charger is unplugged. |
| `<ExecutionTimeLimit>` | `PT0S` | **Defaults to three days.** A long-running process under a task with the default limit is one Task Scheduler eventually kills. |
| `<StartWhenAvailable>` | `true` | A logon missed because the machine was asleep still starts. |
| `<RestartOnFailure>` | `PT1M`, 3 | D8's semantics: a crash comes back, a clean stop stays stopped. |

The task name is `MixEngine` at the root of the task library, not `MixEngine\mixengined` in a folder
of its own. One entry (D3) does not need a folder, and `schtasks` has no way to delete an empty one —
a folder would be a leftover T87 could not remove.

### D6 — `changed` means "this call wrote", and each system answers it as well as it can

`PathState::changed` is *"whether this call is what put it there"*, and `AutostartState` keeps that
meaning: it is what lets a client say *"already set up"* instead of claiming a write it did not
perform.

On both Unixes the entry is a file, so the document is generated, compared byte for byte with what is
on disk, and written only if they differ. On Windows the entry is inside the Task Scheduler service
and the only way back out is `schtasks /Query /XML ONE`, whose output arrives in the console codepage
(measurement 4) — so a home whose path is not ASCII may not round-trip, compare unequal, and be
rewritten. That is stated in the trait's documentation and is harmless: `/F` replaces the task with
an identical one, and the only visible consequence is `changed: true` where `false` would have been
truer.

The same readback is what fills `AutostartState::command`, which is D3's *"registered — for another
home"*, and it is best-effort for the same reason.

### D7 — `AutostartMechanism::None` is a valid answer, and Linux is why

[`ResolverMethod::None`](../../../crates/mixengine-platform/src/traits/resolver.rs) is the precedent
and the sentence is copied deliberately: *a valid answer, not an error*. A Linux machine with no
systemd user manager — a container, a stripped image, `ubuntu-latest` on GitHub — has no way to start
something at login that MixEngine is willing to write.

- `state()` answers `Ok` with `mechanism: None`, `enabled: false`, and a `location` naming what was
  looked for. A status command must never fail on a machine that simply cannot do the thing.
- `enable()` answers `Err(UnsupportedPlatform)` with a reason naming the manual workaround — the
  `mixengined --home <root>` line to put in whatever the person's session does use.

The probe is `systemctl --user is-system-running`: any exit status is fine (a `degraded` machine has
a manager), and what says "no mechanism" is the command not being there, or failing to reach a bus.
Windows and macOS never answer `None`: Task Scheduler and launchd are part of the OS.

### D8 — A clean exit stays stopped, on all three

`mix daemon stop` must not be undone half a second later by the thing that starts the daemon at
login. So no mechanism restarts a process that exited zero:

| System | Setting |
| --- | --- |
| Windows | `<RestartOnFailure>`, which by definition only applies to a task that ended in failure |
| macOS | `KeepAlive: { SuccessfulExit: false }` — **not** `KeepAlive: true`, which restarts anything |
| Linux | `Restart=on-failure` — **not** `Restart=always` |

And no mechanism is told to start the daemon *now*: `enable` registers, it does not launch.
`systemctl --user enable` without `--now` is the shape all three follow. Somebody who asked for
"start it at login" did not ask for "start it".

`loginctl enable-linger` is deliberately not called. Without it a systemd user manager stops at
logout, which is exactly the lifetime
[overview.md](../../../.claude/architecture/overview.md) states for the daemon — *"login → logout"*.

### D8a — Each system is asked in the way that system actually registers, and macOS is asked for nothing

The three legs do not look alike, and the difference is not an inconsistency — it is what
*registration* means on each system.

- **macOS: writing the file is the registration.** `loginwindow` bootstraps this user's LaunchAgents
  domain from `~/Library/LaunchAgents` at every login, so a plist in that directory *is* an agent
  that starts at login. `enable` therefore writes the plist and calls `launchctl` for nothing, and
  `disable` removes it and calls `launchctl` for nothing. This is not a shortcut: `enable` does not
  start the daemon (D8) so there is nothing to `bootstrap`, and `disable` must not *stop* the daemon
  somebody is using — a `bootout` would terminate the running job, which is a person turning off
  "start at login" and losing their running daemon. It also removes the one real risk this leg had:
  `launchctl bootstrap gui/<uid>` needs a session that an SSH-only Mac and some CI runners do not
  have, and nothing here asks for one.
- **Linux: the unit file is not the registration; the symlink is.** A unit in
  `~/.config/systemd/user/` starts nothing until something wants it, and what creates the
  `default.target.wants` link is `systemctl --user enable`. The symlink could be written by hand —
  it is a file, and `WantedBy=default.target` fully determines its path — but `systemctl` is the
  authority on what an `[Install]` section means, and re-deriving that here would be a second
  implementation of it. So Linux runs `systemctl --user daemon-reload` and `systemctl --user enable`,
  without `--now`, and D7's probe is what decides whether there is a manager to run them against.
- **Windows: there is no file at all.** A task lives inside the Task Scheduler service, and
  `schtasks` is the only way in — D5.

`disable` on all three leaves a running daemon running, for macOS's reason above.

### D9 — The document is generated where a test can read it, and registered where only the OS can

`.claude/standards/testing.md` rule 1 forbids a test touching the real machine outside a system
suite. Every implementation is therefore split in two:

- **A pure function** that renders the entry — the task XML, the plist, the unit file — from an
  `AutostartPlan`. Unit-tested in `#[cfg(test)]` against fixed inputs: the escaping, the settings
  above, a path with a space, a path with `&` and `<` in it.
- **A method that hands that document to the OS.** Each implementation holds the entry's *location*
  in a field — the task name on Windows, the plist path on macOS, the unit directory on Linux — the
  way `windows/path.rs` holds `key: &'static str` so its own tests can exercise the real registry
  calls against a scratch value instead of against the `Path` that decides whether the person running
  them can find `git` tomorrow.

The end-to-end round trip is a system suite, `#[ignore]` and gated on `MIXENGINE_SYSTEM_TESTS=1`,
added to the `system` job in `.github/workflows/ci.yml` on all three runners. It asserts the honest
thing on each: where `state()` reports a mechanism, enable → state → disable → state round-trips and
the entry is gone at the end; where it reports `None`, `enable` refuses with a reason that is not
empty. A test that passed by doing nothing would be worse than none, so the two branches assert
different things and the suite prints which it took.

### D10 — `autostart.*`, not `daemon.autostart_*`

`daemon.*` is about *this running daemon* — its status, its version, its shutdown. An entry in Task
Scheduler outlives every daemon that ever registered it and is a property of the machine. `path.*` is
the precedent for a capability holding a namespace of its own with a `status`/`install`/`uninstall`
shape, and this is the second one. The CLI mirrors it one-for-one, as
[daemon-and-ipc.md](../../../.claude/architecture/daemon-and-ipc.md) requires: `mix autostart status`,
`mix autostart enable`, `mix autostart disable`, none taking parameters — there is exactly one entry
and one home this can be about, and an argument would be an API for registering arbitrary programs to
run at somebody's login.

## Data flow

```
mix autostart enable
  └─ autostart.enable                       (no params)
      └─ daemon::autostart::Autostart::enable
          ├─ AutostartPlan { program: current_exe(), home: <root> }
          └─ host.service_installer().enable(&plan)      on a blocking thread
              ├─ windows  render task XML → UTF-16LE file in a TempDir
              │           schtasks /Create /TN MixEngine /XML <file> /F
              ├─ macos    render plist → atomic write to ~/Library/LaunchAgents/…
              │           and nothing else — D8a
              └─ linux    probe systemctl --user
                          render unit → atomic write to $XDG_CONFIG_HOME/systemd/user/…
                          systemctl --user daemon-reload
                          systemctl --user enable mixengined.service
          └─ AutostartReport { mechanism, location, enabled, changed, command, for_this_home }
```

`disable` is each of those backwards and leaves nothing behind: the task is deleted, the plist is
removed, and on Linux `systemctl --user disable` drops the symlink before the unit file goes and
`daemon-reload` runs again so systemd forgets it rather than reporting it as `not-found` forever.
None of the three stops a running daemon — D8a.

At the next login the entry runs `mixengined --home <root>`, which reaches `main`, parses its
arguments, calls `process::release_unattended_console()` (D4) and serves.

## Testing

| Layer | What it proves |
| --- | --- |
| Unit, `mixengine-platform` | Each document renders exactly, including a path with a space, an `&` and a `<`; the readback extracts `<Command>`/`<Arguments>`, `ProgramArguments`, `ExecStart=`; a malformed entry reads as absent rather than panicking |
| Unit, `mixengine-proto` | `AutostartReport` round-trips through the wire; `mechanism` decodes every variant; an absent `command` decodes as empty |
| Component, `mixengine-daemon` | `autostart.*` against `mock::Host`: the recorded operations, `for_this_home` false for an entry naming another root, `UnsupportedPlatform` reaching the wire as `unsupported` |
| Component, `mixengine-cli` | The three commands reach the three methods, and the rendering of each of `None` / enabled / enabled-for-another-home |
| System, all three OSes | D9's round trip, `#[ignore]` + `MIXENGINE_SYSTEM_TESTS=1` |

**Preservation test** (testing.md rule 4): a `~/.config/systemd/user/` and a `~/Library/LaunchAgents/`
holding an unrelated file are left byte-identical by an enable and a disable. The Windows equivalent
is a second scratch task, which must still be there afterwards.

## Risks, and where each is answered

- **The Windows console flash.** Real, measured in principle, tens of milliseconds. D4 states it
  rather than claiming it away. The alternative that removes it entirely is a windows-subsystem
  binary, refused there.
- **A macOS plist that is already loaded when `disable` removes it.** The agent stays loaded until
  the next logout, and `KeepAlive: { SuccessfulExit: false }` means a crash in that window still
  restarts it. Deliberate, and the alternative is worse: D8a's `bootout` would terminate the daemon
  the person is using, for a command that only said "do not start at login".
- **A logon task registered for a home that is then deleted.** The task starts a `mixengined` that
  creates the home again. That is what `--home` means today and is not this task's to change; T87's
  uninstall calls `disable` before removing anything.
- **Two homes fighting over one entry.** D3: the last `enable` wins and `status` says whose it is.
- **`schtasks` output in a codepage that mangles a non-ASCII home.** D6: a needless rewrite, never a
  wrong answer.
- **A machine where `systemctl --user enable` succeeds and the unit still never starts** — no linger,
  no graphical session. That is the correct lifetime (D8) and `mix autostart status` reports the unit
  as enabled, which it is.

## What this leaves

- **T47** — a `mix doctor` finding for an entry naming a `mixengined` that is not there.
- **T86 / T88** — an update that moves the binary leaves a stale entry. The updater is what knows an
  update happened, and `autostart.enable` is what it calls.
- **T87** — uninstall, which now has one more thing to remove and one method to remove it with.
- **The client surface** — `client-surface.md` item 9 lists autostart among Settings; this task makes
  it a switch a client can actually render.
