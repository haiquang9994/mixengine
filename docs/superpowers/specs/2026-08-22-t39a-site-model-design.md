# T39a — The site model, and the domain that can only belong to one site

*Design, 2026-08-22. Roadmap task [T39a](../../../.claude/roadmap/phase-4-sites-and-elevation.md), Phase 4.*

## What this closes

T39 registered directories and stopped there. A project today is a name, a root and a map of
constraints; nothing in this build can say *what is served out of it*, so `sites`, `site_domains`
and `site_service_links` have stood in `0001_initial.sql` since the initial migration with no writer
at all. T39a gives them one.

It also closes the sentence T39 left in `core::manifest`: "`[site]` and `[[services]]` survive an
export without this type having to hold them **until T39a gives them meaning**". Those two sections
are preserved byte for byte today *precisely because* nothing types them. Typing them is this task,
and it forces the second half of the question — whether `project.export` writes them — which is D9.

T43 renders what this declares. Nothing here writes a config file, starts a process, or touches the
hosts file.

## What already exists, and is reused unchanged

- The three tables, their five indexes and both cascades, in `0001_initial.sql`. No new table.
- `ProjectRef` and `core::projects::find` — a site is reached through its project's ancestor walk
  (D5), which is the walk T39 already built.
- `mixengine_platform::paths::in_full` — and the rule T39/D5 paid for: **both** sides of a path
  comparison are normalised, or a `/tmp` project and a `/private/tmp` doc root disagree on macOS.
- `core::resolve` — the four-step order answers what php-fpm pool a new site gets by default (D3).
- `ServiceId`, already validated on construction and on deserialisation, already the shape
  `name@instance`. A site's pool and a site's links are both `ServiceId`s; nothing new is invented to
  name a service.
- `VersionConstraint::parse` / `matches` — a `version` in `[[services]]` is exactly a constraint,
  matched exactly as a runtime pin is (D8).
- T39's `RuntimeUninstall { #[serde(flatten)] target, force }`, copied shape for shape into
  `ServiceDelete` (D4).
- The `toml` / `toml_edit` pair `core::manifest` already reads and edits with.

## Decisions

### D1 — No new table; one migration, and it only closes `state`

`sites`, `site_domains` and `site_service_links` are declared, indexed and commented already. The one
thing missing is a vocabulary for `sites.state`, deliberately left open — `0001_initial.sql` says why:

> `sites.state` is still not [CHECKed], because its state machine belongs to a later phase and does
> not exist yet; a CHECK written before the vocabulary does is guesswork, and SQLite has no way to
> drop a constraint short of rebuilding the table.

This is that phase, and the vocabulary is two words. A site is `enabled` or `disabled` — whether the
web server should have a server block for it — and nothing more. `starting`, `running` and `failed`
belong to the *processes* a site uses, which are `services` rows with their own seven states; giving
a site a lifecycle of its own would mean two answers to "is blog.test up".

`0006_site_state.sql` therefore rebuilds the three tables together, closing `state` as
`CHECK (state IN ('enabled', 'disabled')) DEFAULT 'enabled'`. SQLite cannot add a constraint in
place, and the two children hold foreign keys into the parent, so they are dropped in child-first
order and recreated verbatim.

**The rebuild drops rather than copies, and that is a statement of fact rather than a shortcut:** no
shipped code path has ever written a row into any of the three tables — there are no `site.*` methods
before this task — so the only writer in the workspace is a test fixture, and every test builds its
database from the migrations. A copy-out and copy-back over a table that is empty on every machine
would be ceremony that still has to be read by whoever audits the migration.

**A new file rather than an edit to `0001_initial.sql`.** T14 edited that file in place, for a reason
it wrote down: "nothing has shipped, so the forward-only rule has nothing yet to protect." Five
migrations later that is no longer true — every developer database has already run `0001`, sqlx
records its checksum, and changing a byte of it turns the next `cargo test` on those machines into a
migration failure with nothing to say for itself.

