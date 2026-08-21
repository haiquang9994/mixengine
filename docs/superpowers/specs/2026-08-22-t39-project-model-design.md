# T39 — The project model, and the first pin that refuses something

*Design, 2026-08-22. Roadmap task [T39](../../../.claude/roadmap/phase-4-sites-and-elevation.md), Phase 4.*

## What this closes

Step 3 of the resolution order — *a project registered in this home* — has been shipped since T25 and
has never once run. `core::resolve`'s own module documentation says so: "a project record cannot
exist at all in this build — there are no `project.*` methods until Phase 4, so the `projects` table
is empty on every machine". Every shim, every `mix runtime resolve`, every GUI panel walks that step
and finds nothing, on every machine, always. This task is what makes the step live.

It also pays the debt [todo.md](../../../.claude/roadmap/todo.md) records against
`runtime.uninstall`. [runtime-versions.md](../../../.claude/features/runtime-versions.md) promised
two refusals; T32 delivered the php-fpm pool and left the other written down in a doc comment on
`Runtimes::uninstall` — "a *project* pinning the version is unchecked because there are no projects
until Phase 4". T39 is the task that sentence names, so T39 removes both the sentence and the gap it
describes.

No migration. `0001_initial.sql` already declares `projects` with everything this needs, including
the two unique columns that make one directory one project.

## The roadmap says "project & site model". This is half of it.

T39 as written covers projects, sites, four site kinds, doc roots and manifest read/write — four
tables, two RPC namespaces and a file format. T31a got its own spec for one namespace and two
methods. Splitting it:

- **T39** — the `projects` table, `project.*`, `core::manifest`, `mix project`, and the uninstall
  refusal. This document.
- **T39a** — `sites`, `site_domains`, `site_service_links`, site kinds, doc roots, and the `[site]`
  and `[[services]]` halves of the manifest.

T43 already owns "site → config → reload end-to-end", so T39a sits between them: it declares a site,
T43 renders one. `projects.blueprint_id` stays `NULL` and no method here writes it — that column is
Phase 8's, and a create that quietly filled it would be a promise this build does not keep.

The roadmap file is amended in the same change rather than left to disagree with the specs.

## What already exists, and is reused unchanged

- `core::resolve` — the four-step order, `MANIFEST_FILE_NAME`, `install_command`,
  `manifest_candidates`. Its ancestor walk over `projects` becomes `core::projects::find` and
  `resolve` becomes a caller; the order itself does not move.
- `mixengine_platform::paths::in_full` — a path spelled the way the filesystem spells it, written for
  nginx's `ngx_win32_check_filename` and identity on every system that keeps one name per file.
- `VersionConstraint::parse` / `matches`, and `PackageVersion::cmp_precedence` — a pin is exactly a
  `VersionConstraint`, matched exactly as the resolver matches one.
- `core::runtimes::records` — the installed set, which is the only set a constraint is ever matched
  against.
- `Error::Manifest` and `Error::UnreadableProjectRow`, both already carrying the path or the row that
  could not be read.
- `toml` 1.1.4, and `toml_edit` 0.25.13 already in `Cargo.lock` underneath it — see D10.

## Decisions

### D1 — The database is the source of truth; the manifest is input, and output only when asked

`project.create` writes a `projects` row and does not write into the user's repository.
`project.export` is the one method that touches `mixengine.toml`, and it exists because the point of
the file is that a colleague gets it — not because MixEngine needs it.

This follows the state-ownership table in
[data-model.md](../../../.claude/architecture/data-model.md): projects are declared state living in
`mixengine.db`, and the manifest is the user's file in the user's repository. A daemon that wrote to
a checked-out working tree on every `project.update` would be a daemon producing diffs nobody asked
for, in a directory it does not own.

**But the row is not what resolves.** The manifest outranks it — step 2 above step 3, decided in
`resolve`'s own comment: "A file checked into the repository outranks a registration on this machine
even when the registration is nearer, because the file is the half a colleague also has." So a row
pin that contradicts the manifest can never take effect. It is not wrong to store; it is inert for
that one language, and the row only ever speaks for languages the manifest is silent about.

