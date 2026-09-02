# T81 — the extension registry, and installing what it lists (design)

Roadmap task **T81**, phase 8. T80 wrote the format and one read-only way to look at it:
`extension.inspect` renders a manifest into the `ServiceSpec` that *would* run, and installs nothing.
This task is the other half — where a manifest comes from, what installing one writes, and how the
thing it installed is started, stopped and removed.

**What T80 handed over, verbatim from its own closing note**: a `services` row has `Origin::Package`
or `Origin::RuntimeInstall` with a `CHECK` that exactly one is set, and an installed `service`
extension is neither. The third origin arrives here, with the task that writes rows.

## Goal

An extension can be found, installed, run and removed, and every one of those steps can say what it
did on the strength of something checked rather than something claimed:

- the registry is one signed document, and a *listing* is what the compiled-in key vouched for;
- an artifact is what its SHA-256 said it was;
- an extension installed from a directory is marked unsigned everywhere it is ever printed;
- and an extension that is gone is gone from the supervisor, from `etc/`, from the port allocator
  and from the database, in that order.

## Scope

**In:** `mixengine-core` (`extensions::registry`, `extensions::install`, `extensions::store`, the
`index::Client` generic, the `ports` union, the `ExtensionRecipe`); `mixengine-proto`
(`extension_api`: list, search, install, uninstall, start, stop, and the consent type);
`mixengine-daemon` (those methods, and the install job); `mixengine-cli` (`mix extension <same>`);
`mixengine-core/migrations/0016_extensions.sql`; `mixengine-testkit` (a registry fixture and a
packaged fake extension). Documentation: [features/extensions.md](../../../.claude/features/extensions.md),
[architecture/security-model.md](../../../.claude/architecture/security-model.md), and the roadmap.

**Out:**

- **Publishing `extensions.json`.** T81 verifies; nothing yet produces the document it verifies,
  and nothing needs to until there is an extension worth publishing. That is **T81a**, on the
  T79→T79a shape, and this task's tests sign their fixture with a key the test itself makes
  ([`MockRegistry`](../../../crates/mixengine-testkit/src/registry.rs)) — which is what proves the
  verification path rather than switching it off.
- **`desktop-app` detection and handoff.** A `desktop-app` extension may be listed and its row
  written; *finding* an installed application is platform-layer work and stays T83's.
- **`[recipe] front_end` fragments** — refused at install, D10.
- The first real extensions, which are T82's.

## Decisions

### D1 — A second document, and the key that already exists

`extensions.json` is published beside `index.json`, under the same moved tag, with a `.minisig`
beside it, and it is signed with **`index::PUBLIC_KEY`** — no third key.

The precedent that might argue for a third one is the blueprint gallery, which took a key of its own
because its blast radius differed: one compromise of the index key costs the package index, one
compromise of a key that vouches for a `[scaffold]` costs the right to run arbitrary code on every
machine that took a blueprint in. **An extension has the package index's blast radius exactly** — a
binary downloaded and supervised — so a separate key would separate nothing. What it would add is a
third constant to rotate, a third Actions secret, and a third half-finished rotation to get wrong.

**Two documents rather than one array added to `index.json`**, and the reason is failure isolation
rather than tidiness. D4 requires an entry this build cannot read to be skipped; skipping inside a
document also holding every runtime means the code that skips has to be exactly right or
`mix runtime list` dies of an extension. Two documents make that structural: no reading of
`extensions.json` can fail the reading of `index.json`, because they are not the same read.

### D2 — An entry *is* a manifest

`extensions.json` is `{ schema, generated_at, extensions: [ …ExtensionManifest… ] }`, each entry the
T80 manifest as JSON. `ExtensionManifest` derives `Deserialize` and nothing about it is TOML-shaped,
so the registry reader is the manifest reader, and a `--path` install and a registry install are
proved by the same parse.

