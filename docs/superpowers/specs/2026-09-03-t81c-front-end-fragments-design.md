# T81c — wiring `[recipe] front_end` fragments (design)

Roadmap task **T81c**, phase 8. T80 gave `[recipe]` two forms and T81 wired one of them: `php_ini`
becomes a generated `60-<id>.ini` in every managed PHP's `conf.d`, through
[`runtimes::extensions`](../../../crates/mixengine-core/src/runtimes/extensions.rs). The other form
was **refused by name** — T81's D10 — because both front-end templates would have had to grow an
`import`, each rendering would have had to be revalidated against the real server, and no extension
in T82 asks for one. The choice T81 faced was between wiring it for nobody and *accepting a fragment
that does nothing*, and it took the refusal, naming this task in the error.

This is that task. It removes the refusal by making the field take effect.

## Goal

A manifest that declares a front-end fragment gets that fragment into the configuration the front
end actually reads, judged by the front end's own binary before anything is installed, and swept
away when the extension is uninstalled — on the machinery `sites/` already uses, not beside it.

## What this does not buy, said first

**No extension in T82 needs one.** Mailpit's recipe is `sendmail_path`, which is `php_ini`;
phpMyAdmin and Adminer are `web-app`s and get a site of their own from T81b. After this task
`front_end` is a field that works and that nothing published uses.

It is worth doing anyway, and for one reason: T81 refused the field rather than ignoring it, and a
refusal that names a task is a debt. The alternative to paying it is deleting `front_end` from the
format, which is a decision about a format T80 argued from the products it describes — not a
decision to take because the first three products happened not to need it.

**And at the top level, a fragment cannot express very much.** Scope D3 says where a fragment lands,
and the honest consequence is written there: a Caddy fragment can define a snippet or a whole site
block, and a site block needs a hostname, which the manifest is forbidden to write. nginx's `http`
context is roomier — `map`, `upstream`, a `server` on a loopback port. Neither can reach *inside* the
site blocks MixEngine renders. Wiring that would be a second fragment kind rendered into
`site.caddy` and `site.conf`, doubling the surface that has to be revalidated for a need nobody has
stated; it stays out, and the day something asks for it, this design's D3 is where the argument
resumes.

## Scope

**In:** `mixengine-core` (`extensions::manifest` grows `server` on the fragment,
`extensions::install` loses its refusal, `generate::recipe` carries the additions and grows one trait
method, `generate::document` grows a judge-only path, `generate::recipes::caddy` and
`::nginx` render and sweep them, both templates grow one line); `mixengine-proto`
(`RecipeAddition::FrontEnd` gains `server`, `Error::ExtensionRecipeUnsupported` goes);
`mixengine-daemon` (the install refuses a fragment the front end refuses; a recipe-carrying
extension regenerates on install and on uninstall); `mixengine-cli` (`inspect` prints which server a
fragment is for); `mixengine-testkit` (a fifth fixture). Documentation:
[features/extensions.md](../../../.claude/features/extensions.md),
[features/services.md](../../../.claude/features/services.md) where it describes what a front end
renders, and the roadmap.

**Out:**

- **Fragments inside a site block.** Argued above and in D3.
- **A third front end.** `server` is an enum of the two this build ships. A third one is a recipe,
  a package and a variant, in the task that adds it.
- **Validating a fragment for a front end this home does not run.** D6 says what is judged, when,
  and what guarantees a home that took a fragment its next front end will refuse can still get out.
- **A `front_end` fragment that reaches the daemon API, a project, or another extension.** The
  placeholder vocabulary is T80's and gains nothing here.

## Decisions

### D1 — `server` is required, and it names a configuration language

`[[recipe.front_end]]` becomes:

```toml
[[recipe.front_end]]
server = "caddy"
fragment = """
(mailpit) {
	reverse_proxy {listen}:{ui_port}
}
"""

[[recipe.front_end]]
server = "nginx"
fragment = """
upstream mailpit {
	server {listen}:{ui_port};
}
"""
```

