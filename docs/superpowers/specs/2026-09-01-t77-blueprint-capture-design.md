# T77 — the blueprint manifest, `blueprint.capture`, and the plan (design)

Roadmap task **T77**, phase 8. The first half of blueprints: a file that says what a project is made
of, the command that writes one from a project that already works, and the plan that
`mix blueprint apply --dry-run` prints. T78 executes that plan; nothing here executes anything.

## Goal

Capture a working project into one TOML file that carries **what this project uses** and nothing
about this machine — then show, before anything happens, exactly what applying it somewhere else
would do.

The second half of that sentence is the one with teeth. "Never data, credentials or absolute paths"
is a promise the feature doc has been making since it was written; this task turns it into a test
that reads the rendered file and refuses to find them.

## Scope

In: the `BlueprintManifest` type and its reader/writer, `blueprint.capture`, `blueprint.list`,
`blueprint.apply { dry_run: true }` returning a `BlueprintPlan`, and the `mix blueprint` command
group.

Out: execution, resumption and rollback (T78). Scaffold trust and signing (T78a) — the plan
*renders* a `[scaffold]` step because a hand-written blueprint may carry one, but capture never
writes one and nothing runs one. `blueprint.export`, `blueprint.import`, `blueprint.delete` — import
is where the untrusted marking of T78a lives and does not belong to a task that cannot apply
anything. The gallery (T79).

## Decisions

**D1 — The blueprint manifest is its own type, overlapping `mixengine.toml` rather than sharing its
struct.** [`data-model.md`](../../../.claude/architecture/data-model.md) says "same schema as the
project manifest plus a `[blueprint]` header", and the example in
[`features/blueprints.md`](../../../.claude/features/blueprints.md) disagrees with it: a blueprint
carries `domain_pattern = "{project}.test"` where a project manifest carries `domain` and `aliases`,
and it carries `database` and `user`, which the project manifest deliberately does not interpret.

They are also two different kinds of file. `mixengine.toml` is written by a person, lives in their
repository under their comments, and is edited byte-preservingly by
[`manifest::write`](../../../crates/mixengine-core/src/manifest.rs); a blueprint is generated, read
once and thrown away. Forcing one struct to serve both makes every key an `Option` and hands the
comment-preserving writer a second file shape to preserve.

So: `core::blueprints::manifest` owns `BlueprintManifest`, and shares with `manifest.rs` only the
leaf vocabulary that is genuinely one thing — `RuntimeKind`, `VersionConstraint`, `SiteKind`,
`ServiceId`, all from `mixengine-proto`. The sentence in `data-model.md` is corrected to say
*overlaps* rather than *same schema*.

**D2 — `[php]` carries `extensions` and not `ini`, because no ini value on this machine deviates
from any other machine's.** The feature doc's example has
`ini = { memory_limit = "512M", upload_max_filesize = "64M" }`. There is nothing to read it out of.
PHP's ini settings are written as **generated constants** in
[`runtimes::extensions`](../../../crates/mixengine-core/src/runtimes/extensions.rs) — the same
`memory_limit = 512M` on every machine this product runs on — and the only per-service override map
a php-fpm pool has, `config_overrides_json`, holds `max_children`, `max_requests`,
`request_timeout`, `ready_timeout_ms` and `stop_grace_ms`: process-supervision knobs, not ini.

Capturing `memory_limit` would therefore capture a global default, which is the one thing this task
is defined against. And a key in the vocabulary that nothing writes and nothing applies is the
promise `manifest.rs`'s own D8 refuses to make. `ini` arrives with the task that gives a project its
own ini, and the feature doc is corrected to match.

`extensions` survives, because `extension_choices_json` **already is a deviation** — the user's
turned-on and turned-off, not the set the build ships. Capture takes the choices of the PHP that the
site's pool runs on, and no others.

