# T31a — Service packages and service creation: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user install a service package from the signed index and create a service instance from it, so that a Caddy reaches a home through the API instead of through a test that unpacks one.

**Architecture:** `package.*` mirrors `runtime.*` one layer along — the same `Installer`, the same job system, a new `core::packages` module and a new `daemon::packages::Packages`. `service.create|delete` live beside the walk methods because they need the registry and the generator. A `Recipe` gains two questions it must now answer: how many instances it has, and how to prove an install runs.

**Tech Stack:** Rust (workspace), `sqlx` (SQLite, compile-time checked queries), `tokio`, `minijinja`, `serde`; tests use `mixengine-testkit` (`MockRegistry`, `FakePackage`, `Home`, `Daemon`).

**Spec:** [docs/superpowers/specs/2026-08-19-t31a-service-packages-design.md](../specs/2026-08-19-t31a-service-packages-design.md)

## Global Constraints

- `cargo clippy --workspace -- -D warnings` must be clean before any commit; `cargo fmt --all --check` too — clippy clean is not fmt clean.
- `cargo sqlx prepare --workspace -- --all-targets --all-features` after editing **any** `sqlx::query!` or `sqlx::query_scalar!`, or the offline build breaks.
- No `#[cfg(windows)]` or other OS branching outside `mixengine-platform`.
- Cross-platform or not merged: every task must compile on Windows, macOS and Linux. An unsupported path returns a typed `Unsupported` error, never `todo!()`.
- `mixengine-testkit` is a dev-dependency only and must never be reachable from a shipped binary.
- Generated config under `etc/` is disposable and is never parsed back into state.
- Commit messages: `<type>(<scope>): <message>`, imperative, English, **no** `Co-Authored-By` trailer.
- **Commits are gated.** The user's standing rule is that nothing is committed unless they ask, and one request authorises one commit. Every "Commit" step below means: show the diff, ask, then commit if told to.
- Doc comments carry the *why*. A decision that is in the code belongs beside it; a decision that crosses crates belongs in an ADR; only what neither can hold goes in the roadmap phase file.

## File Structure

**Created**

| File | Responsibility |
| --- | --- |
| `crates/mixengine-proto/src/version.rs` | `PackageVersion`, `PackageChannel`, `VersionError`, `VersionConstraint` — the version vocabulary, no longer runtime-specific |
| `crates/mixengine-proto/src/package_api.rs` | What `package.*` asks and answers |
| `crates/mixengine-core/src/packages.rs` | The `packages` table and `packages/<name>/<version>/` on disk |
| `crates/mixengine-daemon/src/packages.rs` | The four `package.*` methods, and the install job |
| `crates/mixengine-daemon/src/services/create.rs` | `service.create` and `service.delete` |
| `crates/mixengine-daemon/tests/packages.rs` | The whole lifecycle over a real socket against a mock index |

**Modified**

| File | Change |
| --- | --- |
| `crates/mixengine-proto/src/runtime.rs` | version types move out; `RuntimeKind`, `RuntimeSummary` stay |
| `crates/mixengine-proto/src/lib.rs` | re-exports for the moved and new types |
| `crates/mixengine-proto/src/rpc.rs` | six method constants |
| `crates/mixengine-proto/src/service_api.rs` | `ServiceCreate`, `ServiceRemoval` |
| `crates/mixengine-core/src/generate/recipe.rs` | `Instancing`, `Recipe::instancing`, `Recipe::smoke_test` |
| `crates/mixengine-core/src/generate.rs` | data-directory fallback consults instancing |
| `crates/mixengine-core/src/generate/recipes/caddy.rs` | answers both new questions |
| `crates/mixengine-core/src/lib.rs` | `pub mod packages` |
| `crates/mixengine-daemon/src/api/rpc.rs` | six dispatch arms |
| `crates/mixengine-daemon/src/api/mod.rs` | `Packages` on the API struct |
| `crates/mixengine-daemon/src/services/fakeservice.rs` | answers both new questions |
| `crates/mixengine-cli/src/main.rs` | `mix package`, `mix service create|delete` |
| `crates/mixengine-testkit/src/declare.rs` | `installed`/`installed_blocking` deleted; only the `packages` half kept |
| `crates/mixengine-daemon/tests/{service,lifecycle,logs}.rs`, `crates/mixengine-cli/tests/{service,daemon}.rs` | fixture ids become `fakeservice@…`; rows come from `service.create` |
| `crates/mixengine-cli/tests/caddy.rs` | installs the real Caddy through `package.install` |
| `.claude/roadmap/phase-3-services.md`, `.claude/roadmap/todo.md` | tick T31a; correct the packaging claims |

---

### Task 1: The version vocabulary stops being runtime-specific

`RuntimeVersion` is "upstream's version string, validated because it is a path component". `package.*` needs exactly it, and a second newtype with the same rules is the drift this codebase avoids. Rename, and move the types to a file whose name is true.

