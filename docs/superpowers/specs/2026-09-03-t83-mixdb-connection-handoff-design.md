# T83 — MixDB integration: the connection handoff (design)

Roadmap task **T83**, phase 8. T77a made a database and stored its account's password in the OS
keyring; T82a handed that password to a process of MixEngine's own. Both stopped at the same line:
*handing a credential to a program that needs one is T83's*. This is that program — a desktop
database client the user installed, which MixEngine finds, starts, and points at one of its own
servers, with the password read from the keyring at that moment and placed in no argument, no URL
and no file.

## Goal

`mix database open mariadb@main` on a machine with MixDB installed starts MixDB with that server's
address, port and account already in hand, signed in as `root`, and nothing on the command line, in
a log, in a shell history or on disk holds the password. On a machine without MixDB the same
command says so as a state — and says what to install — rather than failing. A graphical client can
ask both questions through the API and draw the affordance from the answer.

## Measured, not assumed

Every line below was read off this workspace, off the neighbouring `mixdb` checkout, or off the
machine this was designed on.

- **MixDB registers no URL scheme today.** `src-tauri/src/lib.rs` loads the opener, store, dialog,
  clipboard, updater, process and window-state plugins and nothing else; the only
  `register_uri_scheme_protocol` is the REST module's preview, which is an in-app scheme. There is no
  `tauri-plugin-deep-link` and no `tauri-plugin-single-instance`. A `mixdb://` URL handed to the
  operating system today lands in a "no application" dialog.
- **Tauri's NSIS installer writes no App Paths entry.** The upstream template
  (`crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`) writes
  `Uninstall\${PRODUCTNAME}` with `DisplayIcon = "$INSTDIR\${MAINBINARYNAME}.exe"` (quoted) and
  `InstallLocation`, installs into `$LOCALAPPDATA\${PRODUCTNAME}` for the current user, and registers
  `Software\Classes\<scheme>` only for the deep-link protocols a build declares. On this machine:
  `HKCU\…\Uninstall\MixDB` holds `DisplayIcon = "C:\Users\…\AppData\Local\MixDB\mixdb.exe"`, the
  binary is `mixdb.exe` in lower case, and neither `App Paths\MixDB.exe` nor `App Paths\mixdb.exe`
  exists. T80's `DetectHints.windows` says *"an executable name, looked for under App Paths"*, which
  would find nothing.
- **The variable a credential travels in already has a name.** T82a's D2 fixed
  `MIXENGINE_DB_PASSWORD` (`extensions::render::CREDENTIAL_ENV`) as the name a manifest cannot write
  and a pool's environment carries. A second name for the same thing would be two names to keep in
  step.
- **Every database recipe already answers the account.** `Recipe::administrator()` is `root` for the
  MySQL family and `postgres` for PostgreSQL, `None` for everything else, and
  `Context::secret_address(user)` composes `<service-id>/<user>` — the address `database.create`
  reports and `services::databases::read` reads. Redis declares no administrator: its recipe sets no
  password, so there is nothing to hand over and nothing to refuse.
- **`extensions::database::endpoint` already resolves a service to host, port and account**, off
  `services.port`, `services.bind_addr` and the recipe — and answers `None` for a service whose recipe
  names no administrator, which for phpMyAdmin is right and for a client that opens Redis is not.
- **`process::spawn_detached` inherits this process's environment and takes none of its own**, and on
  Unix its whole detachment is `setsid`: the child stays this process's child, so a daemon that
  starts one and lives on owns a zombie the moment it exits. `Detached::exited` is `try_wait` and
  never blocks.
- **The mock host records one recorder per capability**, and `SecretOp` deliberately records that a
  credential was stored and never what it was.
- **A `desktop-app` extension installs today**: `install.rs` gives a kind with no artifact its
  directory and its row, and `manifest::DesktopApp` carries `scheme` and per-OS `detect` hints —
  *declared only*, with T83 named as the consumer. The testkit fixture `mixdb.toml` is one.
- **MixDB speaks four kinds** — `DbKind::{Mysql, Postgres, Mongo, Redis}` — and its
  `ConnectionConfig` is host, port, username, password, database. What a handoff has to say fits in
  five fields.

## Scope