**The guard is written before the rebuild, not after.** `crates/mixengine-core/tests/store.rs` is the
only thing standing over these tables, and today it checks one cascade. It is thickened first — every
index by name, both cascades (`projects` → `sites` → `site_domains`, and `services` →
`site_service_links`), and both refusals the unique indexes exist for: one domain claimed by two
sites, and two primaries on one site. Then the tables are rebuilt. A hand-copied `CREATE TABLE` that
silently loses a cascade is the failure mode here, and it is invisible for months; the thickened test
failing on `state = 'stopped'` is the proof that the guard is real before it is relied on.

`http_port` and `https_port` keep their 80/443 defaults and stay off the API — per-site ports are
Phase 5/7, and an API field nothing can vary is a promise. `https_enabled` *is* on the API, as a
declaration T43 and Phase 5 read; it changes nothing today and the field's documentation says so.

### D2 — `doc_root` is stored relative to the project root

A row holding `/home/ana/blog/public` and a project holding `/home/ana/blog` say the same prefix
twice, and disagree the moment the project moves — which `project.update { root }` exists to allow.
So `sites.doc_root` holds `public`, and `""` means the root itself.

The argument may be typed either way. An absolute path is made relative against the project's root,
with `in_full` applied to **both** sides before the comparison; a relative one is taken as given. A
doc root that resolves outside the root is `invalid_argument` — it would be a site belonging to a
project whose files are somewhere else, which no renderer can express.

**A doc root that does not exist on disk is accepted and reported, not refused**, on T39/D6's
reasoning: a colleague clones a repository whose `public/` is built by `npm run build`, and a create
that refuses that manifest refuses the exact case the import path was written for. `SiteDetail`
carries `doc_root_exists`, so the answer is visible rather than assumed.

### D3 — The kind carries its own payload, and php-fpm's pool is an `Option` because the database can lose it

```rust
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SiteKind {
    PhpFpm { pool: Option<ServiceId> },
    Static,
    ReverseProxy { upstream: String },
    NodeApp { port: u16 },
}
```

A tagged enum rather than four nullable columns on the wire: a static site with an `upstream`, or a
reverse proxy with a pool, are then unspellable rather than merely undocumented.

**`pool` is an `Option`, and the reason is in the schema.** `sites.php_service_id` is
`ON DELETE SET NULL`. A `service.delete` today therefore produces a php-fpm site with no pool, and a
type that cannot say `None` would be a type that lies about a state the database holds. It carries
one meaning in both directions — *this site names no pool* — and the difference is only who acts on
it: on a create the daemon fills it from `core::resolve` at the project root (pools are named
`php-fpm@<full version>`, so `^8.3` becomes `php-fpm@8.3.34`), and in an answer it means the pool it
named is gone.

The row cannot remember who chose the pool, so nothing pretends it can. What `SiteDetail` reports
instead is both answers side by side: `declared` — what the row holds — and `resolved` — what
`core::resolve` answers at that root today. The pool is frozen at create while the project's shell
keeps following the default, so 8.3.35 arriving tomorrow moves the shell and not the site, and those
two fields are how a person sees that rather than guesses at it.

`upstream` is validated as an absolute URL with scheme `http` or `https` and a host; a path is
allowed, a query or fragment is not — a proxy target is an address, and a `?` in one is a typo the
renderer would carry into a config file.

**`node-app` is kept, and stated plainly as a declaration.** Nothing in this roadmap supervises
`npm run dev` — the string `node-app` occurs in exactly one place in the whole document tree, on the
T39a line — so the kind records "a node process the user runs, on this port" and no more. What
distinguishes it from `reverse-proxy` today is the scope of the address (a loopback port MixEngine
intends to own later, versus an arbitrary URL), not a rendering mechanism. If T43 renders the two
identically, that gets recorded there rather than discovered. The gap is written into the phase file
as a known one.

### D4 — `service.delete` earns the fourth refusal, and `--force` crosses a declaration and never a process

T39a is what creates this debt, so T39a pays it. `service.delete` refuses a service any site names —
as its php-fpm pool, or as a link — and names the sites:

```rust
pub struct ServiceDelete {
    #[serde(flatten)]
    pub target: ServiceQuery,
    #[serde(default)]
    pub force: bool,
}
```

