# T80 — the extension model (design)

Roadmap task **T80**, phase 8. `extension.toml` has been named in this workspace since before there
was anything to read it: `ServiceSpec`'s own module doc says an `extension.toml` declares one,
[ADR 0006](../../../.claude/decisions/0006-servicespec-in-proto-and-secret-free.md) was written with
that file among its three callers, and
[security-model.md](../../../.claude/architecture/security-model.md) carries a bullet marked *"not
built — arrives with T80"*. This task writes the file format those lines were written against, and
finds that two of them said something the types cannot do.

**Nothing is installed here.** T81 owns the registry, `--path`, install, uninstall, start and stop.
What T80 leaves behind is the vocabulary, the reading of it, and one read-only way to see the
result.

## Goal

A manifest that can be read, checked and rendered into the thing that would actually run — before
anything can be installed, so that T81 installs a file that has already been proved to make sense,
and T82 writes Mailpit and phpMyAdmin against a format that exists.

And the one claim the roadmap line makes about enforcement, made true: `network = "loopback"` is
what stops an extension reaching the LAN, **enforced rather than documented**.

## Scope

**In:** `mixengine-proto` (a new `extension` module: `ExtensionId`, `ExtensionKind`,
`ExtensionPermissions`, `NetworkReach`, `FilesystemReach`, `ApiAccess`, `ExtensionInspection` and
the per-kind summaries); `mixengine-core` (`extensions::manifest`, `extensions::render`);
`mixengine-daemon` (`extension.inspect`); `mixengine-cli` (`mix extension inspect`, one rendering);
`mixengine-testkit` (four manifest fixtures). Documentation:
[features/extensions.md](../../../.claude/features/extensions.md),
[architecture/security-model.md](../../../.claude/architecture/security-model.md),
[architecture/process-supervision.md](../../../.claude/architecture/process-supervision.md), and a
new ADR 0014.

**Out:** the `extensions` table, install, uninstall, start, stop, the registry, signatures, port
allocation, and any change to how a compiled-in recipe works. No `services` row is written and no
third `Origin` is added — T81 needs one and this design says so, but adding a column nothing writes
is the thing T79b's D5 spent a migration correcting.

## Decisions

### D1 — The manifest is its own type, and the spec is built rather than deserialised

[features/extensions.md](../../../.claude/features/extensions.md) says `[service]` *"deserialises
into the `ServiceSpec` vocabulary"*, and
[process-supervision.md](../../../.claude/architecture/process-supervision.md) says twice that a
spec arrives by `Deserialize` from an `extension.toml`. **It cannot.**

`ServiceSpec` has sixteen fields and the table in the feature doc has four. It has no `id` — the
name a service is known by everywhere is not something an author of a manifest is naming when they
write `program`. And every path and address in the file is a *template*: `{install_dir}/mailpit` is
not a `PathBuf` any check would accept, `{listen}:{ui_port}` is not a `SocketAddr` at all. A
`Deserialize` that succeeded on that file would be one that had stopped checking.

So `mixengine-core::extensions::manifest` holds its own types, which use the *vocabulary* —
`EnvValue`, `StopBehaviour`, `RestartPolicy`, `ReloadBehaviour`, `Millis`, `VersionConstraint` — and
carry a template where a rendered value belongs:

```rust
pub struct ServiceTemplate {
    program: String,
    cwd: String,
    args: Vec<String>,
    env: BTreeMap<String, EnvValue>,
    ready: ReadyTemplate,
    health: Option<HealthTemplate>,
    restart: RestartPolicy,
    stop: StopBehaviour,
    reload: Option<ReloadBehaviour>,
}
```

`ReadyTemplate` mirrors `ReadyCheck` variant for variant, with `addr`, `path` and `url` as `String`.
Rendering produces the real one.

This is T77's finding arriving a second time — *the manifest is its own type overlapping the struct
rather than sharing it* — and for the same reason: the two are read by different people at different
moments, and the moment a check can be reported against a line in a file is not the moment a
`Deserialize` impl runs.

The alternative was `#[serde(default)]` on the twelve missing fields. It was refused because those
fields have no default *on purpose*: a spec with no `stop` is a spec that has not decided how to
stop, and defaulting it for an extension defaults it for every compiled-in recipe at the same time.
It also solves nothing — `program: PathBuf` would still take `{install_dir}/mailpit` and
`validate` would report "not absolute" against a line the author wrote correctly.