**Files:**
- Create: `crates/mixengine-proto/src/version.rs`
- Modify: `crates/mixengine-proto/src/runtime.rs`, `crates/mixengine-proto/src/lib.rs`, and every use across `mixengine-core`, `mixengine-daemon`, `mixengine-cli`, `mixengine-shim`
- Test: the existing tests move with the types; no new behaviour

**Interfaces:**
- Consumes: nothing
- Produces: `PackageVersion` (was `RuntimeVersion`) with `parse`, `as_str`, `MAX_LEN`, `cmp_precedence`; `PackageChannel` (was `RuntimeChannel`) with `as_str`; `VersionError`; `VersionConstraint` unchanged in name

- [ ] **Step 1: Move the types**

Cut `RuntimeVersion`, `RuntimeChannel`, `VersionError` and `VersionConstraint` — with their doc comments and their `mod tests` — out of `runtime.rs` into a new `version.rs`. Rename in the new file only:

```rust
//! Version strings and the constraints that select between them.
//!
//! Not runtime-specific and never was: a `PackageVersion` is upstream's own string, validated
//! because it becomes a path component, and `packages/<name>/<version>/` needs that as much as
//! `runtimes/<kind>/<version>/` does. The `Runtime` prefix these carried until T31a described the
//! only caller there happened to be.

pub struct PackageVersion(String);
pub enum PackageChannel { Stable, Rc, Dev }
```

`RuntimeKind`, `RuntimeSummary`, `RuntimeRelease` and the rest of `runtime.rs` keep their names — they really are about runtimes.

- [ ] **Step 2: Wire the module and re-exports**

In `lib.rs`, add `mod version;` and re-export `PackageChannel`, `PackageVersion`, `VersionConstraint`, `VersionError` from it instead of from `runtime`.

- [ ] **Step 3: Rename every use across the workspace**

```bash
grep -rln "RuntimeVersion\|RuntimeChannel" --include=*.rs crates/
```

Replace `RuntimeVersion` → `PackageVersion` and `RuntimeChannel` → `PackageChannel` in each. Fix the prose in doc comments that reads wrong afterwards (`"a runtime version"` → `"a version"`), do not leave a mechanical substitution inside a sentence.

- [ ] **Step 4: Verify nothing is left and everything still builds**

```bash
grep -rn "RuntimeVersion\|RuntimeChannel" --include=*.rs crates/   # expect: no matches
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: no matches from grep; clippy clean; the whole suite green, because nothing changed but names.

- [ ] **Step 5: Commit** (ask first — see Global Constraints)

```bash
git add -A
git commit -m "refactor(proto): a version string is not runtime-specific, so name it PackageVersion"
```

---

### Task 2: A recipe answers how many instances it has, and how to prove an install runs

**Files:**
- Modify: `crates/mixengine-core/src/generate/recipe.rs`, `crates/mixengine-core/src/generate.rs`, `crates/mixengine-core/src/generate/recipes/caddy.rs`, `crates/mixengine-daemon/src/services/fakeservice.rs`
- Test: `crates/mixengine-core/src/generate.rs`'s `mod tests`, `crates/mixengine-core/src/generate/recipes/caddy.rs`'s `mod tests`

**Interfaces:**
- Consumes: `PackageVersion` from Task 1
- Produces:
  - `mixengine_core::generate::recipe::Instancing` — `Single` | `Named`
  - `Recipe::instancing(&self) -> Instancing` — **required**, no default body
  - `Recipe::smoke_test(&self) -> Option<mixengine_core::install::SmokeTest>` — defaults to `None`
  - Data-directory rule: `data/<package>` for `Single`, `data/<package>/<instance>` for `Named`

- [ ] **Step 1: Write the failing tests**

In `generate.rs`'s test module, beside the existing context tests:

```rust
/// A singleton's data directory is `data/<package>`, and not `data/<package>/<package>`.
///
/// The fallback was written for the case that has an instance name to use. A recipe that exists
/// once has no such half, and repeating the package name reads as a mistake in a directory
/// listing — which is where somebody meets it.
#[tokio::test]
async fn a_single_instance_recipe_keeps_its_data_directly_under_the_package() {
    let home = TempHome::new();
    let paths = home.paths();
    let generator = generator(&paths, Catalogue::default().with(Arc::new(SingleFixture)));

    declare_row(&generator, "solo", "solo", "1.0.0").await;

    let context = generator.context_for("solo").await.expect("a context");

    assert_eq!(context.data(), paths.data().join("solo"));
}

/// A named-instance recipe keeps the shape it always had.
#[tokio::test]
async fn a_named_instance_recipe_keeps_its_data_under_the_instance() {
    let home = TempHome::new();
    let paths = home.paths();
    let generator = generator(&paths, Catalogue::default().with(Arc::new(NamedFixture)));

    declare_row(&generator, "many@first", "many", "1.0.0").await;

    let context = generator.context_for("many@first").await.expect("a context");

    assert_eq!(context.data(), paths.data().join("many").join("first"));
}
```

In `caddy.rs`'s test module:

```rust
/// There is one Caddy. `caddy@main` would be a distinction without a difference, and two of them
/// fighting over port 80 is not a configuration anybody meant to ask for.
#[test]
fn caddy_exists_once() {
    assert_eq!(Caddy.instancing(), Instancing::Single);
}

