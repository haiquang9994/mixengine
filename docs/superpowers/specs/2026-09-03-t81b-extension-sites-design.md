# T81b — the site a `web-app` extension is served on (design)

Roadmap task **T81b**, phase 8. T81 installs a `web-app` extension and then serves nothing: its
`write_rows` writes the `extensions` row and stops, because the one thing a `web-app` needs — a
site — cannot be written. `sites.project_id` is `NOT NULL`, and an administrative interface belongs
to no project. T81 held this back on purpose, so that one PR did not carry two table rebuilds; this
is the second rebuild, and what is built on it.

**What T81 handed over, from its own D14 and its closing note**: a `web-app` runs on a runtime
MixEngine picks inside `[web-app.runtime].requires`, never the project's pin; `network = "lan"` is
already refused for the kind at parse; and *"the site, for a `web-app`"* is a step in the install
order with nothing behind it yet. This task is that step.

## Goal

Installing phpMyAdmin gives a person `https://phpmyadmin.mixengine.test`, served by a PHP MixEngine
chose, with a certificate, listed where every other site is listed — and uninstalling it takes the
site away with everything else. Nothing about how a site is served, named, certified or resolved is
duplicated to get there: a `web-app`'s site is a row in `sites`, read by the same code that reads
every other row.

## Scope

**In:** `mixengine-core/migrations/0017_extension_sites.sql`; `mixengine-core` (`sites` grows an
owner, `generate::served` learns a second root, `extensions::install` and `extensions::uninstall`
write and remove the site, `extensions::manifest` checks the label); `mixengine-proto` (`SiteOwner`
on the wire, and three optional fields on the extension answers); `mixengine-daemon` (`Extensions`
held after `Sites` and holding it, the refusals on an extension-owned site, `runtime.uninstall`'s
third refusal); `mixengine-cli` (an `OWNER` column, a `SITE` column, and what `install` prints);
`mixengine-testkit` where a fixture is missing. Documentation: [architecture/data-model.md](../../../.claude/architecture/data-model.md),
[features/extensions.md](../../../.claude/features/extensions.md),
[features/client-surface.md](../../../.claude/features/client-surface.md), and the roadmap.

**Out:**

- **`[web-app].template`.** Rendering `config.inc.php` out of the extension's own template is what
  makes phpMyAdmin *reach* a database, and it is T82's, with the credentials question that comes
  with it. This task serves the directory; what the application finds in it is the next one's.
- **Linking the site to a database service.** `site_service_links` stays empty for an extension
  site. T82 decides whether phpMyAdmin declares MariaDB.
- **`[recipe] front_end` fragments** — T81c, still refused by name.
- **The archive's top-level directory.** `Installer` unpacks an artifact as it is, so phpMyAdmin's
  real zip lands at `{install_dir}/phpMyAdmin-5.2.1-all-languages/` and not at the `{install_dir}/app`
  the testkit fixture names. That is a fact about T82's manifest, which can name the directory the
  archive actually has; `doc_root_exists` on `site.show` is what reports it. Written into T82's
  roadmap line rather than solved here.

## Decisions

### D1 — A nullable parent and an `extension_id` beside it, not an internal project

`sites.project_id` becomes nullable and `sites.extension_id TEXT REFERENCES extensions (id)` joins
it, with `CHECK ((project_id IS NULL) <> (extension_id IS NULL))` — the exclusive-or 0001 spelled for
`services` before T81 made it a sum. A site has exactly one owner, and the owner is what gives its
`doc_root` a root.

Two alternatives were weighed and refused:

- **An internal `projects` row per extension.** No rebuild, and every reader of `projects` pays for
  it forever: `mix project list` shows a project nobody registered, `project.export` writes it,
  `blueprint.capture` captures it, and `project.delete` on it takes an extension's site down by
  cascade with nothing to say why. A column to hide it is a filter every listing has to remember.
  The roadmap already refused this on the second and fourth of those.
- **A table of its own.** `extension_sites` with its own domains is a second site model. `served`,
  `hosts::desired`, `Certificates::issue`, `domain.status` and `mix doctor` would each read two
  tables to answer one question, which is the "second door onto a table" `sites.rs`' module note
  exists to forbid.