`server` is `caddy | nginx`, deserialised into an enum, and it is **required**: a fragment with no
server is a fragment that is a syntax error in one of the two places it could go, and there is no
answer to "which one did the author mean" that is not a guess.

The alternative shape — one `[[recipe.front_end]]` carrying optional `caddy = "…"` and
`nginx = "…"` — was weighed and refused. It reads better for an author writing both, and it makes
the third front end a change to every existing entry rather than a new variant. A list keyed by
`server` is the shape that grows.

**No manifest anywhere can be broken by making the field required**, and this is worth checking
rather than assuming: T81 refuses `recipe.front_end` at install, so no `extensions` row in any home
carries one, and the published `extensions.json` is empty. The `extensions` table stores the
manifest as canonical JSON and this build reads it back — a required field added to a shape nothing
has ever stored needs no migration.

### D2 — The fragment is rendered by the same substitution `php_ini` uses, and gains nothing

`{install_dir}`, `{data_dir}`, `{listen}` and each name in `[ports]`, through
[`extensions::render::value`](../../../crates/mixengine-core/src/extensions/render.rs). No new
placeholder: an extension still cannot write an address, and every path it can name still grows from
a directory it was handed. A fragment is rendered into the *front end's* file, and that is exactly
why it must not be able to name anything the extension could not name in its own `[service]`.

In particular there is **no placeholder for the front end's own paths**. A fragment cannot name
`etc/caddy/`, the certificate directory or a site's document root. What it gets is its own two
directories and its own ports.

### D3 — One file per extension, in a swept `extensions/` directory, imported at the top level

`etc/<front-end-service>/extensions/<id>.caddy` and `…/extensions/<id>.conf`, and the two templates
grow one line each:

- `Caddyfile`: `import extensions/*.caddy`, beside `import sites/*.caddy` and before `{{ extra }}`.
- `nginx.conf`: `include extensions/*.conf;` inside `http`, beside `include sites/*.conf;`.

`Recipe::swept` for both becomes `["sites", "extensions"]`, which is what makes an uninstalled
extension's fragment leave with it: the sweep already removes anything in a swept directory that the
recipe did not render, and that pass is what `sites/` has relied on since T43.

The import is at the top level (Caddy) and in `http` (nginx) because that is where the *set* of
fragments can be imported by one glob. Reaching inside a site block would mean one import per site
file, in a template rendered once per site, with every fragment's text judged in *n* places instead
of one.

**One file per extension is failure isolation for the sweep and for nothing else.** Both servers
judge a configuration whole: a fragment that does not parse fails the whole rendering exactly as a
broken site file would. Saying otherwise would be the more comfortable sentence and it would be
false; what actually keeps a bad fragment out is D5, and what keeps a home from being stuck with one
is D6.

### D4 — The configuration language decides how a path is spelled