**Of those, only the ones turned on.** A blueprint says what a project needs loaded; *turning
something off* on the receiving machine is somebody else's optimisation, not this project's
requirement, and a blueprint that arrived and disabled `mongodb` for every other project on that
machine would be doing harm it was never asked to do. So `PlanAction::SetPhpExtension` enables and
has no `enabled` flag — a field only the "off" direction would need is a field nothing writes.

**D3 — `database` and `user` become read.** [`manifest.rs:137`](../../../crates/mixengine-core/src/manifest.rs)
says these two keys pass through untouched because "a key read and then quietly ignored is a promise
not kept". That was right while nothing could act on them. Capture can: it is the one reader, and
what it does with them is copy them into the blueprint. Two fields are added to `ManifestService`,
documented as read by capture, and the writer keeps preserving them as it does today.

Without this, a blueprint captured from a Laravel project carries no database name, and M8's
"capture a project, apply it under a new name" produces a site that cannot connect to anything.

**D4 — Tokenising is substitution, never invention.** `{project}` is written where the project's own
name literally appears, and nowhere else:

- `blog.test` for project `blog` becomes `domain_pattern = "{project}.test"`.
- `shop-staging.test` for project `blog` does **not** become a pattern. The manifest keeps the
  literal domain, and applying it under a second name is `blocked` at plan time because the domain
  is taken. A guessed pattern is a blueprint that silently breaks on somebody else's machine.
- `database = "blog"`, `user = "blog"` tokenise by the same rule.

The same rule reads `instance_name`, and this one is a trap rather than a nicety: a project `blog`
using `mariadb@blog` has a **dedicated** instance, and copying the literal name into the blueprint
would make applying it as `shop` plug the new project into the old project's database server. So an
instance whose name is the project's name is written `instance = "per-project"`; any other name is
copied as it stands, which is how `main` stays `main`.

**D4a — A runtime the project never asked for is not a runtime it uses.** `core::resolve`
answers with a [`RuntimeSource`](../../../crates/mixengine-proto/src/runtime_api.rs), and that is
exactly the fact this task needs: a version decided by `RuntimeSource::Default` was decided by the
machine, not by the project, and capturing it would write this home's default into a file meant for
somebody else's. So a kind is captured when its source is the project's pin or its manifest, and
skipped when it is the default.

PHP is the exception and not by special-casing: it is read from the `runtime_installs` row behind
the pool the site actually names, which is a fact about the site rather than a fall-through.

**D5 — One site, and a project with two is refused by name.** The manifest has one `[site]`, the
schema allows a project many, and picking the first silently loses the rest. Capture writes no
`[site]` for a project that has none (a project that pins PHP for CLI work is a real thing to
capture), captures the site of a project that has one, and refuses a project that has more with a
typed error that names them. `[[sites]]` is where this widens; it is not needed to capture anything
the gallery will ship.

**D6 — What is never captured, and the test that proves it.** Absolute paths of any kind:
`root_path` never enters the manifest, and `doc_root` is already relative. Credentials, keyring
entries, database contents. `http_port` and `https_port` — a property of this machine's front end,
not of the project. LAN sharing state, which T74 through T76 established is a thing a person turns
on for a machine and a moment. The front-end service (caddy, nginx) and the php-fpm pool itself:
the pool is already said by `[runtimes] php` plus `kind = "php-fpm"`, and the web server belongs to
whoever receives the blueprint.

The enforcement test is written against the **rendered TOML string**, not against the struct: a
fixture project with a password in `config_overrides_json`, the home directory inside its doc root
and the machine's name in its domain, captured, and then asserted to contain no absolute path, no
`MIXENGINE_HOME`, and no key from a credential deny-list. Asserting on the struct would only prove
that the fields we remembered are empty.

**D7 — The row is the truth; the file is a rendering.** `blueprints.manifest_toml` is the blueprint.
`blueprints/<name>.toml` is written beside it so a person can read and copy it, and is **never**
parsed back into state — the rule `etc/` has lived under since the beginning. `blueprint.list` reads
the table.