Flattened for T39's reason: `ServiceQuery` is also `service.status`'s parameter, where a `force`
would mean nothing, and the flatten keeps today's wire shape while adding one optional key an older
client's request still parses without.

The boundary is T39/D8's line, unchanged. **A site declaring a service is a statement about the next
`site.start` — refusable, and `--force` crosses it.** A running process is a fact about now: the
existing refusal for a live or supervised service stands, and no flag buys it. After a forced delete
the pool is `NULL` and `SiteDetail` says so out loud, which is the whole reason `pool` is an
`Option` in D3.

### D5 — A site is addressed by domain or by path

`sites` has no name column, and adding one would invent a second identifier for a thing that already
has a globally unique one: `site_domains_domain`.

```rust
pub enum SiteRef {
    /// Any of the site's domains — the primary or an alias; the index makes them unambiguous.
    Domain(String),
    /// Any directory at or inside the project's root.
    Path(String),
}
```

`Path` walks up through `core::projects::find`, exactly as `ProjectRef::Path` does, and then takes
that project's site. Where a project has more than one, it is `invalid_argument` naming them, because
picking one would be picking at random. The CLI sends `Path(cwd)` when nothing is typed, so
`mix site show` three directories deep answers.

`site.list` takes `Option<ProjectRef>`: absent lists every site in the home, present narrows to one
project's.

### D6 — Domains travel as an ordered list, and the head is the primary

`site_domains.is_primary` is a storage detail, kept off the wire. On the API a site has
`Vec<String>`, ordered, and the first element is the primary. A list with no primary, or with two, is
then unspellable rather than checked.

`site.update { domains }` **replaces** the list — T39/D6's rule for pins, for the same reason: with a
merge there is no way to remove one. Inside the transaction the deletes run before the inserts, so
moving `api.blog.test` from one site to another is expressible in two calls rather than blocked by the
unique index halfway through.

A site, its primary, its aliases and its links are written in one transaction. `0001_initial.sql`
already assigns that: "at least one [primary] is not expressible here … and it stays an invariant the
site module upholds inside the transaction that creates a site." This is that module.

### D7 — One reader for `[site]`, and it is the same type the wire uses

`#[serde(tag = "kind")]` is *internally* tagged, and its representation in TOML is a flat table:
`kind = "reverse-proxy"` sitting beside `upstream = "…"`. So `SiteKind` reads a manifest and a
JSON-RPC request with no conversion function in between and no second definition to drift.

```toml
[site]
domain = "blog.test"
aliases = ["api.blog.test"]
doc_root = "public"
kind = "php-fpm"
https = true
```

`kind` must be *optional* — `[site] domain = "blog.test"` alone is a manifest this build already has
a test for. It is therefore read out of the flattened remainder of `[site]`: present, and the
remainder is deserialised into `SiteKind`, so a `reverse-proxy` with no `upstream` is refused by the
enum's own definition; absent, and it is `None`, which falls through to php-fpm with the resolved
pool. Unknown keys in `[site]` stay unknown — the file also has to hold what T43 and Phase 8 will
add.

`SiteCreate` falls through the way `ProjectCreate` does, argument then manifest then default:
`domains` → `[site] domain` + `aliases` → `<slug>.test`; `doc_root` → `[site] doc_root` → `""`;
`kind` → `[site]` → php-fpm; `services` → `[[services]]` → none. One method, one code path, and
"import a colleague's site" is `site.create { project }` with nothing else typed.

### D8 — `[[services]]` names an instance; a version mismatch is reported, not refused

```toml
[[services]]
name = "mariadb"
instance = "main"     # optional; "main" is the default, and `mariadb@main` is the id
version = "11.4"
database = "blog"     # not this task's; read by nobody, preserved by the writer
```

`name` plus `instance` is a `ServiceId`, which is the only thing a link row holds. `version`, when
present, is a `VersionConstraint` — its *syntax* is refused, and its *satisfiability* is reported,
which is T39/D6's split applied one table over. Refusing an import because the machine has MariaDB
11.5 would break the clean-machine case the import path exists to serve.

