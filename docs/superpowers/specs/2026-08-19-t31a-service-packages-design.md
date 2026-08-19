# T31a — Install a service package, and create a service

*Design, 2026-08-19. Roadmap task [T31a](../../../.claude/roadmap/phase-3-services.md), Phase 3.*

## What this closes

T31 ships a Caddy recipe that is judged against a real server, and nothing can reach it. Nothing
writes to `paths.packages()`, and nothing creates a `services` row — so a Caddy arrives in a home
only because a test unpacked one and wrote both rows itself
(`mixengine_testkit::declare::installed`). This task is the difference between *MixEngine can run
Caddy* and *a user can ask it to*.

The two halves are one task because either alone is unreachable: a package with no `services` row is
a directory, and a row with no package is a foreign key violation.

## What already exists, and is reused unchanged

- `core::install::Installer::install(&Artifact, into, Option<&SmokeTest>, &Watcher)` — resumable
  download, checksum, staged unpack, smoke test, atomic rename. It takes an artifact and a
  destination rather than anything runtime-shaped, which is why it needs no change here.
- `core::index::Client` — the signed index, cached, served stale rather than not at all.
- The job system (T22) and `JobHandle`'s `Watcher` impl (T23).
- `core::generate::Generator` — turns a `services` row plus its `packages` row into `etc/<id>/` and a
  `ServiceSpec`, and already reads `packages/<name>/<version>/` as `Context::install_path`.
- `mixengine_testkit::{MockRegistry, FakePackage}` — a signed index over a real socket and real
  archives in all three shapes. Everything the new tests need exists.

## Decisions

### D1 — `package.*` offers only what this build can run

`package.list_available` and `package.install` know a kind only when `Catalogue::builtin()` has a
recipe for it. The index publishes six service kinds today (caddy, nginx, mariadb, postgres, redis,
memcached) and this build has one recipe (caddy), so five are refused with the message naming what
exists — the same shape `core::generate` already uses for an unknown package.

Rejected: offering every non-runtime kind. It creates a state — *installed, but nothing can run it* —
that every later listing has to account for, and it lets somebody spend a 200 MB download on a
directory MixEngine cannot use.

The cost is accepted deliberately: `package.list_available` answers *what this build can run*, not
*what the index publishes*, so two MixEngine versions reading one index answer differently. That is
correct, and it is the reason there is no second list to keep in step — T33–T37 each unlock their own
kind with the recipe they write, in the same commit.

Runtimes stay out of `package.*` entirely. They have `runtime_installs`, `runtimes/`, a default
version and a shim that reads it; a second door to the same room would either duplicate all of that
or produce a PHP the shim cannot see.

### D2 — Two listings, not one

The roadmap names `package.list`. It needs to be two:

- `package.list` — what is installed. Answers `PackageList { packages: Vec<PackageSummary> }`.
- `package.list_available` — what the index offers for this machine, for the kinds D1 allows.
  Answers `PackageCatalogue { packages: Vec<PackageRelease>, stale: bool }`.

Merging them into one row type with an `installed` flag was considered and rejected on
`runtime_api.rs`'s own stated reasoning: what is knowable about something installed and about
something merely offered is different, and one type carrying both is a type where half the fields are
meaningless in half the answers. `PackageRelease` still carries `installed: bool`, exactly as
`RuntimeRelease` does, composed by the daemon rather than left to a client to cross-reference.

The roadmap names four methods (`package.install|uninstall|list`, `service.create`); `service.delete`
is the fifth (D7) and this listing split is the sixth.

### D3 — A service's package is its id

`ServiceId::name()` already documents itself as "the part before `@` — the package this is an
instance of". So `service.create` takes the id and a version, and derives the package from the id.

Rejected: taking `package` as a separate parameter. It is either redundant with the id or a pair that
has to be policed for agreement, and the invariant is better held by construction.

`version` stays explicit and required, on `RuntimeTarget`'s reasoning: choosing a version for
somebody is a decision, and there is no `resolve` for services to make it.

**This reaches the test suites, and it is the rule doing its job.** The fixtures currently call
themselves `mariadb@main`, `php-fpm@8.3`, `kept` and `lost` while every one of them is backed by the
`fakeservice` package, which `mixengine_testkit::declare` can write because it writes both rows
itself. Once the `services` row comes from `service.create`, each of those ids has to be
`fakeservice@…` — a fixture that calls itself MariaDB while running something else is exactly the
lie D3 removes.

### D4 — A recipe declares how many instances it has

