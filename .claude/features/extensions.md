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

[service]
program = "mailpit"
args = ["--listen", "127.0.0.1:{ui_port}", "--smtp", "127.0.0.1:{smtp_port}"]
ready = { tcp = "127.0.0.1:{ui_port}", timeout = "10s" }
ports = { ui_port = 8025, smtp_port = 1025 }

[permissions]
services = ["read"]        # what the extension may call on the daemon API
network = "loopback"       # loopback | lan
filesystem = ["own-data"]  # own-data | project-roots:read
```

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

[MixDB](https://github.com/haiquang9994/mixdb) is your Tauri database client for MySQL, MongoDB and
Redis — the natural companion to MixEngine's managed databases. Integration, in increasing order of
effort:

1. **Detect & launch** — find an installed MixDB and add "Open in MixDB" to every database service in
   the GUI. Ship this first.
2. **Connection handoff** — a `mixdb://` deep link (or a one-shot connection file in MixDB's import
   format) carrying host, port, user and a credential fetched from the OS keyring at click time.
   Never write a password into a URL that lands in a shell history or a log.
3. **Install from the registry** — MixDB's own release artifacts listed as a `desktop-app` extension
   so users can install it from inside MixEngine.
4. **Shared keyring convention** — agree on one service-name convention so both apps read the same
   stored credentials instead of duplicating them.

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
- "Open in MixDB" launches MixDB with the right connection preselected.
- An extension with `network = "loopback"` cannot be shared to the LAN — enforced, not documented.
