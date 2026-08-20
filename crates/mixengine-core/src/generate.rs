//! Turning declared state into configuration on disk, and into something the supervisor can run.
//!
//! Roadmap task **T30**. Two halves of one job, which is why they are one module:
//!
//! - **Config generation.** Every managed service's configuration is rendered from a template plus
//!   the user's overrides into `etc/<service-id>/`, atomically, skipping what has not changed and
//!   installing nothing that fails validation. That is [`document`] and [`settings`].
//! - **The `ServiceSpec` in front of it.** A `services` row is not a spec — it carries
//!   `package_id`, `port`, `data_dir`, `config_overrides_json` and `limits_json` — and what turns
//!   one into a runnable specification is the same knowledge that renders its config: a [`recipe`].
//!
//! So this module is also the answer to the port `mixengine-daemon` has been asking through since
//! T19 (`SpecSource`), and the thing that answers it is [`Generator`]. Before this, the daemon's
//! shipped source was `Undeclared` — an empty set, honest for a build that could not render a spec
//! at all.
//!
//! # What is generated is never read back
//!
//! `CLAUDE.md`'s rule, and this module is where it is kept: nothing here parses a file under `etc/`
//! into state. The only read is [`document::install`] comparing a rendering against what is on disk
//! to decide whether to write it, which is a *checksum* rather than a parse — it can tell you the
//! file is the one we would write, and it can tell you nothing about a file that is not.
//!
//! # Rendered on every walk
//!
//! [`Generator::declared`] renders and installs, and the registry asks it at the top of every
//! `service.*` call. That sounds like a lot of writing and is almost none: the diff in
//! [`document::install`] means a home whose state has not changed does no I/O beyond reading each
//! file once. The alternative — rendering only when something is started — leaves the two able to
//! disagree, and the way that is discovered is a service reloading a config from before the change
//! the user just made.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mixengine_proto::{ResourceLimits, ServiceId, ServiceSpec};

pub mod document;
pub mod first_run;
pub mod recipe;
pub mod recipes;
pub mod settings;

pub use document::{Document, Validator, Written};
pub use first_run::{DataDirectory, FirstRun, Ritual, SecretSpec, Step};
pub use recipe::{Catalogue, Context, Endpoints, Instancing, Recipe, Source, TemplateFile};
pub use recipes::{Caddy, Mariadb, PhpFpm};
pub use settings::{Preset, Setting, Settings, Value};

use crate::{Error, Paths, Result, Store};

/// The `services` table, rendered.
///
/// Holds the home's layout, its database and the recipes this build can find — everything needed to
/// answer "what does this home declare" with no argument at all, which is what the daemon's
/// `SpecSource` is asked.
#[derive(Debug, Clone)]
pub struct Generator {
    /// Where `etc/`, `data/`, `run/` and `logs/` are.
    paths: Paths,

    /// The declared state.
    store: Store,

    /// What this build knows how to run.
    catalogue: Catalogue,
}

/// One service, generated: what it will run, and what changed on the way.
#[derive(Debug, Clone)]
pub struct Generated {
    /// What the supervisor is to run.
    pub spec: ServiceSpec,

    /// Every file the recipe renders, and what installing it did.
    ///
    /// What a reload decision is made of, which is the reason it is reported at all — see
    /// [`Written::changed`].
    pub files: Vec<(PathBuf, Written)>,

    /// What has to happen once before this service is ever started, if anything.
    ///
    /// Computed here because this is the only place both halves are in hand — the recipe, and a
    /// [`Context`] built from the row. Assembling it costs a clone of that context and nothing else;
    /// it is *performed* by the daemon, once, and only when the markers say it has not been.
    pub first_run: Option<FirstRun>,
}

impl Generated {
    /// Whether anything on disk is different from what it was.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.files.iter().any(|(_, written)| written.changed())
    }
}