`database` and `user` are not typed here. This build creates no databases, and a key that is read and
then quietly ignored is a promise not kept — so they pass through the reader untouched and survive
the writer untouched (D9), and provisioning stays Phase 8's `blueprint.apply`. Unknown keys in
`[[services]]` are allowed for the same reason they are allowed in `[site]`.

`site.create` does not create the services it links. A named instance that does not exist is
`not_found` carrying the `mix service create …` line, exactly as an uninstalled runtime carries its
install command.

### D9 — `project.export` writes both sections, adds and updates, and never deletes

Export exists so a colleague can clone and get the project. A file with the runtimes and not the site
loses the thing worth sending, so `[site]` and `[[services]]` are written now that they have types.

`manifest::write` therefore stops taking a name and a map and starts taking what an export *is*:

```rust
pub struct Export {
    pub name: String,
    pub pins: BTreeMap<RuntimeKind, VersionConstraint>,
    /// The project's site, when it has exactly one — see the second rule below.
    pub site: Option<ExportSite>,
}

pub struct ExportSite {
    pub domain: String,          // the primary
    pub aliases: Vec<String>,
    pub doc_root: String,        // relative, as stored
    pub https: bool,
    pub kind: SiteKind,
    pub services: Vec<ExportService>,
}

pub struct ExportService {
    pub name: String,
    pub instance: String,
    pub version: PackageVersion, // the instance's, so a colleague knows what to install
}
```

Three rules, and the first is D10 of T39 one level deeper:

- **Add and update; never delete.** `[[services]]` is an array of tables, so it is merged by identity
  (`name` + `instance`): an existing entry has its `version` updated, a missing one is appended, and
  an entry the daemon knows nothing about is left exactly as it is. A hand-written
  `database = "blog"` must survive an export. The honest consequence, stated rather than discovered:
  **export is a merge, not a mirror** — removing a link in the database does not remove its line from
  the file, and there is no `--prune`.
- **A manifest holds one `[site]`.** Exactly one site is written. More than one and none is, with the
  omitted names carried back in `ProjectExport` — a limit of the file format, not of the model.
- The kind's keys are written from an exhaustive `match` on `SiteKind`, which the compiler refuses to
  leave unhandled when a fifth kind arrives. `doc_root` is written relative, as stored. Domains are
  written as `domain` plus `aliases` — one key more than the example in
  [data-model.md](../../../.claude/architecture/data-model.md), which is updated in the same change
  rather than left to disagree.

### D10 — `core::domains` holds the whole policy, and the default name is refused rather than invented

One module owns what a domain may be: normalised to lowercase; ASCII labels of `[a-z0-9-]` neither
starting nor ending in `-`; each label at most 63 bytes and the whole name at most 253; at least two
labels. No `*` — a wildcard is what T44 answers by pattern, not a row in a table. No IDN: punycode is
recorded as unsupported rather than half-handled.

The TLD table is [domains-and-dns.md](../../../.claude/features/domains-and-dns.md)'s, unchanged:
`.test` is the default, `.localhost` is accepted, `.local` needs the explicit acknowledgement that
doc already names `--i-know` on the CLI (`accept_risky_tld` on the wire), and every public TLD is
refused with `.test` in the hint. The managed set is compiled in; if it ever belongs in
`config.toml`, T44 or T46 is where that is decided, and this spec is not the place to guess.

The default domain is `<slug>.test`, slugged from the project's name. **A slug that collides is
refused, naming the site that holds it** — appending `-2` would give somebody a domain they never
typed and would not remember, and `mix site create --domain …` is one flag away.

## API surface

New `crates/mixengine-proto/src/site_api.rs`, holding `SiteRef` (D5) and `SiteKind` (D3):