```rust
/// How many instances of this package a home may have, which is what an id may look like.
pub enum Instancing {
    /// Exactly one, and its id carries no `@`: there is one Caddy.
    Single,
    /// As many as are named, and every id carries an `@`: `mariadb@main`, `mariadb@legacy`.
    Named,
}
```

`Recipe::instancing(&self) -> Instancing` has **no default body**, so each of T33–T37 answers the
question rather than inheriting a silence. Caddy is `Single`.

`service.create` enforces it: `caddy@x` is refused for a `Single` recipe, and a bare `mariadb` is
refused for a `Named` one with `mariadb@main` in the hint.

This settles the part of T36 that `service.create` cannot avoid — what a second instance of one
package *means* — while leaving what T36 is actually about (two servers running side by side with
independent ports and data directories) to T36.

Consequence in `core::generate`: the data-directory fallback is `data/<package>/<instance>` today,
which for a `Single` recipe would be `data/caddy/caddy`. It becomes `data/<package>/` for `Single`
and stays `data/<package>/<instance>` for `Named`.

The `services.instance_name` column takes `id.instance()` for `Named` and `id.name()` for `Single`
(the column is `NOT NULL`). `UNIQUE (package_id, instance_name)` is then implied by the primary key
rather than doing new work — kept as the belt it always was.

### D5 — An installed package is smoke-tested by its own recipe

`Recipe::smoke_test(&self) -> Option<SmokeTest>`, default `None`. Caddy returns `caddy version`.

T20a's finding is that an artifact which unpacks and cannot *run* is one the user discovers against
their own site. `Installer::install` already takes `Option<&SmokeTest>`; this is what fills it for a
service, where `core::runtimes::smoke_test(kind)` fills it for a runtime.

The test is on the recipe rather than in the index because the index describes a download and the
recipe is what knows which of the artifact's executables is the server.

### D6 — `service.create` renders before it answers

A row is written, and then `Generator::declared` runs. A service that cannot be rendered — a
misspelled override, a template that will not take these settings — fails at create, not at the first
`service start` hours later. If rendering fails, the row and any `etc/<id>/` written for it are
removed and the generation error is what comes back.

**The rollback is not politeness, it is the only way back.** T30 decided that one `services` row that
cannot be generated fails the *whole* declared set, so a bad row left behind by a failed create would
take `service.list`, every walk and the next start down with it — a home broken by a create that
already reported itself as failed. Removing the row puts the home back where it was.

### D7 — `service.delete` removes the configuration and never the data

- Refused while the service is running or supervised.
- Removes the `services` row and `etc/<id>/`. Generated configuration is disposable by `CLAUDE.md`'s
  rule, so removing it is not a loss.
- Leaves `data/` and `logs/services/<id>/` alone, and **says so**: the answer carries the data
  directory that was kept. Deleting a MariaDB data directory because somebody typed
  `mix service delete` is the destructive accident this product cannot have. A `--purge` is a later
  decision with a confirmation attached to it.

### D8 — `package.uninstall` refuses a package something is an instance of

The schema already says this (`ON DELETE RESTRICT`); the method says it in words, naming the services
that hold it, rather than letting a foreign-key error reach a person.

Removes `packages/<name>/<version>/` and the row otherwise.

### D9 — `RuntimeVersion` is renamed to `PackageVersion`

The type is "upstream's version string, validated because it is a path component". Nothing about it
is runtime-specific, and `package.*` needs exactly it. A second newtype with the same rules is the
drift this codebase avoids everywhere else, and using `RuntimeVersion` inside `package.install` would
be a name that lies at every call site.

A mechanical rename across the workspace, with `VersionError` and `cmp_precedence` going with it.

`RuntimeChannel` is renamed to `PackageChannel` for the same reason and in the same commit: it is
the index's `channel`, it appears in `PackageRelease`, and renaming one of the pair while a
`Runtime`-prefixed sibling sits in a `Package` type is worse than renaming neither. `RuntimeKind`,
`RuntimeSummary` and the rest of `runtime.*` keep their names — they really are about runtimes.

## API surface

New `crates/mixengine-proto/src/package_api.rs`:

```rust
pub struct PackageTarget { pub package: String, pub version: PackageVersion }

/// Both listings take this, and every field has a default, so both are questions a person can type.
/// `None` is every package, on `RuntimeFilter`'s reasoning: a GUI's first paint asks about all of
/// them, and a kind nobody has installed is an empty list rather than an error.
pub struct PackageFilter { pub package: Option<String> }
pub struct PackageList { pub packages: Vec<PackageSummary> }
pub struct PackageSummary {
    pub package: String, pub version: PackageVersion,
    pub path: String, pub installed_at: Timestamp, pub bytes: u64,
    pub services: Vec<ServiceId>,   // what holds it, which is what an uninstall refuses over
}
pub struct PackageCatalogue { pub packages: Vec<PackageRelease>, pub stale: bool }
pub struct PackageRelease {
    pub package: String, pub version: PackageVersion, pub channel: RuntimeChannel,
    pub eol: Option<String>, pub bytes: u64, pub installed: bool,
}
pub struct PackageRemoval { pub removed: PackageSummary }
```