/// An artifact that unpacks and will not run is one the user meets against their own site, which
/// is T20a's finding and the reason `Installer::install` takes a smoke test at all.
///
/// `caddy version` and not `caddy --version`: Caddy's is a subcommand, and a flag that is not one
/// exits non-zero — which would fail every install of a perfectly good archive.
#[test]
fn caddy_proves_itself_by_running() {
    let smoke = Caddy.smoke_test().expect("a server proves it runs");

    assert_eq!(smoke.executable, "caddy");
    assert_eq!(smoke.args, ["version"]);
}
```

Match the fixture-recipe and helper names already used in each test module rather than the placeholders above — read the surrounding tests first and follow them.

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p mixengine-core generate
```

Expected: FAIL — `Instancing` does not exist, `instancing` and `smoke_test` are not members of `Recipe`.

- [ ] **Step 3: Add the vocabulary and the two methods**

In `recipe.rs`:

```rust
/// How many instances of this package a home may have, which is what an id may look like.
///
/// **A recipe must answer**, which is why [`Recipe::instancing`] has no default body: the question
/// is different for every server in `.claude/features/services.md`'s catalogue, and a default here
/// would be a decision made by whoever wrote this enum on behalf of a recipe nobody had written
/// yet. It is also the half of T36 that `service.create` cannot avoid — what a *second* instance
/// of one package means — while running two of them side by side stays T36's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instancing {
    /// Exactly one, and its id carries no `@`: there is one Caddy, and one active front end.
    Single,

    /// As many as are named, and every id carries one: `mariadb@main`, `mariadb@legacy`.
    Named,
}
```

On the trait:

```rust
    /// How many instances of this package a home may have. See [`Instancing`].
    fn instancing(&self) -> Instancing;

    /// What proves an installed copy of this package actually runs here.
    ///
    /// Handed to [`Installer::install`](crate::install::Installer::install) after the archive is
    /// unpacked and before the staging directory is renamed into place, so a build that will not
    /// start on this machine leaves nothing behind. [`None`] for a package with nothing cheap to
    /// run — but a server almost always has one, and T20a's whole finding is that unpacking is not
    /// evidence.
    ///
    /// The executable is named by its key in `Artifact::provides`, not by a path: the path inside
    /// the archive belongs to the publisher and the name belongs to us.
    fn smoke_test(&self) -> Option<crate::install::SmokeTest> {
        None
    }
```

In `generate.rs`, replace the data fallback with one that consults the recipe:

```rust
            data: row.data_dir.map_or_else(
                || match recipe.instancing() {
                    // A server that exists once has no instance half to name a directory after, and
                    // `data/caddy/caddy` reads as a mistake to whoever finds it.
                    Instancing::Single => self.paths.data().join(&row.package),
                    Instancing::Named => self
                        .paths
                        .data()
                        .join(&row.package)
                        .join(&row.instance_name),
                },
                PathBuf::from,
            ),
```

In `caddy.rs`:

```rust
    fn instancing(&self) -> Instancing {
        Instancing::Single
    }

    fn smoke_test(&self) -> Option<SmokeTest> {
        Some(SmokeTest {
            executable: PACKAGE.to_owned(),
            // A subcommand and not a flag: `caddy --version` exits non-zero, which would fail the
            // install of an archive that is perfectly good.
            args: vec!["version".to_owned()],
        })
    }
```

In `fakeservice.rs`, `Instancing::Named` and no smoke test — the fixture is reached as `fakeservice@…` from Task 8 onwards, and it has nothing to prove by running twice.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p mixengine-core generate
cargo test -p mixengine-core --all-targets
```

Expected: PASS. Any test asserting the old `data/<package>/<instance>` path for a `Single` fixture is updated, not worked around.

- [ ] **Step 5: Commit** (ask first)

```bash
git commit -am "feat(services): a recipe says how many instances it has and how to prove it runs"
```

---

### Task 3: `core::packages` — the table and the directory

**Files:**
- Create: `crates/mixengine-core/src/packages.rs`
- Modify: `crates/mixengine-core/src/lib.rs`
- Test: `crates/mixengine-core/src/packages.rs`'s own `mod tests`

**Interfaces:**
- Consumes: `PackageVersion` (Task 1), `Paths`, `Store`, `Timestamp`
- Produces:
  ```rust
  pub fn directory(paths: &Paths, package: &str, version: &PackageVersion) -> PathBuf;
  pub struct Installation { pub package: String, pub version: PackageVersion,
                            pub path: PathBuf, pub url: String, pub sha256: String, pub bytes: u64 }
  pub async fn remember(store: &Store, installation: &Installation, at: Timestamp) -> Result<PackageSummary>;
  pub async fn forget(store: &Store, package: &str, version: &PackageVersion) -> Result<()>;
  pub async fn record(store: &Store, package: &str, version: &PackageVersion) -> Result<PackageSummary>;
  pub async fn records(store: &Store, filter: Option<&str>) -> Result<Vec<PackageSummary>>;
  pub async fn holders(store: &Store, package: &str, version: &PackageVersion) -> Result<Vec<ServiceId>>;
  ```

- [ ] **Step 1: Write the failing tests**

```rust
/// `packages/<name>/<version>` — the layout `Context::install_path` has always read.
#[test]
fn a_package_lands_under_its_name_and_version() {
    let home = TempHome::new();
    let paths = home.paths();

    assert_eq!(
        directory(&paths, "caddy", &version("2.11.4")),
        paths.packages().join("caddy").join("2.11.4")
    );
}