Two consequences. The rendering is **deterministic**: fixed section order, `BTreeMap` inside each
section, so capturing the same project twice produces two byte-identical files and a golden test
means something. And the name is a **slug**, validated at the boundary before anything touches the
disk, because it becomes a filename: `--name "../../etc/x"` is refused, not resolved.

The name is also the wire handle and the table's `id`, which makes uniqueness the primary key's job
and needs no migration. The `name` column carries a display name, equal to the slug for a captured
blueprint; the gallery is where the two differ. Capturing onto an existing name is refused unless
`overwrite` is set, because `blueprint.delete` is not in this task and a typo would otherwise be
permanent.

**D8 — The plan is a value, and it fixes the set and the order — not the numbers the machine
assigns.** The acceptance criterion is that `--dry-run` matches exactly what the real run performs,
which is only enforceable if there is one place that decides. `core::blueprints::plan()` is that
place; T78's executor consumes `Vec<PlanStep>` and may **fail**, but may not add a step, drop one or
reorder them.

What is deliberately *not* in the plan is anything only the execution can know: the port a new
instance gets, a generated password, a rowid. The plan says *create `mariadb@shop`*; which port that
lands on is not a decision anybody can make in advance, and a plan that pretended to would be a plan
the executor has to contradict.

The order is part of the contract, and is dependency order: project, runtimes, services, databases,
site, domains, certificate, scaffold.

**D9 — The plan reads this home's own tables and nothing else. No index, no network.** The mismatch
prompt in the feature doc shows a download size, and a size means asking the index, and the index has
a network behind a six-hour cache. A `--dry-run` that hangs on DNS resolution is worse than one that
cannot say how many megabytes: this is the command a person runs *because* they do not want anything
to happen yet.

Reading the *cached* index instead was considered and dropped: `Client::cached` is private, the plan
would print nothing it learned there, and a second failure mode bought nothing. So a version nothing
installed satisfies is `create`, without a size and without a promise that the index still publishes
it — which the real run discovers, on the network, where that question belongs.

**D10 — Every blocking condition is decided at plan time.** The point of a plan is that T78 does not
get five actions into a project directory before discovering the sixth was impossible. So:

- A root directory that is already another project's `root_path` (which is `UNIQUE`) is `blocked`.
- A **non-empty** directory is *not* blocked — applying a blueprint onto a repository somebody just
  cloned is the normal case — **unless** the blueprint carries `[scaffold]`, whose whole shape
  (`composer create-project laravel/laravel .`) requires an empty one.
- A project name longer than `NAME_LIMIT` (64, [`projects.rs`](../../../crates/mixengine-core/src/projects.rs))
  is blocked; so is one whose `{project}` expansion exceeds a limit belonging to something else —
  MySQL's user names are 32 characters, and a 60-character project is a failure that must surface at
  dry-run rather than halfway through an apply.
- A domain the blueprint resolves to that another site already owns is blocked, with the owner named.

**D11 — Elevation is named in the plan.** Adding a domain writes the hosts file; a first certificate
may need the CA in the trust store. A dry-run that does not say "this will ask you for a password
once" fails to answer the question it was run to answer. `PlanStep` carries an `elevates` flag, and
the rendering gathers them into one closing sentence — one prompt for the apply, in keeping with
every other elevated operation in this product.

**D12 — `dry_run: false` is a typed refusal naming T78.** Not `todo!()`, not a silent success, and
not a CLI that refuses to send it: the client does not hold the rule, it renders the daemon's answer.

**The code is `PreconditionFailed`, not `UnsupportedPlatform`** — corrected while implementing.
`UnsupportedPlatform` means *this operating system genuinely cannot do it*, and answering with it
here would be a lie about the machine: every OS this product ships to will execute a plan the moment
T78 lands. What is missing is the build, which is exactly what `PreconditionFailed` describes — "not
in a state where this can be done yet, and the user can get it there".