`package` is a plain `String`, validated by the daemon against `Catalogue::packages()` — the closed
set is a property of the build, not of the wire, and the error already exists in `core::generate`.

New in `service_api.rs`:

```rust
pub struct ServiceCreate {
    pub id: ServiceId,
    pub version: PackageVersion,
    pub port: Option<u16>,
    pub bind_addr: Option<String>,
    pub data_dir: Option<String>,
    pub autostart: Option<bool>,
    pub overrides: Option<serde_json::Map<String, serde_json::Value>>,
}
pub struct ServiceRemoval {
    pub removed: ServiceSummary,
    /// The data directory left in place, when the instance had one.
    pub data_kept: Option<String>,
}
```

`service.delete` takes the existing `ServiceQuery` — the id is required there for the same reason it
is on `service.status`, and a delete with no subject is not a delete of everything.

`port` and `data_dir` are optional because the row's columns are nullable and both already have
meaning when null: the Caddy template wraps `http_port` in `{%- if service.port %}`, and the
generator falls back to `data/<package>[/<instance>]`.

Method constants in `rpc::method`: `PACKAGE_LIST`, `PACKAGE_LIST_AVAILABLE`, `PACKAGE_INSTALL`,
`PACKAGE_UNINSTALL`, `SERVICE_CREATE`, `SERVICE_DELETE`.

## Crate changes

**`mixengine-proto`** — `package_api.rs`; the two new `service_api` types; the `PackageVersion`
rename; six method constants.

**`mixengine-core`** — new `packages.rs` beside `runtimes.rs`: `directory(paths, package, version)`
→ `packages/<package>/<version>`, `remember`, `forget`, `records`, `holders(store, package)`.
`generate::recipe` gains `Instancing`, `Recipe::instancing` and `Recipe::smoke_test`; the caddy
recipe answers both; `generate.rs`'s data fallback consults instancing.

**`mixengine-daemon`** — new `packages.rs` holding `Packages`, shaped after `runtimes.rs`: an install
that answers a `JobSummary`, and an install already running answered with the job that is running it.
`service.create|delete` go into `services/` beside the walk methods, because they need the registry
and the generator. Six arms in `api/rpc.rs`.

**`mixengine-cli`** — `mix package list|available|install|uninstall`, `mix service create|delete`.
`install` follows `mix runtime install`'s job-watching exactly.

**`mixengine-testkit`** — `declare::installed` and `installed_blocking` deleted. `declare` keeps only
the half that writes a `packages` row for `fakeservice`, which is a fixture no index will ever
publish, and the `services` rows it wrote come from `service.create` in each suite.

## Testing

- `crates/mixengine-core/src/packages.rs` unit tests: the directory layout, the record round trip,
  `holders`.
- `generate` tests: a `Single` recipe's data fallback, and the two instancing refusals.
- `crates/mixengine-daemon/tests/packages.rs`, over a real socket against a `MockRegistry` serving a
  signed index and a `FakePackage` archive: install → `package.list` → `service.create` →
  `service.list` sees it → `service.delete` → `package.uninstall`. Plus the refusals: a kind with no
  recipe, an id whose shape contradicts the recipe's instancing, an uninstall of a package a service
  holds, a delete of a running service.
- `crates/mixengine-cli/tests/caddy.rs` switches from unpacking an archive and writing rows to
  publishing the CI-fetched Caddy through `MockRegistry::publish_asset` and installing it through
  `package.install`. The rest of the suite is unchanged, and the install path gets covered against a
  real artifact on all three systems for free.

## Out of scope, and where each goes

- **T36 proper** — two instances of one server running side by side, with port allocation and
  independent data directories. D4 declares the rule; running two is T36.
- **`service.configure`** — editing `config_overrides_json` through the API. `testkit::reconfigure`
  still writes it, and the expiry date on that one is not this task.
- **Purging a data directory** — D7 keeps it, and a flag that deletes databases needs a confirmation
  design, not a boolean.
- **Orphan removal under `etc/<id>/`** — belongs to T43 with the site files that make it possible to
  get right (T31's note).
- **`php-fpm`** — its package is a PHP runtime install rather than an index entry, which is T32's
  problem and is why `service.create` checks for a `packages` row rather than for an index entry.
