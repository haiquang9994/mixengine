# T32 — php-fpm pools: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every installed PHP a supervised FastCGI pool — `php-fpm@<version>` — created by the runtime install itself, configured from one set of overrides on all three systems, and reloaded by `SIGUSR2` where a signal exists.

**Architecture:** `services` grows a second, typed parent so a row can point at `runtime_installs` instead of `packages`, and the generator joins whichever column is filled. One `Recipe` renders two spec shapes — php-fpm reading a rendered `php-fpm.conf` on Unix, `php-cgi.exe -b` reading `PHP_FCGI_CHILDREN` on Windows — with the program looked up in the artifact's own `provides` map so no recipe carries a platform conditional. `ReloadBehaviour` gains a `Signal` variant, `mixengine-platform` gains one function to send it, and Windows answers `Unsupported` the way it already does for `ask_to_stop`.

**Tech Stack:** Rust 2024, sqlx + SQLite (`STRICT`, compile-time-checked queries with an `.sqlx` cache), minijinja templates, tokio, `libc` on Unix only inside `mixengine-platform`.

**Spec:** [docs/superpowers/specs/2026-08-19-t32-php-fpm-pools-design.md](../specs/2026-08-19-t32-php-fpm-pools-design.md)

## Global Constraints

- **Cross-platform or not merged.** Every task must compile on Windows, macOS and Linux. No `#[cfg]` in `mixengine-core` or `mixengine-daemon`; unsupported paths return a typed `Unsupported` error, never `todo!()`.
- **No direct OS calls outside `mixengine-platform`.** The signal lives there and nowhere else.
- **Workspace layering** (`crates/mixengine-proto/tests/workspace_layering.rs`): `mixengine-platform` currently depends on **no** workspace crate. Do not add an edge to `mixengine-proto` — the daemon maps `mixengine_proto::ReloadSignal` onto a platform-local enum.
- **Generated config is disposable.** Nothing parses a file under `etc/` back into state.
- **`0001_initial.sql` is edited, not migrated.** Nothing has shipped. `sqlx::migrate!` checksums migrations, so **every existing development home must be deleted** after Task 1; CI builds one from nothing each run.
- **After editing any `sqlx::query!`**, run `cargo sqlx prepare --workspace -- --all-targets --all-features` and commit the `.sqlx/` changes. Offline CI fails without it.
- **Gates before every commit:** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- **Instance ids use the full version**: `php-fpm@8.3.33`, never `php-fpm@8.3`.
- **One pool per PHP version**, shared by every site. No `pm = dynamic`, no `pm = ondemand`, no `pm.status_path`, no slowlog, no `request_terminate_timeout` on Windows.
- **Doc comments are prose that gives reasons**, in English, matching the surrounding files. `missing_docs` and `rustdoc::all` are denied workspace-wide.

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/mixengine-core/migrations/0001_initial.sql` | `services` gains `runtime_install_id`, loses `NOT NULL` on `package_id`, gains the `CHECK` and a second `UNIQUE` |
| `crates/mixengine-core/src/services.rs` | `Origin`, and `create` resolving whichever parent it was given |
| `crates/mixengine-core/src/services/pools.rs` | **new** — the idempotent hook that gives every installed PHP its pool row, and the port it allocates on Windows |
| `crates/mixengine-core/src/generate.rs` | the two declared-set queries join both parents; `Parent` resolution feeds `Context` |
| `crates/mixengine-core/src/generate/recipe.rs` | `Context::provides`/`provided`, `Recipe::source`, `Source` |
| `crates/mixengine-core/src/generate/recipes/php_fpm.rs` | **new** — the recipe |
| `crates/mixengine-core/src/generate/recipes/php_fpm/php-fpm.conf` | **new** — the pool template, rendered everywhere and read by php-fpm on the two systems that have one |
| `crates/mixengine-core/src/generate/recipes.rs` | php-fpm joins the module list |
| `crates/mixengine-core/src/lib.rs` | one new `Error` variant, `ServiceProvidesNothing` |
| `crates/mixengine-proto/src/service.rs` | `ReloadBehaviour::Signal`, `ReloadSignal`, and `validate` |
| `crates/mixengine-platform/src/process.rs` | `CAN_SIGNAL`, `Signal`, `Supervised::signal` |
| `crates/mixengine-platform/src/{unix,windows}/process.rs` | the per-OS half of both |
| `crates/mixengine-daemon/src/services/runner.rs` | `reload` honours a signal |
| `crates/mixengine-daemon/src/api/create.rs` | `service.create` refuses a runtime-backed recipe |
| `crates/mixengine-daemon/src/runtimes.rs` | the pool is created after an install and removed by an uninstall that is allowed |
| `crates/mixengine-daemon/src/main.rs` | the hook runs at boot |
| `crates/mixengine-testkit/src/fastcgi.rs` | **new** — a minimal FastCGI responder client |
| `crates/mixengine-testkit/src/package.rs` | `FakePackage::directory`, to pack a whole unpacked runtime |
| `crates/mixengine-testkit/src/declare.rs` | `rebind`, so a suite can move a pool off a port it did not choose |
| `crates/mixengine-testkit/tests/supervision.rs` | what a signal reaches, and what Windows says instead |
| `crates/mixengine-daemon/tests/runtimes.rs` | an install creates a pool, and `service.create` refuses one |
| `crates/mixengine-cli/tests/php_fpm.rs` | **new** — the `#[ignore]`d suite against a real PHP |
| `.github/workflows/ci.yml` | fetch a pinned PHP, run that suite |

---

### Task 1: `services` gets a second, typed parent

**Files:**
- Modify: `crates/mixengine-core/migrations/0001_initial.sql:70-101`
- Modify: `crates/mixengine-core/src/services.rs:88-210` (`Declaration`, `create`) and its `mod tests`
- Modify: `.sqlx/` (regenerated)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `mixengine_core::services::Origin` with variants `Package { name: String, version: PackageVersion }` and `Runtime { kind: RuntimeKind, version: PackageVersion }`; `Declaration` with field `origin: Origin` replacing `package: String` and `version: PackageVersion`. Every other `Declaration` field is unchanged.

- [ ] **Step 1: Edit the `services` table**

In `crates/mixengine-core/migrations/0001_initial.sql`, replace the `package_id` line and the trailing `UNIQUE` of `CREATE TABLE services` with:

```sql
    -- RESTRICT, not CASCADE: uninstalling a package while an instance still refers to it is a
    -- mistake to report, not one to carry out. The instance owns data_dir.
    --
    -- **Two possible parents, and exactly one of them set** — roadmap task T32. Every service up to
    -- php-fpm came out of a `packages` row; php-fpm comes out of a `runtime_installs` one, because
    -- the process that serves a user's sites lives inside the PHP they installed with
    -- `runtime.install`. Giving it a `packages` row as well would be a second table describing one
    -- directory, with `package.uninstall` able to see and delete it and an `install_path` that goes
    -- stale the moment the runtime is removed. The foreign key here is also what gives
    -- `runtime.uninstall` its refusal for nothing.
    package_id            INTEGER REFERENCES packages (id) ON DELETE RESTRICT,
    runtime_install_id    INTEGER REFERENCES runtime_installs (id) ON DELETE RESTRICT,
```

and, after `pid_start_time`:

```sql
    -- One parent or the other, never both and never neither. `(x IS NULL)` is 0 or 1 in SQLite, so
    -- `<>` over the pair is exclusive-or spelled in what this database has.
    CHECK ((package_id IS NULL) <> (runtime_install_id IS NULL)),

    -- Two constraints rather than one over a coalesced column: SQLite treats NULLs as distinct in a
    -- UNIQUE, so each of these only ever sees the rows whose parent it names, and the other kind's
    -- rows are invisible to it rather than colliding on a shared NULL.
    UNIQUE (package_id, instance_name),
    UNIQUE (runtime_install_id, instance_name)
) STRICT;
```

- [ ] **Step 2: Write the failing tests**

Add to `crates/mixengine-core/src/services.rs`'s `mod tests`. The module already has `store() -> (tempfile::TempDir, Store)` and `service_row(&Store, &str, ServiceState)`; these two tests need a bare parent row rather than a whole service, so they write their own inserts:

```rust
    /// A row whose binary comes from an installed runtime rather than from a package.
    ///
    /// The whole of T32's schema change seen from the only place that writes it: `create` resolves
    /// `runtime_installs` instead of `packages`, and the row that lands names one parent.
    #[tokio::test]
    async fn a_service_can_come_from_a_runtime_install() {
        let (_home, store) = store().await;

        sqlx::query(
            "INSERT INTO runtime_installs
                 (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256)
             VALUES ('php', '8.3.33', 'stable', '/runtimes/php/8.3.33', '2026-08-19T00:00:00Z',
                     1, 'https://example.invalid/php', 'abc')",
        )
        .execute(store.pool())
        .await
        .expect("a runtime install");

        let service = ServiceId::parse("php-fpm@8.3.33").expect("a valid id");

        create(
            &store,
            &Declaration {
                service: service.clone(),
                origin: Origin::Runtime {
                    kind: mixengine_proto::RuntimeKind::Php,
                    version: PackageVersion::parse("8.3.33").expect("a version"),
                },
                instance_name: "8.3.33".to_owned(),
                port: None,
                bind_addr: None,
                data_dir: None,
                autostart: false,
                overrides: "{}".to_owned(),
            },
        )
        .await
        .expect("a pool for an installed PHP");

        let (package_id, runtime_install_id): (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT package_id, runtime_install_id FROM services WHERE id = 'php-fpm@8.3.33'",
        )
        .fetch_one(store.pool())
        .await
        .expect("the row that was written");

        assert_eq!(package_id, None, "a pool has no package to point at");
        assert!(
            runtime_install_id.is_some(),
            "the row points at the PHP it runs out of"
        );
    }

    /// The `CHECK` is the whole guarantee that `Origin` is not a suggestion.
    ///
    /// Written through raw SQL rather than through `create`, because `create` cannot express either
    /// of these — which is the point: what is being asserted is that a hand-edited database, or a
    /// future writer nobody has written yet, cannot express them either.
    #[tokio::test]
    async fn a_row_names_one_parent_and_not_two_and_not_none() {
        let (_home, store) = store().await;

        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES ('caddy', '2.11.4', '/packages/caddy', '2026-08-19T00:00:00Z',
                     'https://example.invalid/caddy', 'ab')",
        )
        .execute(store.pool())
        .await
        .expect("a package");

        sqlx::query(
            "INSERT INTO runtime_installs
                 (kind, version, channel, install_path, installed_at, size_bytes, source_url, sha256)
             VALUES ('php', '8.3.33', 'stable', '/runtimes/php/8.3.33', '2026-08-19T00:00:00Z',
                     1, 'https://example.invalid/php', 'abc')",
        )
        .execute(store.pool())
        .await
        .expect("a runtime install");

        let both = sqlx::query(
            "INSERT INTO services (id, package_id, runtime_install_id, instance_name, state)
             VALUES ('both', (SELECT id FROM packages LIMIT 1),
                     (SELECT id FROM runtime_installs LIMIT 1), 'both', 'stopped')",
        )
        .execute(store.pool())
        .await;
        assert!(both.is_err(), "a service with two parents was accepted");

        let neither = sqlx::query(
            "INSERT INTO services (id, instance_name, state) VALUES ('orphan', 'orphan', 'stopped')",
        )
        .execute(store.pool())
        .await;
        assert!(neither.is_err(), "a service with no parent was accepted");
    }
```

`declare::package` above stands for whatever the module's existing tests already use to put a `packages` row in place — reuse it; if the module inserts the row inline in `service_row`, copy that insert rather than adding a helper.

- [ ] **Step 3: Run them and watch them fail**

Run: `cargo test -p mixengine-core services::tests -- a_service_can_come_from_a_runtime_install a_row_names_one_parent`
Expected: FAIL — `Origin` is not defined, and the `CHECK` test fails because both inserts succeed.

- [ ] **Step 4: Add `Origin` and teach `create` to resolve either parent**

In `crates/mixengine-core/src/services.rs`, add above `Declaration`:

```rust
/// Where the binary a service runs comes from.
///
/// **Two tables, one of which is not a package** — roadmap task T32. Everything up to php-fpm was
/// installed from the index by `package.install` and has a `packages` row; a pool has no such row
/// and must not be given a fake one, because the directory it runs out of belongs to
/// `runtime.install` and is removed by `runtime.uninstall`. Which one a service names is what the
/// `CHECK` on `services` enforces, and this enum is that constraint said in Rust so a caller cannot
/// even assemble the row the database would refuse.
#[derive(Debug, Clone)]
pub enum Origin {
    /// A `packages` row: Caddy, MariaDB, Redis — anything the signed index publishes as a server.
    Package {
        /// `packages.name`, as the caller resolved it from the id.
        ///
        /// Passed rather than read off the id, because the caller has already held it to the
        /// catalogue and this is that answer rather than a second derivation of it.
        name: String,

        /// Which installed version of that package to run.
        version: PackageVersion,
    },

    /// A `runtime_installs` row: php-fpm, whose process lives inside an installed PHP.
    Runtime {
        /// Which language.
        kind: RuntimeKind,

        /// Which installed version of it, in full — `8.3.33` and not `8.3`, because
        /// `runtime_installs` is `UNIQUE (kind, version)` over the full version and two patch
        /// releases of one minor can both be installed.
        version: PackageVersion,
    },
}
```

Replace `Declaration`'s `package` and `version` fields with:

```rust
    /// Which table supplies the binary, and which row in it.
    pub origin: Origin,
```

and rewrite the head of `create`:

```rust
    let Declaration {
        service,
        origin,
        instance_name,
        port,
        bind_addr,
        data_dir,
        autostart,
        overrides,
    } = declaration;

    let id = service.as_str();
    let port_column = port.map(i64::from);
    let autostart_column = i64::from(*autostart);

    // Checked here as well as by the caller, because the alternative is a constraint violation
    // whose message names a column: the row and the lookup are one statement otherwise, and a
    // subquery that found nothing is not a failure SQLite explains.
    let (package_id, runtime_install_id) = match origin {
        Origin::Package { name, version } => {
            let version = version.as_str();

            let found: Option<i64> = sqlx::query_scalar!(
                "SELECT id FROM packages WHERE name = ? AND version = ?",
                name,
                version
            )
            .fetch_optional(store.pool())
            .await
            .map_err(|source| store.failure("read", source))?;

            let found = found.ok_or_else(|| Error::NotFound {
                kind: "package",
                id: format!("{name} {version}"),
            })?;

            (Some(found), None)
        }

        Origin::Runtime { kind, version } => {
            let (kind_column, version_column) = (kind.as_str(), version.as_str());

            let found: Option<i64> = sqlx::query_scalar!(
                "SELECT id FROM runtime_installs WHERE kind = ? AND version = ?",
                kind_column,
                version_column
            )
            .fetch_optional(store.pool())
            .await
            .map_err(|source| store.failure("read", source))?;

            let found = found.ok_or_else(|| Error::NotFound {
                kind: "runtime",
                id: format!("{kind_column} {version_column}"),
            })?;

            (None, Some(found))
        }
    };

    let written = sqlx::query!(
        "INSERT INTO services
             (id, package_id, runtime_install_id, instance_name, state, autostart, port, bind_addr,
              data_dir, config_overrides_json)
         VALUES (?, ?, ?, ?, 'stopped', ?, ?, COALESCE(?, '127.0.0.1'), ?, ?)
         ON CONFLICT DO NOTHING",
        id,
        package_id,
        runtime_install_id,
        instance_name,
        autostart_column,
        port_column,
        bind_addr,
        data_dir,
        overrides
    )
    .execute(store.pool())
    .await
    .map_err(|source| store.failure("write", source))?;
```