`{install_dir}` on Windows renders `C:\Users\…\extensions\mailpit`. In an nginx configuration that
is not a path: `ngx_conf_read_token` treats `\` inside a quoted string as an escape, and unquoted the
directive stops at the first space — which is the trap `nginx.conf`'s own header comment already
carries seven lines about. In a Caddyfile a backslash is fine inside backticks and is an escape
inside double quotes, which is the trap `Caddyfile` carries its own comment about.

So a rendered fragment's paths are spelled the way its **server** spells one, not the way the
extension's `[service]` spells one:

- `server = "nginx"` → every substituted path is forward-slashed, which nginx accepts on all three
  systems. The quoting stays the author's.
- `server = "caddy"` → substituted as-is. Backticks are the author's to write.

This is why `server` is not merely a filter. A fragment written on macOS by an author who never saw
a backslash is a fragment that would silently mean something else on Windows, and the substitution
is the only place that knows both the path and the language it is about to land in.

The quoting cannot be done for the author — a fragment is arbitrary configuration and MixEngine does
not parse it — so an unquoted path with a space in it is a fragment the server refuses, at install,
by D5. That is the honest arrangement: the failure is early, named, and carries the server's own
complaint.

### D5 — A fragment is judged by the real server before anything is downloaded

`document::install` already stages a rendering into a fresh directory, runs the recipe's validator
over the staging directory, and installs nothing if the validator refuses. This task splits the first
half out as `document::judge(directory, documents, validator)`: stage, judge, remove the staging
directory, install nothing and create nothing.

`Generator::would_serve(&self, pending: &Installed) -> Result<()>` renders this home's front end with
`pending`'s fragments merged into the set the table already holds, and judges it. The daemon calls it
in `Extensions::perform` **before `install::install` runs at all** — before the download, before the
directory, before the rows.

That placement is deliberate and it corrects the obvious guess. A fragment's substitution does not
touch the filesystem, and neither `caddy validate` nor `nginx -t` opens a `root`, so nothing about
this check needs the artifact to be on disk. Checking first is T81's D2 applied one field further
along: *asking afterwards is asking after doing the thing somebody was about to refuse* — and a
fragment the front end will not accept is a refusal.

It also settles where the code goes. `install::install` lives in `mixengine-core` and holds a
`Store`, a `Paths` and a `Host`; a `Generator` additionally needs the recipe catalogue and this
system's port bindings, both of which the daemon has already assembled in
[`services::spec::generator`](../../../crates/mixengine-daemon/src/services/spec.rs). Threading a
generator into a core function to check one field would put the daemon's assembly into core; calling
the check where the generator already lives does not.

**What is judged is the fragment with the ports the manifest asked for, not the ports it will
hold.** Allocation happens inside `write_rows`, after this check, and the numbers can differ — an
extension asking for 8025 on a machine where 8025 is taken gets something else. A port number is a
token in both languages and cannot change whether a configuration parses, so the judgement carries
over; what does not carry over is the claim that the server judged the exact bytes that will be
installed. It judged the exact *shape*. Writing that down here is the point of this paragraph: the
next reader must not conclude more from the check than it proves.

A home with **no front end installed** has nothing to judge with and the check passes. The fragment
is then judged the first time a front end renders, which is when the front end is installed — and a
front end whose installation fails because of an extension's fragment is diagnosed by D6's escape
hatch, not by this check.

### D6 — Uninstall removes the row before it regenerates, and that is an invariant with a test

A fragment can be refused later even though it was accepted at install: the front-end package is
upgraded to a version that parses differently, or the home switches from Caddy to nginx and the
nginx fragment — never judged, because nginx was not here — turns out to be wrong. In that state the
front end will not regenerate, and every operation that regenerates fails with it.

The way out is `mix extension uninstall`, and it works because `uninstall::uninstall` deletes the
rows **first** and the daemon regenerates **after** — by the time anything renders, the fragment is
not in the table. That ordering is currently true by accident of how T81b wrote the site removal.
This task makes it a stated invariant and puts a test on it, because the refactor that swaps those
two lines would leave no failing test and would remove the only exit from a wedged home.

The alternatives were considered:

- **Judging every fragment against every front end this build knows.** Impossible without the other
  server's binary, which a home that does not run it does not have.
- **Skipping a fragment that fails to render.** That is `accepting a fragment that does nothing`,
  which is the failure T81 refused to live with and the reason this task exists.

### D7 — The additions travel on the `Context`, and one trait method renders them

`Generator::declarations` already reads every installed extension once per walk, for T81's reason —
an extension's recipe is built out of its row. The same read now also produces the front-end
additions, and they are placed on the `Context` of every row whose recipe reports
`Role::FrontEnd`, filtered to the fragments whose `server` matches that recipe.

`Recipe` grows:

```rust
fn fragments(&self, context: &Context) -> Result<Vec<Document>> { Ok(Vec::new()) }
```

defaulted to empty, implemented by `Caddy` and `Nginx`, and called from `Generator::documents`
beside the existing `sites` call under the same `Role::FrontEnd` condition. A second method rather
than more return values from `sites`: they are two questions — what this home serves, and what its
extensions added — and a recipe that answers one and not the other should not have to say so with an
empty vector.

### D8 — `supported` and `ExtensionRecipeUnsupported` are removed, not emptied

With `front_end` wired, `install::supported` has nothing left to refuse and
`Error::ExtensionRecipeUnsupported` has no constructor. A function that takes a manifest and always
returns `Ok` is a check that reads as if it were checking something, and a dead error variant is a
message the CLI can render and nothing can produce. Both go. The task that finds the next field this
build cannot honour writes them again, which is a smaller cost than either one left standing.

### D9 — A fifth testkit fixture, labelled for what it is

The four fixtures in `mixengine-testkit` are, in T80's words, *"the manifests T82 and T83 will
ship"* — a format proved against files invented for it proves only that it is self-consistent.
Adding a `front_end` to `sendmail.toml` or `mailpit.toml` would make that sentence false: neither
product needs one.

So the fixture for this task is a fifth file, and its doc comment says plainly that it is not a
product's manifest — it exists because T81c wires a field no shipping manifest uses yet. Labelling
it is what stops it being read later as evidence that something needed a fragment.

## Data flow

```
extensions table ──┐
                   ├─► Generator::declarations ─► front-end additions (rendered, per server)