## Data model

No migration. The `blueprints` table has existed since `0001_initial.sql`; this is the task that
first writes to it.

| Column | Written by capture |
|---|---|
| `id` | the slug, which is the wire handle and the filename stem |
| `name` | the display name; equal to the slug when captured |
| `description` | what `--description` said, or empty |
| `manifest_toml` | the rendered manifest — the truth |
| `created_at` | ISO-8601 UTC, this schema's convention for a moment a person reads |
| `source` | `captured` |

The manifest opens with `schema = 1`. A reader that meets a higher number refuses with a typed
error rather than reading what it half-understands; a lower one does not exist yet.

What a capture of a working Laravel project looks like:

```toml
schema = 1

[blueprint]
name = "laravel-php82"
description = "Laravel + MariaDB + Redis"
created_at = "2026-09-01T09:00:00Z"
created_on = { os = "windows", version = "0.1.0" }

[runtimes]
php = "8.2.23"

[site]
kind = "php-fpm"
doc_root = "public"
https = true
domain_pattern = "{project}.test"
aliases = ["api.{project}.test"]

[[services]]
name = "mariadb"
version = "11.4.3"
instance = "main"
database = "{project}"
user = "{project}"

[[services]]
name = "redis"
instance = "main"

[php]
extensions = ["redis", "xdebug"]
```

No `[scaffold]`, no `[php] ini`, no ports, no paths.

## API

```
blueprint.capture { project, name, description, overwrite } -> BlueprintSummary
blueprint.list                                              -> [BlueprintSummary]
blueprint.apply   { blueprint, project, root, dry_run }     -> BlueprintPlan
```

`project` on capture is a name, not a rowid (the wire handle is the name, never a rowid); absent, it falls through
`core::resolve` to the project of the current directory, the way every other project-scoped command
does. `BlueprintSummary` carries the slug, display name, description, `created_at`, source and the
path of the rendered file — which is what makes a `blueprint.get` the namespace does not have
unnecessary.

In `mixengine-proto`: `blueprint.rs` for the domain types, `blueprint_api.rs` for requests and
responses, matching the convention every other namespace follows.

```rust
pub struct BlueprintPlan { blueprint: String, project: String, root: PathBuf, steps: Vec<PlanStep> }
pub struct PlanStep { action: PlanAction, disposition: Disposition, elevates: bool }

pub enum PlanAction {
    RegisterProject { name, root },
    InstallRuntime { kind, version },
    EnsureService { id, version, dedicated: bool },
    CreateDatabase { service, database, user },
    CreateSite { kind, doc_root, https },
    AddDomain { domain, primary },
    IssueCertificate { domains },
    SetPhpExtension { runtime, name },
    RunScaffold { command },
}

pub enum Disposition {
    Satisfied,                      // already true; the apply does nothing here
    Create,                         // will be done
    Choice { installed, wanted },   // a version mismatch — T78 asks, T77 reports
    Confirm { what },               // a scaffold command — T78a's gate
    Blocked { reason },             // decided here, never discovered mid-apply
    Unsupported { reason },         // this OS cannot
}
```

`SetPhpExtension` carries the fact that it reaches beyond this project: enabling `xdebug` changes
the PHP that **every** project on the receiving machine runs on, and that belongs in the printed
line rather than in a paragraph of documentation nobody reads at the moment they need it.

## CLI

```
mix blueprint capture --name <slug> [--project <name>] [--description <text>] [--overwrite]
mix blueprint list [--json]
mix blueprint apply <slug> --project <name> [--path <dir>] --dry-run [--json]
```

`--path` absent means `<cwd>/<project>`. `apply` without `--dry-run` is sent, and the daemon's
`Unsupported` is what gets printed.