A method that accepted such a pin silently would leave a person reading `8.3` in the GUI while their
shell ran `8.4`, indefinitely, with nothing anywhere saying which was in charge. So `ProjectDetail`
reports, for every pin, **where it came from and whether anything installed satisfies it** (D6).

Rejected: writing the manifest on create — it makes a write to somebody's repository the side effect
of a registration. Rejected: refusing a row pin the manifest overrides — the file can change under us
at any moment, so a refusal that is correct at create is wrong an hour later.

### D2 — One method writes a project, and `import` is a second name for it

The roadmap lists create *and* import. Once D1 is settled the distinction is hollow: both produce one
row, and the only difference is where `name` and `pins` came from — arguments, or the manifest lying
in the directory. The daemon's work is identical either way, so a second method would be a second
code path to keep in step in exchange for no difference in outcome.

```
project.create { root, name?, pins? }
```

`name` falls through: the argument, then `[project] name` in the manifest at the root, then the
directory's own base name. `pins` falls through: the argument, then `[runtimes]` in that manifest,
then empty. So `project.create { root }` in a directory holding a colleague's manifest *is* the
import, with no flag and no second method.

`mix project import <path>` stays, as `#[command(alias = "import")]` on `create`. That keeps the
CLI's stated rule intact — `main.rs` says "one subcommand per `<ns>.*` method, and nothing that is
not one" — because an alias is the same subcommand under a second name, not a subcommand without a
method behind it.

This is a deliberate deviation from the roadmap's wording, recorded as one.

### D3 — A project is addressed by name or by path, and the daemon does the resolving

```rust
pub enum ProjectRef { Name(String), Path(String) }   // externally tagged
```

`Path` is resolved **by walking up**: the nearest registered root at or above that directory. So
`mix project show` typed three directories deep inside a repository finds the project, which is the
only behaviour that matches what the shim already does two feet away.

That walk is not new code. It is `resolve::in_a_project`'s loop, lifted into
`core::projects::find(store, &ProjectRef) -> Result<Option<ProjectRecord>>`, with `resolve` calling
it for the pin and the API calling it for everything else. Two implementations of "which project is
this directory in?" would be two answers to a question that has one — the same rule that put
`resolve` in `core` to begin with.

The CLI sends `Path(cwd)` when no name is typed and `Name` when one is; the GUI sends `Name`. Neither
decides anything, so `CLAUDE.md`'s rule about clients holds.

### D4 — The wire handle is the name, not the row id

`projects.id` is an `INTEGER PRIMARY KEY` and stays inside the store. `projects.name` is `UNIQUE`,
typed by a person, and stable across a delete and re-create; it is what `ProjectSummary` carries and
what `ProjectRef::Name` names. This matches `ServiceId`, which is a human-stable string for the same
reason, and it keeps a rowid — a number meaningful only to one SQLite file — off an API that a GUI
stores and reuses.

A name is validated on the way in: non-empty after trimming, at most 64 characters, no control
characters, and no `/` or `\`. It is a handle typed on a command line, and T39a will have to consider
it when a site takes a default domain from it; a name holding a path separator can be neither.

### D5 — One directory is one project, on both sides of the comparison

`root_path` is normalised through `paths::in_full` **before** it is written. The column is `UNIQUE`,
so normalising on the way in is what makes `C:\Users\RUNNER~1\blog` and `C:\Users\runneradmin\blog`
one project rather than two.

`resolve`'s existing comment says that much and leaves a hole:

> **Paths are compared as they were written.** Canonicalising here would be the wrong place for it …
> normalising on the way *in* is what makes one directory one project — doing it on the way out
> would leave two spellings able to register twice and only one of them findable.

The first half is right; the second is about a *different* normalisation. `in_a_project` compares
`Path::new(&row.root_path) == directory` byte for byte. If the row was normalised on the way in and
the caller's `cwd` was not, those are two different strings for one directory on Windows — and step 3
misses, on the very day it first has a row to hit. So **the query side is normalised too**: one
`paths::in_full` call on the incoming directory before the walk starts, not one per ancestor. The
comment is rewritten to say which normalisation belongs where.

`in_full` expands 8.3 aliases and settles case. It does **not** follow symlinks or junctions, so two
paths reaching one directory through a junction can still register as two projects. That is a known
limit written down as one rather than papered over: `std::fs::canonicalize` on Windows returns a
`\\?\` verbatim path, a spelling nothing else in this workspace uses and which would leak into every
message and every rendered file.

### D6 — A pin's syntax is refused; its satisfiability is reported

`VersionConstraint::parse` failing is `invalid_argument`, immediately. A pin that nothing installed
satisfies is **accepted**, and named in the answer.

Refusing the second would break the two cases the feature exists for. `project.create` on a
colleague's freshly cloned repository would fail on a clean machine — precisely the machine that
needs telling which PHP to install, by the command that has to succeed in order to tell it. And
`blueprint.apply` (Phase 8) applies pinned exact versions to a machine that by definition has none of
them yet.

So `ProjectDetail` carries, per language:

```rust
pub struct ProjectPin {
    pub kind: RuntimeKind,
    pub constraint: VersionConstraint,
    /// Which of the two would win at resolve time, and where it was read.
    pub source: PinSource,
    /// The installed version this resolves to today.
    pub resolved: Option<PackageVersion>,
    /// When it resolves to nothing: `resolve::install_command`, the same sentence a failed
    /// resolution already gives.
    pub hint: Option<String>,
}

