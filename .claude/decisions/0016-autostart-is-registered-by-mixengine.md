# 0016. MixEngine registers the daemon's autostart entry, and the installer does not

**Status**: Accepted
**Date**: 2026-09-04

## Context

[build-and-release.md](../operations/build-and-release.md) lists, as item 3 of *"What the installer
does"*: *"Registers daemon autostart (logon task / LaunchAgent / systemd **user** unit)."* It was
never built — the `ServiceInstaller` row in
[platform-abstraction.md](../architecture/platform-abstraction.md)'s table had no implementation on
any of the three systems until T85b — so nothing has ever depended on that sentence, which is what
makes it cheap to correct now.

This is the second line of that list to be rewritten in two days. [ADR 0015](0015-the-helper-installs-itself.md)
took item 2 — *"places `mixengine-elevate` in a root-owned directory"* — away from the installer on
the grounds that **four of the six shipped formats install entirely as the user**: a per-user NSIS
installer, a portable zip, an AppImage, and a `cargo build`. That fact applies here unchanged. Three
more are this item's own:

1. **The three formats that *do* run as root are the three that cannot do this.** A Task Scheduler
   logon task lives under one account's SID; a LaunchAgent lives in one user's
   `~/Library/LaunchAgents`; a systemd *user* unit lives in one user's `~/.config`. A `.deb`, an
   `.rpm` or a `.pkg` is installed by root, which does not know — and on a shared machine cannot know
   — which account will use MixEngine. This is ADR 0015 reversed: there, only root could place the
   file; here, only the user can.
2. **An install that silently arranges a background process at every login is a consent question.**
   `crates/mixengine-daemon/src/shims.rs` already carries the argument for the neighbouring
   capability: *"A daemon that edited `~/.zprofile` because it happened to start at login would be a
   program that changed the shell of somebody who had only installed it."* An autostart entry is the
   same category, one step louder, and `path.install` already answers it by being asked for.
3. **One behaviour on all six formats is worth more than a Windows-only convenience.** The per-user
   NSIS installer *could* do it, being per-user. A product where autostart is on after installing on
   Windows and off after installing on macOS is one nothing in it can state truthfully — the same
   sentence ADR 0015 refused about the helper.

## Decision

**`autostart.enable` is the mechanism, asked for by a person, and no installer is.**

- Three methods, `autostart.status`, `autostart.enable` and `autostart.disable`, taking no
  parameters and reachable from `mix autostart`. There is exactly one entry this can be about and one
  home it can name — the daemon's own — and an argument would be an API for registering arbitrary
  programs to run at somebody's login.
- **Nothing about it is elevated, and the privileged-operation list does not grow.** All three
  mechanisms belong to the account MixEngine runs as. This puts the capability beside
  `PathIntegration` — the other thing MixEngine writes outside `MIXENGINE_HOME` without a token —
  rather than among the operations that need `mixengine-elevate`.
- **`enable` registers and does not start; `disable` removes and does not stop.** There is a daemon
  running by the time either can be called — it is the one answering the call — and a person turning
  off "start at login" must not lose the daemon they are using.
- **One entry per user, naming one home.** A label keyed by the home would produce entries nobody
  could find by looking and none that `mix uninstall` could enumerate. Enabling from a second home
  replaces the entry, and the answer carries what the entry actually runs so a client can say
  *"registered — for another home"* rather than *"registered"*.
- **No mechanism restarts a process that exited zero**: `<RestartOnFailure>`,
  `KeepAlive: { SuccessfulExit: false }`, `Restart=on-failure`. Three vocabularies, one rule, so that
  `mix daemon stop` is not undone half a second later by the thing that starts the daemon at login.
- A `.deb`, an `.rpm` or a `.pkg` **does not** ship an entry the way it ships the helper. There is no
  equivalent of ADR 0015's `AlreadyDone` here, because there is nothing a root-run package could
  write that would be right for the user who later runs `mix`.

## Consequences

- One story on six formats: whether MixEngine comes back after a reboot does not depend on how it
  arrived on the machine, and it is a thing somebody chose.
- **A fresh install does not start at login until somebody asks.** That is a discoverability cost,
  and it is paid deliberately: `mix autostart status` answers the question, and first-run guidance is
  where the offer belongs rather than in an installer's silence.
- **An update that moves `mixengined` leaves an entry naming the old path.** `autostart.enable` run
  again fixes it and `autostart.status` reports it, but nothing does so automatically — that belongs
  to the task that knows an update happened (**T88**), and to `mix doctor`'s finding set (**T47**).
- Uninstall (**T87**) gains one more thing to remove, and `autostart.disable` to remove it with.
- **On Windows it required a change inside `mixengined` itself.** A console program started by Task
  Scheduler under `InteractiveToken` is handed a *visible* console window in the user's session —
  measured, and `<Hidden>true</Hidden>` does not stop it. The daemon therefore releases a console it
  is the only process attached to, which is a discriminator that was also measured: 1 attached
  process under Task Scheduler, 4 from a shell. See the
  [T85b design](../../docs/superpowers/specs/2026-09-04-t85b-autostart-design.md), D4.

## Alternatives considered

- **The per-user NSIS installer registers it, and the other five formats do not.** Refused on point 3
  above: it is the "it depends how you installed it" that ADR 0015 exists to have removed from this
  product's vocabulary.
- **An entry per home, labelled by a hash of the root.** More correct in the abstract and worse in
  every concrete way: a `MixEngine (a3f9c1)` task left by a home somebody deleted is undiscoverable,
  and T87 cannot remove what it cannot enumerate. A person with two homes starts the second with
  `mix`, which autostarts a daemon for any home the moment it is asked.
- **`daemon.autostart_enable`, in the `daemon.*` namespace.** Refused: `daemon.*` is about the daemon
  that is running, and this entry outlives every daemon that ever registered it. `path.*` is the
  precedent for a capability holding a namespace of its own.
- **`enable` also starts the daemon, and `disable` also stops it.** Refused on the second half: a
  command that said "do not start at login" and took away the running daemon would be a command
  nobody could use safely. The first half is then pointless on its own.