**In:** `mixengine-platform` — a `DesktopApps` capability on `Host` (locate, launch), three
implementations, a mock that records, a shared launcher over `spawn_detached`, which gains an
environment argument; the pure readers of a desktop entry's `Exec` line and of a quoted registry path,
compiled on all three systems. `mixengine-core` — `Recipe::protocol()`, and `services::handoff`, which
resolves a service to an address and renders the URL. `mixengine-proto` — `database.client` and
`database.open` with their types. `mixengine-daemon` — both methods, and the reaper. `mixengine-cli` —
`mix database client` and `mix database open`. `mixengine-testkit` — nothing new; the fixture manifest
is already there.

Documentation: [features/extensions.md](../../../.claude/features/extensions.md) gains the handoff
contract; [features/client-surface.md](../../../.claude/features/client-surface.md)'s gap becomes a
claim; [architecture/daemon-and-ipc.md](../../../.claude/architecture/daemon-and-ipc.md) gains the
`database.*` namespace it never listed;
[architecture/platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md) gains
the trait; the roadmap.

**Out:**

- **MixDB's receiving side.** Reading the URL from `argv`, the password from the environment, and
  opening the tab is work in the `mixdb` repository. This design writes the contract it implements
  (D2) and changes nothing there; the coupling stays one-directional, as `features/extensions.md`
  requires.
- **MixDB in the registry** and a shared keyring naming convention — T84. Until it lands, the
  extension is installed from a directory with `mix extension install --path`.
- **A `mixdb://` scheme registration on MixDB's behalf.** MixEngine never writes another
  application's registry keys, `Info.plist` or desktop entries.
- **Flatpak and Snap launches.** A desktop entry whose `Exec` is `flatpak run …` starts the sandbox
  without the environment this design relies on; it is found, launched, and the credential does not
  arrive. Named in the risks, not solved.

## Decisions

### D1 — The scheme is a wire format, not a dispatch

`features/extensions.md` says *"a `mixdb://` deep link … carrying host, port, user and a credential
fetched from the OS keyring"*, and the roadmap says *"never placed in an argument or a URL"*. Both are
kept, by not handing the URL to the operating system.

Following a URL scheme — `ShellExecute`, `open`, `xdg-open` — means three things this design cannot
accept. The credential cannot travel beside it, because none of the three lets the caller set the
launched process's environment without putting it on a command line. Whatever program has
*registered* the scheme receives the handoff, and a scheme registration is a claim any program can
make. And on the MixDB that exists today nothing has registered it at all.

So the platform layer **locates the installed application and starts its binary directly**, with the
URL as its one argument and the credential in its environment. The URL is the format the address is
spelled in — the one MixDB will read out of `argv` on Windows and Linux and out of the Apple event on
macOS the day it registers the scheme — and MixEngine never asks the OS who owns `mixdb://`.

### D2 — The credential is in the environment of the process this daemon starts, under the name T82a fixed

Four places a password could be put, and three are refused.

- **An argument or the URL**: readable through `/proc/<pid>/cmdline` by every account on a Linux
  machine, logged by process auditing on Windows, and one `ps` away everywhere. Refused by the
  roadmap line itself.