This falls out of the format T80 already wrote: `[artifact.<target>]` carries `url` and `sha256`, so
a manifest is already the entry a downloader needs. The alternative — an entry pointing at an
`<id>.toml` with its own hash — buys one thing (the author's own bytes stay the signed bytes) and
costs a second round trip, a second hash check, and two sources that can disagree about what an
extension asks for.

**The consequence that matters is the order of the install.** Because permissions arrive with the
listing, the question *"this wants to reach the LAN and read your project roots — go on?"* is asked
**before a single artifact byte is fetched**. Asking after the download is asking after doing the
thing somebody was about to refuse.

`ExtensionManifest` and everything under it gain `Serialize` alongside `Deserialize`, because the
row stores a canonical re-rendering (D5) and T81a will need to write the document.

### D3 — `index::Client` becomes generic over its document; the error family does not move

The client owns freshness, the cache, the rollback refusal and the verify-then-parse order, and all
four are wanted here unchanged. It becomes `Client<D>` over a small trait giving `schema` and
`generated_at`, with `Index` and `Registry` as the two implementors.

**The `Error::Index*` variants stay exactly as they are.** Renaming them to something document-
neutral would touch every call site and every test that asserts one, to rename a thing that is still
accurate: `extensions.json` is an index. Each variant already carries `url`, which is what says
*which* document failed; what becomes a parameter is the one fixed string in the log line
("package index" / "extension registry").

Copying the client instead was refused for the obvious reason — two copies of a verification path is
one copy that eventually skips a step — and the cache/rollback halves are precisely where such a
skip would be invisible.

### D4 — An entry this build cannot read is skipped, and *counted*

The array deserialises as `Vec<serde_json::Value>` and each element is then tried as an
`ExtensionManifest`. A failure is dropped from the listing and added to a count the listing carries,
so `mix extension list` ends with *"2 entries this build cannot read — update MixEngine"*.

Silence was the alternative, and it is worse than the failure it hides: an extension that vanishes
from a listing is one somebody goes looking for in the wrong place, and the answer they need is
"your MixEngine is older than this entry", which nothing else in the product can tell them.

The document's own `schema` is still checked first and still fails the whole read: a document shape
this build does not know is not a set of entries it can pick through.

### D5 — What the database holds

`0016_extensions.sql`:

```sql
CREATE TABLE extensions (
    id            TEXT PRIMARY KEY,          -- ExtensionId, and the directory name
    name          TEXT NOT NULL,
    version       TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('service', 'web-app', 'desktop-app', 'recipe')),
    manifest_json TEXT NOT NULL,             -- canonical, written by the reader
    install_dir   TEXT NOT NULL,
    data_dir      TEXT NOT NULL,
    source        TEXT NOT NULL CHECK (source IN ('registry', 'path')),
    signed        INTEGER NOT NULL CHECK (signed IN (0, 1)),
    installed_at  TEXT NOT NULL
) STRICT;

CREATE TABLE extension_ports (
    extension_id TEXT    NOT NULL REFERENCES extensions (id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,           -- the `[ports]` key, which is the placeholder
    port         INTEGER NOT NULL UNIQUE,
    PRIMARY KEY (extension_id, name)
) STRICT;
```

**`manifest_json` is a re-rendering and not the bytes that arrived**, which is T79's finding applied
here: a manifest kept as the author's text would make the file on disk, the column, and what the
renderer reads three texts for one extension. What is stored is what the reader produced, so what
runs tomorrow is what `inspect` showed today.

**The row is the source of truth for the spec, and the installed directory is not.** Nothing
re-reads `extension.toml` out of `install_dir` after the install: that file is inside a directory the
user can edit, and a manifest re-read from it would be a manifest nobody consented to.

### D6 — The third origin, and the migration that is the riskiest thing in this task

`services` gains `extension_id TEXT REFERENCES extensions (id) ON DELETE RESTRICT`, the `CHECK`
becomes "exactly one of three", and `UNIQUE (extension_id, instance_name)` joins the other two.

`ADD COLUMN` alone cannot do this: the existing `CHECK ((package_id IS NULL) <> (runtime_install_id
IS NULL))` *refuses* a row that names neither, so an extension service is unwritable until the
constraint moves, and SQLite cannot alter a `CHECK` in place. So 0016 rebuilds the table on
SQLite's twelve-step procedure.

**Two tables point at `services`, and dropping it with foreign keys enforced damages both
differently.** `sites.php_service_id` is `ON DELETE SET NULL`, so every site would quietly lose the
pool it names; `site_service_links.service_id` is `ON DELETE CASCADE`, so every "this site needs
that database" row would be **deleted outright**. The second is the worse one and the easier to miss,
because nothing about a site's own row would look wrong afterwards.

`PRAGMA foreign_keys` is a **no-op inside a transaction**, and sqlx wraps a migration in one by
default, so 0016 is a `-- no-transaction` migration — which in sqlx 0.9 means that string must be the
**first line of the file**, ahead of the explanatory comment every other migration here opens with.
It then: turns the pragma off, opens its own `BEGIN`, creates `services_new` with the three-way
`CHECK`, copies every row across, drops `services`, renames `services_new` into place, runs
`PRAGMA foreign_key_check`, commits, and turns the pragma back on. Losing sqlx's transaction is the
price of getting the pragma to apply at all, and it is bounded by migrations being the first thing
the daemon does with the database.

**0006 is not a precedent for this.** It rebuilt `sites` by `DROP`, and said why it was allowed to:
the three tables were empty on every machine in existence because no code path had ever written to
them. `services` is full on every developer's machine, so this one copies.

The test is not "the migration runs": it is a database seeded with a project, a site, a php-fpm pool
and a `site_service_links` row, migrated, and then asserted to still have both — the pool the site
named, and the link nothing would have reported losing.

**Why an extension service is a `services` row at all**, rather than a table of its own with a
supervisor beside it: `features/extensions.md` says an extension is *"managed by the same supervisor
as everything else"*, and everything that sentence is worth — `mix service list`, idle policy,
limits, log capture, activation, crash-loop windows — is keyed off that table. A parallel table
would re-earn each of those, one forgotten feature at a time.

### D7 — The spec comes from the manifest, through a recipe made at run time

[`Generator::prepare`](../../../crates/mixengine-core/src/generate.rs) looks a `Recipe` up by
`packages.name`. An extension has no compiled-in recipe and must not have one — a recipe is what
*this build* knows, and an extension is what a home installed.

So `Parent::of` grows a third arm, and a row whose parent is an extension is given an
`ExtensionRecipe` built from `manifest_json`: no settings, no template files, `spec()` delegating to
`extensions::render::service_spec`, which T80 already wrote and tested.

The one cost is `Recipe::package()` changing from `&'static str` to `&str`; the eight built-in
recipes return literals and do not change a character. The alternative — a second path through
`declared()` that skips recipes entirely — would be a second place where ports, idle policy, limits,
activation and crash-loop accounting have to be remembered, and the way that is discovered is an
extension quietly not honouring one of them.

### D8 — Every allocated port is visible to the allocator, in SQL

Mailpit asks for two ports and `services` has one `port` column. Keeping the second in a JSON blob
would hide it from
[`ports::allocate`](../../../crates/mixengine-core/src/services/ports.rs), which reads
`SELECT port FROM services`: a MariaDB created afterwards could be handed Mailpit's SMTP port, and
the failure would surface as a refused bind with nothing attached to it explaining why. That is the
exact hazard `allocate_activation` is annotated against — *every port any row holds is taken,
whichever column holds it* — so `extension_ports` is a table and `allocate` unions it.

**`services.port` for an extension is the port its `ready` check names** (`{listen}:{ui_port}` →
that port), and `NULL` when `ready` is not a port at all. A rule, not a guess, and it has its own
test: it is what makes `mix service list` show Mailpit's UI port and what gives an idle probe
something to probe.

### D9 — Consent, and where it sits

Install asks once, in `[scaffold]`'s shape: what it is, where it came from, whether it is signed, the
ports it will take, and `permissions` — with `services` printed as **a declaration and not a
boundary**, the wording ADR 0014 requires of every surface that prints it.

`--path` installs a directory: `source = 'path'`, `signed = 0`, and the unsigned marker appears at
install, in the listing's `TRUST` column and beside the id anywhere an extension is named — the
column T79b built for blueprints, doing the same job one table across.

There is no `mismatched` state here, and the reason differs from T79b's: the registry's signature
covers the whole document, so an entry either arrived inside something the key vouched for or the
document was refused entirely. `signed` is two-valued because the situation is.

### D10 — `php_ini` is wired; `front_end` is refused

`[recipe] php_ini` becomes one generated `.ini` in every installed PHP's `conf.d`, through
[`runtimes::extensions`](../../../crates/mixengine-core/src/runtimes/extensions.rs), whose `render`
already removes generated files that stopped being declared — so uninstalling an extension takes its
ini with it, on machinery that exists.

`[recipe] front_end` is **refused at install**, naming the field and saying it is not wired yet.
Both front-end templates would have to grow an `import`, each rendering would have to be revalidated
against the real server, and no extension in T82 asks for one. The choice was between wiring it for
nobody and *accepting a fragment that does nothing* — and a manifest silently not taking effect is
the failure mode this codebase spends whole designs avoiding. It stays parseable, so `inspect` still
shows it, and the refusal names the task that will connect it.

### D11 — Start and stop are the service lifecycle, wearing the extension's name

A `service` extension gets one row, `instance_name = 'default'`, so its `ServiceId` is
`<id>@default` — `mailpit@default`. An extension is a product somebody installed, not a server they
run several of, and `UNIQUE (extension_id, instance_name)` leaves room for the day one is.

`extension.start`/`extension.stop` resolve the extension to its service and delegate. They exist
because an extension is what a person installed and its `ServiceId` is an implementation detail of
that; they add no supervision of their own, and `mix service start mailpit@default` keeps working
because it is the same row.

### D12 — Uninstall unwinds in the reverse order, and does not delete data by default

Stop the service → remove the site and its generated config → delete the `services` row → delete
`extension_ports` → remove `install_dir` → delete the `extensions` row. The data directory is
**kept** unless the person says otherwise, which is what D13 had to move the layout for; the prompt says exactly what would be deleted and where
it is, so that "I meant to keep my captured mail" is not something learned afterwards.

### D13 — `{data_dir}` moves out of `{install_dir}`, because uninstall proves it has to

T80 built the two as `extensions/<id>/` and `extensions/<id>/data`, which reads naturally and cannot
survive D12: an uninstall that removes the install directory removes the data inside it, so *"the
data directory is kept unless the person says otherwise"* would be a promise the layout breaks. The
alternative — deleting everything under `install_dir` except one subdirectory — turns a plain
`remove_dir_all` into a rule somebody has to remember at every future site that removes an extension.

So `{data_dir}` becomes `data/extensions/<id>/`, beside the `data/<package>/<instance>` a service
already gets. Two directories with two lifetimes: one belongs to the version installed and goes with
it, the other belongs to the person and outlives every upgrade.

This changes what `extension.inspect` prints for `{data_dir}`, which is T80's answer being corrected
by the first task that had to act on it rather than describe it — the same way T79a found
`validated_slug` refusing every gallery file.

### D14 — A `web-app` runs on a runtime MixEngine picks

`[web-app.runtime].requires` is matched against installed runtimes, never against the project's
pinned version — an administrative interface that broke because somebody pinned an older PHP would
fail exactly when it is needed. If nothing installed satisfies the constraint, the install says which
runtime to install rather than installing one behind the person's back.

`network = "lan"` for this kind is already refused by T80's parse, so "never exposed to the LAN"
needs no check here.

## The install, in order

1. Resolve the entry — from the verified registry, or from a directory for `--path`.
2. Pick the artifact for this OS/arch; **no artifact is an answer**, not a failure to install.
3. Ask for consent (D9). Nothing has been fetched.
4. Download with resume, verify SHA-256, unpack into staging — [`install`](../../../crates/mixengine-core/src/install.rs) whole.
5. Allocate every `[ports]` entry, under the allocation lock (D8).
6. Render the `ServiceSpec` — a manifest that cannot render here fails now, with the staging
   directory removed, rather than at first start.
7. Rename staging into `extensions/<id>/`.
8. Write `extensions`, `extension_ports`, and the `services` row (or the site, for a `web-app`), in
   one transaction.
9. Regenerate config, so an installed extension is declared before anything is asked to run it.

Steps 4–7 are the runtime installer's ordering unchanged, for the invariant it exists to hold: a
half-installed extension must never appear in a listing.

## Testing

- **Registry**: a document signed by `MockRegistry`'s own key; a tampered one refused; a rolled-back
  one refused; an unreadable entry skipped *and counted*; a `schema` from the future failing the
  whole read.
- **Migration**: the seeded-database test in D6, plus a home with existing services surviving.
- **Ports**: an extension's second port is invisible to nothing — a service created afterwards does
  not receive it.
- **Spec**: a service extension installed from a fixture starts under the supervisor and its rendered
  spec matches what `inspect` answered before the install.
- **Uninstall**: leaves no row, no port, no directory, no generated ini — and leaves the data dir.
- **CLI end-to-end**: install → start → stop → uninstall against a packaged fake extension, on the
  shape [`declare.rs`](../../../crates/mixengine-testkit/src/declare.rs) uses for Caddy.

## Documentation

`features/extensions.md`'s Registry and Lifecycle sections become what was built; `security-model.md`
gains the unsigned-marker sentence; the roadmap ticks T81 and adds **T81a** (publish `extensions.json`)
and a follow-up for `front_end` fragments, each in its place in the order rather than at the end.

## Risks

The migration is the one place this task can lose something a person cannot get back. Everything else
fails forward: a bad signature refuses, a bad hash refuses, a manifest that will not render refuses
before the rename. The migration rewrites a table two other tables point at, one of them by cascade,
and it gives up sqlx's transaction to get its pragma applied — so it is written first, tested against
a seeded database, and reviewed on its own.