Reuse is where reuse is real: `EnvValue` comes over whole, so ADR 0006's rule that a spec cannot
express a secret by value — including that writing `value` beside `from = "keyring"` is an error
rather than a quietly dropped field — costs this task nothing to obey.

### D2 — The manifest never writes an address

`{listen}` is a placeholder like `{install_dir}`, and **a literal host anywhere in the manifest is
refused at parse** — including `127.0.0.1`, which is the one an author would write in good faith.

```toml
args = ["--listen", "{listen}:{ui_port}", "--smtp", "{listen}:{smtp_port}"]
ready = { type = "tcp", addr = "{listen}:{ui_port}", timeout = "10s" }
```

`{listen}` renders from `permissions.network`, and from nothing else. An extension that declared
`loopback` has no way to spell any other address, because the address is not a thing it writes.

This is what makes the roadmap's sentence true. The alternative — a `network` column checked
wherever exposure could happen — is a rule somebody has to remember to consult at every future site
that could expose something, and T76 is the task that found out what one forgotten check of that
shape costs. A value that can only render one way needs consulting nowhere.

The refusal covers `args`, every `env` literal, `ready`/`health` (`addr`, `url`, `path`) — a
`ReadyCheck::Http` whose `url` names a host is the same escape by another door.

### D3 — `lan` renders `0.0.0.0`, not the machine's LAN address

The specific address is what a person wants to read, and it is the wrong thing to bind: it changes
when the network changes, so the spec would have to be re-rendered and the service restarted on
every DHCP renewal and every wake from sleep. T76 already paid for learning what watching that
signal costs, and it bought a *site* being unshared — not a supervised process being restarted
underneath whoever is using it.

`0.0.0.0` is also the honest reading of what the permission says: `lan` means *reachable from off
this machine*, and the interface it arrives on is not the extension's business. Which address a
person should type is a question about a URL, and the answer to that already exists in T74's
sharing.

### D4 — Every path grows from a placeholder, which is `own-data` enforced

`program` must start with `{install_dir}`. `cwd` must start with `{install_dir}` or `{data_dir}`.
A `ReadyCheck::UnixSocket` path must start with one of the two. Anything else — an absolute path, a
relative one, a `..` climbing out — is refused with the field named.

So `filesystem = ["own-data"]` is not a flag: it is the whole placeholder vocabulary. An extension
cannot reach a path it was not handed, because it cannot *write* one. A manifest that names
`C:\Users\dev\code` is refused before anything reads it, and one that names `{data_dir}/../..` is
refused for the same reason.

### D5 — `project-roots:read` grants nothing, and the document says so

The second `FilesystemReach` is declared, parsed and displayed, and **unlocks no placeholder**.
There is no `{project_root}`, and there could not be a useful one: an extension is per-home, not
per-project, and there are as many roots as there are projects.

It stays in the vocabulary because removing it would have to be argued about a use nobody has yet
described, and it is written down as a disclosure rather than left to look like a boundary. Adding
it a placeholder is a task with a consumer, not a field.

### D6 — `permissions.services` is a disclosure, not a boundary — and there is no token

The roadmap line and [security-model.md](../../../.claude/architecture/security-model.md) both
promise extensions "their own scoped token". **It would not be a control.**

An extension runs as the user's own account. The IPC endpoint's access control *is* the account —
owner-only socket permissions, both ends naming an account and checking the other. An extension
holding a scoped token can therefore ignore it and open its own connection, unauthenticated, and
reach everything `mix` can. A token in an extension's environment is readable by anything running as
that user, which is the same set.

Making it a boundary means requiring a token on *every* connection, `mix` included — a second
access-control story, which is exactly what the same document already refused for the TCP listener,
in the words *"a second transport and a second access-control story for a case nobody has yet"*. And
there is no case: not one extension in the plan (Mailpit, phpMyAdmin, Adminer, MixDB) calls the
daemon API.

So `[permissions] services` stays in the manifest as a **declaration shown before an extension is
installed** — the same shape as the `[scaffold]` consent T78a built, where a person is told what
they are about to allow — and it is described everywhere as that. `ApiAccess::{Read, Write}` is
parsed and rendered; nothing is minted, and nothing checks it.

ADR 0014 records this, and the security-model bullet is rewritten from a promise into the decision.
That document opens by saying a control described but not built is how a later reader concludes the
control exists; this task is that sentence being applied to the document's own bullet.

