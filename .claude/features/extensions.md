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

**`[recipe]` may accompany any kind, and `kind = "recipe"` means an extension that is *only* that.**
The table above called `recipe` "config-only" and T82 asks for Mailpit *"with the `sendmail_path`
recipe for every managed PHP"* — one product that is both a supervised service and a php.ini change.
Two extensions for it would be two things to install, start and uninstall in step. Corrected by
**T80**, whose design records it as D7.

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
args = ["--listen", "{listen}:{ui_port}", "--smtp", "{listen}:{smtp_port}"]
ready = { type = "tcp", addr = "{listen}:{ui_port}", timeout = "10s" }

[permissions]
services = ["read"]        # what it says it would call — a declaration, see below
network = "loopback"       # loopback | lan — enforced, and this is what `{listen}` renders from
filesystem = ["own-data"]  # own-data | project-roots:read
```

`[service]` is written in the `ServiceSpec` vocabulary from `mixengine-proto`
([ADR 0006](../decisions/0006-servicespec-in-proto-and-secret-free.md)) — one definition, so what an
extension declares and what the supervisor runs cannot drift. Each choice carries its own `type`
discriminator, the way every other enum on the wire does. A duration is written the way a person
writes one (`"10s"`, `"500ms"`) and read into `Millis`.

**The vocabulary, not the struct.** This paragraph used to say `[service]` *deserialises into* a
`ServiceSpec`; **T80 found that it cannot** (the design's D1). A spec has sixteen fields where this
table has four, it names no `ServiceId` — an author writing `program` is not naming a service — and
every path and address here is a template that no `SocketAddr` or absolute-path check would accept.
So `mixengine-core::extensions::manifest` holds its own types over the shared enums, substitutes the
placeholders, and builds the spec through `ServiceSpec::builder` like every other caller — which is
what lets a bad manifest be reported against the line somebody wrote rather than against a spec
nobody did.

**What an extension may declare is what its program *is*, never policy about the machine** (D9):
`program`, `cwd`, `args`, `env`, `ready`, `health`, `restart`, `stop`, `reload`, and its ports.
Resource `limits` belong to the machine's owner, an `idle` policy on something nothing can wake is a
service that stops for good, `logs` are per-home, and `depends_on` is an edge into a graph the
extension cannot see. The `command` forms of `stop` and `reload` are refused too: a second program is
a second path to render, for a capability none of the planned extensions needs.

**And the manifest never writes an address.** `{listen}` renders from `permissions.network` and from
nothing else — `127.0.0.1` for `loopback`, `0.0.0.0` for `lan` — and a host written out anywhere in
the file, `127.0.0.1` included, is refused at parse. That is what makes "an extension with
`network = \"loopback\"` cannot be shared to the LAN" enforced rather than documented: there is no
check to forget, because there is nothing an extension can write that would need one.

The placeholders are substituted between reading the file and building the spec, which is how a
manifest can satisfy the rules a `ServiceSpec` enforces without knowing where it will be installed.
There are four kinds and no others: `{install_dir}` and `{data_dir}` are the paths the installer
chose, so `program` and `cwd` are absolute by the time the spec exists; `{listen}` is the address
`permissions.network` decides; and each key in `[ports]` is a placeholder of its own. Ports live in
that table rather than inside `[service]` because they are an installer concern — a spec has already
been told which port to use. Anything else in braces is refused, naming the field and the
placeholder, rather than left standing to be handed to a program as a literal brace.

The built spec goes through `ServiceSpec::validate` — `ServiceSpec::builder` runs it — so a bad
manifest is reported against the file it came from rather than at the moment the extension is
started. In practice the format's own rules are the stricter of the two, and `restart` is the one
field an author may state that the supervisor will then refuse.

An extension's `[service]` may not carry a secret, because the type has nowhere to put one: an
environment value is either a bare literal (`TZ = "UTC"`) or
`{ from = "keyring", service = …, key = … }`, which the supervisor resolves at spawn time. Writing a
`value` beside `from = "keyring"` is an error rather than a field that is quietly dropped.

`permissions` splits into two that hold and one that discloses — **T80**, and
[ADR 0014](../decisions/0014-an-extension-is-not-an-api-client.md).

- `network` and `filesystem` are enforced by the **format itself**, above: an address exists only as
  `{listen}`, and a path exists only as a placeholder it grew from. Neither is a check the daemon
  performs and could skip.