Change the `tracing::info!` below it to name the origin rather than a package:

```rust
    tracing::info!(%id, origin = ?origin, "a service was created");
```

Add `RuntimeKind` to the `mixengine_proto` import list at the top of the file, and update the module note at `services.rs:11` to say that a row now has two possible parents and why.

- [ ] **Step 5: Fix the one existing caller**

`crates/mixengine-daemon/src/api/create.rs:123` builds a `Declaration`. Change the two fields to:

```rust
                origin: mixengine_core::services::Origin::Package {
                    name: package.to_owned(),
                    version: create.version.clone(),
                },
```

- [ ] **Step 6: Regenerate the query cache and run everything**

```bash
cargo sqlx prepare --workspace -- --all-targets --all-features
cargo test -p mixengine-core
cargo test --workspace
```
Expected: PASS. If a suite fails because an existing development home has an old migration checksum, delete that home — this is the cost stated in the design.

- [ ] **Step 7: Commit**

```bash
git add crates/mixengine-core/migrations/0001_initial.sql crates/mixengine-core/src/services.rs \
        crates/mixengine-daemon/src/api/create.rs .sqlx
git commit -m "feat(services): let a service name a runtime install as its parent (T32)"
```

---

### Task 2: The generator reads whichever parent is filled

**Files:**
- Modify: `crates/mixengine-core/src/generate.rs:88-100` (`Row`), `:135-160` and `:178-195` (both queries), `:206-300` (`render`)
- Modify: `crates/mixengine-core/src/generate/recipe.rs` (`Context`, `Context::for_test`)
- Modify: `crates/mixengine-core/src/lib.rs` (one new `Error` variant)
- Test: `crates/mixengine-core/src/generate.rs`'s `mod tests`

**Interfaces:**
- Consumes: `services::Origin` and the two-parent schema from Task 1.
- Produces: `Context::provided(&self, name: &str) -> Result<PathBuf>`; `Context::for_test` gains a `provides: BTreeMap<String, String>` parameter, fourth of six, immediately after `root`; `Error::ServiceProvidesNothing { service: String, executable: String, known: Vec<String> }`.

- [ ] **Step 1: Write the failing test**

Add to `crates/mixengine-core/src/generate.rs`'s `mod tests`. It reuses the `Fake` recipe already there and lays the home out the way `home_of` does, but declares the service against a `runtime_installs` row instead of a `packages` one — so it builds its own fixture rather than taking `home_of`'s:

```rust
    /// A service whose binary comes from an installed runtime renders exactly like one whose binary
    /// comes from a package.
    ///
    /// What is being asserted is the join and nothing else: the recipe is the same `Fake`, the
    /// context it receives carries the runtime's version and install path, and **the name the recipe
    /// was found under is the id's own** — a pool is `php-fpm@8.3.33` and the row beneath it says
    /// `php`, which is the one place those two differ.
    #[tokio::test]
    async fn a_runtime_backed_row_renders_from_the_runtime_it_names() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let paths = Paths::new(directory.path().to_path_buf(), &PathOverrides::default());
        let store = Store::open(paths.database_file())
            .await
            .expect("a database");

        sqlx::query(
            "INSERT INTO runtime_installs
                 (kind, version, channel, install_path, installed_at, size_bytes, source_url,
                  sha256, provides_json)
             VALUES ('php', '8.3.33', 'stable', '/runtimes/php/8.3.33', '2026-08-19T00:00:00Z',
                     1, 'https://example.invalid/php', 'abc', '{\"php-fpm\":\"sbin/php-fpm\"}')",
        )
        .execute(store.pool())
        .await
        .expect("a runtime install");

        sqlx::query(
            "INSERT INTO services (id, runtime_install_id, instance_name, state, port)
             VALUES ('fakeservice@8.3.33', (SELECT id FROM runtime_installs LIMIT 1),
                     '8.3.33', 'stopped', 9000)",
        )
        .execute(store.pool())
        .await
        .expect("a service over it");

        let generator = Generator::new(
            paths.clone(),
            store,
            Catalogue::default().with(Arc::new(Fake)),
        );

        let generated = generator.declared().await.expect("one rendered service");

        assert_eq!(
            generated.len(),
            1,
            "a row with no packages parent was dropped by the join"
        );
        assert_eq!(generated[0].spec.id().as_str(), "fakeservice@8.3.33");

        let rendered = std::fs::read_to_string(
            paths
                .etc()
                .join("fakeservice@8.3.33")
                .join("fakeservice.conf"),
        )
        .expect("the rendered file");

        assert!(rendered.contains("port = 9000"), "{rendered}");
    }
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p mixengine-core generate::tests::a_runtime_backed_row_renders`
Expected: FAIL — the `JOIN packages` drops the row, so `declared()` returns an empty vector.

- [ ] **Step 3: Add the error variant**

In `crates/mixengine-core/src/lib.rs`, beside `RuntimeProvidesNothing`:

```rust
    /// A recipe asked the install behind its service for an executable it does not publish.
    ///
    /// Distinct from [`Error::RuntimeProvidesNothing`], which is the shim's question — *which file
    /// is `php`* — asked of a runtime by kind and version. This one is asked by a **recipe**, of
    /// whatever the service's row points at, and it names the service because that is what the
    /// person reading has in their hand. The usual cause is an artifact packed without the SAPI the
    /// recipe needs: a PHP whose `provides` has `php` and no `php-fpm`.
    #[error(
        "{service} runs out of an install that publishes no executable called {executable} (it has: {})",
        if known.is_empty() { "nothing recorded".to_owned() } else { known.join(", ") }
    )]
    ServiceProvidesNothing {
        /// Which service.
        service: String,
        /// The name the recipe looked up.
        executable: String,
        /// What the install does publish, in the order a listing shows them.
        known: Vec<String>,
    },
```

- [ ] **Step 4: Widen `Row` and both queries**

In `crates/mixengine-core/src/generate.rs`, replace the `Row` struct with:

```rust
/// One `services` row joined to **both** of the tables a parent could be in.
///
/// A struct rather than the query's own anonymous record, because two call sites read it and
/// `sqlx::query!` gives each of them a different type.
///
/// Six nullable columns rather than three, because the join that did not match contributes nulls
/// and the `CHECK` on `services` guarantees exactly one of the two triples is whole. Resolving that
/// into one answer is [`Parent::of`], which is also where a row that matched neither is refused.
#[derive(Debug)]
struct Row {
    id: String,
    instance_name: String,
    port: Option<i64>,
    bind_addr: String,
    data_dir: Option<String>,
    overrides: String,
    limits: String,
    package: Option<String>,
    package_version: Option<String>,
    package_path: Option<String>,
    runtime: Option<String>,
    runtime_version: Option<String>,
    runtime_path: Option<String>,
    runtime_provides: Option<String>,
}
```

and both `SELECT`s (in `declared` and in `generate`) with this projection — only the trailing `ORDER BY s.id` / `WHERE s.id = ?` differs between them:

```sql
SELECT s.id                    AS "id!: String",
       s.instance_name         AS "instance_name!: String",
       s.port                  AS "port: i64",
       s.bind_addr             AS "bind_addr!: String",
       s.data_dir              AS "data_dir: String",
       s.config_overrides_json AS "overrides!: String",
       s.limits_json           AS "limits!: String",
       p.name                  AS "package: String",
       p.version               AS "package_version: String",
       p.install_path          AS "package_path: String",
       r.kind                  AS "runtime: String",
       r.version               AS "runtime_version: String",
       r.install_path          AS "runtime_path: String",
       r.provides_json         AS "runtime_provides: String"
FROM services s
LEFT JOIN packages p         ON p.id = s.package_id
LEFT JOIN runtime_installs r ON r.id = s.runtime_install_id
```

- [ ] **Step 5: Resolve the two halves into one origin**

Add to `crates/mixengine-core/src/generate.rs`, above `impl Generator`:

```rust
/// Which install supplies the binary behind one service, resolved from the two halves of a [`Row`].
///
/// `Parent` and not `Origin`: the public [`services::Origin`](crate::services::Origin) is what a
/// caller *asks* for and this is what a row *has*, and `recipe.rs` already has a private `Origin` of
/// its own for the `package` half of a rendering.
#[derive(Debug)]
struct Parent {
    /// The name the recipe is found under, which is also what `data/<package>` is named after.
    package: String,

    /// The installed version, as upstream writes it.
    version: String,

    /// Where that install is unpacked.
    install_path: String,

    /// What it calls its executables, and where each one is inside the directory.
    provides: BTreeMap<String, String>,
}

impl Parent {
    /// Read a row's parent, whichever of the two it has.
    ///
    /// **The recipe's name for a runtime is the id's own half**, and that is the one asymmetry worth
    /// stating: a `packages` row names itself `caddy` and the service is `caddy`, while a
    /// `runtime_installs` row names itself `php` and the service is `php-fpm@8.3.33`. What finds a
    /// recipe is `ServiceId::name()` either way — the rule `recipe.rs` already states — so a pool
    /// takes its name from the id and the runtime's kind stops here.
    ///
    /// **A package publishes no `provides` map**, and the empty one is honest rather than a
    /// placeholder: the `packages` table records none, because a package is published as one server
    /// whose name is the package's own and [`Context::program`] is enough to find it. A runtime is
    /// the case where that is not true — see [`Context::provided`].
    fn of(row: &mut Row, service: &ServiceId) -> Result<Self> {
        let unreadable = |value: &str| Error::UnreadableServiceRow {
            service: service.as_str().to_owned(),
            column: "package_id",
            value: value.to_owned(),
        };

        match (
            row.package.take(),
            row.package_version.take(),
            row.package_path.take(),
        ) {
            (Some(package), Some(version), Some(install_path)) => {
                return Ok(Self {
                    package,
                    version,
                    install_path,
                    provides: BTreeMap::new(),
                });
            }
            (None, None, None) => {}
            _ => return Err(unreadable("a packages row that is only half there")),
        }

        match (
            row.runtime.take(),
            row.runtime_version.take(),
            row.runtime_path.take(),
            row.runtime_provides.take(),
        ) {
            (Some(_kind), Some(version), Some(install_path), Some(provides)) => {
                let provides = serde_json::from_str(&provides).map_err(|source| {
                    Error::UnreadableServiceDocument {
                        service: service.as_str().to_owned(),
                        column: "provides_json",
                        source,
                    }
                })?;

                Ok(Self {
                    package: service.name().to_owned(),
                    version,
                    install_path,
                    provides,
                })
            }

            // The `CHECK` on `services` makes this unreachable through the database's own rules, so
            // reaching it means a row somebody wrote by hand or a runtime removed out from under
            // one. Named rather than defaulted, because a service silently rendered against no
            // install is a service that fails much later and somewhere else.
            _ => Err(unreadable("neither a package nor a runtime install")),
        }
    }
}
```

Add `use std::collections::BTreeMap;` to the file's imports.

- [ ] **Step 6: Feed it into `render`**

In `render`, replace the recipe lookup and the `Context` construction so they read from the origin. The changed lines:

```rust
    async fn render(&self, mut row: Row) -> Result<Generated> {
        let service =
            ServiceId::parse(row.id.clone()).map_err(|source| Error::UnreadableServiceRow {
                service: row.id.clone(),
                column: "id",
                value: source.to_string(),
            })?;

        let parent = Parent::of(&mut row, &service)?;

        let recipe = self
            .catalogue
            .recipe(&parent.package)
            .ok_or_else(|| Error::NoRecipe {
                service: row.id.clone(),
                package: parent.package.clone(),
                known: self.catalogue.packages().map(str::to_owned).collect(),
            })?
            .clone();
```

and, inside the `Context { … }` literal, replace the three origin fields and add the fourth:

```rust
            data: row.data_dir.map_or_else(
                || match recipe.instancing() {
                    Instancing::Single => self.paths.data().join(&parent.package),
                    Instancing::Named => self
                        .paths
                        .data()
                        .join(&parent.package)
                        .join(&row.instance_name),
                },
                PathBuf::from,
            ),
            …
            package: parent.package,
            version: parent.version,
            install_path: PathBuf::from(parent.install_path),
            provides: parent.provides,
```

Change `render`'s signature to take `mut row: Row`, and leave everything after the `Context` alone.

- [ ] **Step 7: Give `Context` the map and the lookup**

In `crates/mixengine-core/src/generate/recipe.rs`, add a field to `Context` after `install_path`:

```rust
    /// What that install calls its executables, and where each one is inside the directory.
    ///
    /// `runtime_installs.provides_json`, and **empty for a service that came from a `packages`
    /// row** — see [`Context::provided`], which is the only thing that reads it.
    pub(super) provides: BTreeMap<String, String>,
```

and the accessor:

```rust
    /// The executable this install publishes under `name`, wherever the publisher put it.
    ///
    /// [`program`](Self::program) is the other half of the pair and the right one for a package: it
    /// joins a name to the install path and lets this OS spell the suffix, which works because
    /// `mixengine-packages` publishes a server as one executable named after its package. **A
    /// runtime is the case where that is not true.** `php-fpm` is `sbin/php-fpm` inside a Unix
    /// build and does not exist at all inside a Windows one, where the same job is done by
    /// `php-cgi.exe` at the root — so a recipe that wrote either path down would be right on one
    /// system and wrong on the other. This looks the name up in the index's own answer, and the
    /// recorded value already carries whatever suffix it needs.
    ///
    /// # Errors
    ///
    /// [`Error::ServiceProvidesNothing`], naming the service and listing what the install does
    /// publish — which is the whole of what somebody looking at a PHP packed without a SAPI needs.
    pub fn provided(&self, name: &str) -> Result<PathBuf> {
        self.provides
            .get(name)
            .map(|relative| self.install_path.join(relative))
            .ok_or_else(|| Error::ServiceProvidesNothing {
                service: self.service.as_str().to_owned(),
                executable: name.to_owned(),
                known: self.provides.keys().cloned().collect(),
            })
    }
```

Add `use std::collections::BTreeMap;` if it is not already imported (it is, for `Catalogue`), and give `Context::for_test` a `provides: BTreeMap<String, String>` parameter placed after `root`, defaulting nothing — every existing call site passes `BTreeMap::new()`.

- [ ] **Step 8: Update every `Context::for_test` call site**