### D7 — Four kinds, one table each, and `[recipe]` beside any of them

`kind` decides which tables are legal. A table belonging to another kind is a parse error and not an
ignored key — a `[service]` under `kind = "desktop-app"` is somebody who thinks their extension will
be supervised.

| kind | its table | also legal |
|---|---|---|
| `service` | `[service]` | `[ports]`, `[artifact.*]`, `[recipe]` |
| `web-app` | `[web-app]` | `[artifact.*]`, `[recipe]` |
| `desktop-app` | `[desktop-app]` | `[artifact.*]` |
| `recipe` | `[recipe]` | — |

**`[recipe]` accompanies any kind**, because the feature document asks for exactly that without
noticing: its table calls `recipe` a "config-only addition", and T82 is *"Mailpit (with the
`sendmail_path` recipe for every managed PHP)"* — one extension that is both a supervised service
and a php.ini change. Two extensions for one product would be a second thing to install, start and
uninstall in step with the first. `kind = "recipe"` therefore means *only* that, and is not the only
way to have one.

- `[web-app]` — `root` (the docroot, under `{install_dir}`), `domain` (one label, placed under the
  internal domain by whoever generates the site), `runtime = { kind = "php", requires = "^8.1" }`
  using the existing `VersionConstraint`, and `template` (a file inside the extension rendered into
  the app's own configuration, so an upgrade does not clobber what a person changed).
- `[desktop-app]` — `scheme` (`"mixdb"`) and per-OS detection hints. T80 only declares them; finding
  an installed application and following a URL scheme are both platform-layer work, and are T83's.
- `[recipe]` — two forms, both with a consumer named in the roadmap: `php_ini` (key/value applied to
  every managed PHP, which is `sendmail_path`) and `front_end` (a directive fragment). No third form
  until something reads one.

### D8 — A `web-app` cannot ask for `lan`

[features/extensions.md](../../../.claude/features/extensions.md) says web-app extensions are
"never exposed to the LAN". That is a sentence today; here it is the parse refusing
`network = "lan"` under `kind = "web-app"`.

These are administrative interfaces onto the machine's own databases. The difference between them
and a site a person chose to share is that nobody chose.

### D9 — An extension may declare what it *is*, not policy about the machine

Settable from `[service]`: `program`, `cwd`, `args`, `env`, `ready`, `health`, `stop`, `reload`,
`restart`, and the ports it wants through `[ports]`. Everything else takes the builder's own
default — the same answer a compiled-in recipe gets when it says nothing.

**`stop` and `reload` may not be their `command` forms.** Both carry a `program: PathBuf`, so
allowing them means a second program to render and a second place to repeat D4's path rule, for a
capability none of Mailpit, phpMyAdmin, Adminer or MixDB needs. `signal` and `kill` are accepted;
`command` is refused, saying that a stop command is a second program and arrives with something that
needs one. What this leaves the render layer is two templates — `ReadyTemplate` and
`HealthProbeTemplate` — rather than four.

Not settable, each for its own reason:

- `limits` — T68's ceilings belong to the machine's owner. A manifest naming one is a program
  deciding how much of somebody else's machine it takes.
- `idle` — T69 stops a service only where something can start it again (T70). Nothing wakes an
  extension, so an idle policy would be a service that stops for good.
- `logs` — T16's policy is per-home.
- `depends_on` — an edge into a service graph the extension cannot see, and a name it would have to
  guess.

`id` is never in the file: the service is named by the extension. **And an extension id a
compiled-in recipe already claims is refused** — an id of `mariadb` would be two definitions of one
service, and core holds the recipe registry, so it can be said here rather than discovered at
install.

### D10 — `extension.inspect` renders, and calls a port a wish

The method takes an absolute path — the client resolves it against its own directory, as every path
in this API is resolved — to the directory holding `extension.toml`, and the file itself is accepted
too, because that is what a person types.

It does not stop at reporting the declaration. It builds **the context an install on this machine
would build** — `install_dir = <root>/extensions/<id>`, `data_dir` beneath it, the ports from
`[ports]`, `{listen}` from `network` — renders, and runs the result through
`ServiceSpec::builder(…).build()`. What comes back is what would run, which is
`blueprint apply --dry-run`'s position: a plan is worth having because it was computed, not because
it was described.

The answer says plainly that a port here is **what the extension asked for, not what it has** —
allocation is T81's, through the existing `Port::Allocate`. A line that reads like a reservation is
how somebody concludes a port is held.

For `desktop-app` and `recipe` there is nothing to render, and the answer is the declaration plus
the checks that passed. It does not invent a spec to have something to show.

A daemon method rather than parsing in `mix`, because reading a manifest is business logic and
`CLAUDE.md` puts none of it in a client — and because the render context needs `<root>`, which the
daemon owns.

### D11 — The PHP-extension names stay; one private module is renamed

`mixengine-proto` already exports `ExtensionList`, `ExtensionChoice`, `ExtensionSource` and
`RuntimeExtension`, and every one of them is about a **PHP** extension. None of the names this task
adds collides, and renaming four public types across proto, daemon, CLI and tests is a change with
no bearing on T80. Both module docs say which is which; that is the whole fix.

The one exception is a file name, where there is no choice: `mixengine-daemon/src/extensions.rs` is
`runtime.list_extensions` and `runtime.set_extension`, and two modules cannot share a name. It
becomes `php_extensions.rs`, which is what it has always been about — a private module, three lines
of rename — and `extensions.rs` is this task's.

`mixengine-core` needs nothing: its PHP extension code is already `runtimes::extensions`, so
`extensions` at the crate root is free.

### D12 — What T81 is handed

Written down here so it is not rediscovered: a `services` row today has `Origin::Package` or
`Origin::RuntimeInstall`, with a `CHECK` that exactly one is set. An installed `service` extension is
neither, so T81 adds the third — with the migration, in the task that writes rows.

## Delivery

1. `mixengine-proto::extension` — the vocabulary and `ExtensionInspection`, with the module doc that
   separates it from the PHP-extension types.
2. `mixengine-core::extensions::manifest` — parse and check, every refusal naming its field.
3. `mixengine-core::extensions::render` — the context, the substitution, and the build into a
   `ServiceSpec`.
4. `mixengine-daemon` — `extension.inspect`, and the new core errors classified in `ToWire` rather
   than falling into `_ => internal`, which is the defect T77a had to go back and fix.
5. `mixengine-cli` — `mix extension inspect <path>`, rendered as a block, plus `--json`.
6. Fixtures, tests.
7. Documentation and ADR 0014.

## Testing

- **Four fixtures, and they are the real manifests** — Mailpit, phpMyAdmin, MixDB and a
  `sendmail_path` recipe: the files T82 and T83 will ship. The format is tried against its actual
  consumers rather than against examples written to fit it.
- **A refusal table**: an absolute path; a literal `0.0.0.0`; a literal `127.0.0.1`; `lan` under
  `web-app`; an unknown placeholder; `value` beside `from = "keyring"`; `[service]` under
  `kind = "desktop-app"`; an id a recipe claims; `schema = 2`; a `..` in `cwd`; a host in a
  `ReadyCheck::Http` url.
- **The loopback proof reads the rendered output.** Every address in a rendered `loopback`
  manifest's `args`, `env` and `ready` is `127.0.0.1` — asserted against what came out, the way T77
  proved "never data, credentials or absolute paths" by reading the rendered TOML rather than by
  trusting the writer.
- **One CLI test end to end**: `mix extension inspect` against a fixture, through a real daemon.

## Risks

- **The `[web-app]`, `[desktop-app]` and `[recipe]` tables have no consumer yet.** Each is designed
  from a task that names what it needs — T82's phpMyAdmin and `sendmail_path`, T83's MixDB handoff —
  and each is kept to that. `schema` is the escape: a field added later is a schema a build can
  refuse to read, which is the versioning the registry section already asks for.
- **`0.0.0.0` will read as alarming** in `mix extension inspect` output for a `lan` extension. The
  rendering says what it means in words beside it.
- Nothing here is reachable by a person until T81 installs something. `extension.inspect` is the
  guard against that: the format is exercised, and by a command rather than only by a test.

## Acceptance

- `mix extension inspect` on the Mailpit fixture prints the program, arguments, working directory
  and readiness check that would run, with `127.0.0.1` in place of `{listen}` and the declared ports
  in place of `{ui_port}` and `{smtp_port}`, and says the ports are wishes.
- The same command on each of the other three fixtures reports the declaration and the checks that
  passed, and invents no spec.
- Every row of the refusal table is refused, naming the field.
- No manifest declaring `network = "loopback"` can be rendered into anything that mentions an
  address other than `127.0.0.1`.
- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check`, `cargo test --workspace` and
  the rustdoc build are clean.