- **A one-shot file** (`features/extensions.md`'s alternative): a password on disk for the length of
  a race, in a format MixDB does not read. T82a's *"on no disk"* applies.
- **The environment of a process the daemon spawns**: `/proc/<pid>/environ` is owner-only and
  ptrace-guarded, `ps -E` shows another user's environment to root alone, and nothing audits it.
  It is what T82a chose for php-fpm and what the recipes already do for `mariadb-admin ping`.

The variable is **`MIXENGINE_DB_PASSWORD`** — `extensions::render::CREDENTIAL_ENV`, moved to a place
both consumers reach — and the URL names it, so the contract describes itself:

```
mixdb://connect?kind=mysql&host=127.0.0.1&port=3306&user=root&database=blog&label=mariadb%40main&password_env=MIXENGINE_DB_PASSWORD
```

`kind` is one of `mysql`, `postgres`, `redis` (D5). `user`, `database` and `password_env` are absent
when there is nothing to say: a Redis handoff carries `kind`, `host`, `port` and `label`. `label` is
the service id, for the tab MixDB will name. Values are percent-encoded by a ten-line encoder in
`services::handoff` — everything outside the unreserved set becomes `%XX` — rather than by a crate
taken for one function.

**What the receiving side owes**, written into `features/extensions.md` as the contract:
read `argv[1]`, read the variable, **remove it from the process environment before anything else
starts** — a Tauri application forks webview helpers and MixDB's terminal module spawns shells, and
each inherits what the parent still holds — and never write it to its saved-connections file. The
password is the user's own, on a machine the security model calls single-user; what the removal
buys is that a shell opened inside MixDB does not print it.

### D3 — The client is the installed `desktop-app` extension, and having none is a state

The hints that find MixDB — `MixDB.exe`, a bundle identifier, `mixdb.desktop` — live in
`[desktop-app.detect]`, and T80 put them there so that *the manifest says what each system looks it
up by*. This design reads them from the installed extension's manifest and nowhere else. A MixDB
identity compiled into the daemon would be a second copy T84's registry entry then has to agree
with, and a product name in a crate that has so far named none.

Three answers, and every method that touches a client says which:

| `DesktopClient` | Meaning |
| --- | --- |
| `installed { extension, name, program }` | An extension of kind `desktop-app` is installed, and this machine has the application |
| `not_installed { extension, name, searched, homepage }` | The extension is installed; the application is not on this machine. `searched` says where this system looked |
| `no_client` | No `desktop-app` extension is installed here |

Where more than one `desktop-app` extension is installed the first by id is the client; nothing today
installs two, and a preference is a setting nobody has asked for.

`no_client` and `not_installed` are **states, not errors** — the roadmap line — because a client
renders them as an absent affordance with a sentence beside it, and an error would be rendered as a
failure of something the person did. `mix` prints what to install and exits `1`, for the reason
`mix service start` exits non-zero when nothing came up: `mix database open db && …` is a sentence
about a client having opened.

### D4 — Windows looks in App Paths and then in the uninstall table; the hint is a file name

Measured above: Tauri's installer writes no App Paths entry, so the lookup T80 documented finds
nothing. The Windows hint stays what it is — an executable's file name — and the lookup becomes:

1. `HKCU` then `HKLM`, `Software\Microsoft\Windows\CurrentVersion\App Paths\<hint>`, default value.
   The documented mechanism, honoured by Inno Setup, MSI and most hand-written installers.
2. `HKCU` then `HKLM`, `Software\Microsoft\Windows\CurrentVersion\Uninstall\*` and the
   `WOW6432Node` table beside it: every subkey's `DisplayIcon`, with its quotation marks and any `,0`
   icon index stripped, whose file name equals the hint **case-insensitively** — `mixdb.exe` is what
   the installer wrote and `MixDB.exe` is what the manifest says, and NTFS agrees they are one file.
   A subkey with no `DisplayIcon` but an `InstallLocation` under which `<hint>` exists counts too.

Enumerating the uninstall table is a few hundred `RegEnumKeyExW` calls and takes milliseconds; it is
what Programs and Features does. The registry is reached through `windows-sys` as `windows/path.rs`
already does, never through `reg.exe` or PowerShell. The `DetectHints.windows` doc comment is
corrected to say both places.

macOS: `mdfind "kMDItemCFBundleIdentifier == '<id>'"`, the identifier first held to
`[A-Za-z0-9.-]` so the query is a literal; among the paths Spotlight answers, one under
`/Applications` or `~/Applications` is preferred and anything under a `.Trash` is skipped; the
executable is `<bundle>/Contents/MacOS/<name>` with `<name>` read by `defaults read
<bundle>/Contents/Info CFBundleExecutable`, because `Info.plist` may be binary. A machine where
`mdfind` cannot run answers `Error::Command`, which the daemon reports as it is — Spotlight being off
is a fact about the machine and not a "not installed".

Linux: `$XDG_DATA_HOME/applications` (default `~/.local/share/applications`) and each entry of
`$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`) with `/applications`, in that order; the
first file named `<hint>` wins. `TryExec=` names the program where present, else the first word of
`Exec=` with field codes (`%u`, `%U`, `%f`, …) removed and the remaining words kept as fixed
arguments; a bare name resolves on this process's `PATH`. The reader of that line is a pure function
compiled on every system and tested on every system, like `reserved`'s and `prompt`'s tables.

### D5 — The protocol is the recipe's answer, and it is not MixDB's vocabulary

The URL has to say what to speak. `mariadb` speaks MySQL's protocol; that is a fact about the server,
not about any client. So `Recipe` grows `fn protocol(&self) -> Option<DatabaseProtocol>` beside
`administrator()`, answering `Mysql` for MariaDB and MySQL, `Postgres` for PostgreSQL, `Redis` for
Redis, and `None` by default — the front ends, the pool and memcached are not something a database
client opens. `DatabaseProtocol` lives in `mixengine-proto`, since the report carries it, and its
`as_str` is the word in the URL.