/// One `services` row joined to **both** of the tables a parent could be in.
///
/// A struct rather than the query's own anonymous record, because two call sites read it and
/// `sqlx::query!` gives each of them a different type.
///
/// Seven nullable columns rather than three, because the join that did not match contributes nulls
/// and the `CHECK` on `services` guarantees exactly one of the two groups is whole. Resolving that
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
    package_provides: Option<String>,
    runtime: Option<String>,
    runtime_version: Option<String>,
    runtime_path: Option<String>,
    runtime_provides: Option<String>,
}

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
    /// **A package publishes a `provides` map now, and a row written before migration 0004 carries
    /// an empty one** — which is honest rather than a placeholder. A Caddy installed before that
    /// column existed is served by [`Context::program`], which asks this map nothing. What an empty
    /// map costs is a recipe that does ask — MariaDB, whose seven commands are not one binary at the
    /// install root — and the answer it gets names the reinstall that would fill it in. See
    /// [`Context::provided`].
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
            row.package_provides.take(),
        ) {
            (Some(package), Some(version), Some(install_path), Some(provides)) => {
                let provides = serde_json::from_str(&provides).map_err(|source| {
                    Error::UnreadableServiceDocument {
                        service: service.as_str().to_owned(),
                        column: "provides_json",
                        source,
                    }
                })?;

                return Ok(Self {
                    package,
                    version,
                    install_path,
                    provides,
                });
            }
            (None, None, None, None) => {}
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

impl Generator {
    /// A generator for this home.
    #[must_use]
    pub fn new(paths: Paths, store: Store, catalogue: Catalogue) -> Self {
        Self {
            paths,
            store,
            catalogue,
        }
    }

    /// Every service this home declares, configured and ready to run.
    ///
    /// **The whole set**, because the caller's next move is a
    /// [`ServiceGraph`](crate::services::ServiceGraph): dependencies, cycles and start order are
    /// properties of a set.
    ///
    /// **[`Generated`] and not [`ServiceSpec`]**, because the specification is only half of what a
    /// walk needs to know. The other half is whether anything on disk moved — [`Generated::changed`]
    /// — which is what turns "the configuration is up to date" into "the process reading it has been
    /// told", and it is knowledge only this call has: by the time a spec is in a caller's hands the
    /// file has already been written and a second look at it would compare a rendering with itself.
    ///
    /// **All or nothing.** One row that cannot be generated fails the call rather than being left
    /// out of the answer, because a service that silently disappears from `mix service list` is a
    /// service somebody goes looking for in the wrong place. What they get instead is a message
    /// naming the row and what is wrong with it.
    ///
    /// # Errors
    ///
    /// [`Error::Database`] when the rows cannot be read; and per row, whatever
    /// [`generate`](Self::generate) reports.
    pub async fn declared(&self) -> Result<Vec<Generated>> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT s.id                    AS "id!: String",
                      s.instance_name         AS "instance_name!: String",
                      s.port                  AS "port: i64",
                      s.bind_addr             AS "bind_addr!: String",
                      s.data_dir              AS "data_dir: String",
                      s.config_overrides_json AS "overrides!: String",
                      s.limits_json           AS "limits!: String",
                      p.name                  AS "package: String",
                      p.version               AS "package_version: String",
                      p.install_path          AS "package_path: String",
                      p.provides_json         AS "package_provides: String",
                      r.kind                  AS "runtime: String",
                      r.version               AS "runtime_version: String",
                      r.install_path          AS "runtime_path: String",
                      r.provides_json         AS "runtime_provides: String"
               FROM services s
               LEFT JOIN packages p         ON p.id = s.package_id
               LEFT JOIN runtime_installs r ON r.id = s.runtime_install_id
               ORDER BY s.id"#
        )
        .fetch_all(self.store.pool())
        .await
        .map_err(|source| self.store.failure("read", source))?;

        let mut generated = Vec::with_capacity(rows.len());

        for row in rows {
            generated.push(self.render(row).await?);
        }

        Ok(generated)
    }

    /// Generate one service's configuration and specification.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when there is no such service; [`Error::Database`] when the row cannot be
    /// read; and everything rendering the row itself can report — a package with no recipe, an
    /// override that names nothing, a template that will not render, a configuration the service's
    /// own checker refuses.
    pub async fn generate(&self, service: &ServiceId) -> Result<Generated> {
        let id = service.as_str();

        let row = sqlx::query_as!(
            Row,
            r#"SELECT s.id                    AS "id!: String",
                      s.instance_name         AS "instance_name!: String",
                      s.port                  AS "port: i64",
                      s.bind_addr             AS "bind_addr!: String",
                      s.data_dir              AS "data_dir: String",
                      s.config_overrides_json AS "overrides!: String",
                      s.limits_json           AS "limits!: String",
                      p.name                  AS "package: String",
                      p.version               AS "package_version: String",
                      p.install_path          AS "package_path: String",
                      p.provides_json         AS "package_provides: String",
                      r.kind                  AS "runtime: String",
                      r.version               AS "runtime_version: String",
                      r.install_path          AS "runtime_path: String",
                      r.provides_json         AS "runtime_provides: String"
               FROM services s
               LEFT JOIN packages p         ON p.id = s.package_id
               LEFT JOIN runtime_installs r ON r.id = s.runtime_install_id
               WHERE s.id = ?"#,
            id
        )
        .fetch_optional(self.store.pool())
        .await
        .map_err(|source| self.store.failure("read", source))?
        .ok_or_else(|| Error::NotFound {
            kind: "service",
            id: id.to_owned(),
        })?;

        self.render(row).await
    }

    /// One row, all the way to a spec: look up the recipe, merge the overrides, render, install,
    /// build.
    ///
    /// The order is forced and each step depends on the last. Installing before building the spec
    /// is the one that could be argued: a spec that will not build leaves a configuration on disk
    /// for a service that cannot start. That is the right way round — the config is what a person
    /// reads to work out *why* it will not start, and a spec that does not build is a bug in a
    /// recipe rather than a state anybody has to recover from.
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

        let port = match row.port {
            None => None,
            Some(port) => Some(
                u16::try_from(port).map_err(|_| Error::UnreadableServiceRow {
                    service: row.id.clone(),
                    column: "port",
                    value: port.to_string(),
                })?,
            ),
        };

        let settings = Settings::merge(recipe.settings(), &row.overrides, &service)?;

        let limits: ResourceLimits = serde_json::from_str(&row.limits).map_err(|source| {
            Error::UnreadableServiceDocument {
                service: row.id.clone(),
                column: "limits_json",
                source,
            }
        })?;

        let mut context = Context {
            etc: self.paths.etc().join(service.as_str()),
            etc_root: self.paths.etc().to_path_buf(),

            // The row wins, and the fallback is the package's rather than `data/<service-id>`: two
            // instances of one server are `mariadb@main` and `mariadb@legacy`, and a directory named
            // after the *package* is what makes them siblings in a listing rather than two unrelated
            // names. How far down that goes is the recipe's answer, not this function's.
            data: row.data_dir.map_or_else(
                || match recipe.instancing() {
                    // A server that exists once has no instance half to spend, and `data/caddy/caddy`
                    // reads as a mistake to whoever finds it.
                    Instancing::Single => self.paths.data().join(&parent.package),
                    Instancing::Named => self
                        .paths
                        .data()
                        .join(&parent.package)
                        .join(&row.instance_name),
                },
                PathBuf::from,
            ),
            run: self.paths.run().to_path_buf(),
            logs: self.paths.service_logs(&service),
            package: parent.package,
            version: parent.version,
            install_path: PathBuf::from(parent.install_path),
            provides: parent.provides,
            port,
            bind: row.bind_addr,
            settings,
            endpoints: recipe::Endpoints::default(),
            secrets: BTreeMap::new(),
            service,
        };

        // Asked once and stored, rather than recomputed by the template and again by the spec: the
        // whole point is that there is one answer. Before the render, because the template reads it.
        context.endpoints = recipe.endpoints(&context)?;

        // Before the render is judged, because a validator judges a *running* configuration and a
        // running configuration names places. php-fpm opens its `error_log` during `--test` and
        // fails the whole file when the directory is not there — and the service log directory is
        // otherwise created by the log sink, which is to say at the first start, which is after
        // this. The supervisor still creates it: this is the earlier of two idempotent calls, not a
        // move of the responsibility.
        crate::paths::create_dir(&context.logs)?;

        let documents = recipe::render(recipe.as_ref(), &context)?;
        let written = document::install(
            &context.etc,
            &documents,
            recipe.validator(&context).as_ref(),
        )
        .await?;

        let files = documents
            .iter()
            .map(|document| document.relative().to_path_buf())
            .zip(written)
            .collect();

        let spec = recipe
            .spec(&context)?
            .limits(limits)
            .build()
            .map_err(|source| Error::Unrunnable {
                service: context.service.as_str().to_owned(),
                source,
            })?;

        let first_run = recipe
            .ritual()
            .map(|ritual| FirstRun::new(&context, ritual));

        Ok(Generated {
            spec,
            files,
            first_run,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mixengine_proto::{Millis, ReadyCheck, RestartPolicy, ServiceSpecBuilder, StopBehaviour};
    use mixengine_testkit::FakeService;

    use super::*;
    use crate::config::PathOverrides;

    /// A recipe over `fakeservice`, with one file and one setting.
    ///
    /// Deliberately not the daemon's own fixture recipe: this one is about the *generator*, so it
    /// renders something whose content is worth asserting and takes a setting worth overriding.
    #[derive(Debug)]
    struct Fake;

    const GREETING: &str = "greeting";

    impl Recipe for Fake {
        fn package(&self) -> &'static str {
            "fakeservice"
        }

        fn instancing(&self) -> Instancing {
            Instancing::Named
        }

        fn settings(&self) -> &'static [Setting] {
            &[Setting {
                key: GREETING,
                default: Preset::Text("hello"),
            }]
        }

        fn files(&self) -> &'static [TemplateFile] {
            &[TemplateFile {
                path: "fakeservice.conf",
                source: "say = {{ settings.greeting }}\nport = {{ service.port }}\n{{ extra }}",
            }]
        }

        fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
            Ok(
                ServiceSpec::builder(context.service().clone(), FakeService::program())
                    .cwd(context.data())
                    .ready(ReadyCheck::PidAlive { settle: Millis(10) })
                    .restart(RestartPolicy::Never)
                    .stop(StopBehaviour::Signal { grace: Millis(500) }),
            )
        }
    }

    /// A recipe for a package that exists once, so its id carries no `@`.
    ///
    /// Nothing but the instancing and the working directory: what it is here to show is where the
    /// generator puts a singleton's data, and a template would only be noise around that.
    #[derive(Debug)]
    struct Solo;

    impl Recipe for Solo {
        fn package(&self) -> &'static str {
            "solo"
        }

        fn instancing(&self) -> Instancing {
            Instancing::Single
        }

        fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
            Ok(
                ServiceSpec::builder(context.service().clone(), FakeService::program())
                    .cwd(context.data())
                    .ready(ReadyCheck::PidAlive { settle: Millis(10) })
                    .restart(RestartPolicy::Never)
                    .stop(StopBehaviour::Signal { grace: Millis(500) }),
            )
        }
    }

    /// A home with a database, a package row and one service row for `recipe`'s package.
    ///
    /// `instance` is the `instance_name` column rather than something derived here, because what a
    /// row puts in it is exactly what these tests are about: an instance of a named package writes
    /// the half after the `@`, and a package that exists once writes its own name.
    async fn home_of(
        recipe: Arc<dyn Recipe>,
        id: &str,
        instance: &str,
        overrides: &str,
    ) -> (tempfile::TempDir, Generator) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let paths = Paths::new(directory.path().to_path_buf(), &PathOverrides::default());
        let store = Store::open(paths.database_file())
            .await
            .expect("a database");
        let package = recipe.package();

        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES (?, '1.0.0', '/packages/x', '2026-08-15T00:00:00Z', 'https://example', 'ab')",
        )
        .bind(package)
        .execute(store.pool())
        .await
        .expect("a package row");

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state, port,
                                   config_overrides_json)
             VALUES (?, (SELECT id FROM packages WHERE name = ?), ?, 'stopped', 4321, ?)",
        )
        .bind(id)
        .bind(package)
        .bind(instance)
        .bind(overrides)
        .execute(store.pool())
        .await
        .expect("a services row");

        let catalogue = Catalogue::builtin().with(recipe);

        (directory, Generator::new(paths, store, catalogue))
    }

    /// A home holding one `fakeservice@main`, which is what most of these tests want.
    async fn home(overrides: &str) -> (tempfile::TempDir, Generator) {
        home_of(Arc::new(Fake), "fakeservice@main", "main", overrides).await
    }

    #[tokio::test]
    async fn a_row_becomes_a_spec_and_a_file_on_disk() {
        let (_home, generator) = home("{}").await;

        let generated = generator
            .generate(&ServiceId::parse("fakeservice@main").expect("an id"))
            .await
            .expect("a generated service");

        assert_eq!(generated.spec.id().as_str(), "fakeservice@main");
        assert!(generated.changed(), "nothing was written on a first render");

        let rendered = std::fs::read_to_string(
            generator
                .paths
                .etc()
                .join("fakeservice@main")
                .join("fakeservice.conf"),
        )
        .expect("the rendered file");

        assert!(rendered.contains("say = hello"), "{rendered}");
        assert!(rendered.contains("port = 4321"), "{rendered}");
    }

    #[tokio::test]
    async fn an_override_reaches_the_rendered_file() {
        let (_home, generator) = home(r##"{"greeting": "guten tag", "extra": "# mine\n"}"##).await;

        generator.declared().await.expect("the declared set");

        let rendered = std::fs::read_to_string(
            generator
                .paths
                .etc()
                .join("fakeservice@main")
                .join("fakeservice.conf"),
        )
        .expect("the rendered file");

        assert!(rendered.contains("say = guten tag"), "{rendered}");
        assert!(rendered.contains("# mine"), "{rendered}");
    }

    /// The second walk over an unchanged home is what the registry does on every `service.*` call.
    #[tokio::test]
    async fn generating_twice_changes_nothing_the_second_time() {
        let (_home, generator) = home("{}").await;

        generator.declared().await.expect("a first walk");
        let again = generator
            .generate(&ServiceId::parse("fakeservice@main").expect("an id"))
            .await
            .expect("a second walk");

        assert!(!again.changed(), "an unchanged home rewrote its config");
    }

    /// The failure a home meets after an upgrade that dropped a recipe, and the one every home
    /// meets today for a package this build does not know.
    #[tokio::test]
    async fn a_package_with_no_recipe_names_itself_and_what_is_known() {
        let (_home, generator) = home("{}").await;

        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES ('meilisearch', '1.0.0', '/packages/x', '2026-08-15T00:00:00Z',
                     'https://example', 'ab');
             INSERT INTO services (id, package_id, instance_name, state)
             VALUES ('meilisearch@main', (SELECT id FROM packages WHERE name = 'meilisearch'),
                     'main', 'stopped')",
        )
        .execute(generator.store.pool())
        .await
        .expect("a service belonging to something this build cannot run");

        let error = generator
            .declared()
            .await
            .expect_err("a package with no recipe");

        let message = error.to_string();
        assert!(message.contains("meilisearch"), "{message}");
    }

    /// A singleton's data directory is `data/<package>`, and not `data/<package>/<package>`.
    ///
    /// The fallback was written for the case that has an instance name to spend. A recipe that
    /// exists once has no such half, and repeating the package name reads as a mistake to whoever
    /// meets it in a directory listing.
    #[tokio::test]
    async fn a_single_instance_recipe_keeps_its_data_directly_under_the_package() {
        let (_home, generator) = home_of(Arc::new(Solo), "solo", "solo", "{}").await;

        let generated = generator
            .generate(&ServiceId::parse("solo").expect("an id"))
            .await
            .expect("a generated service");

        assert_eq!(generated.spec.cwd(), generator.paths.data().join("solo"));
    }

    /// A named-instance recipe keeps the shape it always had: siblings under one package.
    #[tokio::test]
    async fn a_named_instance_recipe_keeps_its_data_under_the_instance() {
        let (_home, generator) = home("{}").await;

        let generated = generator
            .generate(&ServiceId::parse("fakeservice@main").expect("an id"))
            .await
            .expect("a generated service");

        assert_eq!(
            generated.spec.cwd(),
            generator.paths.data().join("fakeservice").join("main")
        );
    }

    /// A row that is fine except for its overrides fails as *that service*, not as a broken home.
    #[tokio::test]
    async fn a_misspelled_override_names_the_service_it_is_on() {
        let (_home, generator) = home(r#"{"greting": "hello"}"#).await;

        let error = generator
            .declared()
            .await
            .expect_err("a misspelled setting");
        let message = error.to_string();

        assert!(message.contains("fakeservice@main"), "{message}");
        assert!(message.contains("greting"), "{message}");
    }
    /// A service whose binary comes from an installed runtime renders exactly like one whose binary
    /// comes from a package.
    ///
    /// What is being asserted is the join and nothing else: the recipe is the same [`Fake`], the
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
            r#"INSERT INTO runtime_installs
                   (kind, version, channel, install_path, installed_at, size_bytes, source_url,
                    sha256, provides_json)
               VALUES ('php', '8.3.33', 'stable', '/runtimes/php/8.3.33', '2026-08-19T00:00:00Z',
                       1, 'https://example.invalid/php', 'abc', '{"php-fpm":"sbin/php-fpm"}')"#,
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
}