- `services` is a **declaration shown before the extension is installed**, and enforces nothing.
  There is no scoped token. An extension runs as the user's own account and the access control on
  the endpoint *is* the account, so a token it held is one it could put down and open its own
  connection instead; making it a boundary would mean a token on every connection, `mix` included.
  What it is for is telling a person what they are about to allow — the shape `[scaffold]` consent
  already has — and every surface that prints it says so.

An extension that needs more than this is not an extension: what it wants is a client's standing,
through the same door `mix` uses.

## Registry

**`extensions.json`, published beside `index.json` and signed with the same key** — **T81**. Under
the same moved tag, with a `.minisig` beside it, verified against `index::PUBLIC_KEY` before it is
parsed, cached under the home's cache directory and refused when it walks backwards. Artifacts are
verified by SHA-256 through the runtime installer itself — the download, the staging directory and
the atomic rename are that code and not a second copy of it (see
[../operations/runtime-packaging.md](../operations/runtime-packaging.md)).

**No key of its own.** The blueprint gallery took one because its blast radius differs; an extension
has the package index's exactly — a binary downloaded and supervised — so a third key would separate
nothing and add a third rotation to get half-finished.

**Two documents rather than one array added to the index**, and the reason is failure isolation: an
entry a newer build published has to be skippable, and skipping inside the document that also lists
every runtime would mean `mix runtime list` can die of an extension.

**An entry *is* a manifest**, not a pointer to one. `[artifact.<target>]` already carries the URL and
the hash, so a manifest is the entry a downloader needs — and because permissions arrive with the
listing, what a person is agreeing to can be asked **before a byte of artifact is fetched**. Asking
afterwards is asking after doing the thing they were about to refuse.

**An entry this build cannot read costs that entry and nothing else — and is counted.** `mix
extension available` ends with *"2 entries this build cannot read"* rather than leaving them out in
silence: an extension missing from a listing is one somebody goes looking for in the wrong place.

**Local development**: `mix extension install --path ./my-ext`, recorded as unsigned in its row and
marked on every surface that names it for as long as it is installed.

**Where the document comes from** — **T81a**. The roster is `data/extensions/<id>.toml` in
`mixnz/mixengine-packages`, beside the package index it is published with, and *not* in this
repository: no extension manifest is compiled into MixEngine, so what this repository owns is the
format and the reader while that one owns the roster and the key. `publish-extensions.yml` builds
`mixengine-core`'s `extensions_json` example out of a checkout at the ref being published and renders
every file through `manifest::read` and `manifest::to_value` — the same reader a `--path` install
calls, the same rendering the `manifest_json` column stores — so a published entry and a local file
are one parse and not two. One rule is added that the reader cannot have, because it sees one file
and not the directory around it: **a file's stem must be the `[extension] id` it declares**, which is
also what makes a repeated id impossible.

**The run proves the key before it reads anything.** The generator is compiled from the checkout
being published, so it *holds* `index::PUBLIC_KEY` rather than scraping it out of a source file the
way the blueprint gallery's Python has to, and a `minisign.pub` that disagrees fails the run before a
manifest is opened. A half-finished key rotation is a red run instead of a document at a stable URL
that nothing will accept — rotating the index key is an application release, and the MixEngine
carrying the new key goes out first. The generator then reads its own output back through
`Registry::listing` and refuses to hand over a document holding an entry it cannot itself read: an
unreadable entry is survivable on a user's machine on purpose, and here it can only mean the
generator is older than its own inputs.

Design:
[docs/superpowers/specs/2026-09-02-t81a-publishing-the-extension-registry-design.md](../../docs/superpowers/specs/2026-09-02-t81a-publishing-the-extension-registry-design.md).

## MixDB integration (`desktop-app`)