/// What was written is what comes back, including the services holding it — which is none.
#[tokio::test]
async fn a_recorded_package_is_listed_with_nothing_holding_it() {
    let store = store().await;

    let written = remember(&store, &installation("caddy", "2.11.4"), Timestamp(1_760_000_000_000))
        .await
        .expect("a package is recorded");

    assert_eq!(written.package, "caddy");
    assert_eq!(written.services, Vec::<ServiceId>::new());
    assert_eq!(records(&store, None).await.expect("a listing"), vec![written]);
}

/// The same version twice is two clients asking at once, and deserves a sentence naming it rather
/// than SQLite's unique-index error.
#[tokio::test]
async fn recording_a_package_twice_is_refused_by_name() {
    let store = store().await;
    let at = Timestamp(1_760_000_000_000);

    remember(&store, &installation("caddy", "2.11.4"), at).await.expect("the first");
    let error = remember(&store, &installation("caddy", "2.11.4"), at).await
        .expect_err("the second");

    assert!(format!("{error}").contains("2.11.4"), "{error}");
}

/// What an uninstall has to refuse over, and the reason `PackageSummary` carries it.
#[tokio::test]
async fn a_package_names_the_services_that_are_instances_of_it() {
    let store = store().await;
    remember(&store, &installation("caddy", "2.11.4"), Timestamp(0)).await.expect("recorded");
    insert_service_row(&store, "caddy", "caddy", "2.11.4").await;

    let held = holders(&store, "caddy", &version("2.11.4")).await.expect("a lookup");

    assert_eq!(held, vec![ServiceId::parse("caddy").unwrap()]);
}
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p mixengine-core packages
```

Expected: FAIL — `mixengine_core::packages` does not exist.

- [ ] **Step 3: Implement the module**

Write `packages.rs` against the `packages` table as `0001_initial.sql` defines it (`name`, `version`, `install_path`, `installed_at`, `source_url`, `sha256`, `UNIQUE (name, version)`), following `runtimes.rs` line for line where the shape is the same:

- `remember` inserts with `ON CONFLICT (name, version) DO NOTHING` and turns `rows_affected() == 0` into a named error rather than letting the index raise. There is no default-version concept here, so no transaction is needed — a single insert is atomic.
- `records` selects the rows and joins `services` for each summary's `services` list. One query with a `LEFT JOIN` and a grouped read, not N+1.
- `holders` selects `services.id` by `package_id`.
- `forget` deletes the row and returns `Error::NotFound` when there was none.

Add `Error` variants beside the runtime ones — `PackageAlreadyRecorded { package, version }` — and give each a wire mapping in `mixengine-daemon/src/error.rs` next to the runtime equivalents.

Note `packages.installed_at` is ISO-8601 **text**, unlike `services.last_started_at` which is epoch milliseconds. Use `Timestamp::to_rfc3339` on write, as `runtimes::remember` does.

- [ ] **Step 4: Run the tests and prepare the offline queries**

```bash
cargo test -p mixengine-core packages
cargo sqlx prepare --workspace -- --all-targets --all-features
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS, and `.sqlx/` gains the new query files.

- [ ] **Step 5: Commit** (ask first)

```bash
git commit -am "feat(core): record an installed service package, and what holds it"
```

---

### Task 4: The wire vocabulary for `package.*`

**Files:**
- Create: `crates/mixengine-proto/src/package_api.rs`
- Modify: `crates/mixengine-proto/src/lib.rs`, `crates/mixengine-proto/src/rpc.rs`, `crates/mixengine-proto/src/service_api.rs`
- Test: `package_api.rs`'s and `service_api.rs`'s own `mod tests`

**Interfaces:**
- Consumes: `PackageVersion`, `PackageChannel` (Task 1), `ServiceId`, `Timestamp`
- Produces: the types listed in the spec's "API surface" section, plus `rpc::method::{PACKAGE_LIST, PACKAGE_LIST_AVAILABLE, PACKAGE_INSTALL, PACKAGE_UNINSTALL, SERVICE_CREATE, SERVICE_DELETE}`

- [ ] **Step 1: Write the failing tests**