**`ON DELETE CASCADE`, where `services.extension_id` is `RESTRICT`.** A service row is a process
that may be running, and removing its parent under it is a mistake to report. A site is declared
state: nothing runs out of it, and the front end is re-rendered from the rows whenever they change.
The cascade is also what makes two things right without code: `extension_store::forget` is the
rollback `write_rows` takes when a later step fails, and an uninstall interrupted after the site
row went and before the extension row did can be run again.

**One site per extension**, enforced: `CREATE UNIQUE INDEX sites_one_per_extension ON sites
(extension_id) WHERE extension_id IS NOT NULL`. A `web-app` declares one `[web-app]` table, so a
second site under one extension is a row nothing this build writes — and the partial unique index is
also the index the cascade walks, so it does the job `sites_project` does for the other parent.

### D2 — The fourth rebuild of `sites`, by copy, on 0016's pattern

SQLite cannot drop `NOT NULL`, so the table is rebuilt. `sites` is full on every developer's
machine, so 0017 copies rather than drops — 0016's reasoning, and 0006's is explicitly *not* a
precedent.

Two tables point at `sites` by cascade — `site_domains.site_id` and `site_service_links.site_id` —
and a `DROP TABLE sites` with foreign keys enforced deletes every row of both. So 0017 is the second
`-- no-transaction` migration: first line the sqlx marker, `PRAGMA foreign_keys = OFF`, its own
`BEGIN`, `sites_new` with the two parents and the CHECK, a copy of every column 0006 through 0013
gave the table, `DROP`, `RENAME`, `PRAGMA foreign_key_check`, `COMMIT`, pragma back on.

**What a drop takes with it and the file has to put back**: the index `sites_project`, and the two
triggers `sites_sharing_is_all_or_nothing_insert` / `_update` exactly as 0013 last wrote them —
SQLite drops a table's triggers with the table. The seeded test in Testing is what proves the
triggers are back: a home with a shared site survives the copy, and an update that sets an address
without an interface is still refused afterwards.

### D3 — `SiteOwner`, in core and on the wire, and `doc_root` keeps its meaning

`SiteRecord.project_id: i64` becomes `owner: SiteOwner`:

```rust
pub enum SiteOwner {
    Project(i64),
    Extension(ExtensionId),
}
```

`NewSite` takes the same. `records(store, Some(project))` keeps filtering by project — every
caller of it is asking a project's question — and `of_extension(store, &id) -> Option<SiteRecord>`
is the other door, one row by D1's index.

**`doc_root` is relative to the owner's root**, whichever owner. For a project that is `root_path`,
as today. For an extension it is `extensions.install_dir`, so phpMyAdmin's row holds `app` and its
absolute doc root is `<install_dir>/app` — the same `under(root, doc_root)` join `served` already
does, against a second map of roots. `generate` reads every installed extension once per walk
already (T81); `served` takes that map as an argument rather than reading the table again.