row → Prepared ────┘                                        │
                                                            ▼
                                              Context (Role::FrontEnd rows only)
                                                            │
                       Recipe::fragments ◄───────────────────┘
                                │
                                ▼
                     extensions/<id>.{caddy,conf}  ──►  document::install
                                                          (staged, judged, swept)

extension.install ─► Generator::would_serve(pending) ─► document::judge ─► refuse or continue
                                                                              │
                                                        install::install ◄────┘
                                                                              │
                                                                    regenerate (front end)
```

## Errors

- `Error::ExtensionField` — already exists, already used by `php_ini`: a fragment whose placeholders
  cannot be rendered, naming `recipe.front_end[<n>]`.
- The manifest parse refuses a missing or unknown `server`, by serde, with the field named.
- `Error::ConfigRejected` — already exists, raised by `document::judge` through the validator, and it
  carries the server's own text. The daemon wraps it so the message names the extension: installing
  *this* is what was refused, and the reader needs to know which of the two documents was judged.
- `Error::ExtensionRecipeUnsupported` — removed, D8.

## Testing

**Manifest (`mixengine-core`, unit).** `server` missing is refused; an unknown `server` is refused;
a fragment naming a host is refused exactly as `[service]`'s arguments are; a manifest carrying both
servers parses to two entries in order.

**Rendering (`mixengine-core`, unit, no binary).** A fragment reaches
`extensions/<id>.caddy` for a Caddy front end and not for an nginx one; two extensions produce two
files; an extension that is not installed leaves no file, which is the sweep; a `{install_dir}` in an
nginx fragment comes out forward-slashed and in a Caddy fragment comes out as the system spells it.

**Judging (`mixengine-core`, unit).** `document::judge` installs nothing and creates nothing on
success, removes its staging directory on both paths, and returns `ConfigRejected` carrying the
validator's text on failure.

**The real servers (`mixengine-daemon`, `caddy.rs` and `nginx.rs`).** Install a `--path` extension of
`kind = "recipe"` carrying a valid fragment: the front end validates, reloads, and answers what the
fragment added. Then a fragment the server refuses: `extension.install` fails, `etc/` is byte-identical
to what it was, and `mix extension list` does not know the id. Then uninstall the valid one and
assert the file is gone from `extensions/` — which is D6's ordering, observed from outside.

**The escape hatch (`mixengine-daemon`).** An extension whose fragment the front end will not accept
can still be uninstalled. Constructed by installing a fragment that is valid, then making the
rendering refuse it — the cheapest lever being a second extension whose fragment collides with the
first, installed while the first is present — and asserting that uninstalling either one succeeds and
leaves a home that regenerates.

## Documentation

- [features/extensions.md](../../../.claude/features/extensions.md): the *"Not yet wired, and refused
  rather than ignored"* paragraph becomes what it now is, including what a fragment cannot express.
- [features/services.md](../../../.claude/features/services.md): the swept set of a front end is two
  directories, not one.
- The roadmap's T81c line, ticked, with what the task found.