[MixDB](https://github.com/mixnz/mixdb) is your Tauri database client for MySQL, MongoDB and
Redis — the natural companion to MixEngine's managed databases. Integration, in increasing order of
effort:

1. **Detect & launch** — find an installed MixDB and expose, per database service, the handoff that
   opens that service in it. Ship this first.
2. **Connection handoff** — a `mixdb://` URL carrying host, port and user, **naming** the variable
   the password is in, and the password itself fetched from the OS keyring **at the moment the
   handoff is asked for** and placed in the started process's environment — never in the URL, an
   argument, a file or a log. This section once offered "a one-shot connection file in MixDB's
   import format" as the alternative; T83 refused it, because a password on disk for the length
   of a race is still a password on disk.
3. **Install from the registry** — MixDB's own release artifacts listed as a `desktop-app` extension
   so users can install it from inside MixEngine.
4. **Shared keyring convention** — agree on one service-name convention so both apps read the same
   stored credentials instead of duplicating them.

**"Open in MixDB" is a capability, not a button.** This section said *offer it on every database
service* because it was written while a GUI was still planned inside this workspace, and
[ADR 0011](../decisions/0011-no-gui-in-this-repository.md) removed that GUI. What **T83** built is
therefore two daemon methods — `database.client` answering, per service, what a client would speak
and whether one is here, and `database.open` performing the handoff — and the `mix database`
commands that ask for them — a gap in the CLI is a gap in the product, and there is no screen here
to hide one behind. Whichever graphical client renders an actual button does so out of repo, from
the same methods, which is why the demand is written down in [client-surface.md](client-surface.md)
rather than assumed. Design:
[docs/superpowers/specs/2026-09-03-t83-mixdb-connection-handoff-design.md](../../docs/superpowers/specs/2026-09-03-t83-mixdb-connection-handoff-design.md).

Detection answers a state, not a launch — **three of them, and none is an error**: `installed`, with
the executable this machine would start; `not_installed`, with where this system looked and the
manifest's homepage; and `no_client`, when no `desktop-app` extension is installed at all. A client
renders the last two as an absent affordance with a sentence beside it, and `mix` prints what to
install and exits `1`. "Installed and failed to open" is the one that *is* an error
(`process_failed`), and it is told apart from all three. Locating an installed desktop application
and starting it are both OS-specific, so both live behind `mixengine-platform`'s `DesktopApps` —
which is what makes T83 a task with a platform component on all three systems. The hints it looks
by are the manifest's `[desktop-app.detect]`, and on Windows the lookup reads App Paths *and* the
uninstall table's `DisplayIcon`, because Tauri's installer — MixDB's — writes no App Paths entry.

**The scheme is a wire format, not a dispatch.** MixEngine starts the binary it located, directly,
with the URL as its one argument; it never hands the URL to the operating system's scheme handler.
Following the scheme could not carry the environment the password travels in, and would hand a
credential to whatever program registered `mixdb://` — a claim any program can make.

**The handoff contract, which the `mixdb` repository implements:**

```
argv[1]  mixdb://connect?kind=<mysql|postgres|redis>&host=<ip>&port=<n>[&user=<account>][&database=<name>]&label=<service-id>[&password_env=MIXENGINE_DB_PASSWORD]
env      MIXENGINE_DB_PASSWORD=<the password>   — present exactly when `user` is
```

The receiver reads the variable and **removes it from its environment before anything else
starts** — a Tauri application forks webview helpers and MixDB's terminal module spawns shells, each
inheriting what the parent still holds — and never writes it to its saved-connections file. A
launch that exits `0` within a second is reported as `handed_on`: that is what a single-instance
application does when a copy is already running, and on that day MixDB's second process reads the
variable before it forwards and carries it over its own channel, because the daemon only reaches the
process it started.

Keep the coupling one-directional: MixEngine knows how to hand off to MixDB; MixDB does not need
MixEngine to exist.

## web-app extensions

phpMyAdmin and friends are just sites we own: extracted into `extensions/<id>/app`, given a generated
site config on an internal domain, bound to a runtime version we pick (not the user's project
version), and never exposed to the LAN — **which since T80 is the parse refusing `network = "lan"`
for this kind**, rather than a sentence somebody has to remember. These are administrative interfaces
onto the machine's own databases, and the difference between one of them and a site somebody chose to
share is that nobody chose. Their config is generated from our template so upgrades do
not clobber user settings.

**The template is text the manifest carries, not a file inside the artifact** — **T82**, the design's
D1, which overturns what T80 wrote here. `[web-app].template` named *a file inside the extension*,
and for a registry install the extension's files **are** upstream's archive, verified against a hash
upstream published: there is no step between the download and the rename where a file of ours could
be added without making that hash a hash of something else. So `[web-app.config]` carries a `path`
and the `text` it is rendered from, and a registry entry stays the self-contained thing T81 made it.

**And it is written into the served root, because the application says so.** phpMyAdmin's
`libraries/vendor_config.php` fixes `'configFile' => ROOT_PATH . 'config.inc.php'` with no
environment override — measured, not assumed — so there is one place it can go. Nothing verifies an
install directory after the install, `extension.uninstall` removes the directory whole, and the file
is written from the rows and thrown away, which is the rule `etc/` follows rather than an exception
to it. The user half lives in `{data_dir}`, which outlives an uninstall: a manifest ends its text
with an `@include` of a file there, and that is the split `template` was in the format to provide.

**A `web-app` may declare the database it administers**, and the declaration becomes a row.
`[web-app.database].engines` is a preference order; install resolves it exactly the way T81b resolves
the PHP — before anything is fetched, refused by name when this machine runs none of them, frozen
into `site_service_links`. Writing that row is also what makes `mix service delete <db>` refuse:
`sites::declaring` reads `WHERE s.php_service_id = ? OR l.service_id = ?`, so a link counts, and T82
adds **no** second refusal. Crossing it with `--force` leaves the extension's configuration alone
rather than rewriting it to point nowhere, with a warning naming what to put back.

**One server, and that is the honest limit.** A machine running both MariaDB and MySQL gets one of
them configured, because listing both needs a loop and a loop needs a template language this
workspace does not have. The second server is three lines in `config.user.php`, which the split above
makes survive everything.

**Built by T81b.** The site is a `sites` row owned by the extension — `sites.extension_id`, exclusive
with `project_id` — and is read by everything that reads sites: `served`, the hosts file, the
certificate issuer, `domain.status`, `mix doctor`. Its name is `<label>.mixengine.test`, its pool is
the newest installed PHP inside `[web-app.runtime].requires` at install time, frozen into the row
like a project site's, and `runtime.uninstall` refuses to remove that PHP without `--force`. Through
`site.*` it can be shown, started and stopped, and nothing else: an update, a delete, a share or a
domain change answers *"belongs to the phpmyadmin extension — `mix extension uninstall phpmyadmin`
removes it"*. Design:
[docs/superpowers/specs/2026-09-03-t81b-extension-sites-design.md](../../docs/superpowers/specs/2026-09-03-t81b-extension-sites-design.md).

**And that pool is the extension's own** — **T82a**, which is where this line's "its pool is the
newest installed PHP" was overturned. The PHP is still chosen that way and still frozen; what
changed is that a `web-app` is served on `php-fpm@<extension-id>`, a second `services` row on the
same `runtime_installs` parent, rather than on the `[www]` pool every project site shares. It is
every `web-app`'s and not only a signing-in one, for three reasons: a manifest field must not decide
what runs on the machine, the isolation belongs to the kind rather than to the credential — five
workers shared with an administrative interface walking a large schema is a fact about `web-app`
whether or not a password is involved — and two shapes would be two shapes to test at every site
that installs, uninstalls, repairs or refuses. It costs one php-fpm master per installed `web-app`,
bounded by the idle stop and the on-demand activator T69 and T70 already built. `mix site create`
and `mix site update` refuse that pool to anybody else's site, and a `web-app` installed before T82a
is moved onto one of its own at the next boot rather than by a migration.

## Lifecycle

`extension.inspect <path>` reads a manifest and answers what installing it *here* would produce —
the rendered `ServiceSpec` and all — and installs nothing. **T80**'s, and still the only read-only
one that needs no registry.

**T81** built the rest. `extension.plan` says what installing something would do and changes
nothing; `extension.install` is a job (download, verify, unpack, rename, allocate, write the rows);
`extension.list` and `extension.available` say what is here and what is published; `extension.start`
and `extension.stop` resolve an extension to the `services` row it already **is** and take the walk
`service.start` takes — they add no supervision of their own, which is what *"managed by the same
supervisor as everything else"* means in practice.

**Consent names what was read.** A client shows the plan and sends it back as an
`ExtensionConsent`; the daemon compares the version, the signature and the network reach against the
manifest it is about to install, and refuses if the registry moved in between. That is
`[scaffold]` consent's shape (T78a), for its reason.

**A `services` row for an extension is a third origin**, beside a `packages` row and a
`runtime_installs` one, with a `CHECK` that exactly one of the three is set. Its `ServiceSpec` is
rendered from the manifest stored in its own row — nothing re-reads `extension.toml` out of the
install directory, where a user could have edited it. Every port it holds lives in
`extension_ports`, so the allocator can see it: a port kept where SQL cannot reach is one that gets
handed out twice.

**Uninstall unwinds in reverse** — stop, remove the service row, release the ports, remove the
install directory — and **keeps the data directory** unless asked otherwise, saying where it still
is. That promise is why `{data_dir}` sits at `data/extensions/<id>` rather than inside
`{install_dir}`: T80 nested them, and the first task that had to *act* on the layout found it could
not keep the promise.

**A `[recipe] front_end` fragment is wired, and its server is part of it** — **T81c**. Each
`[[recipe.front_end]]` names `server = "caddy" | "nginx"`, because the two are configuration
languages rather than two files: a Caddyfile fragment is a syntax error in an `nginx.conf`, and the
same value decides how a substituted path is spelled — forward-slashed for nginx, whose parser eats
a backslash, and left as this system writes it for Caddy. Each installed extension renders to one
file in the front end's swept `extensions/` directory, imported by a glob beside `sites/`, so an
uninstall takes the fragment with it on machinery that already existed.

**A fragment is judged by the real server before anything is fetched.** `extension.install` renders
the front end's whole configuration with the prospective fragment in it and runs `caddy validate` or
`nginx -t` over the staged copy; a fragment the server will not parse stops the install, carries the
server's own complaint, and leaves the home byte-identical. That check is what stands in for the
refusal T81 wrote — and it is not the same as failure isolation, which one file per extension does
*not* buy: both servers judge a configuration whole. Two things follow. What is judged is the
fragment with the ports the manifest *asked* for, since allocation happens after; and a fragment can
still be refused later, by an upgraded front end or by the other one after a switch — in which case
the way out is `mix extension uninstall`, which works because it removes the row **before** anything
renders.

**What a fragment can express is what the top level of each language allows**: a Caddy snippet or
site block, an nginx `map`, `upstream` or `server`. Neither reaches inside the site blocks MixEngine
renders. Nothing in T82 asks for one at all — this is a declared field made to take effect rather
than a capability anything published uses yet.

**A `web-app`'s site is written by `extension.install`** where a `service`'s row would be, and the
install then does what `site.create` does after its row — hosts, certificate, regeneration — through
the `Sites` the daemon's `Extensions` holds. `extension.uninstall` removes the site first and answers
with the domain it released. **T81b.**

**And its pool goes with it** — **T82a**. The install writes `php-fpm@<extension-id>` before the site
that names it, and the uninstall takes it away after the site is gone: `sites.php_service_id` is
`ON DELETE SET NULL`, so the other order would leave a site pointing at nothing for one statement,
and an interruption there would leave it that way for good. The daemon **stops** that pool first —
the stop-then-this order `uninstall`'s own note already states for a `service` — because
`services::delete` looks at no process, and `extension.uninstall` answers with the pool it removed
rather than leaving a `mix service list` entry to be discovered.

## Acceptance criteria

- Install Mailpit from the registry and have PHP `mail()` captured, with no manual php.ini edit
  (the recipe sets `sendmail_path` for every managed PHP). **The line lands when the extension is
  installed** — T81 wrote it only at boot and after a runtime install, so it took a restart to
  appear; **T82** found that by installing the real Mailpit and made the install rewrite the ini set,
  which is T81c's lesson arriving for the other half of `[recipe]`.
- phpMyAdmin reaches the managed MariaDB on an internal domain with a valid certificate, its server,
  port and account already filled in, **and signs itself in**. **Split between two tasks, and the
  reason is where a password can safely be** — **T82**, its design's D6. T82 delivered everything but
  the password: nothing this system generates writes a credential to disk, which is why
  `mix database` answers the address a credential is stored under rather than the credential, and why
  `generate::step::SecretFile` exists only to remove one afterwards. Signing in without typing it
  means putting the credential in one process's environment, and the single `[www]` pool per PHP
  version is shared with every project on the machine.
  **T82a is that process.** Every `web-app` gets a php-fpm pool of its own — always, not only the
  ones that ask, because a manifest field must not decide what runs on the machine — and one
  declaring `[web-app.database].signs_in` has the superuser's password in that pool's environment,
  resolved from the OS keyring by the supervisor at spawn. On no disk, in no other project's
  process, and reachable only from this machine, because an extension's site binds loopback in both
  front ends and cannot be shared. The variable's name is MixEngine's rather than a manifest's —
  `{db_password_env}` renders it — which is T80's *"there is no check to forget"* applied to a second
  field. A locked keyring costs that one site rather than every project's, which is the same
  dedicated pool paying for itself twice. Design:
  [docs/superpowers/specs/2026-09-03-t82a-a-pool-of-the-extensions-own-design.md](../../docs/superpowers/specs/2026-09-03-t82a-a-pool-of-the-extensions-own-design.md).
- `mix` hands a managed database service to MixDB and MixDB opens with that connection preselected,
  its password never appearing in an argument, a URL or a log.
- Where MixDB is not installed the same call answers that as a state, not as a failure, and the CLI
  says what to install rather than what went wrong.
- An extension with `network = "loopback"` cannot be shared to the LAN — enforced, not documented.