```rust
pub struct SiteCreate {
    pub project: ProjectRef,
    /// Ordered; the head is the primary. Falls through to `[site]`, then to `<slug>.test`.
    pub domains: Option<Vec<String>>,
    /// Absolute or relative to the root. Falls through to `[site]`, then to the root itself.
    pub doc_root: Option<String>,
    pub kind: Option<SiteKind>,
    pub services: Option<Vec<ServiceId>>,
    pub https: Option<bool>,
    /// `.local`, acknowledged. `--i-know` on the CLI.
    pub accept_risky_tld: bool,
}

pub struct SiteUpdate {
    pub site: SiteRef,
    pub domains: Option<Vec<String>>,      // replaces; see D6
    pub doc_root: Option<String>,
    pub kind: Option<SiteKind>,
    pub services: Option<Vec<ServiceId>>,  // replaces
    pub https: Option<bool>,
    pub state: Option<SiteState>,
    pub accept_risky_tld: bool,
}

pub enum SiteState { Enabled, Disabled }

pub struct SiteQuery { pub site: SiteRef }
pub struct SiteListQuery { pub project: Option<ProjectRef> }
pub struct SiteList { pub sites: Vec<SiteSummary> }

pub struct SiteSummary {
    pub domain: String,        // the primary
    pub project: String,       // the project's name, which is its wire handle
    pub kind: SiteKind,
    pub doc_root: String,      // relative, as stored
    pub https: bool,
    pub state: SiteState,
}

pub struct SiteDetail {
    pub site: SiteSummary,
    pub root: String,               // the project's root
    pub doc_root_full: String,      // root + doc_root, as the filesystem spells it
    pub doc_root_exists: bool,      // reported, never refused — D2
    pub domains: Vec<String>,       // ordered, head is the primary
    pub pool: Option<SitePool>,     // php-fpm sites only
    pub services: Vec<SiteServiceLink>,
}

pub struct SitePool {
    /// What the row names. `None` after a `service.delete --force` — D3.
    pub declared: Option<ServiceId>,
    /// What `core::resolve` answers at this root today.
    pub resolved: Option<ServiceId>,
}

pub struct SiteServiceLink { pub service: ServiceId, pub state: ServiceState }

pub struct SiteCreation { pub site: SiteDetail }

pub struct SiteRemoval {
    pub removed: SiteSummary,
    /// Freed for another site, and said out loud.
    pub domains_released: Vec<String>,
    /// The files were never ours — `ProjectRemoval::root_kept`'s rule.
    pub doc_root_kept: String,
}
```

`ProjectExport` gains `sites_omitted: Vec<String>` (D9). In `service_api.rs`, `ServiceDelete`
replaces `ServiceQuery` as `service.delete`'s parameter (D4).

New in `rpc::method`: `SITE_CREATE`, `SITE_LIST`, `SITE_SHOW`, `SITE_UPDATE`, `SITE_DELETE`.

### Errors

| Case | Code |
| --- | --- |
| a domain that is syntactically wrong, non-ASCII, or holds `*` | `invalid_argument` |
| a TLD that is not managed (`dev`, `app`, `com`…) | `invalid_argument`, hinting `.test` |
| `.local` without `accept_risky_tld` | `invalid_argument`, hinting `.test` |
| a domain another site already owns | `already_exists`, naming that site |
| the same domain twice in one request | `invalid_argument` |
| an empty domain list on create, or a project name that slugs to nothing | `invalid_argument` |
| a `doc_root` resolving outside the project's root | `invalid_argument` |
| a `reverse-proxy` with no `upstream`, or one that is not an http/https URL with a host | `invalid_argument` |
| a `ProjectRef` matching nothing | `not_found` |
| a `SiteRef` matching nothing | `not_found` |
| a `SiteRef::Path` reaching a project with several sites | `invalid_argument`, naming them |
| a pool or a linked service that does not exist | `not_found`, carrying `mix service create …` |
| no default pool, because `core::resolve` has no answer at that root | `not_found`, carrying `RuntimeUnresolved`'s own sentence |
| `service.delete` over a service a site declares, without `force` | `precondition_failed`, naming the sites |
| a manifest that does not parse, on import or on export | `invalid_argument`, carrying `Error::Manifest`'s path |

A `version` in `[[services]]` that nothing installed satisfies is **not** in this table; it is
reported (D8).

## Crate changes