Run: `cargo test -p mixengine-core --no-run` and fix each error by adding `BTreeMap::new()` (the Caddy tests in `recipes/caddy.rs`, and the generator's own).

- [ ] **Step 9: Regenerate the cache and run the tests**

```bash
cargo sqlx prepare --workspace -- --all-targets --all-features
cargo test -p mixengine-core
```
Expected: PASS, including the new test from Step 1.

- [ ] **Step 10: Commit**

```bash
git add crates/mixengine-core .sqlx
git commit -m "feat(generate): render a service from either of its two possible parents (T32)"
```

---

### Task 3: `ReloadBehaviour::Signal` in the wire vocabulary

**Files:**
- Modify: `crates/mixengine-proto/src/service.rs:583-628` (`ReloadBehaviour`), `:1045-1060` (`validate`)
- Modify: `crates/mixengine-proto/src/lib.rs:49` (the re-export list)
- Test: `crates/mixengine-proto/src/service.rs`'s `mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `mixengine_proto::ReloadBehaviour::Signal { signal: ReloadSignal, patience: Millis }` and `mixengine_proto::ReloadSignal` with variants `Hup`, `Usr1`, `Usr2`, all re-exported from the crate root.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` in `crates/mixengine-proto/src/service.rs`, which already has a `spec() -> ServiceSpecBuilder` helper returning a MariaDB builder with a `cwd` and a `ready`:

```rust
    /// A reload by signal is a spec like any other, and its patience is checked like a command's.
    ///
    /// Zero is not "wait as long as it takes" and not "do not wait": it is a window closed before
    /// anything could have happened in it, which would report every reload as not done while every
    /// reload was in fact happening.
    #[test]
    fn a_signal_reload_needs_a_window_to_happen_in() {
        let built = spec()
            .reload(ReloadBehaviour::Signal {
                signal: ReloadSignal::Usr2,
                patience: Millis(0),
            })
            .build();

        assert!(
            matches!(built, Err(SpecError::Invalid { field, .. }) if field == "reload"),
            "{built:?}"
        );

        spec()
            .reload(ReloadBehaviour::Signal {
                signal: ReloadSignal::Usr2,
                patience: Millis(5_000),
            })
            .build()
            .expect("a signal and a window to send it in");
    }
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p mixengine-proto a_signal_reload_needs_a_window`
Expected: FAIL — `ReloadBehaviour::Signal` and `ReloadSignal` do not exist.

- [ ] **Step 3: Add the variant and the enum**

In `crates/mixengine-proto/src/service.rs`, add to `ReloadBehaviour` after `Command`:

```rust
    /// Send a signal to the running process — php-fpm's `SIGUSR2` — roadmap task **T32**.
    ///
    /// **There is nothing to run.** php-fpm's reload is a signal to the master, which finishes the
    /// requests its workers are serving and replaces them with workers that read the new file; the
    /// daemon already holds the pid it goes to. A `Command` variant spelled with a `kill` would be a
    /// program looked up on a `PATH` to do something this process can do directly, and would not
    /// exist on Windows at all.
    ///
    /// **Unavailable on Windows**, where there is no signal a daemon can send a process it gave no
    /// console to — `.claude/decisions/0008-no-signal-stop-on-windows.md`. A recipe there returns no
    /// reload at all rather than this, so nothing is ever asked for and then refused: the supervisor
    /// says once, in `daemon.log`, that the running process is still on its previous configuration.
    Signal {
        /// Which signal.
        signal: ReloadSignal,

        /// How long the daemon treats the reload as in progress before it stops waiting.
        ///
        /// Not a grace period: nothing is killed when it expires, and unlike a command there is no
        /// exit status to read either way. What it bounds is the window in which a second reload is
        /// not sent on top of the first.
        patience: Millis,
    },
```

and after the enum:

```rust
/// Which signal a [`ReloadBehaviour::Signal`] sends.
///
/// **A closed list and not a number.** This crate is the wire vocabulary, is compiled for three
/// systems and must not leak `libc` — the same reason [`StopBehaviour::Signal`] names no signal
/// either. Three variants because three are what the servers MixEngine runs use: `SIGHUP` almost
/// everywhere, `SIGUSR1` for a log reopen, `SIGUSR2` for php-fpm's graceful pool restart. A fourth
/// is a variant added the day a recipe needs it, which is a decision somebody makes — where an
/// `i32` would be a number a recipe could invent and the platform layer would have to trust.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReloadSignal {
    /// `SIGHUP` — re-read the configuration, the convention almost every daemon follows.
    Hup,

    /// `SIGUSR1` — reopen log files, without touching anything else.
    Usr1,

    /// `SIGUSR2` — php-fpm's graceful pool restart: the workers finish what they are serving and
    /// their replacements read the new file.
    Usr2,
}
```

Delete the paragraph in `ReloadBehaviour`'s doc comment that begins "One variant, and the second is deliberately not written yet" and replace it with one sentence saying there are two and that Windows has neither of the signals.

- [ ] **Step 4: Extend `validate`**

Replace the `if let Some(ReloadBehaviour::Command { … })` block at `service.rs:1045` with:

```rust
        // Said once because two variants carry it: zero is a window closed before anything could
        // have happened in it, which would report every reload as not done while every reload was in
        // fact happening.
        const ZERO_PATIENCE: &str =
            "it is given no time at all, so it could only ever be abandoned";

        match &self.reload {
            Some(ReloadBehaviour::Command {
                program, patience, ..
            }) => {
                check_program(&self.id, "reload", program)?;

                if patience.is_zero() {
                    return Err(invalid("reload", ZERO_PATIENCE.to_owned()));
                }
            }

            Some(ReloadBehaviour::Signal { patience, .. }) => {
                if patience.is_zero() {
                    return Err(invalid("reload", ZERO_PATIENCE.to_owned()));
                }
            }

            // `#[non_exhaustive]` is this crate's own promise to its consumers and does not bind a
            // match written here, so a third variant added later fails to compile at this line —
            // which is the reminder it should be.
            None => {}
        }
```

- [ ] **Step 5: Re-export it**

In `crates/mixengine-proto/src/lib.rs:49`, add `ReloadSignal` to the `pub use service::{…}` list, keeping alphabetical order.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p mixengine-proto && cargo clippy -p mixengine-proto --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/mixengine-proto
git commit -m "feat(proto): a service can be reloaded by signal (T32)"
```

---

### Task 4: A signal the platform can send, and a supervisor that sends it

**Files:**
- Modify: `crates/mixengine-platform/src/process.rs:70-80` (`CAN_ASK_TO_STOP`'s neighbourhood) and `:270-292` (`Supervised`)
- Modify: `crates/mixengine-platform/src/unix/process.rs:108-115` and `impl Group`
- Modify: `crates/mixengine-platform/src/windows/process.rs:380-390` and `impl Group`
- Modify: `crates/mixengine-daemon/src/services/runner.rs:770-782` and `:906-965`
- Test: `crates/mixengine-testkit/tests/supervision.rs`

**Interfaces:**
- Consumes: `mixengine_proto::ReloadSignal` from Task 3.
- Produces: `mixengine_platform::process::CAN_SIGNAL: bool`; `mixengine_platform::process::Signal` with variants `Hup`, `Usr1`, `Usr2`; `mixengine_platform::process::Supervised::signal(&self, signal: Signal) -> Result<()>`.

- [ ] **Step 1: Write the failing test**

Add to `crates/mixengine-testkit/tests/supervision.rs`, beside `a_group_on_windows_cannot_be_asked_to_stop_at_all`. That file's helpers are `supervised(FakeService) -> Supervised`, `wait_until_held(&Path)` and `is_free(&Path)`, and its tests are plain `#[test]`, not `#[tokio::test]`:

```rust
/// A signal reaches the process that leads the group, and the process survives it.
///
/// **The leader and not the group**, which is what separates this from a stop: a stop is meant for
/// every process holding the port, and a reload is meant for the master, whose whole job is to
/// decide what its workers do about it. `SIGUSR2` is a signal a program either handles or dies on,
/// and `fakeservice` neither handles it nor is killed by it — Rust installs no handler and the
/// default disposition for `SIGUSR2` is termination, so what this really asserts is delivery
/// *without* a wrong target: the lock is still held afterwards, which it would not be if the signal
/// had gone to a group that includes something we did not mean to reach.
#[cfg(unix)]
#[test]
fn a_supervised_process_can_be_signalled() {
    use mixengine_platform::process::{CAN_SIGNAL, Signal};

    const {
        assert!(CAN_SIGNAL, "this system has signals");
    }

    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("service.lock");

    let mut service = supervised(FakeService::new().hold_lock(&lock));
    wait_until_held(&lock);

    service
        .signal(Signal::Hup)
        .expect("a signal to a process this daemon started");

    // `SIGHUP` and not `SIGUSR2`: the fixture handles neither, and the claim being made is about the
    // call and the target rather than about what a program does with what it is sent. What php-fpm
    // does with `SIGUSR2` is judged in `crates/mixengine-cli/tests/php_fpm.rs`, against php-fpm.
    assert!(
        !is_free(&lock),
        "the signal reached something that was not the process it named"
    );

    service.stop().expect("the fixture stops");
}

/// Windows says so rather than pretending, exactly as it does for `ask_to_stop`.
///
/// In a `const` block for that test's reason: the claim is about a constant, so the day it changes
/// should be a build that fails at this line rather than a run that fails after starting a process.
#[cfg(windows)]
#[test]
fn a_process_on_windows_cannot_be_signalled_at_all() {
    use mixengine_platform::Error;
    use mixengine_platform::process::Signal;

    const {
        assert!(
            !mixengine_platform::process::CAN_SIGNAL,
            "this system now claims it can signal a process — `ReloadBehaviour::Signal` and ADR \
             0008 both need revisiting"
        );
    }

    let home = tempfile::tempdir().expect("a directory to keep a lock in");
    let lock = home.path().join("service.lock");

    let mut service = supervised(FakeService::new().hold_lock(&lock));
    wait_until_held(&lock);

    let refused = service
        .signal(Signal::Usr2)
        .expect_err("there are no signals on this system");

    assert!(
        matches!(
            &refused,
            Error::UnsupportedPlatform { capability, .. } if capability.contains("signal")
        ),
        "a system with no signals has to say so in the typed way: {refused:?}"
    );

    service.stop().expect("the fixture stops");
}
```

`Signal` is unused on Windows in the `#[cfg(unix)]` test and vice versa, which is why each imports its own inside the function body rather than at the top of the file.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p mixengine-testkit --test supervision a_supervised_process_can_be_signalled`
Expected: FAIL — `CAN_SIGNAL` and `Signal` do not exist.

- [ ] **Step 3: Add the shared half**

In `crates/mixengine-platform/src/process.rs`, beside `CAN_ASK_TO_STOP`:

```rust
/// Whether a running process can be sent a signal on this system.
///
/// True on Unix, false on Windows, and for the reason [`CAN_ASK_TO_STOP`] is false there: a daemon
/// has no signal to send a process it gave no console to. Its own constant rather than a second
/// reading of that one, because they are two capabilities that happen to be absent together —
/// [`Supervised::ask_to_stop`] addresses a *group* and this addresses a *leader*, and a system that
/// gained one without the other would need to say so.
///
/// **A caller checks this before it waits**, not after. A reload that could never be delivered is a
/// line in the log at the moment it is asked for, rather than a patience spent on nothing.
pub const CAN_SIGNAL: bool = sys::CAN_SIGNAL;

/// A signal a running service can be sent.
///
/// This crate's own list rather than [`mixengine_proto::ReloadSignal`]: `mixengine-platform` depends
/// on no other crate in this workspace, and one enum is not a reason to open that edge — the daemon
/// holds both and maps one onto the other in three lines. The numbers stay inside `unix/process.rs`,
/// which is the only file in the workspace that may name a `libc` constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Signal {
    /// `SIGHUP`.
    Hup,
    /// `SIGUSR1`.
    Usr1,
    /// `SIGUSR2`.
    Usr2,
}
```

and on `Supervised`, after `ask_to_stop`:

```rust
    /// Send `signal` to the process this handle names.
    ///
    /// **To the leader and not to the group**, which is the whole difference between this and
    /// [`ask_to_stop`](Self::ask_to_stop). A stop is addressed at every process holding the port,
    /// because a master that has already crashed cannot pass one on. A reload is addressed at the
    /// master precisely because it has not crashed: php-fpm's `SIGUSR2` is a instruction to *replace
    /// the workers*, and the same signal delivered to a worker mid-request is that request dropped.
    ///
    /// **Check [`CAN_SIGNAL`] first.** On Windows there is no such thing and this says so rather
    /// than succeeding quietly, because a caller that believed it would go on waiting for an effect
    /// nothing was ever asked to produce.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`] where the system has no signals, and [`Error::Os`] if it has
    /// them and refused. A process that has already gone is not a failure.
    pub fn signal(&self, signal: Signal) -> Result<()> {
        self.group.signal_leader(self.child.id(), signal)
    }
```

- [ ] **Step 4: The Unix half**

In `crates/mixengine-platform/src/unix/process.rs`, beside `CAN_ASK_TO_STOP`:

```rust
/// Whether a process can be signalled here. See [`crate::process::CAN_SIGNAL`].
pub(crate) const CAN_SIGNAL: bool = true;
```

and in `impl Group`, beside `signal`:

```rust
    /// Send one signal to the process that *leads* the group, and forgive one that is not there.
    ///
    /// The pid is **not** negated, which is the entire difference from [`signal`](Self::signal): a
    /// positive target is one process. See `crate::process::Supervised::signal` for why a reload
    /// goes there rather than to everybody.
    pub(crate) fn signal_leader(&self, pid: u32, signal: crate::process::Signal) -> Result<()> {
        let (kind, action) = match signal {
            crate::process::Signal::Hup => (libc::SIGHUP, "hang up a supervised process"),
            crate::process::Signal::Usr1 => (libc::SIGUSR1, "signal a supervised process"),
            crate::process::Signal::Usr2 => (libc::SIGUSR2, "signal a supervised process"),
        };

        signal(pid as libc::pid_t, kind, action).map(drop)
    }
```

Take care with the shadowing the existing file already warns about: the free function `signal` is what this calls, and the parameter is named `signal` — rename the parameter to `which` if the compiler complains, keeping the doc comment's wording.

- [ ] **Step 5: The Windows half**

In `crates/mixengine-platform/src/windows/process.rs`, beside `CAN_ASK_TO_STOP`:

```rust
/// There are no signals here. See [`crate::process::CAN_SIGNAL`], and
/// `.claude/decisions/0008-no-signal-stop-on-windows.md` for the alternatives that lost.
pub(crate) const CAN_SIGNAL: bool = false;
```

and in `impl Group`, beside `request_stop`:

```rust
    /// There is no signal to send; see [`CAN_SIGNAL`].
    ///
    /// Reached only by a caller that ignored that constant, so it says what it is rather than
    /// pretending to have sent one — a silent success here would be a configuration a user believes
    /// is live and is not.
    pub(crate) fn signal_leader(&self, _pid: u32, _signal: crate::process::Signal) -> Result<()> {
        Err(Error::UnsupportedPlatform {
            capability: "signalling a supervised process",
            reason: "Windows has no signal a daemon can send a process it did not give a console \
                     to — a service that needs to re-read its configuration on this system is \
                     restarted instead"
                .to_owned(),
        })
    }