pub enum PinSource {
    /// The row in this home.
    Registered,
    /// `[runtimes]` in a manifest, which outranks the row.
    Manifest { path: String },
}
```

Pins are listed in **effective** order — the manifest's entry where there is one, the row's where
there is not — because that is what the shim will actually do, and a panel showing anything else is a
panel that lies.

`pins` on `project.update` **replaces** the map rather than merging into it. An absent field means
unchanged; `{}` clears every pin. A merge would leave no way to remove one.

### D7 — `runtime.uninstall` refuses when the removal is what breaks a pin

`core::projects::pins_broken_by(store, kind, &version) -> Result<Vec<BrokenPin>>` re-runs the match
against the installed set **minus the version being removed**, and reports the projects whose pin
goes from having an answer to having none.

The transition is the whole of it. A pin that is *already* unsatisfiable stays unsatisfiable and must
refuse nothing — otherwise one stale pin would make unremovable a runtime it never mentions. And a
pin matched by three installed versions is not broken by losing one of them.

The pin is read in effective order (D6), for the reason D1 gives: the manifest wins at resolve time,
so a refusal based on a row the manifest overrides is a refusal that is simply wrong. Reading a few
dozen files is affordable here — an uninstall deletes hundreds of megabytes and runs perhaps monthly.
It is affordable nowhere near the shim, which is why D9 has a condition attached to it.

The refusal is `precondition_failed`, naming each project and its constraint, with the hint that
`--force` proceeds.

### D8 — `--force` crosses the pin and never the running pool

`runtime.uninstall` gains `force`. It skips D7's refusal and **nothing else** — in particular not
T32's running-pool refusal.

The line is between a statement about the future and a fact about the present. A broken pin fails the
next time somebody `cd`s into that directory, and the failure is `RuntimeUnresolved`, which already
names the install that fixes it; a person who has been shown the affected projects and typed
`--force` has made a decision they are entitled to make. A running php-fpm pool is a process serving
requests right now, and removing the files under it produces a live process with no files and a
`services` row naming a runtime that is gone. No flag should buy that.

Worth stating plainly, because an earlier reading of this had it backwards: the pool constraint
*looks* like the foreign key `runtime_install_id … ON DELETE RESTRICT`, but the code never reaches
the key — a **stopped** pool is deleted along with the runtime, deliberately, and only a **running**
one refuses. The asymmetry is therefore a decision inside `uninstall`, not something the schema
enforces for free, and it has to be written and tested as one.

### D9 — One reader for `mixengine.toml`, gated by the bench that already guards the shim

`resolve` deserialises a deliberately narrow `Manifest` — `[runtimes]` and nothing else, with unknown
sections allowed through so this build does not refuse the manifests T39a will write. T39 needs the
whole file: `[project] name` on import, and a writer for export.

Two structs describing one file would be two answers to one question. So `core::manifest` becomes the
single reader — `Manifest { project: Option<ProjectSection>, runtimes: BTreeMap<…>, … }`, unknown
sections still allowed, `[site]` and `[[services]]` captured as raw `toml::Value` until T39a gives
them types — and `resolve::in_a_manifest` calls it.

**The condition.** That puts a full-file deserialise on the shim's ancestor walk, which T29 measured
at 0.58 ms on macOS, 0.74 ms on Linux and 1.71 ms on Windows against a 15 ms budget, with a `bench`
CI job guarding it. If the job moves, `resolve` keeps its narrow struct and `core::manifest` serves
only the write path, with a test asserting the two agree about `[runtimes]`. The measurement decides
this, not this document.

### D10 — `project.export` merges into the file; it does not rewrite it

The manifest is a file in the user's repository, under version control, possibly with comments, a
deliberate key order, and — after T39a — a hand-written `[site]` block. An export that serialised a
fresh document over it would destroy all of that, and would do it to the one file whose entire
purpose is to be read by a person.

So export **edits**: it sets `[project] name` and the `[runtimes]` keys it owns, and leaves every
other byte alone. `toml::Value` cannot do that — it carries no formatting — and `toml_edit` can. It is
already in `Cargo.lock` at 0.25.13 as `toml`'s own dependency, so this adds a line to two manifests
and no new subtree; it is still a new *direct* dependency, and is called out rather than slipped in.

A directory with no manifest gets one written. No `force` is offered: there is nothing to overwrite
that the merge does not already preserve.

## API surface

New `crates/mixengine-proto/src/project_api.rs`, which also holds `ProjectRef` (D3) and
`ProjectPin` / `PinSource` (D6):

```rust
pub struct ProjectCreate {
    pub root: String,
    pub name: Option<String>,
    pub pins: Option<BTreeMap<RuntimeKind, VersionConstraint>>,
}

