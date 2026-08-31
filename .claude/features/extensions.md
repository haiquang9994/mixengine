# Extensions

**Goal**: a small, curated store for the tools developers reach for — phpMyAdmin, pgAdmin, Mailpit,
MinIO, MeiliSearch, **MixDB** — installable in one click, managed by the same supervisor as
everything else.

## Extension kinds

| Kind | What it is | Example | How it runs |
| --- | --- | --- | --- |
| `web-app` | PHP/Node source served by our stack | phpMyAdmin, Adminer | A generated internal site (`phpmyadmin.mixengine.test`) on a managed runtime |
| `service` | A binary we supervise | Mailpit, MinIO, MeiliSearch | A `ServiceSpec`, same as any bundled service |
| `desktop-app` | A separate installed application we detect and launch | **MixDB** | We do not bundle it; we detect/install it and pass connection details |
| `recipe` | Config-only addition | extra Caddy directives, a php.ini profile | Merged into config generation |

## Manifest (`extension.toml`)

```toml
schema = 1

[extension]
id = "mailpit"
name = "Mailpit"
version = "1.20.0"
kind = "service"
description = "Local SMTP capture and web UI"
homepage = "https://mailpit.axllent.org"

[artifact.windows-x86_64]
url = "https://…/mailpit-windows-amd64.zip"
sha256 = "…"

[ports]
ui_port = 8025
smtp_port = 1025

[service]
program = "{install_dir}/mailpit"
cwd = "{data_dir}"
args = ["--listen", "127.0.0.1:{ui_port}", "--smtp", "127.0.0.1:{smtp_port}"]
ready = { type = "tcp", addr = "127.0.0.1:{ui_port}", timeout = "10s" }

[permissions]
services = ["read"]        # what the extension may call on the daemon API
network = "loopback"       # loopback | lan
filesystem = ["own-data"]  # own-data | project-roots:read
```

`[service]` deserialises into the `ServiceSpec` vocabulary in `mixengine-proto`
([ADR 0006](../decisions/0006-servicespec-in-proto-and-secret-free.md)) — one definition, so what an
extension declares and what the supervisor runs cannot drift. Each choice carries its own `type`
discriminator, the way every other enum on the wire does. A duration is written the way a person
writes one (`"10s"`, `"500ms"`) and read into `Millis`.

The placeholders are substituted before that table is read, which is how a manifest can satisfy the
rules a `ServiceSpec` enforces without knowing where it will be installed: `{install_dir}` and
`{data_dir}` are the paths the installer chose, so `program` and `cwd` are absolute by the time the
spec exists, and each `{…_port}` is the allocation made from `[ports]`. Ports live in their own
table rather than inside `[service]` because they are an installer concern — a spec has already been
told which port to use. The manifest is then put through `ServiceSpec::validate`, so a bad one is
reported against the file it came from rather than at the moment the extension is started.

An extension's `[service]` may not carry a secret, because the type has nowhere to put one: an
environment value is either a bare literal (`TZ = "UTC"`) or
`{ from = "keyring", service = …, key = … }`, which the supervisor resolves at spawn time. Writing a
`value` beside `from = "keyring"` is an error rather than a field that is quietly dropped.

`permissions` is enforced by the daemon: the extension's scoped token grants exactly these. No
extension can call `daemon.*`, `cert.*`, or any `PrivilegedOp`. An extension that needs more is not
an extension.

## Registry

- A signed `index.json` in a public git repo, fetched over HTTPS and verified with an Ed25519 key
  compiled into the binary; artifacts verified by SHA-256 (same pipeline as runtimes, see
  [../operations/runtime-packaging.md](../operations/runtime-packaging.md)).
- Local development: `mix extension install --path ./my-ext` with a loud "unsigned" marker.
- The registry is versioned by `schema`; an older MixEngine ignores entries it cannot parse instead
  of failing the whole index.

## MixDB integration (`desktop-app`)

[MixDB](https://github.com/mixnz/mixdb) is your Tauri database client for MySQL, MongoDB and
Redis — the natural companion to MixEngine's managed databases. Integration, in increasing order of
effort:

1. **Detect & launch** — find an installed MixDB and expose, per database service, the handoff that
   opens that service in it. Ship this first.
2. **Connection handoff** — a `mixdb://` deep link (or a one-shot connection file in MixDB's import
   format) carrying host, port, user and a credential fetched from the OS keyring **at the moment
   the handoff is asked for**, never stored ahead of it. Never write a password into a URL that
   lands in a shell history or a log.
3. **Install from the registry** — MixDB's own release artifacts listed as a `desktop-app` extension
   so users can install it from inside MixEngine.
4. **Shared keyring convention** — agree on one service-name convention so both apps read the same
   stored credentials instead of duplicating them.

**"Open in MixDB" is a capability, not a button.** This section said *offer it on every database
service* because it was written while a GUI was still planned inside this workspace, and
[ADR 0011](../decisions/0011-no-gui-in-this-repository.md) removed that GUI. What **T83** builds is
therefore a daemon method answering the handoff for one database service, and the `mix` command that
asks for it — a gap in the CLI is a gap in the product, and there is no screen here to hide one
behind. Whichever graphical client renders an actual button does so out of repo, from the same
method, which is why the demand has to be written down in [client-surface.md](client-surface.md)
rather than assumed.

Detection answers a state, not a launch. "MixDB is not installed" is an ordinary answer a client
renders as an absent affordance; it is not an error, and it is not the same answer as "MixDB is
installed and failed to open". Locating an installed desktop application and following a URL scheme
are both OS-specific, so both live behind `mixengine-platform` like every other per-OS behaviour —
which is what makes T83 a task with a platform component on all three systems.

Keep the coupling one-directional: MixEngine knows how to hand off to MixDB; MixDB does not need
MixEngine to exist.

## web-app extensions

phpMyAdmin and friends are just sites we own: extracted into `extensions/<id>/app`, given a generated
site config on an internal domain, bound to a runtime version we pick (not the user's project
version), and never exposed to the LAN. Their config is generated from our template so upgrades do
not clobber user settings.

## Lifecycle

`extension.install` → job (download, verify, extract, register services/sites) → `extension.start`.
Uninstall removes services, generated sites, and the directory; it asks before deleting the
extension's data dir.

## Acceptance criteria

- Install Mailpit from the registry and have PHP `mail()` captured, with no manual php.ini edit
  (the recipe sets `sendmail_path` for every managed PHP).
- phpMyAdmin reaches the managed MariaDB with credentials taken from the keyring, on an internal
  domain with a valid certificate.
- `mix` hands a managed database service to MixDB and MixDB opens with that connection preselected,
  its password never appearing in an argument, a URL or a log.
- Where MixDB is not installed the same call answers that as a state, not as a failure, and the CLI
  says what to install rather than what went wrong.
- An extension with `network = "loopback"` cannot be shared to the LAN — enforced, not documented.