`services::handoff::address(store, service)` reads the row and the recipe the way
`extensions::database::endpoint` does and answers `Address { protocol, host, port, administrator }`
— or `None` for a service with no protocol. It is a second function rather than a change to
`endpoint` because the two disagree on Redis on purpose: phpMyAdmin cannot administer a cache, and
MixDB can open one.

`database.client` answers `protocol: null` for such a service, as a state. `database.open` refuses
it with `invalid_argument` naming the service — the T77a distinction: this operating system can do
it, and the package is what cannot.

### D6 — Two methods, because a button is drawn before it is pressed

`client-surface.md` asks for *"whether a desktop database client is installed to hand the connection
to, and the handoff itself, answered per service"*. Detection that only happens inside the handoff
would leave a graphical client probing the filesystem to decide whether to draw the button — the
business logic `CLAUDE.md` keeps out of clients.

- **`database.client`** takes `DatabaseClientQuery { service }`, answers
  `DatabaseClientReport { service, protocol, client }`. **Reads only**: it starts nothing, launches
  nothing, and touches the keyring not at all.
- **`database.open`** takes `DatabaseOpen { service, user?, database? }`, answers
  `DatabaseHandoff { service, protocol, user, database, secret, client, launched }` where `secret` is
  the keyring address the password was read from (the field `DatabaseAccount` already has, and never
  the value) and `launched` is `running { pid }` or `handed_on` (D8), present exactly when `client`
  is `installed`.

Both are `mix database client <SERVICE>` and `mix database open <SERVICE>`: every method reachable
from the one client this repository ships.

### D7 — `open` starts the instance, on `database.create`'s road

A client opened onto a stopped server shows "connection refused", and since T69 a server that nobody
has used for a while *is* stopped. `database.open` calls `Registry::ensure_running` — the same graph,
plan and walk `service.start` uses, so a dependency comes up first and a first run is performed,
which is also what puts the superuser credential in the keyring for step 9 below to read. It is asked
**after** the client is located and **before** the credential is read: a machine without MixDB
should not first pay for a database server coming up, and a credential should be read as late as the
order allows.

### D8 — A launch is judged for one second, and a clean exit inside it is a handoff, not a failure

Two things can happen in the second after a desktop application starts, and they look alike from
a daemon that only has a pid.

The application can die at once: a Linux daemon started by a `systemd --user` unit with no `DISPLAY`
imported, a bundle whose binary the installer left unsigned on a machine that refuses it. Answering
`running { pid }` for that would report success for something a person is looking at nothing of.

And the application can **hand on and exit 0**: this is what every Tauri application with the
single-instance plugin does when it is already running — the second process forwards its `argv` to
the first and ends. MixDB will do this the day it adopts that plugin, and a design that read a fast
exit as a failure would fail on the most common case of all, the client already being open.

So the launcher waits up to one second on `Detached::exited`: still running is `running { pid }`;
exited with success is `handed_on`; exited otherwise is `Launch::Failed { status }`, which the daemon
reports as `process_failed` — *"MixDB exited a moment after it was started (exit code 1)"* — with the
program's path in the hint so a person can run it by hand and read what it says. One second is a
heuristic and is written down as one; it is also what nobody typing `mix database open` will notice.

**A handoff to a running instance cannot carry the environment**, since the daemon only reaches the
process it started. That is MixDB's to solve on the day it forwards — its second process reads the
variable before it forwards and sends it over its own channel — and the contract in
`features/extensions.md` says so.

### D9 — One reaper thread, not one per launch

On Unix the launched application stays this daemon's child (measured above), and a daemon that
never waits on it leaves a zombie for as long as the daemon runs. `spawn_blocking(child.wait())` per
launch would be a thread held for the life of every MixDB window somebody opens. Instead the shared
launcher hands each `Detached` to **one** thread, started on the first launch, that polls
`exited()` every few seconds and drops what has ended, logging the exit at `debug`. On Windows the
same thread costs nothing and closes the handle. It is in `mixengine-platform` beside the launcher,
because what it exists for is an OS mechanism.

### D10 — The account is the administrator unless asked, and a named account must already be ours

`--user blog` opens the database as the account `database.create` made, which is what milestone M8's
*"open its database"* means for a project. The credential address is
`<service-id>/<user>` — `Context::secret_address`'s composition, the same string `database.create`
reported — and it is read through `services::databases::read`. Nothing there means
`precondition_failed`: for the administrator, T77a's sentence about the first run; for a named
account, *"MixEngine holds no credential for `blog` on `mariadb@main`"* with `mix database create`
in the hint. `--database blog` preselects a database and is validated by `validated_identifier`,
as `--user` is, before anything is started.