pub struct ProjectUpdate {
    pub project: ProjectRef,
    pub name: Option<String>,
    pub root: Option<String>,
    pub pins: Option<BTreeMap<RuntimeKind, VersionConstraint>>,   // replaces; see D6
}

pub struct ProjectQuery { pub project: ProjectRef }
pub struct ProjectList { pub projects: Vec<ProjectSummary> }

pub struct ProjectSummary {
    pub name: String,
    pub root: String,
    pub created_at: String,
    /// The `mixengine.toml` at the root, when there is one — which is what decides whether the
    /// row's pins can take effect at all.
    pub manifest: Option<String>,
}

pub struct ProjectDetail { pub project: ProjectSummary, pub pins: Vec<ProjectPin> }

pub struct ProjectRemoval {
    pub removed: ProjectSummary,
    /// The directory left exactly as it was, and said out loud — `ServiceRemoval::data_kept`'s rule.
    pub root_kept: String,
    pub manifest_kept: Option<String>,
}

pub struct ProjectExport { pub path: String, pub created: bool }
```

`project.show`, `project.delete` and `project.export` all take `ProjectQuery`; export writes to
`<root>/mixengine.toml` and nowhere else, and `created` says whether the file was made or merged
into.

`created_at` is ISO-8601 text, matching the column and
[data-model.md](../../../.claude/architecture/data-model.md)'s split: it is written once, read by a
person, and branched on by nobody.

In `runtime_api.rs`:

```rust
pub struct RuntimeUninstall {
    #[serde(flatten)]
    pub target: RuntimeTarget,
    #[serde(default)]
    pub force: bool,
}
```

Flattened rather than made a field on `RuntimeTarget`: that type is also `runtime.install`'s and
`runtime.set_default`'s parameter, where a `force` would mean nothing. The flatten keeps today's wire
shape and adds one optional key, so an older client's request still parses.

New in `rpc::method`: `PROJECT_CREATE`, `PROJECT_LIST`, `PROJECT_SHOW`, `PROJECT_UPDATE`,
`PROJECT_DELETE`, `PROJECT_EXPORT`.

### Errors

| Case | Code |
| --- | --- |
| `root` relative, missing, or not a directory | `invalid_argument` |
| a name empty, over 64 characters, or holding a separator | `invalid_argument` |
| a pin `VersionConstraint::parse` refuses | `invalid_argument` |
| a name already registered | `already_exists` |
| a root already registered, after `in_full` | `already_exists`, naming the project that holds it |
| a `ProjectRef` matching nothing | `not_found` |
| `runtime.uninstall` breaking a pin, without `force` | `precondition_failed` |
| a manifest that does not parse, on import or on export | `invalid_argument`, carrying `Error::Manifest`'s path |

A root **inside** another project's root is allowed. The walk takes the nearest, so nesting already
has a defined answer; only the same directory twice is refused, by the unique column.

## Crate changes

**`mixengine-proto`** — `project_api.rs`; `RuntimeUninstall`; six method constants.

**`mixengine-core`** — new `projects.rs`: `create`, `records`, `find`, `update`, `delete`,
`effective_pins`, `pins_broken_by`. New `manifest.rs` (D9), with `resolve::in_a_manifest` calling it
and `resolve::in_a_project` becoming a thin call into `projects::find`. Two of `resolve`'s comments —
the one about path comparison and the module note about the table being empty on every machine — are
rewritten, because this task is what stops both from being true.

**`mixengine-daemon`** — new `projects.rs` beside `packages.rs`, following `api/create.rs`'s order:
every check first, cheapest and most specific ahead of the rest, and the row written only once they
all pass. `Runtimes::uninstall` gains the D7 call and loses the doc sentence that named this task.
Six arms in `api/rpc.rs`.

**`mixengine-cli`** — `mix project create|list|show|update|delete|export`, with `import` aliasing
`create`. `show` and `export` default to the current directory. `mix runtime uninstall` gains
`--force`.

**Workspace** — `toml_edit` in `[workspace.dependencies]` and in `mixengine-core`.

## Testing

**The one that matters most.** Register a project through `project.create`, then resolve from a
directory *below* its root with no manifest anywhere, and assert `RuntimeSource::Project`. Step 3 has
been covered until now only by tests that wrote a `projects` row by hand; this is the first time it
runs the way a user's machine will run it, end to end, through the method that creates the row.

**The refusal is re-resolution, not name-matching**, proven in two steps with one pin. Pin `^8.3`;
install 8.3.33 and 8.3.34. Uninstall 8.3.33 → **allowed**, because `^8.3` still has an answer.
Uninstall 8.3.34 → **refused**, naming the project. Same pin, same command, two outcomes; only a
correct reading produces both. Then `--force` on the second, and the runtime goes.

Beside it: a pin that is already unsatisfiable refuses nothing, and a running php-fpm pool refuses
even with `--force` (D8).

- `core::projects` units: the fall-through for `name`, `in_full` applied on both sides of `find`, a
  nearer root winning over a further one, and `pins_broken_by`'s transition rule.
- `core::manifest` units: a manifest with unknown sections still reads; `[site]`, `[[services]]`,
  comments and key order all survive a round trip through the writer.
- `crates/mixengine-daemon/tests/projects.rs`, over a real socket: the lifecycle; an import picking up
  a manifest's name and pins with no arguments; a manifest pin reported as `PinSource::Manifest` and
  outranking a contradicting row pin in `ProjectDetail`; `already_exists` for a second registration of
  the same directory under an 8.3 alias; a delete naming the directory it kept.
- `crates/mixengine-cli/tests/project.rs`: `create` and `import` reaching the same state, `show` from
  a subdirectory, `export` writing a file that `create` then reads back.

## Out of scope, and where each goes

- **Sites, domains, service links, `[site]`, `[[services]]`** — T39a. `core::manifest` reads those
  sections as opaque values here, so the writer preserves them before anything can interpret them.
- **Rendering a site's configuration, `site.start|stop`** — T43.
- **`projects.blueprint_id`** — Phase 8. No method here writes it.
- **Deleting the project's files** — `project.delete` removes the row and names the directory it
  kept, on `service.delete`'s reasoning. A `--purge` needs a confirmation design, not a boolean.
- **A manifest `schema = N` version key** — data-model.md reserves it for the first breaking change.
  There has not been one, and inventing the mechanism before then would be guessing at what it has to
  migrate.