```rust
/// Both halves or it does not decode, on `RuntimeTarget`'s reasoning: a package with no version is
/// not an installable thing, and a call that guessed one would be a client deciding something.
#[test]
fn a_target_names_both_halves_or_does_not_decode() {
    let target: PackageTarget =
        serde_json::from_str(r#"{"package":"caddy","version":"2.11.4"}"#).expect("both halves");
    assert_eq!(target.package, "caddy");
    assert_eq!(target.version.as_str(), "2.11.4");

    serde_json::from_str::<PackageTarget>(r#"{"package":"caddy"}"#)
        .expect_err("a package with no version is not an installable thing");
}

/// Every field has a default, so both listings are questions a person can type.
#[test]
fn a_filter_with_no_parameters_means_every_package() {
    let filter: PackageFilter = serde_json::from_str("{}").expect("every field has a default");
    assert_eq!(filter.package, None);
}

/// A create names the service and the version, and derives the package from the id — which is what
/// `ServiceId::name()` has always said it is.
#[test]
fn a_create_takes_an_id_and_a_version_and_nothing_redundant() {
    let create: ServiceCreate =
        serde_json::from_str(r#"{"id":"mariadb@main","version":"11.4.2"}"#)
            .expect("the two required fields");

    assert_eq!(create.id.name(), "mariadb");
    assert_eq!(create.id.instance(), Some("main"));
    assert_eq!(create.version.as_str(), "11.4.2");
    assert_eq!(create.port, None, "a port nobody named is the server's own default");
}

/// A delete says what it kept, because what it kept is somebody's databases.
#[test]
fn a_removal_names_the_data_it_did_not_touch() {
    let removal = ServiceRemoval {
        removed: summary("mariadb@main"),
        data_kept: Some("/home/me/.local/share/mixengine/data/mariadb/main".to_owned()),
    };

    let encoded = serde_json::to_value(&removal).unwrap();
    assert_eq!(encoded["data_kept"], "/home/me/.local/share/mixengine/data/mariadb/main");
    assert_eq!(serde_json::from_value::<ServiceRemoval>(encoded).unwrap(), removal);
}
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p mixengine-proto
```

Expected: FAIL — none of the types exist.

- [ ] **Step 3: Write the module**

Write `package_api.rs` exactly as the spec's API surface section lists, with a module doc explaining the same split `runtime_api.rs` draws (`crate::package_api` is what the methods answer; the vocabulary is elsewhere) and the D2 reasoning for two listings rather than one merged type. Derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`; use `#[serde(default, skip_serializing_if = "Option::is_none")]` on every optional field, as the neighbouring modules do.

Add the six constants to `rpc::method`, each with the doc comment saying what it takes, what it answers, and — for `PACKAGE_INSTALL` — that it answers a `JobSummary` for the same reason `RUNTIME_INSTALL` does.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p mixengine-proto
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Commit** (ask first)

```bash
git commit -am "feat(proto): what package.* asks and answers, and what a service.create takes"
```

---

### Task 5: `package.list`, `list_available`, `install`, `uninstall`

**Files:**
- Create: `crates/mixengine-daemon/src/packages.rs`, `crates/mixengine-daemon/tests/packages.rs`
- Modify: `crates/mixengine-daemon/src/main.rs` (module), `crates/mixengine-daemon/src/api/mod.rs`, `crates/mixengine-daemon/src/api/rpc.rs`
- Test: `crates/mixengine-daemon/tests/packages.rs`

**Interfaces:**
- Consumes: `core::packages` (Task 3), `package_api` types (Task 4), `Recipe::smoke_test` (Task 2), `Installer`, `Jobs`, `JobHandle`, `index::Client`
- Produces: `Packages::{list, list_available, install, uninstall}` on the daemon API struct

- [ ] **Step 1: Write the failing integration test**

`crates/mixengine-daemon/tests/packages.rs`, modelled on `tests/runtimes.rs`'s `Fixture` — read that file first and copy its shape. The package it publishes is **`fakeservice`**, because a debug build has a recipe for it and the archive can be a real executable that needs no server:

```rust
/// A version the index offers is installed, listed, and removed.
#[tokio::test]
async fn a_service_package_is_installed_listed_and_removed() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let started: JobSummary = client
        .expect(rpc::method::PACKAGE_INSTALL, json!({"package": "fakeservice", "version": VERSION}))
        .await;
    let finished = fixture.wait_for(&mut client, started.id).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");

    let list: PackageList = client.expect(rpc::method::PACKAGE_LIST, json!({})).await;
    assert_eq!(list.packages.len(), 1);
    assert_eq!(list.packages[0].package, "fakeservice");
    assert!(fixture.installed_at(VERSION).is_dir());

    let removal: PackageRemoval = client
        .expect(rpc::method::PACKAGE_UNINSTALL, json!({"package": "fakeservice", "version": VERSION}))
        .await;
    assert_eq!(removal.removed.version.as_str(), VERSION);
    assert!(!fixture.installed_at(VERSION).exists(), "the directory goes with the row");
}

/// D1: a kind this build has no recipe for is refused at install, not at create — nobody spends a
/// download on a directory MixEngine cannot use.
#[tokio::test]
async fn a_package_this_build_cannot_run_is_refused_with_what_it_can() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let error = client
        .refuse(rpc::method::PACKAGE_INSTALL, json!({"package": "redis", "version": "8.10.0"}))
        .await;

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("redis"), "{error:?}");
    assert!(error.message.contains("caddy"), "it names what exists: {error:?}");
}

/// Only what this build can run is offered, which is the same rule seen from the listing side.
#[tokio::test]
async fn the_catalogue_offers_only_kinds_with_a_recipe() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let catalogue: PackageCatalogue =
        client.expect(rpc::method::PACKAGE_LIST_AVAILABLE, json!({})).await;

    assert!(catalogue.packages.iter().all(|package| package.package != "redis"));
    assert!(!catalogue.stale, "the registry answered");
}
```