A service whose protocol has no accounts — Redis — refuses `--user` with `invalid_argument` rather
than opening a client with an account the server will ignore.

### D11 — The launched process inherits the daemon's environment, and that is the opposite of a supervised child on purpose

`Runner::spawn_environment` clears a service's environment down to a per-OS floor (T82a's
measurement), because a service that behaves differently by how the daemon was started is a bug
nobody can reproduce. A desktop application is the other case: it needs `DISPLAY`,
`WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`, `XDG_RUNTIME_DIR`, the locale, the Keychain session —
the user's session, which is what the daemon's own environment is when a person started it or the
login item did. `spawn_detached` already inherits for its own reason; it grows an `extra` map applied
on top, and the launcher's map holds one entry or none. Nothing in the daemon's environment is a
secret, which is what makes inheriting it safe to say in one sentence.

### D12 — The mock records the program, the arguments and the variable *names*

`SecretOp` set the rule: a recorder holding a credential would be the one place in the tree where
one sits in memory after the process that needed it is gone. `mock::Host::launched()` answers
`Vec<Launched { program, args, env_names }>`, and every daemon test about the handoff asserts on
that: the URL is in `args` and carries no password, `env_names` is exactly
`["MIXENGINE_DB_PASSWORD"]` for a server with an account and empty for Redis. `mock::Host::with_desktop_app(home, program)` is a host on which every hint locates `program`; the default host locates
nothing, which is the ordinary machine.

## Data flow

```
mix database client mariadb@main
  daemon: handoff::address        → { mysql, 127.0.0.1, 3306, root }   (or protocol: null)
          extension_store::all    → kind desktop-app → mixdb, hints
          host.desktop_apps().locate("MixDB.exe")   [spawn_blocking]
             windows: App Paths → Uninstall\*\DisplayIcon
             macos:   mdfind bundle id → defaults read CFBundleExecutable
             linux:   XDG applications/<hint>.desktop → TryExec/Exec
  answer: { service, protocol: "mysql", client: installed { mixdb, "MixDB", program } }

mix database open mariadb@main --user blog --database blog
  daemon: validated_identifier(user), validated_identifier(database)
          handoff::address        → mysql … root       (None → invalid_argument)
          locate                  → not_installed / no_client → answered, exit 1 in mix
          services.ensure_running(mariadb@main)        (D7; first run stores root's credential)
          databases::read(keyring, "mariadb@main/blog") → password   (None → precondition_failed)
          handoff::url(scheme, address, user, database)
            mixdb://connect?kind=mysql&host=127.0.0.1&port=3306&user=blog&database=blog
                            &label=mariadb%40main&password_env=MIXENGINE_DB_PASSWORD
          host.desktop_apps().launch(program, [fixed args…, url], {MIXENGINE_DB_PASSWORD: password})
            spawn_detached(program, args, program's directory, extra env)
            judged for one second → running { pid } | handed_on | failed
            reaper takes the Detached
  answer: { …, secret: "mariadb@main/blog", client: installed {…}, launched: running { pid } }

  mixdb:  argv[1] = the URL; getenv MIXENGINE_DB_PASSWORD, then unset it; opens the tab   (out of repo)
```

## Testing

Where the rule lives, per `.claude/standards/testing.md`.