**`mixengine-proto`** — `site_api.rs`; `ServiceDelete`; `ProjectExport::sites_omitted`; five method
constants.

**`mixengine-core`** — `migrations/0006_site_state.sql` (D1). New `sites.rs`: `create`, `records`,
`find`, `update`, `delete`, and the one-transaction write of a site with its domains and links. New
`domains.rs`: the policy and the slug (D10). `manifest.rs` gains the typed `site` and `services`
fields and a writer that takes an export struct rather than a name and a map (D7, D9), and loses the
"until T39a gives them meaning" sentence this task answers. `projects.rs` gains `sites_of`;
`services.rs` gains `sites_declaring`, which is D4's query. `.sqlx` regenerated.

**`mixengine-daemon`** — new `api/sites.rs`, following `api/create.rs`'s order: every check first,
cheapest and most specific ahead of the rest, and the rows written only once they all pass. Five arms
in `api/rpc.rs`. `service_delete` gains the fourth refusal and the `force` flag.

**`mixengine-cli`** — `mix site create|list|show|update|delete`, with `show` and `update` defaulting
to the current directory, `--i-know` for `.local`, and `mix service delete --force`.

**Docs** — `data-model.md`'s manifest example gains `aliases` and `instance`; the phase-4 file records
that nothing supervises a node process (D3).

## Testing

**The one that matters most is the one that guards the migration.** `crates/mixengine-core/tests/store.rs`
is thickened *before* the rebuild: every index named, both cascades walked
(`projects` → `sites` → `site_domains`, and `services` → `site_service_links`), and both unique
refusals asserted — one domain on two sites, two primaries on one site. Then the tables are rebuilt,
and the fixture's `state = 'stopped'` turns red, which is the proof the guard exists. A cascade lost
in a hand-copied `CREATE TABLE` is invisible until a delete leaves orphans months later.

- `core::domains` units: the policy table row by row, `.local` in both directions, and the slug.
- `core::sites` units: a doc root relativised **through a symlinked temporary directory** — the
  `/tmp` → `/private/tmp` case that cost T39 once; a domain list replaced while the primary invariant
  holds; a domain moved between two sites, which passes only because the deletes precede the inserts.
- `core::manifest` units: `[site]` with no `kind` reads as `None` rather than failing; a
  `reverse-proxy` with no `upstream` is refused by the enum; a round trip through the writer keeps
  `database = "blog"`, an unknown `[[services]]` entry, the comments and the key order.
- `crates/mixengine-daemon/tests/sites.rs`, over a real socket: the lifecycle; `site.create { project }`
  with nothing else typed building a site out of a colleague's manifest; a second site refused the
  first's domain, naming it; deleting the project cascading into its sites and domains;
  `service.delete` refused while a site declares the service, then `--force` going through and
  `SiteDetail` reporting `declared: None` rather than falling silent.
- `crates/mixengine-cli/tests/site.rs`: create from the current directory, `show` from a
  subdirectory, and a `project export` that leaves a hand-written `database = "blog"` where it was.

## Out of scope, and where each goes

- **Rendering a site's config, `site.start|stop`** — T43. This task declares; that one serves.
- **`domain.add|remove`, `domain.dns_status`** — T46. Domains are edited here through
  `site.update { domains }` only.
- **The hosts file and the DNS server** — T41, T44, T45, T46a. Creating a site writes no file outside
  the database.
- **Per-site ports** — Phase 5/7. The columns keep their defaults and stay off the API (D1).
- **Real HTTPS** — Phase 5. `https` is a declaration nothing reads yet, and says so.
- **Creating databases and users from `database =` / `user =`** — Phase 8's `blueprint.apply`. Here
  they are preserved and not interpreted (D8).
- **`domain_pattern`** — Phase 8, with blueprints.
- **Supervising a node process** — no task in the roadmap owns it. Recorded in the phase file as a
  known gap rather than left for T43 to discover (D3).
- **A configurable managed-TLD set** — compiled in. T44 or T46 is where moving it to `config.toml`
  gets decided (D10).
- **IDN / punycode** — unsupported, and refused by name rather than mangled.