The plan renders as words, not glyphs — [`render.rs`](../../../crates/mixengine-cli/src/render.rs)
does not contain a single `✓` or `✗`, and a non-ASCII status column on a Windows console is a
rendering problem this product does not need:

```
Plan: laravel-php82 into project shop at C:\dev\shop

  installed   php 8.2.23
  create      mariadb 11.4.3, reusing the shared instance @main
  create      database shop, user shop
  asks        node 22.8.0 — 22.9.1 is installed
  machine     php extension xdebug — changes PHP 8.2.23 for every project here
  create      site php-fpm at public, https
  blocked     domain shop.test is taken by site blog

applying this asks for elevation once, to write the hosts file
```

## Elevation

None in this task. Capture writes inside the home; the plan only reads. What the plan *reports* is
that T78's execution will need one prompt, from the hosts-file write behind `AddDomain` and,
on a machine that has never issued one, the trust-store install behind `IssueCertificate`.

## Testing

Three groups, and the middle one is the group that proves what the task promised.

**Capture reads the right things.** A golden manifest from a store built with `mixengine-testkit`.
Capturing twice yields two byte-identical strings. A project with two sites is refused with both
names in the message. `mariadb@blog` on project `blog` renders `per-project`, `mariadb@main` renders
`main`. A project with no pins gets no invented `[runtimes]` entries. A project with no site gets no
`[site]`.

**What is forbidden does not get out.** The rendered TOML of a deliberately dirty fixture contains no
absolute path, no `MIXENGINE_HOME`, no credential key. `--name "../../x"` is refused before the
write. A password in `config_overrides_json` does not appear anywhere in the output.

**The plan says what would happen.** In one home: capture a project, then plan applying it under a
new name — every service step is `Satisfied`, the domain step is `Create` with `{project}` expanded.
Remove the runtime from the store and that step becomes `Create`; change the installed version and it
becomes `Choice`; give another site the target domain and it becomes `Blocked` naming the owner; a
64-character project name is `Blocked` before anything else is considered. Step order is asserted as
an invariant, not incidentally by a golden file.

CLI: a render test over a plan fixture, and one asserting the `Unsupported` message for
`apply` without `--dry-run`.

## Dependencies

T39/T39a for the manifest and its writer, T28 for extension choices, T32 for the php-fpm pool and
its source in `runtime_installs`, T50 for what an `IssueCertificate` step means. Nothing here needs
the supervisor.

## Risks

**The blueprint pins exact versions and the world moves.** A blueprint captured today asks for
MariaDB 11.4.3 forever, and every apply on a machine with 11.4.5 produces a `Choice`. That is the
feature doc's stated design — exact when captured, ranges when hand-written — and the gallery is
where hand-written ranges live. The risk is noise, not breakage.

**`extension_choices_json` is machine-wide and a blueprint makes it look project-scoped.** Mitigated
by the renderer saying so on the line that proposes it, in the words the sample output shows, not by
dropping the capture: a Laravel project that needs `redis` needs it, and a blueprint that omitted it would be
wrong in the more expensive direction.

## Text that this task makes wrong

- [`features/blueprints.md`](../../../.claude/features/blueprints.md) — the `[php] ini` line in the
  example manifest, and "non-default ini values" in the Capture paragraph. Corrected per D2.
- [`architecture/data-model.md`](../../../.claude/architecture/data-model.md) — "Same schema as the
  project manifest" becomes *overlapping*, per D1.
- [`manifest.rs`](../../../crates/mixengine-core/src/manifest.rs) — the note saying `database` and
  `user` are not read. They are, as of D3, by exactly one caller.
- [`projects.rs`](../../../crates/mixengine-core/src/projects.rs) — `kept_warm`'s note says the
  missing half of "which services does this project use" belongs to T77. It does not: the edge is
  `site_service_links`, which has existed since `0006`, and capture *reads* it rather than creating
  anything. The note is corrected to point at the table, so the next person to widen that query
  knows it is already possible.