The index the fixture publishes carries a `fakeservice` entry and a `redis` entry, so the filter has something to filter.

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p mixengine-daemon --test packages
```

Expected: FAIL — method not found.

- [ ] **Step 3: Implement `Packages`**

Copy `daemon/src/runtimes.rs`'s structure: an `Arc<Self>`, a `running: Mutex<BTreeMap<(String, PackageVersion), JobId>>` so a second install of the same version is answered with the first one's job, `install` returning a `JobSummary` from `Jobs::begin`, and `perform` doing lookup → download → record in that order so a failure leaves either nothing or a directory with no row.

The differences from `runtimes.rs`, each of which needs its own comment:

- The catalogue filter is `Catalogue::builtin().recipe(package).is_some()`, and the refusal names `Catalogue::packages()` — reuse the message shape `core::generate` already produces for an unknown package rather than writing a second wording.
- The smoke test comes from the recipe (`recipe.smoke_test()`), where a runtime's comes from `core::runtimes::smoke_test(kind)`.
- `uninstall` calls `core::packages::holders` first and refuses with `ErrorCode::FailedPrecondition` naming them, before anything is deleted. Only then remove the directory and the row.

Add `packages: Arc<Packages>` to the API struct and the six-arm block to `api/rpc.rs`, following the `RUNTIME_*` arms exactly.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p mixengine-daemon --test packages
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Commit** (ask first)

```bash
git commit -am "feat(services): install a service package from the signed index (T31a)"
```

---

### Task 6: `service.create` and `service.delete`

**Files:**
- Create: `crates/mixengine-daemon/src/services/create.rs`
- Modify: `crates/mixengine-daemon/src/services/mod.rs`, `crates/mixengine-daemon/src/api/rpc.rs`
- Test: `crates/mixengine-daemon/tests/packages.rs` (extended)

**Interfaces:**
- Consumes: `ServiceCreate`, `ServiceRemoval` (Task 4), `Instancing` (Task 2), `core::packages::record` (Task 3), the existing `Registry` and `Generator`
- Produces: `Api::service_create(&ServiceCreate) -> Result<ServiceSummary, Error>` and `Api::service_delete(&ServiceId) -> Result<ServiceRemoval, Error>`

- [ ] **Step 1: Write the failing tests**

Extend `tests/packages.rs`:

```rust
/// The whole point of the task: a package becomes a service a user can start.
#[tokio::test]
async fn an_installed_package_becomes_a_service() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    let created: ServiceSummary = client
        .expect(rpc::method::SERVICE_CREATE, json!({"id": "fakeservice@main", "version": VERSION}))
        .await;
    assert_eq!(created.state, Some(ServiceState::Stopped));

    let list: ServiceList = client.expect(rpc::method::SERVICE_LIST, Value::Null).await;
    assert_eq!(list.services.len(), 1, "a created service is a declared service");

    let removal: ServiceRemoval = client
        .expect(rpc::method::SERVICE_DELETE, json!({"service": "fakeservice@main"}))
        .await;
    assert_eq!(removal.removed.id.as_str(), "fakeservice@main");
    assert!(!fixture.etc_for("fakeservice@main").exists(), "generated config is disposable");
}

/// D4, from the side that says a name is required.
#[tokio::test]
async fn a_named_instance_recipe_refuses_an_id_with_no_instance() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    let error = client
        .refuse(rpc::method::SERVICE_CREATE, json!({"id": "fakeservice", "version": VERSION}))
        .await;

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("fakeservice@"), "it shows the shape: {error:?}");
}

/// D6, and the reason the rollback matters: one row that cannot be generated fails the whole
/// declared set, so a bad row left behind would take `service.list` down with it.
#[tokio::test]
async fn a_create_that_cannot_be_rendered_leaves_the_home_as_it_was() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    let error = client
        .refuse(
            rpc::method::SERVICE_CREATE,
            json!({"id": "fakeservice@bad", "version": VERSION, "overrides": {"prot": 3307}}),
        )
        .await;
    assert_eq!(error.code, ErrorCode::InvalidArgument);

    let list: ServiceList = client.expect(rpc::method::SERVICE_LIST, Value::Null).await;
    assert!(list.services.is_empty(), "the row went with the failure: {list:?}");
}