**Unit, `mixengine-core`.** `handoff::url` renders each protocol, omits `user`/`database`/
`password_env` when absent, and percent-encodes `@` and a space; the rendered URL for a server with an
account never contains the word `password=` — the T77a shape of asserting what is *not* on the wire.
`handoff::address` over an in-memory store (the `extensions::database` tests' fixture): `mariadb`
answers `mysql` and `root`; `redis` answers `redis` and no account; `memcached` answers `None`; a
service with no row is `NotFound`. `Recipe::protocol` per recipe.

**Unit, `mixengine-platform`, on every system.** `desktop::entry::exec_line`: `mixdb %U` →
`mixdb`, no args; `"/opt/My App/bin/mixdb" --flag %u` → program with the space, `["--flag"]`;
`TryExec` wins over `Exec`; a line with only field codes. `desktop::entry::unquoted`: `"C:\a\b.exe"`
→ `C:\a\b.exe`; `C:\a\b.exe,0` → `C:\a\b.exe`; a bare path unchanged.

**Real OS, `crates/mixengine-platform/tests/desktop.rs`.** `launch` against `cmd /c` and `/bin/sh -c`:
a program that exits 0 at once answers `handed_on`; one that exits 3 answers `Failed` naming the
status; one that sleeps answers `running { pid }` and is gone from the reaper afterwards; a program
that exits 0 only when the variable is set (`if not defined X exit 5` / `test -n "$X"`) proves the
environment reaches the child. Windows `locate` against a key the test creates under
`HKCU\Software\MixEngine\tests\…` and deletes — `windows/path.rs`'s `TestKey` pattern, with the
roots a field of `Apps` — for both the App Paths and the `DisplayIcon` route, quoted and with an icon
index. Linux `locate` against a temporary directory named as the only data dir.

**Component, `crates/mixengine-daemon/src/api/rpc.rs`.** With rows for `redis` / `redis@main` and a
`fakeservice` spec declared under that id, on a mock host: no extension → `no_client`; the fixture
manifest installed and the default host → `not_installed` naming what was searched; `with_desktop_app`
→ `database.open` answers `launched: running`, the recorder holds one launch whose last argument
starts with `mixdb://connect?kind=redis` and whose `env_names` is empty; `--user` on Redis →
`invalid_argument`; `memcached@main` → `client` answers `protocol: null` and `open` refuses by name.
With rows for `mariadb` / `mariadb@main`: no credential in the mock keyring → `precondition_failed`
with T77a's sentence; a seeded `mariadb@main/root` → launched with `env_names ==
["MIXENGINE_DB_PASSWORD"]` and a URL that does not contain the seeded value; `--user blog` with no
entry → `precondition_failed` naming `blog`.

**CLI, `crates/mixengine-cli/tests/database.rs`, on all three systems through the real locator.** A
`desktop-app` extension installed with `--path` whose hints name nothing any machine has
(`mixengine-test-nothing.exe`, `test.mixengine.nothing`, `mixengine-test-nothing.desktop`), and rows
for `redis@main`: `mix database client redis@main --json` answers `protocol: "redis"` and
`client.state: "not_installed"` with a non-empty `searched`; `mix database open redis@main` exits `1`
and prints the extension's homepage; with no extension both answer `no_client` and `open` prints
`mix extension install mixdb`. **This is the (P) verification**: each system's own registry, Spotlight
or XDG walk is what answers.

**The real run, `crates/mixengine-cli/tests/mariadb.rs`, Linux only, in CI's keyring session.** A
desktop entry in a temporary `XDG_DATA_HOME` whose `Exec` is a shell script the test writes; the
daemon started with that variable; `mix database open mariadb@main`. The script records its first
argument and whether `MIXENGINE_DB_PASSWORD` is set — **never its value** — and the test asserts the
URL names `kind=mysql`, `user=root` and the instance's port, and that the variable was present. It is
the only test in the workspace in which a real credential reaches a real process through this path,
and it is what proves D2 rather than restating it.

## Risks, and where each is answered

| Risk | Answer |
| --- | --- |
| The password reaches a log, a shell history or a URL | D2 — environment only; the mock records names; the Linux run records presence, never value |
| Whatever registered `mixdb://` receives the handoff | D1 — the located binary is started directly; the OS is never asked who owns the scheme |
| A running MixDB never sees the credential | D8 — `handed_on` is reported honestly; the contract says what MixDB's second process owes |
| A shell opened inside MixDB inherits the variable | D2 — the contract asks the receiver to remove it at start; the exposure is the user's own session |
| The daemon has no display to hand the app | D8 — a fast non-zero exit is `process_failed` with the program's path |
| Zombies on Unix | D9 |
| Tauri's installer writes no App Paths entry | D4 — measured, and the uninstall table is read too |
| Spotlight is off on macOS | D4 — reported as the tool's failure, not as "not installed" |
| A Flatpak or Snap MixDB | Out of scope, named above: found and launched, credential does not cross the sandbox |
| Two `open`s at once | Two windows, and no lock: `ensure_running` is idempotent and nothing here writes |
| A locked keyring | `database.create`'s answer — the read blocks off the runtime and a person is at the machine to unlock it |

## What this leaves

`features/extensions.md`'s integration list has its first two items built: MixDB is found per system,
and `mix` hands it a managed database with the password in one process's environment and nowhere
else. The third and fourth — MixDB in the registry, and one keyring convention both applications
read — are T84. And `mixdb` has a contract to implement: read the URL, read the variable, forget
the variable, open the tab.