On the wire, `SiteSummary.project: String` becomes:

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SiteOwner {
    Project { name: String },
    Extension { id: ExtensionId },
}
```

as `SiteSummary.owner`. **A replacement and not an `Option` beside the old field**: two optionals a
reader has to agree about is the shape this codebase spends triggers avoiding, and the only client
that reads `project` ships with the daemon. `SiteDetail.root` becomes *the owner's root* — the
project's directory or the extension's install directory — which is what `doc_root_full` was always
joined onto. [features/client-surface.md](../../../.claude/features/client-surface.md)'s sites
entry says a listing carries an owner.

`SiteRef::Path` resolves through projects and only projects. A path inside an extension's install
directory is *"under no project"*, which is true, and the hint says to name the site by its domain.
Nothing walks `extensions/` looking for a match: an extension site has exactly one name and it is
printed by `mix extension list`.

`blueprints::plan::domain_step` compares `owner.project_id == project`; it compares
`owner == SiteOwner::Project(project)` instead and its meaning is unchanged. `projects::kept_warm`
is an inner join on `projects` and never sees an extension site, which is right: nothing keeps an
administrative interface's pool warm.

### D4 — The domain is `<label>.mixengine.test`, the label is checked at parse, and nothing is reserved

`[web-app].domain` is one label, and the site is `<label>.mixengine.<DEFAULT_TLD>` —
`phpmyadmin.mixengine.test`, as [features/extensions.md](../../../.claude/features/extensions.md)
has said since before T80. Composed through `domains::normalised`, so it is lowercased and checked
like a name a person typed.

**T80 said "one label" and did not check it.** `manifest::read` accepts `domain = "pma.tools"`
today, and this task would turn that into `pma.tools.mixengine.test`. The check moves to where T80
puts every other refusal: parse. A label containing `.`, or one `domain_syntax` would refuse as a
label, is `Error::ExtensionField` on `web-app.domain` before anything downstream sees it.

**The namespace is not reserved.** A person who creates `phpmyadmin.mixengine.test` as an ordinary
site before installing phpMyAdmin has done something odd and legal, and the unique index on
`site_domains.domain` is the one thing that decides who owns a name — `0001` says why a second
opinion is a bug. What this task adds is *when* the collision is reported: `extension.plan` asks
`sites::by_domain` and refuses with `Error::DomainTaken` naming the holder, so the person is told
before a byte of artifact is fetched rather than by a unique-index failure after it.

### D5 — The pool is resolved at plan, confirmed to exist, and frozen at install

`resolve::runtime` is asked with `explicit: Some(&requires)` and `cwd: None`. `asked_for` returns an
explicit constraint before it looks at any directory, so no manifest and no project pin is consulted
— which is T81's D14 stated as a call rather than a sentence. The newest installed version matching
the constraint wins, as it does for a shell.

**Then `pools::of` is asked whether the pool row exists**, rather than formatting
`php-fpm@<version>` and trusting it. `pools::ensure` is what creates a pool, after an install and at
boot; formatting the id is how the site would name a service the front end cannot find.

Nothing installed satisfying `requires` is `Error::RuntimeUnresolved` with `origin` reading *"the
phpmyadmin extension"*, and the hint is `resolve::install_command` — *"`mix runtime available --kind
php` lists what could satisfy ^8.1"*. The install says which runtime to install and installs none
behind anybody's back (D14 again).

**Frozen at install into `php_service_id`**, exactly as a project's site is at `site.create`. A
site that re-resolved on every render would be a second path deciding a pool beside the column that
holds one, and a configuration that changes because a PHP was installed for something else. What
moves is visible: `SitePool.resolved` for an extension site re-asks the same question — `explicit`
requires, no directory — so `mix site show phpmyadmin.mixengine.test` says *declared 8.3.34,
resolved 8.4.1* the day a newer PHP arrives. Acting on that is a reinstall, and is not this task's.

### D6 — What `site.*` may do to a site it does not own

Allowed: `site.start`, `site.stop`, `site.show`, `site.list`. Refused with `precondition_failed`:
`site.update` (every field, domains included), `site.delete`, `site.share`, and therefore
`domain.add` and `domain.remove`, which go through `replace_domains`. The sentence is one:

> `phpmyadmin.mixengine.test` belongs to the phpmyadmin extension — `mix extension uninstall
> phpmyadmin` removes it

**The guard is in the daemon's `Sites`, immediately after `expect`, and not in the CLI.**
`blueprint.apply` calls `sites.expect` on a domain and adds names to what it finds; `domain.add` does
the same. A refusal that lived in `mix` would be crossed by both.

`site.share` is where T80's D8 becomes true at the site: *"the difference between one of these and a
site somebody chose to share is that nobody chose"*. `site.unshare` on an extension site finds
nothing shared and answers as it does today. Start and stop stay allowed because a person who does
not want phpMyAdmin answering for a while has a `mix site stop` already, and inventing an
`extension.disable` beside it would be a second word for one flag.

### D7 — The install writes the site where it would have written the service, and then does what `site.create` does

In `write_rows`, after the `extensions` row and under the same rollback: a `web-app` writes a site
through `sites::create` with `owner: Extension(id)`, the domain from D4, the pool from D5, `doc_root`
as `[web-app].root` rendered through `render::rooted` and made relative to `install_dir`,
`https_enabled: true`, `state: enabled`, no linked services. A refusal — the domain went to
somebody between plan and install — forgets the extension row and the cascade takes nothing,
because the site was the row that failed.

`extension.plan` gains `site: Option<PlannedSite { domain, pool }>`, so the person sees the name
that will be taken and the PHP it will run on before consenting. **The consent itself does not
change.** `ExtensionConsent` covers what a person could refuse on principle — version, signature,
network reach; the site is derived from a manifest they already agreed to, and a consent that named
it would have to be renewed every time a newer PHP changed the pool.

**Then the three things `site.create` does after its row, in its order**: ask for the hosts file,
issue the certificate, regenerate and reload the front end. Today `Extensions` cannot do any of
them — it holds the store, the paths, the registry, the jobs and the host, and it does not call
`reconfigure()` at all. That was harmless for a `service`, whose `extension.start` walks
`service.start` and regenerates on the way; a site has nothing to walk, so an install that did not
regenerate would be a site in `mix site list` that the front end has never heard of.

`Sites` grows one `pub(crate)` method, `now_declares(&self, site: &SiteRecord)`, which is the three
calls `create` makes with the logging policy each already has (a hosts want and a certificate refusal
are logged and never fail the call; a configuration the server refused fails it). `Extensions`
holds `Arc<Sites>` — "held rather than reached for", the reason `Domains` already holds it.

**Which changes where `Extensions` is built.** `main.rs` builds it before `Api::new` builds `Sites`,
for one reason: `registry::client` can fail on a public key that is not one, and that should fail
the start rather than the first install. The fail-fast piece is the *client*, so the client is what
`main.rs` builds — the `Fetcher`'s shape exactly, built beside it — and `Extensions::new` moves into
`Api::new`, after `Sites`, taking the client and the `Arc<Sites>`. The test constructor in
`api/rpc.rs` follows.

### D8 — Uninstall removes the site first, and says which name it released

`uninstall::uninstall` deletes the site through `sites::of_extension` before `forget`, so the log
and the answer can name what went; the cascade would have removed it anyway, and that is what makes
a re-run after an interruption succeed. `Removed.site: Option<String>` carries the primary domain up,
`ExtensionRemoval.site` puts it on the wire, and `mix extension uninstall` prints *"released
phpmyadmin.mixengine.test"* beside the data directory it kept.

The daemon then asks for the hosts file and regenerates, as `site.delete` does. The certificate is
left on disk as `site.delete` leaves it: T50's leaves are named by domain and reissued on demand, and
sweeping them is `mix cert` territory.

### D9 — `runtime.uninstall` refuses for an extension site the way it refuses for a pin

Today the second check deletes a *stopped* pool along with its runtime, and `sites.php_service_id`
is `ON DELETE SET NULL` — so phpMyAdmin would silently stop being served the day somebody removed
the PHP it was frozen on, and `mix doctor` would be the first to mention it. A project pin gets a
refusal without `--force`; an extension site frozen on this exact version joins that refusal, in the
same sentence:

> removing php 8.3.34 would leave nothing for blog (^8.3), phpmyadmin (extension)

`sites::frozen_on(store, &pool) -> Vec<ExtensionId>` is the query — extension-owned sites whose
`php_service_id` is that pool. After `--force` the site loses its pool and `mix doctor` reports it,
as it does for any php-fpm site whose pool was deleted — T43's D5, unchanged.

### D10 — `extension.start` and `extension.stop` stay refused for a `web-app`, and say what to type instead

Both answer a `ServiceWalk`, and a site has no walk. Mapping them onto `site.start`/`site.stop`
would change the answer type for one kind and leave a client reading two shapes out of one method.
The refusal stays `precondition_failed`; what changes is the hint, from *"runs no process"* to
*"is served as a site — `mix site stop phpmyadmin.mixengine.test`"*.

`ExtensionSummary.site: Option<String>` carries the domain, so `mix extension list` has a `SITE`
column beside `SERVICE`, and a person can find the name without opening `mix site list`.

## The install, in order

T81's order, with step 8 filled in and two steps after it:

1. Resolve the entry — registry, or a directory for `--path`.
2. Pick the artifact for this OS/arch.
3. Plan: refuse what cannot be honoured — and for a `web-app`, resolve the pool (D5) and check the
   domain (D4), so both refusals arrive before a byte is fetched.
4. Ask for consent. Unchanged.
5. Download, verify, unpack into staging.
6. Allocate every `[ports]` entry.
7. Render the `ServiceSpec`, or for a `web-app` the doc root.
8. Rename staging into `extensions/<id>/`.
9. Write `extensions`, `extension_ports`, and **the `services` row or the `sites` row**, under one
   rollback.
10. **Hosts, certificate, regenerate** — `Sites::now_declares` (D7). New.
11. Answer with the summary, which now names the site.

## Testing

- **Migration**: a database seeded on 0016 with a project site, its domains, its links, a shared
  site and a php-fpm site naming a pool survives 0017 with every row, and an update setting
  `shared_address` without `shared_interface` is still refused — the triggers are back.
- **Core, `sites`**: `create` with `owner: Extension` writes `extension_id` and NULL `project_id`; a
  second site for the same extension is refused by the index; `of_extension` reads it back;
  `records(Some(project))` never returns it; deleting the `extensions` row cascades it.
- **Core, `served`**: an extension site renders its doc root under `install_dir`, forward-slashed
  row joined by component, on every OS.
- **Core, `install` / `uninstall`**: the `phpmyadmin` fixture installed from a directory against a
  seeded PHP row and its pool — site row, domain, pool, `doc_root = "app"`; plan refuses without a
  matching PHP naming the install command, and refuses a taken domain naming its holder; uninstall
  leaves no site and no domain row, keeps the data directory, and reports the released name.
- **Core, `manifest`**: `domain = "pma.tools"` and `domain = "-x"` refused at parse.
- **Daemon**: `site.update`, `site.delete`, `site.share`, `domain.add` on the extension site answer
  `precondition_failed` with the D6 sentence; `site.stop` and `site.start` work; `site.show` reports
  the owner and the install directory as `root`; `SiteRef::Path` inside `extensions/` is
  `not_found` with the domain hint; `runtime.uninstall` of the frozen PHP refuses naming the
  extension and succeeds with `--force`.
- **CLI end-to-end**: a home with a seeded PHP row (the pool arrives from `pools::ensure` at boot),
  `mix extension install --path` of a `web-app` fixture, `mix site list` showing `extension
  phpmyadmin` in `OWNER`, `mix extension list` showing the domain, `mix extension uninstall`
  releasing it.

## Documentation

`data-model.md`'s `sites` line gains the second parent and the CHECK; `features/extensions.md`'s
web-app section and Lifecycle become what was built and lose the T81b pointer;
`client-surface.md`'s sites entry says a listing carries an owner; the roadmap ticks T81b and T82's
line gains the archive-layout note from Scope.

## Risks

- **The migration**, for 0016's reason and one more: this one rebuilds a table with triggers on it,
  and a trigger that is not recreated fails silently — a home whose shared-site invariant is no
  longer enforced looks identical to one whose invariant is. The seeded test asserts the refusal
  after the copy, not the row count.
- **A wire field is replaced rather than added.** `SiteSummary.project` is gone. The only client is
  `mix`, shipped with the daemon; a graphical client does not exist yet and `client-surface.md` is
  where it will read the shape from.
- **Construction order in the daemon moves.** `Extensions` is built in three places
  (`main.rs`, `api/mod.rs`, `api/rpc.rs`' test constructor); a missed one is a compile error, not a
  runtime one, which is the right kind.

## Acceptance

- `mix extension install phpmyadmin --path <fixture>` on a home with a matching PHP prints the
  domain and the pool before asking, and afterwards `mix site list` shows the site owned by the
  extension, enabled, HTTPS on.
- The same install on a home with no PHP satisfying `^8.1` refuses before downloading, naming the
  runtime to install.
- `mix site share phpmyadmin.mixengine.test` refuses with the D6 sentence.
- `mix runtime uninstall php <version>` refuses naming `phpmyadmin (extension)` and succeeds with
  `--force`.
- `mix extension uninstall phpmyadmin` removes the site, prints the released domain, and keeps the
  data directory.
- Every existing site on a developer's home survives the migration with its sharing, its pool and
  its links.