/// D7: the configuration goes, the databases do not.
#[tokio::test]
async fn a_delete_keeps_the_data_directory_and_says_so() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    let _: ServiceSummary = client
        .expect(rpc::method::SERVICE_CREATE, json!({"id": "fakeservice@main", "version": VERSION}))
        .await;
    let removal: ServiceRemoval = client
        .expect(rpc::method::SERVICE_DELETE, json!({"service": "fakeservice@main"}))
        .await;

    let kept = removal.data_kept.expect("a data directory is named");
    assert!(kept.ends_with("main"), "{kept}");
}

/// D8, now reachable: a package something is an instance of is not one to remove.
#[tokio::test]
async fn a_package_a_service_is_an_instance_of_cannot_be_uninstalled() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    let _: ServiceSummary = client
        .expect(rpc::method::SERVICE_CREATE, json!({"id": "fakeservice@main", "version": VERSION}))
        .await;

    let error = client
        .refuse(rpc::method::PACKAGE_UNINSTALL, json!({"package": "fakeservice", "version": VERSION}))
        .await;

    assert_eq!(error.code, ErrorCode::FailedPrecondition);
    assert!(error.message.contains("fakeservice@main"), "it names what holds it: {error:?}");
}
```

Add a `refuse_a_running_service` test that starts the service first and asserts `SERVICE_DELETE` is refused with `FailedPrecondition`.

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p mixengine-daemon --test packages
```

Expected: FAIL — method not found.

- [ ] **Step 3: Implement create and delete**

`create.rs`, in this order, because each check is cheaper and more specific than the next:

1. `Catalogue::builtin().recipe(create.id.name())` — no recipe is `invalid_argument` naming what exists.
2. Instancing against the id's shape: `Single` with an `@`, or `Named` without one, is `invalid_argument` with the shape in the message.
3. `core::packages::record(store, create.id.name(), &create.version)` — not installed is `failed_precondition` with `mix package install <name> <version>` in the hint.
4. Insert the `services` row: `instance_name` is `id.instance()` for `Named` and `id.name()` for `Single`; `state` is `stopped`; the optional fields go in as they arrived. A duplicate id is `already_exists`.
5. Run the registry's generation. On any error, delete the row and remove `etc/<id>/`, then return that error — the whole reason being that T30 fails the entire declared set on one bad row.
6. Answer the `ServiceSummary` the listing would give.

`delete`:

1. Refuse when the row says running/starting/restarting, or when the registry is supervising it (`Registry::supervised`) — `failed_precondition`.
2. Read the summary and the resolved data directory **before** deleting anything.
3. Delete the row, then remove `etc/<id>/`. Leave `data/` and `logs/services/<id>/`.
4. Answer `ServiceRemoval { removed, data_kept }`.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p mixengine-daemon --test packages
cargo test --workspace
cargo sqlx prepare --workspace -- --all-targets --all-features
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Commit** (ask first)

```bash
git commit -am "feat(services): create and delete a service instance (T31a)"
```

---

### Task 7: `mix package` and `mix service create|delete`

**Files:**
- Modify: `crates/mixengine-cli/src/main.rs`, `crates/mixengine-cli/src/render.rs`
- Test: `crates/mixengine-cli/tests/package.rs` (create)

**Interfaces:**
- Consumes: the six methods
- Produces: `mix package list|available|install|uninstall`, `mix service create|delete`

- [ ] **Step 1: Write the failing test**