```

- [ ] **Step 6: Teach the runner to send it**

In `crates/mixengine-daemon/src/services/runner.rs`, change the call at `:779` to pass the handle:

```rust
                () = self.asked_to_reload.notified() => {
                    self.reload(&place, &supervised).await;

                    continue;
                }
```

and rewrite `reload` (`:921` onward) as:

```rust
    async fn reload(&self, place: &Surroundings, supervised: &Supervised) {
        match self.spec.reload() {
            Some(ReloadBehaviour::Command {
                program,
                args,
                patience,
            }) => self.reload_by_command(place, program, args, *patience).await,

            Some(ReloadBehaviour::Signal { signal, patience }) => {
                self.reload_by_signal(supervised, *signal, *patience).await;
            }

            // Asked of a service that has no way to be asked. Said at `warn` because the
            // alternative is a person editing an override, watching the daemon accept it, and
            // finding the old value still in force with nothing anywhere saying why.
            _ => tracing::warn!(
                service = self.spec.id().as_str(),
                "this service's configuration changed and it has no reload, so the running process \
                 is still using the previous one; it will be read at the next start"
            ),
        }
    }

    /// The `ReloadBehaviour::Command` half, which is what T31 wrote — unchanged, moved.
    async fn reload_by_command(
        &self,
        place: &Surroundings,
        program: &std::path::Path,
        args: &[String],
        patience: mixengine_proto::Millis,
    ) {
        // …the body of the old `reload` from `place.run(…)` onward, verbatim…
    }

    /// The `ReloadBehaviour::Signal` half — roadmap task **T32**.
    ///
    /// **`CAN_SIGNAL` is read before anything is waited for**, which is `CAN_ASK_TO_STOP`'s lesson
    /// applied one method along: a system with no signals should say so at the moment it is asked,
    /// not after a patience spent on a delivery nobody attempted. A recipe on such a system returns
    /// no reload at all, so this arm is the belt to that braces.
    ///
    /// **The patience is a wait and not a check.** A signal has no exit status: the daemon cannot
    /// learn from the OS whether php-fpm liked the file it was told to re-read, only that the signal
    /// was delivered. What the wait buys is that the next configuration change does not arrive on
    /// top of a pool that is still cycling its workers.
    async fn reload_by_signal(
        &self,
        supervised: &Supervised,
        signal: mixengine_proto::ReloadSignal,
        patience: mixengine_proto::Millis,
    ) {
        if !CAN_SIGNAL {
            tracing::warn!(
                service = self.spec.id().as_str(),
                "this service is reloaded by signal and this system has none, so the running \
                 process is still using its previous configuration; it will be read at the next \
                 start"
            );

            return;
        }

        let which = match signal {
            mixengine_proto::ReloadSignal::Hup => process::Signal::Hup,
            mixengine_proto::ReloadSignal::Usr1 => process::Signal::Usr1,
            mixengine_proto::ReloadSignal::Usr2 => process::Signal::Usr2,
            // The wire enum is `#[non_exhaustive]` and this crate is downstream of it, so a variant
            // added there without a mapping here is reported rather than silently dropped.
            other => {
                tracing::warn!(
                    service = self.spec.id().as_str(),
                    signal = ?other,
                    "this build cannot send the signal this service is reloaded with"
                );

                return;
            }
        };

        match supervised.signal(which) {
            Ok(()) => {
                tokio::time::sleep(patience.as_duration()).await;

                tracing::info!(
                    service = self.spec.id().as_str(),
                    signal = ?signal,
                    "this service was signalled to re-read its configuration"
                );
            }

            Err(error) => tracing::warn!(
                service = self.spec.id().as_str(),
                signal = ?signal,
                %error,
                "this service could not be signalled to re-read its configuration; the process is \
                 still running the previous one"
            ),
        }
    }
```

Add `CAN_SIGNAL` and `process::Signal` to the `mixengine_platform::process` import at `runner.rs:22`, and `ReloadSignal` to the `mixengine_proto` import at `:24`.

- [ ] **Step 7: Run the tests**

```bash
cargo test -p mixengine-testkit --test supervision
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: PASS. On Windows only the second test runs; on Unix only the first.

- [ ] **Step 8: Commit**

```bash
git add crates/mixengine-platform crates/mixengine-daemon/src/services/runner.rs \
        crates/mixengine-testkit/tests/supervision.rs
git commit -m "feat(supervisor): reload a service by sending it a signal (T32)"
```

---

### Task 5: The php-fpm recipe

**Files:**
- Create: `crates/mixengine-core/src/generate/recipes/php_fpm.rs`
- Create: `crates/mixengine-core/src/generate/recipes/php_fpm/php-fpm.conf`
- Modify: `crates/mixengine-core/src/generate/recipes.rs`
- Modify: `crates/mixengine-core/src/generate/recipe.rs` (`Recipe::source`, `Source`, `Catalogue::builtin`)
- Modify: `crates/mixengine-core/src/generate.rs` (re-export)

**Interfaces:**
- Consumes: `Context::provided` (Task 2), `ReloadBehaviour::Signal` and `ReloadSignal` (Task 3).
- Produces: `mixengine_core::generate::recipes::PhpFpm`, in `Catalogue::builtin()`; `mixengine_core::generate::Source` with variants `Package` and `Runtime(RuntimeKind)`; `Recipe::source(&self) -> Source`, defaulting to `Source::Package`; the constant `mixengine_core::generate::recipes::php_fpm::PACKAGE` = `"php-fpm"`.

- [ ] **Step 1: Add `Source` to the recipe trait**

In `crates/mixengine-core/src/generate/recipe.rs`, after `Instancing`:

```rust
/// Which table supplies the binary a recipe runs.
///
/// **A property of the recipe, not a rule in the daemon**, for [`Instancing`]'s reason: where
/// php-fpm's process comes from is a fact about php-fpm, and spelling it here is what lets both the
/// refusal in `service.create` and the hook that creates the pool derive from one answer instead of
/// from a string compared in two places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A `packages` row, put there by `package.install`, named by `service.create`.
    Package,

    /// A `runtime_installs` row of this kind, put there by `runtime.install` — which also creates
    /// the service, because a pool without a PHP is nothing and a PHP without a pool is a language
    /// no site can be served by. `service.create` refuses such a recipe and says which command to
    /// use instead.
    Runtime(mixengine_proto::RuntimeKind),
}
```

and on the trait, after `instancing`:

```rust
    /// Which table supplies the binary. See [`Source`].
    ///
    /// Defaulted, unlike [`instancing`](Self::instancing), because the answer *is* the same for
    /// every server the index publishes and only differs for the one recipe that runs out of a
    /// language.
    fn source(&self) -> Source {
        Source::Package
    }
```

Re-export `Source` from `crates/mixengine-core/src/generate.rs`'s `pub use recipe::{…}` line.

- [ ] **Step 2: Write the template**

Create `crates/mixengine-core/src/generate/recipes/php_fpm/php-fpm.conf`:

```jinja
; Generated by MixEngine — roadmap task T32. Every value here comes from the `services` row and its
; overrides; editing this file is editing something that is rewritten on the next `service.*` call.
; What a user edits is an override.

[global]
; Stated rather than left to php-fpm's own default, which is a path compiled into the build and
; therefore somewhere in whoever packaged it's tree. `--nodaemonize` on the command line means the
; process the supervisor holds is the master itself, so there is no pid file to reconcile — but a
; php-fpm that is asked to log and cannot say where sends the answer nowhere anybody looks.
error_log = {{ paths.logs }}/php-fpm.log

; The master must not fork away from the supervisor. Also given as `--nodaemonize` on the command
; line, and stated in both places on purpose: a flag omitted by mistake is a service that appears to
; exit successfully the moment it starts.
daemonize = no

[www]
; The same path the readiness check waits on, computed twice — once here for the server and once in
; Rust for the daemon — because a template cannot call a function and a spec cannot read a file.
; `the_file_and_the_readiness_check_name_one_socket` is what keeps the two honest: when they disagree
; the pool starts perfectly and is reported as never having come up.
;
; In `run/` and not beside the data directory, and short on purpose: `sockaddr_un` caps a socket path
; at 103 characters (measured in T33a) and a server given a longer one aborts *after* it has started,
; in a way that reads like a storage failure. The recipe refuses such a path by name before php-fpm
; can.
listen = {{ paths.run }}/php-fpm-{{ package.version }}.sock

; `static` and not `dynamic`, and this is the one place the two systems would otherwise diverge:
; Windows has no vocabulary for a pool that grows, so an override that worked on two systems out of
; three would be the split this whole task exists to avoid. A fixed pool of `max_children` is what
; both can express.
pm = static
pm.max_children = {{ settings.max_children }}

; A worker is retired after this many requests and replaced. What it buys is a leak in an extension
; costing a fixed amount of memory instead of a growing one; zero turns it off.
pm.max_requests = {{ settings.max_requests }}

; A worker that runs longer than this is killed and replaced, which is a hung script costing one
; worker for a bounded time rather than one worker forever. **There is no equivalent on Windows** —
; see the recipe's module note.
request_terminate_timeout = {{ settings.request_timeout }}

; Whatever a script writes to stdout beyond its response goes to `error_log` above rather than to
; nowhere, which is the difference between a fatal error somebody can read and a blank page.
catch_workers_output = yes

; Matches nothing, deliberately, and is here rather than in Phase 4 for the same *ownership* reason
; `import sites/*.caddy` is in the Caddyfile — but not by the same mechanism, and the difference is
; worth stating because it is a limitation. php-fpm's `include` takes an absolute glob and does not
; resolve against the file it is written in, so this points at the installed directory rather than at
; the staging one: `php-fpm --test` over a staged rendering therefore judges *this* file and not the
; per-site files a future Phase 4 will drop beside it. What the line still buys is that there is one
; place a site file can go and one recipe that decides where — which is what stops two renderers
; disagreeing about a directory. Whoever renders the first one owes the check its own answer.
include={{ paths.etc }}/pool.d/*.conf

{{ extra }}
```

On Windows this file renders with backslashes in every path and is read by nothing — see `files()`
below. Do not add the backtick-quoting the Caddyfile needs: there is no reader here to confuse, and a
quoting rule that only matters on the system that ignores the file is a rule nobody can check.

- [ ] **Step 3: Write the failing tests**

Create `crates/mixengine-core/src/generate/recipes/php_fpm.rs` with only its `mod tests` filled in first, so the module compiles and the tests fail on missing items. The tests to write:

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use mixengine_proto::ServiceId;

    use super::*;
    use crate::generate::recipe;
    use crate::generate::settings::Settings;

    /// A pool for PHP 8.3.33 in a home at `root`, with `overrides` applied.
    fn context(overrides: &str) -> Context {
        let service = ServiceId::parse("php-fpm@8.3.33").expect("an id");
        let settings =
            Settings::merge(PhpFpm.settings(), overrides, &service).expect("usable overrides");

        let provides = BTreeMap::from([
            ("php".to_owned(), "bin/php".to_owned()),
            ("php-fpm".to_owned(), "sbin/php-fpm".to_owned()),
            ("php-cgi".to_owned(), "php-cgi.exe".to_owned()),
        ]);

        Context::for_test(service, PACKAGE, Path::new(root()), provides, Some(9000), settings)
    }

    const fn root() -> &'static str {
        if cfg!(windows) { r"C:\MixEngine" } else { "/opt/mixengine" }
    }

    /// One pool per installed PHP, named by the version it runs — so its id carries an `@`.
    #[test]
    fn a_pool_is_named_after_the_php_it_runs() {
        assert_eq!(PhpFpm.instancing(), Instancing::Named);
    }

    /// The recipe says where its binary comes from, and it is not the package table.
    ///
    /// This is what `service.create` refuses on and what the install hook keys off, so it is
    /// asserted rather than assumed: a recipe that answered `Package` here would be one a user could
    /// declare against a `packages` row that does not exist.
    #[test]
    fn a_pool_comes_out_of_an_installed_php() {
        assert_eq!(PhpFpm.source(), Source::Runtime(RuntimeKind::Php));
    }

    /// The rendered file carries the values the row and the overrides gave it.
    ///
    /// Rendered through `recipe::render` rather than through a generator, for `caddy.rs`' reason:
    /// what is being checked is the *template*, and running the real validator would need fifty
    /// megabytes of PHP to find out whether a variable name is misspelled.
    #[test]
    fn the_pool_file_says_what_the_overrides_said() {
        let documents = recipe::render(&PhpFpm, &context(r#"{"max_children": 12}"#))
            .expect("a rendering");

        assert_eq!(documents.len(), 1, "php-fpm renders one file");
        assert_eq!(documents[0].relative(), Path::new(POOL_FILE));

        let rendered = documents[0].contents();
        assert!(rendered.contains("pm.max_children = 12"), "{rendered}");
        assert!(rendered.contains("pm = static"), "{rendered}");
        assert!(rendered.contains("php-fpm-8.3.33.sock"), "{rendered}");
    }

    /// **The template and the spec must name the same socket.**
    ///
    /// They are computed twice — once in Jinja for the file php-fpm reads, once in Rust for the
    /// readiness check the daemon makes — and the failure when they disagree is a service that
    /// starts perfectly and is reported as never having come up. Nothing else in this recipe is
    /// worth a test as much as this.
    #[cfg(unix)]
    #[test]
    fn the_file_and_the_readiness_check_name_one_socket() {
        let context = context("{}");
        let rendered = recipe::render(&PhpFpm, &context).expect("a rendering")[0]
            .contents()
            .to_owned();

        let spec = PhpFpm
            .spec(&context)
            .expect("a spec")
            .build()
            .expect("a valid spec");

        let mixengine_proto::ReadyCheck::UnixSocket { path, .. } = spec.ready() else {
            panic!("a pool on this system is proved up by its socket");
        };

        assert!(
            rendered.contains(&path.display().to_string()),
            "the file says one socket and the readiness check waits on another\n{rendered}"
        );
    }

    /// A socket path `sockaddr_un` cannot hold is refused here, by name.
    ///
    /// T33a measured the cap at 103 characters against a real server, and what it costs to find out
    /// the hard way is the reason this is a check: php-fpm aborts *after* it has started, in a way
    /// that reads like a different failure entirely.
    #[cfg(unix)]
    #[test]
    fn a_socket_path_too_long_for_the_kernel_is_refused_by_name() {
        let deep = format!("/{}", "nested/".repeat(20));
        let service = ServiceId::parse("php-fpm@8.3.33").expect("an id");
        let settings = Settings::merge(PhpFpm.settings(), "{}", &service).expect("defaults");
        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(&deep),
            BTreeMap::from([("php-fpm".to_owned(), "sbin/php-fpm".to_owned())]),
            None,
            settings,
        );

        let error = PhpFpm.spec(&context).expect_err("a path no kernel accepts");

        assert!(
            error.to_string().contains("103"),
            "the measurement is what makes this message useful: {error}"
        );
    }

    /// A PHP packed without the SAPI this recipe needs is named as such.
    #[test]
    fn a_php_without_the_right_sapi_is_named() {
        let service = ServiceId::parse("php-fpm@8.3.33").expect("an id");
        let settings = Settings::merge(PhpFpm.settings(), "{}", &service).expect("defaults");
        let context = Context::for_test(
            service,
            PACKAGE,
            Path::new(root()),
            BTreeMap::from([("php".to_owned(), "bin/php".to_owned())]),
            Some(9000),
            settings,
        );

        let error = PhpFpm.spec(&context).expect_err("no SAPI to run");

        assert!(
            matches!(error, Error::ServiceProvidesNothing { .. }),
            "{error:?}"
        );
    }
}
```

- [ ] **Step 4: Run them and watch them fail**

Run: `cargo test -p mixengine-core php_fpm`
Expected: FAIL to compile — `PhpFpm`, `PACKAGE`, `POOL_FILE` do not exist.

- [ ] **Step 5: Write the recipe**

Fill in the top of `crates/mixengine-core/src/generate/recipes/php_fpm.rs`:

```rust
//! php-fpm: the FastCGI pool behind every PHP site — roadmap task **T32**.
//!
//! **The first recipe whose binary does not come from a package.** A PHP is installed with
//! `runtime.install` into `runtime_installs`, and the process that serves its sites lives inside
//! that directory — so this recipe's service row points there, `service.create` refuses to write one
//! by hand, and `runtime.uninstall` is the thing that takes it away.
//!
//! # Two mechanisms, one vocabulary
//!
//! There is **no php-fpm on Windows** and this is upstream's shape rather than an omission of ours:
//! every PHP in `mixengine-packages`' index from 7.0 to 8.5 publishes `php` and `php-fpm` on Linux
//! and macOS, and `php` and `php-cgi` on Windows. What was not obvious, and was measured against the
//! artifact this project publishes rather than read about, is that this costs almost nothing:
//! `php-cgi.exe` given `PHP_FCGI_CHILDREN` **is** a process manager — a master, N children, a child
//! respawned within a second of being killed, recycling at `PHP_FCGI_MAX_REQUESTS`, and every child
//! going with the master when it is terminated. That is php-fpm with `pm = static`, configured
//! through the environment instead of through a file.
//!
//! So the two systems differ only in the mechanism, and a user meets one vocabulary:
//!
//! | | Unix | Windows |
//! | --- | --- | --- |
//! | program | `provides["php-fpm"]` | `provides["php-cgi"]` |
//! | workers | `pm.max_children` in the pool file | `PHP_FCGI_CHILDREN` |
//! | recycling | `pm.max_requests` | `PHP_FCGI_MAX_REQUESTS` |
//! | listen | `run/php-fpm-<version>.sock` | `127.0.0.1:<services.port>` |
//! | reload | `SIGUSR2` | none |
//!
//! Which binary it is comes out of the artifact's own `provides` map rather than being written down
//! here, which is what keeps a `#[cfg]` out of this file: the index says where the executable is,
//! and the recipe asks for it by the name we gave it.
//!
//! # What this recipe deliberately does not do
//!
//! **No `pm = dynamic` and no `pm = ondemand`.** Windows can express neither, and an override that
//! works on two systems out of three is exactly the divide this task exists to avoid.
//!
//! **No `request_terminate_timeout` on Windows.** A hung script holds a worker there for as long as
//! it hangs, and with five of them that is a dead PHP. The fix needs no process manager — the master
//! respawns a killed child, so the daemon would only have to kill a worker that has run too long —
//! but doing it right needs its own measurement of how a hung script behaves on that system, and
//! that is a task of its own.
//!
//! **No `php.ini` and no `conf.d`.** `PHP_INI_SCAN_DIR` was measured to work on all three systems,
//! so T28's model has a road; what a *pool* renders and what a *runtime's* ini set contains are
//! different files with different owners, and this recipe owns the first.
//!
//! **No site.** `pool.d/*.conf` matches nothing until Phase 4.
//!
//! **No `pm.status_path` and no slowlog.** Neither exists on Windows, and nothing reads them yet.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use mixengine_proto::{
    HealthCheck, HealthProbe, Millis, ReadyCheck, ReloadBehaviour, ReloadSignal, RuntimeKind,
    ServiceSpec, ServiceSpecBuilder, StopBehaviour,
};

use crate::generate::document::Validator;
use crate::generate::recipe::{Context, Instancing, Recipe, Source, TemplateFile};
use crate::generate::settings::{Preset, Setting};
use crate::{Error, Result};

/// The `packages.name` this recipe is found under, which for a pool is the id's own half: a service
/// is `php-fpm@8.3.33` and the row beneath it names a `php`.
pub const PACKAGE: &str = "php-fpm";

/// The executable that serves a pool on a system that has php-fpm, as the index names it.
const FPM: &str = "php-fpm";

/// And on Windows, where it does not. See the module note.
const CGI: &str = "php-cgi";

/// The rendered pool configuration, under `etc/<service-id>/`.
const POOL_FILE: &str = "php-fpm.conf";

/// How many workers the pool holds. Five is php-fpm's own `pm.max_children` for a `www` pool, and
/// is a number a laptop can serve a development site with while running everything else.
const MAX_CHILDREN: &str = "max_children";

/// How many requests a worker serves before it is retired and replaced. Bounds what a leaking
/// extension costs; zero turns it off.
const MAX_REQUESTS: &str = "max_requests";

/// How long one request may run before its worker is killed, in seconds. **Unix only** — see the
/// module note. `0` is php-fpm's own "no limit".
const REQUEST_TIMEOUT: &str = "request_timeout";

/// How long the pool is given to be listening before the start is a failure, in milliseconds.
const READY_TIMEOUT: &str = "ready_timeout_ms";

/// How long a stop is given before the process group is killed, in milliseconds.
const STOP_GRACE: &str = "stop_grace_ms";

/// How often the socket is asked whether the master is still accepting.
const HEALTH_INTERVAL: Millis = Millis(10_000);

/// How long one of those may take. Well inside the interval, which `ServiceSpec::validate` insists
/// on: two probes that could overlap are two probes that can queue.
const HEALTH_TIMEOUT: Millis = Millis(2_000);

/// How long a `SIGUSR2` is treated as in progress.
///
/// What it covers is a graceful pool restart: every worker finishes the request it is serving before
/// its replacement takes over, so the wait is really the longest request a site has in flight.
/// Nothing is killed when it expires.
const RELOAD_PATIENCE: Millis = Millis(10_000);

/// What `sockaddr_un` can hold, measured against a real server in T33a.
///
/// A path longer than this does not fail at `bind`: the server starts, gets some way in, and aborts
/// in a way that reads like a different failure entirely. Refusing it here, by name and with the
/// number in the message, is the difference between a sentence somebody can act on and an afternoon.
#[cfg(unix)]
const SOCKET_PATH_LIMIT: usize = 103;

/// php-fpm, as MixEngine runs it.
#[derive(Debug)]
pub struct PhpFpm;

impl Recipe for PhpFpm {
    fn package(&self) -> &'static str {
        PACKAGE
    }

    /// One pool per installed PHP, named by the version it runs.
    ///
    /// The **full** version — `php-fpm@8.3.33` — because `runtime_installs` is
    /// `UNIQUE (kind, version)` over the full version, so 8.3.33 and 8.3.34 can both be installed
    /// and `php-fpm@8.3` would then name neither.
    fn instancing(&self) -> Instancing {
        Instancing::Named
    }

    fn source(&self) -> Source {
        Source::Runtime(RuntimeKind::Php)
    }

    /// One set of overrides on every system, rendered into a file or an environment as the platform
    /// requires. See the module note for what is deliberately absent from it.
    fn settings(&self) -> &'static [Setting] {
        &[
            Setting {
                key: MAX_CHILDREN,
                default: Preset::Number(5),
            },
            Setting {
                key: MAX_REQUESTS,
                default: Preset::Number(500),
            },
            Setting {
                key: REQUEST_TIMEOUT,
                default: Preset::Number(120),
            },
            Setting {
                // Fifteen seconds. A pool is up in tens of milliseconds; what this is really waiting
                // for is a first run on Windows, where Defender reads the whole of a PHP before the
                // process starts.
                key: READY_TIMEOUT,
                default: Preset::Number(15_000),
            },
            Setting {
                key: STOP_GRACE,
                default: Preset::Number(10_000),
            },
        ]
    }

    /// One file, rendered on every system — and read by php-fpm on the two that have one.
    ///
    /// **Windows renders it and runs none of it**, which is deliberate and is the cheaper of the two
    /// mistakes available. A `#[cfg]` here would break this crate's rule about platform conditionals
    /// for a file that costs a few hundred bytes; it would also make a home on one system
    /// structurally different from a home on another, so that a user comparing theirs with a
    /// colleague's finds a directory missing rather than a value differing. And `pool.d/` has to
    /// point somewhere on both the day Phase 4 renders the first per-site file into it.
    fn files(&self) -> &'static [TemplateFile] {
        &[TemplateFile {
            path: POOL_FILE,
            source: include_str!("php_fpm/php-fpm.conf"),
        }]
    }
```

Then the validator and the spec:

```rust
    /// `php-fpm -t`, pointed at the staged file — and nothing on Windows, where there is no file to
    /// test and the SAPI has no such flag.
    fn validator(&self, context: &Context) -> Option<Validator> {
        let program = context.provided(FPM).ok()?;

        Some(Validator::new(program, POOL_FILE).args([
            "--test",
            "--fpm-config",
            crate::generate::document::CONFIG,
        ]))
    }

    fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        if cfg!(windows) {
            self.windows(context)
        } else {
            self.unix(context)
        }
    }
}
```

`cfg!` is a *value* and not an attribute, so both arms compile everywhere — which is what keeps the whole file cross-platform and lets the tests above exercise the branch this machine is not.

Then the two private halves:

```rust
impl PhpFpm {
    /// The pool as a system with php-fpm runs it.
    fn unix(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();
        let program = context.provided(FPM)?;
        let socket = socket_path(context)?;

        Ok(ServiceSpec::builder(context.service().clone(), &program)
            // `--nodaemonize`, so the process the supervisor holds is the master itself. Without it
            // php-fpm forks and the parent exits successfully, which looks from out here exactly
            // like a service that started and immediately stopped.
            .args([
                "--nodaemonize".to_owned(),
                "--fpm-config".to_owned(),
                context.config(POOL_FILE).to_string_lossy().into_owned(),
            ])
            .cwd(context.etc())
            .ready(ReadyCheck::UnixSocket {
                path: socket.clone(),
                timeout: millis(settings.number(READY_TIMEOUT)),
            })
            .health(HealthCheck {
                probe: HealthProbe::UnixSocket { path: socket },
                interval: HEALTH_INTERVAL,
                timeout: HEALTH_TIMEOUT,
                // Three intervals rather than one: a reload cycles every worker, and a pool serving
                // a slow request can miss a probe doing it. That is a busy PHP, not a sick one.
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            // The master finishes what its workers are serving and replaces them with workers that
            // read the new file. This is the service the whole idea is for after Caddy: restarting
            // would drop every request in flight for a change to one site's settings.
            .reload(ReloadBehaviour::Signal {
                signal: ReloadSignal::Usr2,
                patience: RELOAD_PATIENCE,
            })
            // `SIGTERM` to the group, which php-fpm reads as an immediate shutdown; the workers are
            // in that group and go with it.
            .stop(StopBehaviour::Signal {
                grace: millis(settings.number(STOP_GRACE)),
            }))
    }

    /// The pool as Windows runs it: `php-cgi.exe` on a port, with the pool in the environment.
    fn windows(&self, context: &Context) -> Result<ServiceSpecBuilder> {
        let settings = context.settings();
        let program = context.provided(CGI)?;
        let addr = address(context)?;

        Ok(ServiceSpec::builder(context.service().clone(), &program)
            .args(["-b".to_owned(), addr.to_string()])
            .cwd(context.etc())
            // The two variables that make `php-cgi.exe` a process manager rather than a queue of
            // one. Measured, not assumed — see the module note. They are the same two numbers the
            // pool file carries on Unix, which is what makes the override set one set.
            .env(
                "PHP_FCGI_CHILDREN",
                settings.number(MAX_CHILDREN).to_string(),
            )
            .env(
                "PHP_FCGI_MAX_REQUESTS",
                settings.number(MAX_REQUESTS).to_string(),
            )
            .ready(ReadyCheck::Tcp {
                addr,
                timeout: millis(settings.number(READY_TIMEOUT)),
            })
            .health(HealthCheck {
                probe: HealthProbe::Tcp { addr },
                interval: HEALTH_INTERVAL,
                timeout: HEALTH_TIMEOUT,
                failures_before_degraded: 3,
                successes_before_running: 1,
            })
            // **No reload.** There is no signal to send here, so a changed override leaves the
            // running pool on its old configuration until somebody restarts it — and the daemon does
            // not restart a thing nobody asked it to restart. The supervisor says so once, in
            // `daemon.log`, and `mix doctor` (T47) owes the sentence.
            //
            // `StopBehaviour::Signal` degrades to a kill here (ADR 0008), which is safe for this
            // service and for a measured reason: terminating the master was observed to take every
            // child with it, so nothing is left holding the port.
            .stop(StopBehaviour::Signal {
                grace: millis(settings.number(STOP_GRACE)),
            }))
    }
}

/// Where this pool listens on a system with Unix sockets.
///
/// `run/` and not the data directory, and short on purpose: [`SOCKET_PATH_LIMIT`] is the whole
/// reason, and `run/` is near the top of the home while a data directory is two levels down inside
/// one whose name the user chose.
///
/// # Errors
///
/// [`Error::SettingValue`] — reusing the variant that names the service and the reason — when the
/// path this home would need is longer than the kernel accepts.
fn socket_path(context: &Context) -> Result<PathBuf> {
    let socket = context
        .run()
        .join(format!("php-fpm-{}.sock", context.version()));

    #[cfg(unix)]
    if socket.as_os_str().len() > SOCKET_PATH_LIMIT {
        return Err(Error::SettingValue {
            service: context.service().as_str().to_owned(),
            key: "listen",
            value: socket.display().to_string(),
            reason: "a Unix socket path is capped at 103 characters by `sockaddr_un`, and a server \
                     given a longer one aborts after it has started — move the MixEngine home \
                     somewhere shorter",
        });
    }

    Ok(socket)
}

/// Where this pool listens on Windows: the port its row was given, on loopback.
///
/// The port is the row's rather than a number derived here, because it is allocated once when the
/// pool is created and has to be the same on every start — see `services::pools`.
///
/// # Errors
///
/// [`Error::SettingValue`] when the row carries no port, which is a pool created on a system that
/// does not need one and then run on a system that does.
fn address(context: &Context) -> Result<SocketAddr> {
    let port = context.port().ok_or_else(|| Error::SettingValue {
        service: context.service().as_str().to_owned(),
        key: "port",
        value: "none".to_owned(),
        reason: "a pool on this system listens on a TCP port and its row carries none; \
                 `runtime.install` allocates one when it creates the pool",
    })?;

    Ok(SocketAddr::new(
        context
            .bind()
            .parse::<IpAddr>()
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        port,
    ))
}

/// A setting as a length of time, with a negative one read as none at all.
fn millis(number: i64) -> Millis {
    Millis(u64::try_from(number).unwrap_or_default())
}
```

- [ ] **Step 6: Register it**

In `crates/mixengine-core/src/generate/recipes.rs`, add `pub mod php_fpm;` and `pub use php_fpm::PhpFpm;`, and update the module note so php-fpm is no longer listed as pending. In `crates/mixengine-core/src/generate/recipe.rs`, change `Catalogue::builtin`:

```rust
    pub fn builtin() -> Self {
        Self::default()
            .with(Arc::new(super::recipes::Caddy))
            .with(Arc::new(super::recipes::PhpFpm))
    }
```

and update its doc comment: two recipes now, the rest arrive one task at a time. Re-export `PhpFpm` from `generate.rs` beside `Caddy`.

- [ ] **Step 7: Run the tests**

```bash
cargo test -p mixengine-core php_fpm
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/mixengine-core/src/generate
git commit -m "feat(services): a php-fpm recipe with a native mechanism per system (T32)"
```

---

### Task 6: The pool an installed PHP creates for itself

**Files:**
- Create: `crates/mixengine-core/src/services/pools.rs`
- Modify: `crates/mixengine-core/src/services.rs` (`pub mod pools;` and the re-export)
- Modify: `crates/mixengine-daemon/src/api/create.rs` (the refusal)
- Modify: `crates/mixengine-daemon/src/runtimes.rs` (`perform`'s tail, and `uninstall`)
- Modify: `crates/mixengine-daemon/src/main.rs` (the boot call)
- Test: `crates/mixengine-core/src/services/pools.rs`'s `mod tests`, and `crates/mixengine-daemon/tests/runtimes.rs`

**Interfaces:**
- Consumes: `services::Origin` (Task 1), `Recipe::source` / `Source` (Task 5).
- Produces: `mixengine_core::services::pools::ensure(store: &Store, catalogue: &Catalogue) -> Result<Vec<ServiceId>>` returning the ids it created (empty when there was nothing to do), and `mixengine_core::services::pools::of(store: &Store, kind: RuntimeKind, version: &PackageVersion) -> Result<Option<ServiceId>>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/mixengine-core/src/services/pools.rs` with its `mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Every installed PHP ends up with a pool, and asking twice creates nothing.
    ///
    /// **Idempotence is the whole design**, not a nicety: this runs at every boot as well as after
    /// every install, which is what gives a PHP installed before T32 a pool without a data migration
    /// and repairs a home whose row somebody deleted by hand.
    #[tokio::test]
    async fn every_installed_php_gets_one_pool_and_only_one() {
        let (_home, store) = store().await;
        install(&store, "8.3.33").await;
        install(&store, "8.4.1").await;

        let created = ensure(&store, &Catalogue::builtin())
            .await
            .expect("pools for both");

        assert_eq!(
            created.iter().map(ServiceId::as_str).collect::<Vec<_>>(),
            ["php-fpm@8.3.33", "php-fpm@8.4.1"],
            "one pool each, named by the full version"
        );

        let again = ensure(&store, &Catalogue::builtin())
            .await
            .expect("nothing to do");

        assert!(again.is_empty(), "a second pass created {again:?}");
    }

    /// A pool on a system that listens on TCP is given a port when it is created, and keeps it.
    ///
    /// Allocated here rather than derived from the version, because two PHPs whose versions differ
    /// in a digit nobody looks at would otherwise collide — and written into the row rather than
    /// recomputed, because a port that moved between restarts is a Caddy pointed at nothing.
    #[tokio::test]
    async fn a_pool_that_needs_a_port_is_given_a_free_one() {
        let (_home, store) = store().await;
        install(&store, "8.3.33").await;
        install(&store, "8.4.1").await;

        ensure(&store, &Catalogue::builtin()).await.expect("pools");

        let ports: Vec<Option<i64>> =
            sqlx::query_scalar("SELECT port FROM services ORDER BY id")
                .fetch_all(store.pool())
                .await
                .expect("the rows");

        if cfg!(windows) {
            assert_eq!(ports, [Some(9000), Some(9001)]);
        } else {
            assert_eq!(ports, [None, None], "a socket needs no port");
        }
    }
}
```

Write `store()` and `install()` helpers in the same module, mirroring `services.rs`'s.

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p mixengine-core pools`
Expected: FAIL to compile — the module has no `ensure`.

- [ ] **Step 3: Write the module**

```rust
//! The service a runtime install creates for itself — roadmap task **T32**.
//!
//! Nobody calls `service.create` for a pool. `.claude/features/runtime-versions.md` decided this
//! before there was a pool to create: PHP's post-install hook makes the `php-fpm@<version>` record,
//! and an uninstall takes it away. What is here is that hook, and it is written **idempotent and run
//! at boot as well as after an install** — which is what gives a PHP installed before this task a
//! pool without a data migration, and what repairs a home whose row was deleted by hand.
//!
//! **Which runtimes get one is the catalogue's answer, not a list here.** A recipe says where its
//! binary comes from ([`Source`]), so this walks the recipes rather than the languages: the day
//! `node` grows a supervised service, its recipe says so and this needs no edit.

use mixengine_proto::{PackageVersion, RuntimeKind, ServiceId};

use crate::generate::{Catalogue, Source};
use crate::{Error, Result, Store};

use super::{Declaration, Origin};

/// The first port a pool that needs one is offered.
///
/// `.claude/features/services.md`'s own `127.0.0.1:9xxx`, and php-fpm's conventional 9000 — a number
/// somebody debugging a FastCGI connection will recognise on sight.
const FIRST_PORT: u16 = 9000;

/// Give every installed runtime the service its recipe says it should have, and say which were made.
///
/// **A no-op on a home that is already right**, which is what lets it run at boot: the cost of a
/// call with nothing to do is one query.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read or written, and whatever
/// [`create`](super::create) reports for a row that cannot be written — except
/// [`Error::ServiceAlreadyDeclared`], which is this function's own no-op and is swallowed: two
/// daemons racing to repair one home is not a failure of either.
pub async fn ensure(store: &Store, catalogue: &Catalogue) -> Result<Vec<ServiceId>> {
    let mut created = Vec::new();

    for package in catalogue.packages() {
        let Some(recipe) = catalogue.recipe(package) else {
            continue;
        };

        let Source::Runtime(kind) = recipe.source() else {
            continue;
        };

        let listens_on_a_port = !cfg!(unix);
        let kind_column = kind.as_str();

        // Every installed version of that language that has no service pointing at it. One query
        // rather than one per version, because a boot on a home with six PHPs should not be six
        // round trips to answer "nothing to do".
        let missing = sqlx::query_scalar!(
            "SELECT r.version
             FROM runtime_installs r
             WHERE r.kind = ?
               AND NOT EXISTS (SELECT 1 FROM services s WHERE s.runtime_install_id = r.id)
             ORDER BY r.version",
            kind_column
        )
        .fetch_all(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

        for version in missing {
            let version = PackageVersion::parse(&version).map_err(|_| Error::NotFound {
                kind: "runtime",
                id: format!("{kind_column} {version}"),
            })?;

            let service = ServiceId::parse(format!("{package}@{version}")).map_err(|_| {
                Error::NotFound {
                    kind: "service",
                    id: format!("{package}@{version}"),
                }
            })?;

            let port = match listens_on_a_port {
                false => None,
                true => Some(free_port(store).await?),
            };

            match super::create(
                store,
                &Declaration {
                    service: service.clone(),
                    origin: Origin::Runtime {
                        kind,
                        version: version.clone(),
                    },
                    instance_name: version.as_str().to_owned(),
                    port,
                    bind_addr: None,
                    data_dir: None,
                    // **Not on by default.** A user who installs four PHPs to test against has not
                    // asked for four pools at every boot, and `mix service` is one command.
                    autostart: false,
                    overrides: "{}".to_owned(),
                },
            )
            .await
            {
                Ok(()) => created.push(service),

                // Two daemons repairing one home, or an install racing a boot. The row it wanted is
                // there, which is what it wanted.
                Err(Error::ServiceAlreadyDeclared { .. }) => {}

                Err(error) => return Err(error),
            }
        }
    }

    if !created.is_empty() {
        tracing::info!(pools = ?created, "installed runtimes were given the services they need");
    }

    Ok(created)
}

/// The service that runs out of one installed runtime, if there is one.
///
/// What `runtime.uninstall` asks before it removes a directory.
///
/// # Errors
///
/// [`Error::Database`] when the tables cannot be read.
pub async fn of(
    store: &Store,
    kind: RuntimeKind,
    version: &PackageVersion,
) -> Result<Option<ServiceId>> {
    let (kind_column, version_column) = (kind.as_str(), version.as_str());

    let id = sqlx::query_scalar!(
        "SELECT s.id
         FROM services s
         JOIN runtime_installs r ON r.id = s.runtime_install_id
         WHERE r.kind = ? AND r.version = ?",
        kind_column,
        version_column
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?;

    Ok(id.and_then(|id| ServiceId::parse(id).ok()))
}

/// The lowest port from [`FIRST_PORT`] that no `services` row already holds.
///
/// **The table and not the machine**, deliberately: what this is avoiding is two pools configured on
/// one number, which is a fact about this home and is stable across reboots. Whether something else
/// on the machine holds it is a different question with a different answer every day, and the one
/// this cannot usefully ask — a port free at install time may be taken by the time the pool starts,
/// and a start that fails says so with the port in it.
///
/// # Errors
///
/// [`Error::Database`] when the table cannot be read.
async fn free_port(store: &Store) -> Result<u16> {
    let taken: Vec<i64> = sqlx::query_scalar!(
        "SELECT port FROM services WHERE port IS NOT NULL ORDER BY port"
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?
    .into_iter()
    .flatten()
    .collect();

    let mut port = FIRST_PORT;
    for held in taken {
        if i64::from(port) == held {
            port = port.saturating_add(1);
        }
    }

    Ok(port)
}
```

Add `pub mod pools;` to `crates/mixengine-core/src/services.rs` beside `pub mod graph;`.

- [ ] **Step 4: Refuse `service.create` for a runtime-backed recipe**

In `crates/mixengine-daemon/src/api/create.rs`, immediately after the recipe lookup and **before** the instancing check:

```rust
        // A pool is created by the install that puts the PHP on disk, not by hand: a `services` row
        // pointing at a `runtime_installs` row that this call has no way to name would be a row with
        // no parent, and the `CHECK` on the table refuses it. What a person gets instead is the
        // command that does work.
        if let mixengine_core::generate::Source::Runtime(kind) = recipe.source() {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                format!("{package} is created by installing a {kind}, not by hand"),
            )
            .with_hint(format!(
                "`mix runtime install {kind} <version>` gives that version its own {package}, and \
                 `mix runtime uninstall` takes it away again"
            )));
        }
```

- [ ] **Step 5: Create the pool after an install, and remove it before an uninstall**

In `crates/mixengine-daemon/src/runtimes.rs`, at the end of `perform` — after `remember` has succeeded and before the summary is encoded:

```rust
        // **After the row and never before it**, because the pool points at that row: this is the
        // post-install hook `.claude/features/runtime-versions.md` describes, and it is the same
        // idempotent call the daemon makes at boot. A failure here is reported and does not undo the
        // install — a PHP with no pool is a PHP the next boot gives one to, where an install rolled
        // back for it would be eighty megabytes thrown away over a row.
        match mixengine_core::services::pools::ensure(&self.store, &crate::services::catalogue())
            .await
        {
            Ok(created) if !created.is_empty() => {
                tracing::info!(pools = ?created, "the new runtime was given its service");
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(
                kind = kind.as_str(),
                version = version.as_str(),
                %error,
                "this runtime was installed but could not be given its service; the next daemon \
                 start will try again"
            ),
        }
```

and rewrite `uninstall`'s head so the refusal happens before anything is removed:

```rust
        let removed = runtimes::record(&self.store, target.kind, &target.version)
            .await
            .map_err(|error| error.to_wire())?;

        // **The first refusal this method has ever been able to make** — roadmap task T32, and the
        // promise `.claude/features/runtime-versions.md` has been carrying. A PHP whose pool is
        // running is a PHP something is serving sites out of, and removing the directory under it
        // would leave a process with no files and a row naming a runtime that is gone.
        if let Some(service) =
            mixengine_core::services::pools::of(&self.store, target.kind, &target.version)
                .await
                .map_err(|error| error.to_wire())?
        {
            let record = mixengine_core::services::record(&self.store, &service)
                .await
                .map_err(|error| error.to_wire())?;

            if !matches!(
                record.state,
                mixengine_proto::ServiceState::Stopped | mixengine_proto::ServiceState::Failed
            ) {
                return Err(Error::new(
                    ErrorCode::PreconditionFailed,
                    format!("{service} is {}", record.state.as_str()),
                )
                .with_hint(format!("`mix service stop {service}` first")));
            }

            // The row goes before the directory, which is the reverse of the rule the directory
            // follows — and is right for the same reason: a `services` row whose runtime is gone is
            // a row every `service.*` call fails on, where a directory with no row is invisible.
            mixengine_core::services::delete(&self.store, &service)
                .await
                .map_err(|error| error.to_wire())?;

            tracing::info!(%service, "a pool was removed with the runtime it ran out of");
        }
```

Also remove the "Nothing is checked for using it yet" paragraph from `uninstall`'s doc comment and replace it with what the code now does, keeping the note that a *project pinning* a version is still unchecked because there are no projects until Phase 4.

- [ ] **Step 6: Run it at boot**

In `crates/mixengine-daemon/src/main.rs`, after the `services.recover()` block and before the job reconciliation:

```rust
    // **Every installed runtime gets the service its recipe says it should have** — roadmap task
    // T32. Idempotent and run here as well as after an install, which is what gives a PHP installed
    // by an earlier build its pool with no data migration and repairs a home whose row somebody
    // removed by hand. Nothing here fails the start, on the same rule the two blocks around it
    // follow: a runtime with no service is one command away from having one, where refusing to start
    // would leave the user with no daemon at all.
    match mixengine_core::services::pools::ensure(store, &services::catalogue()).await {
        Ok(created) if created.is_empty() => {
            tracing::debug!("every installed runtime already has the service it needs");
        }
        Ok(created) => tracing::info!(pools = ?created, "installed runtimes were given services"),
        Err(error) => tracing::warn!(%error, "could not give every installed runtime its service"),
    }
```

- [ ] **Step 7: Add the daemon-level test**

In `crates/mixengine-daemon/tests/runtimes.rs`, add a test that installs a fake `php` through the mock registry and asserts that `service.list` afterwards names `php-fpm@<version>`, and that `runtime.uninstall` then removes it. Follow the suite's existing fixture exactly; the assertions that must be present:

```rust
    // The pool the install created for itself, which nobody asked for and everybody needs.
    let listed = client.call("service.list", json!({})).await;
    assert!(
        listed["services"]
            .as_array()
            .is_some_and(|services| services
                .iter()
                .any(|service| service["id"] == format!("php-fpm@{VERSION}"))),
        "{listed}"
    );

    // And `service.create` will not write a second one by hand.
    let refused = client
        .call(
            "service.create",
            json!({ "id": format!("php-fpm@{VERSION}"), "version": VERSION }),
        )
        .await;
    assert_eq!(refused["error"]["code"], "invalid_argument", "{refused}");
```

The fake `php` the registry publishes must declare `provides` containing `php-fpm` (Unix) or `php-cgi` (Windows) pointing at the fixture executable, so the recipe can build a spec — the test does not start it.

- [ ] **Step 8: Run everything**

```bash
cargo sqlx prepare --workspace -- --all-targets --all-features
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/mixengine-core crates/mixengine-daemon .sqlx
git commit -m "feat(runtimes): give every installed PHP its own php-fpm pool (T32)"
```

---

### Task 7: A FastCGI client in the testkit

**Files:**
- Create: `crates/mixengine-testkit/src/fastcgi.rs`
- Modify: `crates/mixengine-testkit/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `mixengine_testkit::fastcgi::Pool` with constructors `Pool::socket(path: impl Into<PathBuf>)` (Unix only) and `Pool::port(addr: SocketAddr)`, and `Pool::get(&self, script: &Path) -> std::io::Result<Response>`; `mixengine_testkit::fastcgi::Response { pub headers: String, pub body: String }`.

- [ ] **Step 1: Write the failing test**

Add to the bottom of the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A name-value pair under 128 bytes takes one length byte at each end, and a longer one takes
    /// four with the top bit set.
    ///
    /// The encoding is the only part of this client that can be silently wrong: a wrong length is a
    /// pool that reads a parameter block it cannot parse and answers nothing, which from a test's
    /// side is indistinguishable from a pool that is not there.
    #[test]
    fn a_parameter_is_encoded_the_way_the_protocol_says() {
        let mut short = Vec::new();
        pair(&mut short, "A", "b");
        assert_eq!(short, [1, 1, b'A', b'b']);

        let long = "x".repeat(200);
        let mut wide = Vec::new();
        pair(&mut wide, "A", &long);
        assert_eq!(&wide[..1], &[1]);
        assert_eq!(&wide[1..5], &[0x80, 0, 0, 200]);
    }

    /// A record carries its body length in two big-endian bytes after the request id.
    #[test]
    fn a_record_header_is_eight_bytes() {
        let mut out = Vec::new();
        record(&mut out, STDIN, b"hi");

        assert_eq!(out, [1, STDIN, 0, 1, 0, 2, 0, 0, b'h', b'i']);
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p mixengine-testkit fastcgi`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the client**

```rust
//! A FastCGI responder client, for the one suite that has to prove a pool is serving PHP.
//!
//! **Because connecting to the socket proves nothing.** A php-fpm that is listening and cannot
//! execute a script — a missing SAPI, a `security.limit_extensions` that refuses the file, a
//! `SCRIPT_FILENAME` the pool cannot see — accepts a connection exactly like one that works. The
//! only claim worth making about a pool is that a request went in and a body came out, and that
//! takes speaking the protocol.
//!
//! About eighty lines, because the responder role is small: one `BEGIN_REQUEST`, one block of CGI
//! parameters, an empty `STDIN`, and then records read back until `END_REQUEST`. Nothing here
//! handles multiplexing, filters, authorizers or a request body — a test that needed any of those
//! would be testing this client.
//!
//! **A dev-dependency like everything else in this crate**, so the eighty lines ship to nobody.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};

/// The protocol version, which has been 1 since 1996.
const VERSION: u8 = 1;

/// Record types, from the specification's own table. Only the five a responder needs.
const BEGIN_REQUEST: u8 = 1;
const END_REQUEST: u8 = 3;
const PARAMS: u8 = 4;
const STDIN: u8 = 5;
const STDOUT: u8 = 6;
const STDERR: u8 = 7;

/// The role a web server asks for: run the script and give me its output.
const RESPONDER: u16 = 1;

/// The one request this client ever has in flight. Multiplexing is what the id is for and this does
/// not multiplex.
const REQUEST_ID: u16 = 1;

/// Where a pool listens, in whichever of the two ways this system has.
#[derive(Debug, Clone)]
pub enum Pool {
    /// A Unix domain socket — how a pool listens everywhere php-fpm exists.
    #[cfg(unix)]
    Socket(PathBuf),

    /// A loopback port — how `php-cgi.exe -b` listens on Windows.
    Port(SocketAddr),
}

impl Pool {
    /// A pool on a Unix socket.
    #[cfg(unix)]
    #[must_use]
    pub fn socket(path: impl Into<PathBuf>) -> Self {
        Self::Socket(path.into())
    }

    /// A pool on a TCP port.
    #[must_use]
    pub fn port(addr: SocketAddr) -> Self {
        Self::Port(addr)
    }

    /// Run one script and read what it wrote.
    ///
    /// `GET`, no query string, no body — which is every question this suite asks. The parameters are
    /// the CGI ones php-fpm insists on: without `SCRIPT_FILENAME` there is nothing to run, and
    /// **without `REDIRECT_STATUS` a `php-cgi` built with `cgi.force_redirect` on refuses the
    /// request outright** with a message about being called directly, which is the failure that costs
    /// an afternoon on Windows.
    ///
    /// # Errors
    ///
    /// Whatever the connection or the read reported. A pool that answered something this cannot
    /// parse is an [`std::io::ErrorKind::InvalidData`].
    pub fn get(&self, script: &Path) -> std::io::Result<Response> {
        let script = script.display().to_string();

        let mut request = Vec::new();

        let mut begin = Vec::with_capacity(8);
        begin.extend_from_slice(&RESPONDER.to_be_bytes());
        begin.extend_from_slice(&[0; 6]);
        record(&mut request, BEGIN_REQUEST, &begin);

        let mut params = Vec::new();
        pair(&mut params, "GATEWAY_INTERFACE", "CGI/1.1");
        pair(&mut params, "REQUEST_METHOD", "GET");
        pair(&mut params, "SCRIPT_FILENAME", &script);
        pair(&mut params, "SCRIPT_NAME", "/index.php");
        pair(&mut params, "REQUEST_URI", "/index.php");
        pair(&mut params, "QUERY_STRING", "");
        pair(&mut params, "CONTENT_LENGTH", "0");
        pair(&mut params, "SERVER_PROTOCOL", "HTTP/1.1");
        pair(&mut params, "SERVER_SOFTWARE", "mixengine-testkit");
        pair(&mut params, "REMOTE_ADDR", "127.0.0.1");
        pair(&mut params, "REDIRECT_STATUS", "200");
        record(&mut request, PARAMS, &params);
        // An empty record of a stream type is what closes it.
        record(&mut request, PARAMS, &[]);
        record(&mut request, STDIN, &[]);

        let answer = self.exchange(&request)?;

        parse(&answer)
    }

    /// Send the whole request and read until the connection closes.
    fn exchange(&self, request: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut answer = Vec::new();

        match self {
            #[cfg(unix)]
            Self::Socket(path) => {
                let mut stream = std::os::unix::net::UnixStream::connect(path)?;
                stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
                stream.write_all(request)?;
                stream.read_to_end(&mut answer)?;
            }

            Self::Port(addr) => {
                let mut stream = TcpStream::connect(addr)?;
                stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
                stream.write_all(request)?;
                stream.read_to_end(&mut answer)?;
            }
        }

        Ok(answer)
    }
}

/// What a script wrote, split where CGI splits it.
#[derive(Debug, Clone)]
pub struct Response {
    /// Everything before the blank line — `Content-type`, and a `Status` if the script set one.
    pub headers: String,

    /// Everything after it.
    pub body: String,
}

/// One record: an eight-byte header and a body, with no padding.
fn record(out: &mut Vec<u8>, kind: u8, body: &[u8]) {
    let length = u16::try_from(body.len()).expect("a record body under 64 KiB");

    out.push(VERSION);
    out.push(kind);
    out.extend_from_slice(&REQUEST_ID.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
    // Padding length, then one reserved byte. Nothing here pads: alignment is an optimisation for a
    // server reading millions of these, and this one sends four.
    out.push(0);
    out.push(0);
    out.extend_from_slice(body);
}

/// One name-value pair, in the protocol's two-or-eight-byte length encoding.
///
/// A length under 128 is one byte; anything longer is four with the top bit set, which is why the
/// short case is not merely an optimisation — a 200-byte value written as one byte would be read as
/// a 72-byte one and everything after it would be garbage.
fn pair(out: &mut Vec<u8>, name: &str, value: &str) {
    for length in [name.len(), value.len()] {
        let length = u32::try_from(length).expect("a parameter under 4 GiB");

        if length < 128 {
            out.push(length as u8);
        } else {
            out.extend_from_slice(&(length | 0x8000_0000).to_be_bytes());
        }
    }

    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// Pull the `STDOUT` stream out of a stack of records, and split it where CGI splits it.
///
/// `STDERR` is read and dropped rather than ignored: a pool that wrote a PHP fatal error there and
/// nothing to `STDOUT` should produce an empty body, which is what the caller then asserts against —
/// not a parse failure that says nothing about PHP.
fn parse(answer: &[u8]) -> std::io::Result<Response> {
    let invalid = |what: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("the pool answered something this is not: {what}"),
        )
    };

    let mut stdout = Vec::new();
    let mut at = 0;

    while at + 8 <= answer.len() {
        let kind = answer[at + 1];
        let length = usize::from(u16::from_be_bytes([answer[at + 4], answer[at + 5]]));
        let padding = usize::from(answer[at + 6]);
        let body = at + 8;

        if body + length > answer.len() {
            return Err(invalid("a record whose body is shorter than its header says"));
        }

        match kind {
            STDOUT => stdout.extend_from_slice(&answer[body..body + length]),
            STDERR => {}
            END_REQUEST => break,
            _ => {}
        }

        at = body + length + padding;
    }

    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let (headers, body) = stdout
        .split_once("\r\n\r\n")
        .or_else(|| stdout.split_once("\n\n"))
        .ok_or_else(|| invalid("output with no blank line between headers and body"))?;

    Ok(Response {
        headers: headers.to_owned(),
        body: body.to_owned(),
    })
}
```

Add `pub mod fastcgi;` to `crates/mixengine-testkit/src/lib.rs` and mention it in the module note's list of what lives here — it is the seventh thing, and the sentence should say why connecting to a socket is not enough.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p mixengine-testkit fastcgi
cargo clippy -p mixengine-testkit --all-targets -- -D warnings
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mixengine-testkit
git commit -m "test(testkit): a minimal FastCGI client, so a pool can be proved to serve PHP (T32)"
```

---

### Task 8: The recipe against a real PHP

**Files:**
- Create: `crates/mixengine-cli/tests/php_fpm.rs`
- Modify: `crates/mixengine-testkit/src/package.rs` (`FakePackage::directory`)
- Modify: `crates/mixengine-testkit/src/declare.rs` (`rebind`)
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: everything from Tasks 1–7.
- Produces: `FakePackage::directory(self, root: &Path) -> Self`, which packs a whole unpacked tree; `mixengine_testkit::declare::rebind(database: &Path, id: &str, port: u16)`.

- [ ] **Step 1: Give the testkit the two things this suite needs**

`FakePackage` can add one file (`file`), one built fixture (`executable`) or one named program (`program`). A PHP is a tree, so add beside them:

```rust
    /// Every file under `root`, at the path it has there.
    ///
    /// [`program`](Self::program) generalised, and for the one case it cannot cover: a runtime is a
    /// directory — `bin/`, `lib/`, `sbin/`, an `extensions/` folder on Windows — and a suite that
    /// listed its members would be describing a publisher's layout in a place that cannot check it.
    /// The executable bit is carried across on Unix, because a `php-fpm` unpacked without one is an
    /// artifact that installs and cannot be spawned.
    ///
    /// # Panics
    ///
    /// If `root` cannot be walked, which for a fixture is a broken test rather than a case.
    #[must_use]
    pub fn directory(mut self, root: &std::path::Path) -> Self {
        // …walk `root` depth-first, and for each file push the same entry `program` pushes, with
        // its path relative to `root` and its mode read from the source on Unix…
        self
    }
```

Implement it against whatever internal representation `program` already pushes into, so the two produce identical entries for one file.

And beside `reconfigure` in `declare.rs`:

```rust
/// Move a service to another port, the way nothing in the shipped product yet can.
///
/// `service.configure` does not exist — changing a row is still a direct edit, which is what this
/// module is for. It is here rather than in a suite because the *reason* is general: a port a test
/// did not choose is a port that may already be taken on the machine running it, and a fixture that
/// cannot rebind can only hope.
pub async fn rebind(database: &Path, id: &str, port: u16) {
    // …the same shape as `reconfigure`: open the store, `UPDATE services SET port = ? WHERE id = ?`,
    // assert one row was touched…
}
```

- [ ] **Step 2: Write the suite**

Create `crates/mixengine-cli/tests/php_fpm.rs`, modelled on `crates/mixengine-cli/tests/caddy.rs` line for line — the same `mod harness`, the same `MockRegistry`, the same `#[ignore]`, the same "one test rather than five" shape.

```rust
//! The php-fpm recipe against a **real** PHP — roadmap task **T32**.
//!
//! Everything else about this recipe is provable in one process and is proved there: the template
//! renders, the settings merge, the spec builds, the file and the readiness check name one socket.
//! None of that says the thing the task is about, which is that *a pool MixEngine configured serves
//! a PHP script*. That claim can only be made against the program, so this suite is made against the
//! program — and it is made through the FastCGI protocol, because a pool that is listening and
//! cannot execute anything accepts a connection exactly like one that works.
//!
//! **It is `#[ignore]`d rather than skipped**, for `caddy.rs`' reason: a test that quietly returns
//! when it finds no PHP is a green suite that proved nothing on the day the download broke.
//!
//! **The two systems diverge here on purpose, and the divergence is the assertion.** On Unix the
//! pool is php-fpm on a socket and a changed override is handed to it by `SIGUSR2` — so the same pid
//! serves the new configuration. On Windows it is `php-cgi.exe` on a port with no signal to send, so
//! the running process keeps its old configuration and the suite asserts *that* rather than
//! pretending the two are the same.

mod harness;

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use harness::{Home, json};
use mixengine_testkit::fastcgi::Pool;
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

/// Where an unpacked PHP is, as the CI step and a developer both set it.
///
/// The directory the archive unpacks to — `bin/php` inside it on Unix, `php.exe` at its root on
/// Windows — which is also what a `runtime_installs` row's `install_path` is.
const RUNTIME: &str = "MIXENGINE_PHP_RUNTIME";

/// The version the index publishes it as, and the half after the `@` in the pool's id.
const VERSION: &str = "8.3.33";

/// How long the pool is given to be serving again after its configuration moved under it.
///
/// Long for what it covers, because what it is really waiting for is a runner's next turn plus a
/// graceful pool restart on a runner that may be compiling something else at the same time.
const EVENTUALLY: Duration = Duration::from_secs(30);

/// The service this suite drives, which nobody in it creates.
fn pool() -> String {
    format!("php-fpm@{VERSION}")
}

/// The PHP this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(RUNTIME).unwrap_or_else(|| {
        panic!(
            "{RUNTIME} is not set, so there is no PHP to judge this recipe against. The `php` step \
             in .github/workflows/ci.yml fetches one; by hand, unpack any PHP 8.3 from \
             mixengine-packages' releases and point {RUNTIME} at the directory it unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
///
/// The usual race is the usual price, and here it is paid for a second reason: the pool's port is
/// allocated by the *install*, so this suite cannot choose it up front the way `caddy.rs` chooses
/// Caddy's — it rebinds afterwards instead.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// What the artifact publishes, as an index entry says it.
///
/// **Probed rather than written down**, because the layout is the publisher's: `mixengine-packages`
/// puts the Unix binaries under `bin/` and `sbin/` and the Windows ones at the root, and a suite
/// that hard-coded either would pass on one system while describing the other wrongly.
fn provides(root: &Path) -> serde_json::Map<String, Value> {
    let mut found = serde_json::Map::new();

    for (name, candidates) in [
        ("php", ["bin/php", "php"].as_slice()),
        ("php-fpm", ["sbin/php-fpm", "bin/php-fpm"].as_slice()),
        ("php-cgi", ["php-cgi", "bin/php-cgi"].as_slice()),
    ] {
        for candidate in candidates {
            let relative = format!("{candidate}{}", std::env::consts::EXE_SUFFIX);

            if root.join(&relative).is_file() {
                found.insert(name.to_owned(), Value::String(relative));
                break;
            }
        }
    }

    // The one that decides whether this suite can run at all. Named rather than left to the recipe,
    // because `ServiceProvidesNothing` arriving from three layers down at `service start` says the
    // same thing much later and much less clearly.
    let sapi = if cfg!(windows) { "php-cgi" } else { "php-fpm" };
    assert!(
        found.contains_key(sapi),
        "{} publishes no {sapi}, so there is nothing here for a pool to run — this suite needs the \
         PHP mixengine-packages builds, not a system one",
        root.display()
    );

    found
}

/// An index offering exactly this PHP, for this machine.
///
/// `"kind": "php"` and not a package name: this is a **runtime**, which is the whole difference T32
/// turns on.
fn index(packed: &Packed, url: &str, provides: serde_json::Map<String, Value>) -> Value {
    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-19T06:55:12Z",
        "packages": [{
            "kind": "php",
            "version": VERSION,
            "channel": "stable",
            "artifacts": [{
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "url": url,
                "sha256": packed.sha256,
                "size": packed.size(),
                "provides": Value::Object(provides),
            }],
        }],
    })
}

/// What `mix service status <pool>` says.
fn status(home: &Home) -> Value {
    json(&home.mix(&["service", "status", &pool(), "--json"]))
}

/// A home with a real PHP installed in it, a daemon over it, and the endpoint its pool listens on.
///
/// The archive is packed here out of the directory the CI step unpacked, served by a registry that
/// signs its own index, and installed through `runtime.install` — so this suite covers the whole
/// runtime install path against a real artifact on all three systems at no extra cost, and the pool
/// it then drives is the one the post-install hook created rather than one a fixture inserted.
async fn installed() -> (Home, harness::Daemon, MockRegistry, Pool) {
    let root = package();

    let packing = match cfg!(windows) {
        true => Packing::Zip,
        false => Packing::TarZst,
    };
    let packed = FakePackage::new(packing)
        .directory(&root)
        .build(&format!("php-{VERSION}"));

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-19T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&index(&packed, &url, provides(&root)));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());

    let installed = json(&home.mix(&["runtime", "install", "php", VERSION, "--json"]));
    assert_eq!(
        installed["state"],
        "succeeded",
        "{installed}\n{}",
        home.daemon_log()
    );

    // Where the pool listens. On Windows it is *rebound* first: the port was allocated from 9000 by
    // the install, this suite could not choose it, and a developer running two of these at once — or
    // one with a php-fpm of their own — would otherwise be fighting over a fixed number. Rebinding
    // is also the one place `services.port` is proved to be what the recipe actually reads.
    #[cfg(windows)]
    let listen = {
        let port = free_port();
        mixengine_testkit::declare::rebind(&home.database_file(), &pool(), port).await;

        Pool::port(SocketAddr::from(([127, 0, 0, 1], port)))
    };

    #[cfg(unix)]
    let listen = Pool::socket(
        home.path()
            .join("run")
            .join(format!("php-fpm-{VERSION}.sock")),
    );

    (home, daemon, registry, listen)
}

/// **The whole of T32, in the order a user meets it.**
///
/// One test rather than six, deliberately: each step is the previous one's precondition, and six
/// tests would be six real PHP installs performed to re-reach the state this one is already in.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real PHP — see the module note, and the `php` step in ci.yml"]
async fn a_pool_is_created_started_serves_php_reloaded_and_stopped() {
    let (home, _daemon, _registry, listen) = installed().await;
    let pool = pool();

    // --- created by the install, and by nobody else ----------------------------------------------
    //
    // Nothing in this test asked for a service. The post-install hook did, which is the half of T32
    // that is invisible from inside the daemon and obvious from out here.
    let listed = json(&home.mix(&["service", "list", "--json"]));
    assert!(
        listed["services"]
            .as_array()
            .is_some_and(|services| services.iter().any(|service| service["id"] == pool)),
        "installing a PHP did not give it a pool\n{listed}\n{}",
        home.daemon_log()
    );

    // --- started, and proved up by its own endpoint ------------------------------------------------
    let started = json(&home.mix(&["service", "start", &pool, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let up = status(&home);
    assert_eq!(up["state"], "running", "{up}\n{}", home.daemon_log());
    let pid = up["pid"].as_u64().expect("a running pool has a pid");

    // --- serving PHP -------------------------------------------------------------------------------
    //
    // **The assertion this whole suite exists for.** A pool that is listening and cannot execute
    // anything accepts a connection exactly like one that works, so the claim is made by sending a
    // real FastCGI request and reading back a body only PHP could have produced.
    let script = home.path().join("www").join("hello.php");
    std::fs::create_dir_all(script.parent().expect("a parent")).expect("a document root");
    std::fs::write(
        &script,
        b"<?php echo 'mixengine serves php ', PHP_VERSION, \"\\n\";",
    )
    .expect("a script to serve");

    let answered = listen
        .get(&script)
        .expect("the pool answered a FastCGI request");
    assert!(
        answered.body.contains("mixengine serves php"),
        "the pool is listening and is not running PHP\n{answered:?}\n{}",
        home.daemon_log()
    );
    assert!(
        answered.body.contains(VERSION),
        "the pool is serving a PHP that is not the one installed: {answered:?}"
    );

    // --- handed a configuration that moved under it -------------------------------------------------
    mixengine_testkit::declare::reconfigure(
        &home.database_file(),
        &pool,
        r#"{"max_children": 3}"#,
    )
    .await;

    // Nothing but a listing: the configuration is rendered at the top of every `service.*` call, and
    // a rendering that moved under a running service is handed to it. Nothing here restarts anything.
    let relisted = json(&home.mix(&["service", "list", "--json"]));
    assert!(relisted["services"].is_array(), "{relisted}");

    let deadline = Instant::now() + EVENTUALLY;
    loop {
        if listen
            .get(&script)
            .is_ok_and(|answer| answer.body.contains("mixengine serves php"))
        {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "the pool stopped serving after its configuration changed\n{}",
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let after = status(&home);
    assert_eq!(after["state"], "running", "{after}\n{}", home.daemon_log());
    assert_eq!(
        after["pid"].as_u64(),
        Some(pid),
        "the pool was replaced rather than left alone — which on a system with signals is the cost \
         the whole task avoids, and on one without is a restart nobody asked for: {after}"
    );

    // **The two systems say different things here, and the difference is the assertion.** Unix sent
    // `SIGUSR2` and the master cycled its workers onto the new file; Windows has no signal to send
    // and left the running process on its previous configuration, out loud, in the log. Asserting
    // the second rather than skipping it is what stops a silent regression into "reload did nothing"
    // looking like a pass on the system that reloads.
    let log = home.daemon_log();
    if cfg!(unix) {
        assert!(
            log.contains("re-read its configuration"),
            "no reload was recorded on a system that has signals\n{log}"
        );
    } else {
        assert!(
            log.contains("previous configuration"),
            "Windows either reloaded something it cannot reload, or said nothing about not \
             having\n{log}"
        );
    }

    // --- stopped, with nothing left holding the endpoint ---------------------------------------------
    //
    // The workers are the point: on Unix they are in the master's process group and on Windows they
    // were measured to go with it, and a child left behind is a child the next start collides with.
    let stopped = json(&home.mix(&["service", "stop", &pool, "--json"]));
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );

    assert!(
        listen.get(&script).is_err(),
        "something is still answering on the pool's endpoint after it was stopped\n{}",
        home.daemon_log()
    );

    // --- and removed with the PHP it ran out of ---------------------------------------------------
    let uninstalled = home.mix(&["runtime", "uninstall", "php", VERSION, "--json"]);
    assert!(
        uninstalled.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&uninstalled.stderr),
        home.daemon_log()
    );

    let remaining = json(&home.mix(&["service", "list", "--json"]));
    assert!(
        remaining["services"]
            .as_array()
            .is_some_and(|services| services.iter().all(|service| service["id"] != pool)),
        "the pool outlived the PHP it ran out of\n{remaining}"
    );
}

/// A PHP that is still serving is not removed out from under itself.
///
/// **The first refusal `runtime.uninstall` has ever been able to make**, which is why it gets its
/// own test rather than a line in the one above: what it needs is a *running* pool, and that test
/// ends with a stopped one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real PHP — see the module note, and the `php` step in ci.yml"]
async fn a_running_pool_refuses_to_have_its_php_removed() {
    let (home, _daemon, _registry, _listen) = installed().await;
    let pool = pool();

    let started = json(&home.mix(&["service", "start", &pool, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let refused = home.mix(&["runtime", "uninstall", "php", VERSION, "--json"]);
    assert!(
        !refused.status.success(),
        "a PHP was removed out from under the pool serving out of it\n{}",
        home.daemon_log()
    );

    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains(&pool) && said.contains("stop"),
        "a refusal has to name the thing in the way and the command that clears it: {said}"
    );

    // The other half, and the one a refusal that half-happened would fail: nothing was removed.
    let runtimes = json(&home.mix(&["runtime", "list", "--json"]));
    assert!(
        runtimes.to_string().contains(VERSION),
        "the runtime was removed by a call that refused: {runtimes}"
    );

    home.mix(&["service", "stop", &pool, "--json"]);
}
```

- [ ] **Step 3: Run it against a PHP by hand**

```bash
# unpack any PHP 8.3 from mixengine-packages' releases into <dir>
MIXENGINE_PHP_RUNTIME=<dir> cargo test -p mixengine-cli --test php_fpm -- --ignored --nocapture
```
Expected: PASS. Fix what it finds — this step is where the recipe meets the real program, and it is the point of the whole task.

- [ ] **Step 4: Add the CI steps**

In `.github/workflows/ci.yml`, after the "Fetch a real Caddy" step, add a "Fetch a real PHP" step in the same shape — same `case "$RUNNER_OS-$RUNNER_ARCH"` table, same `curl`/`tar` split, the release tag being `php-$PHP` and the asset `php-$PHP-$target.$ext`, ending with:

```bash
          echo "MIXENGINE_PHP_RUNTIME=$into" >> "$GITHUB_ENV"
          "$into/bin/php$( [ "$RUNNER_OS" = "Windows" ] && echo .exe )" --version || \
            "$into/php$( [ "$RUNNER_OS" = "Windows" ] && echo .exe )" --version
```

Pin `PHP: "8.3.33"`. Add a comment saying what this fetch buys that the Caddy one does not: php-fpm and php-cgi are **different SAPIs with different mechanisms**, and a recipe that renders correctly on both is not a recipe that runs on both.

Then, beside "Test against a real Caddy":

```yaml
      - name: Test against a real PHP
        if: runner.os != 'Linux'
        env:
          CARGO_NET_OFFLINE: "true"
        run: cargo test -p mixengine-cli --test php_fpm --locked --offline -- --ignored
```

and add `--test php_fpm -- --ignored` to whatever `.github/scripts/test-no-network.sh` already runs for the Caddy suite on Linux, matching how that script invokes it.

- [ ] **Step 5: Commit**

```bash
git add crates/mixengine-cli/tests/php_fpm.rs .github
git commit -m "test(services): judge the php-fpm recipe against a real PHP on all three systems (T32)"
```

---

### Task 9: Documentation, and the roadmap

**Files:**
- Modify: `.claude/roadmap/phase-3-services.md:225`
- Modify: `.claude/architecture/data-model.md:62`
- Modify: `.claude/features/services.md:31` and its "reload beats restart" paragraph
- Modify: `.claude/features/runtime-versions.md:68, :92, :97`
- Modify: `.claude/roadmap/todo.md` (Phase 3's count)

- [ ] **Step 1: Correct the short-form ids**

`.claude/architecture/data-model.md:62` and `.claude/features/services.md:31` both show `php-fpm@8.3`. Change both to `php-fpm@8.3.33` and add the reason in the surrounding sentence: `runtime_installs` is `UNIQUE (kind, version)` over the **full** version, so two patch releases of one minor can both be installed and a short id would name neither.

- [ ] **Step 2: Correct the pool-per-site sketch**

`.claude/features/services.md`'s catalogue row and its `php-fpm/8.3/pool.d/<site>.conf` line describe a pool per site. Replace with: one pool per installed PHP version, shared by every site on it, because a pool per site is Unix-only vocabulary and Windows has one master with one set of children. Keep `pool.d/*.conf` as the include that matches nothing until Phase 4, and say why it is rendered here (the glob resolves against the file it is written in).

- [ ] **Step 3: Tick T32**

Replace `.claude/roadmap/phase-3-services.md:225` with a `- [x]` entry in the house style of T31 and T31a: what it closes, what was measured rather than assumed (the `php-cgi` table from the design doc), what it settled (two parents, one recipe two spec shapes, `ReloadBehaviour::Signal`, one pool per version), and **what it deliberately left undone** — `request_terminate_timeout` on Windows, `php.ini`/`conf.d` (T28's), no site until Phase 4, no `--force` on `runtime.uninstall`, and no orphan removal under `etc/<id>/`.

Update `.claude/roadmap/todo.md`'s Phase 3 count from `5 / 12` to `6 / 12`.

- [ ] **Step 4: Note what T28 inherits**

In `.claude/roadmap/phase-2-runtimes.md:651` (T28), add one sentence: T32 has landed, so per-pool reload now exists and `PHP_INI_SCAN_DIR` was measured to work on all three systems, which is the road the `conf.d` model takes.

- [ ] **Step 5: Check the intra-doc links**

```bash
cargo doc --workspace --no-deps --document-private-items
```
Expected: no warnings. `rustdoc::all` is denied workspace-wide, so a link to a renamed item fails here rather than in CI.

- [ ] **Step 6: Commit**

```bash
git add .claude
git commit -m "docs(roadmap): tick T32, and correct the pool ids the sketches guessed at"
```

---

## After the plan

Push the branch and request CI — `master` builds itself, any other branch is pushed and then asked (`.claude/operations/build-and-release.md`). The `php_fpm` suite is the one that has never run on macOS or on ARM64 on this machine, and it is the reason to ask before opening anything.