Model on `crates/mixengine-cli/tests/runtime.rs`. Cover: `mix package list` on an empty home prints an empty listing and exits 0 in both renderings; `mix package install` follows the job and exits non-zero on a refusal; `mix service create` prints the created service; `--json` emits exactly one object on stdout.

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p mixengine-cli --test package
```

Expected: FAIL — unknown subcommand.

- [ ] **Step 3: Add the subcommands**

`mix package install` reuses the existing `install`/`follow` job-watching helpers unchanged — the progress goes to stderr so stdout carries exactly one answer. Add render functions beside `render::job_status` for the two listings and for `ServiceRemoval`, naming the kept data directory in the human rendering because that is the sentence the user needs.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p mixengine-cli
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Commit** (ask first)

```bash
git commit -am "feat(cli): mix package, and mix service create|delete"
```

---

### Task 8: Retire half of `testkit::declare`

**Note.** This is the task with the churn worth reviewing before starting: because a service's package is its id (D3), every fixture service backed by `fakeservice` has to be named `fakeservice@…`. The suites currently call them `mariadb@main`, `php-fpm@8.3`, `kept` and `lost`. Renaming them is exactly what D3 is for — a fixture that calls itself MariaDB while running something else is the lie the rule removes — but it touches many lines in five files and makes the dependency-graph tests read less vividly. Confirm the direction before doing it.

**Files:**
- Modify: `crates/mixengine-testkit/src/declare.rs`, `crates/mixengine-testkit/src/home.rs`, `crates/mixengine-daemon/tests/{service,lifecycle,logs}.rs`, `crates/mixengine-cli/tests/{service,daemon}.rs`
- Test: the existing suites are the test

**Interfaces:**
- Consumes: `service.create` (Task 6)
- Produces: `declare` writes only the `packages` row for `fakeservice`; suites create their services through the API

- [ ] **Step 1: Delete what a real method replaced**

Remove `declare::installed` and `installed_blocking` — `caddy.rs` is their only caller and Task 9 moves it. Keep the `packages`-row half of `declare`, and rewrite its module doc: it is no longer scaffolding for a missing feature but the one thing that cannot go through the API, because `fakeservice` is a fixture no index will ever publish.

- [ ] **Step 2: Move each suite's services onto `service.create`**

Rename every fixture id to `fakeservice@<what it was>`: `mariadb@main` → `fakeservice@main`, `php-fpm@8.3` → `fakeservice@php`, `kept` → `fakeservice@kept`, `lost` → `fakeservice@lost`. Update the `depends_on` overrides with them. Where a suite wrote rows before starting the daemon, it now starts the daemon and calls `service.create` with the same overrides.

- [ ] **Step 3: Run everything**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS. A suite that goes red here is reporting a real difference between what `declare` wrote and what `service.create` writes — fix `service.create`, not the assertion, unless the assertion was about the old hand-written row itself.

- [ ] **Step 4: Commit** (ask first)

```bash
git commit -am "test(services): declare a fixture service through service.create"
```

---

### Task 9: Caddy is installed rather than unpacked, and the roadmap is corrected

**Files:**
- Modify: `crates/mixengine-cli/tests/caddy.rs`, `.claude/roadmap/phase-3-services.md`, `.claude/roadmap/todo.md`
- Test: `crates/mixengine-cli/tests/caddy.rs`

**Interfaces:**
- Consumes: everything above

- [ ] **Step 1: Install the real Caddy through the API**

Replace the hand-unpack and `declare::installed` with: publish the CI-fetched Caddy archive through `MockRegistry::publish_asset`, publish an index naming it, `package.install`, then `service.create caddy --port <n>`. The rest of the suite — validate, reload, refuse a broken override, stop — is unchanged. This covers the install path against a real artifact on all three systems at no extra cost, and it exercises the `Single` instancing rule against the recipe that has it.

- [ ] **Step 2: Run it**

```bash
cargo test -p mixengine-cli --test caddy -- --ignored
```

Expected: PASS on a machine with the pinned Caddy fetched as CI fetches it.

- [ ] **Step 3: Correct what the roadmap says**

Three separate corrections, none of them cosmetic:

1. Tick **T31a** in `phase-3-services.md` and write up what it decided — D1 (only kinds with a recipe), D3 (a service's package is its id), D4 (a recipe declares its instancing, which is the half of T36 that could not wait), D7 (a delete never touches data) — and what it deliberately left: T36 proper, `service.configure`, purging, orphan removal.
2. The index publishes **all six** service kinds already (`caddy`, `nginx`, `mariadb`, `postgres`, `redis`, `memcached`), not the two the phase file implies. T34, T35 and T37 read as though PostgreSQL, Redis, Memcached and Nginx still need packaging tasks, and T33a's note says PostgreSQL "will get an entry when packed". Correct all four.
3. `todo.md`'s phase table says Phase 3 is `3 / 11`; with T30, T30a, T31, T33a and now T31a it is `5 / 12`. Recount and update the "Where we are" section, keeping it under a screen as that file's own rule requires.

- [ ] **Step 4: Commit** (ask first)

```bash
git commit -am "docs(roadmap): tick T31a, and correct what the index already publishes"
```

---

## Self-Review

**Spec coverage.** D1 → Task 5 (install refusal, catalogue filter). D2 → Tasks 4, 5. D3 → Tasks 4, 6, and the churn in Task 8. D4 → Tasks 2, 6. D5 → Tasks 2, 5. D6 → Task 6. D7 → Task 6. D8 → Tasks 3, 5, 6. D9 → Task 1. API surface → Task 4. Crate changes → Tasks 1–8. Testing → Tasks 3, 5, 6, 7, 9. Out-of-scope items appear in no task, which is the point.

**Type consistency.** `PackageVersion` from Task 1 is used in Tasks 3, 4, 5, 6. `Instancing` from Task 2 is used in Tasks 2 (generate) and 6 (create). `core::packages::{record, records, holders, remember, forget}` defined in Task 3 are the names called in Tasks 5 and 6. `PackageSummary.services` from Task 4 is filled by `holders` from Task 3 and refused over in Task 5.

**Known gap, deliberately left to the executor.** The exact helper names inside each existing test module (`TempHome`, `generator`, `declare_row`, `Fixture::client`, `Client::expect`/`refuse`) are written here as they appear in the suites they come from, but each task says to read the surrounding module and follow it rather than trusting these spellings.
