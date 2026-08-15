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

use std::path::PathBuf;

use mixengine_proto::{ResourceLimits, ServiceId, ServiceSpec};

pub mod document;
pub mod recipe;
pub mod settings;

pub use document::{Document, Validator, Written};
pub use recipe::{Catalogue, Context, Recipe, TemplateFile};
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
}

impl Generated {
    /// Whether anything on disk is different from what it was.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.files.iter().any(|(_, written)| written.changed())
    }
}

/// One `services` row joined to the package it belongs to.
///
/// A struct rather than the query's own anonymous record, because two call sites read it and
/// `sqlx::query!` gives each of them a different type.
#[derive(Debug)]
struct Row {
    id: String,
    instance_name: String,
    port: Option<i64>,
    bind_addr: String,
    data_dir: Option<String>,
    overrides: String,
    limits: String,
    package: String,
    version: String,
    install_path: String,
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
    /// **All or nothing.** One row that cannot be generated fails the call rather than being left
    /// out of the answer, because a service that silently disappears from `mix service list` is a
    /// service somebody goes looking for in the wrong place. What they get instead is a message
    /// naming the row and what is wrong with it.
    ///
    /// # Errors
    ///
    /// [`Error::Database`] when the rows cannot be read; and per row, whatever
    /// [`generate`](Self::generate) reports.
    pub async fn declared(&self) -> Result<Vec<ServiceSpec>> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT s.id                    AS "id!: String",
                      s.instance_name         AS "instance_name!: String",
                      s.port                  AS "port: i64",
                      s.bind_addr             AS "bind_addr!: String",
                      s.data_dir              AS "data_dir: String",
                      s.config_overrides_json AS "overrides!: String",
                      s.limits_json           AS "limits!: String",
                      p.name                  AS "package!: String",
                      p.version               AS "version!: String",
                      p.install_path          AS "install_path!: String"
               FROM services s
               JOIN packages p ON p.id = s.package_id
               ORDER BY s.id"#
        )
        .fetch_all(self.store.pool())
        .await
        .map_err(|source| self.store.failure("read", source))?;

        let mut specs = Vec::with_capacity(rows.len());

        for row in rows {
            specs.push(self.render(row).await?.spec);
        }

        Ok(specs)
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
                      p.name                  AS "package!: String",
                      p.version               AS "version!: String",
                      p.install_path          AS "install_path!: String"
               FROM services s
               JOIN packages p ON p.id = s.package_id
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
    async fn render(&self, row: Row) -> Result<Generated> {
        let service =
            ServiceId::parse(row.id.clone()).map_err(|source| Error::UnreadableServiceRow {
                service: row.id.clone(),
                column: "id",
                value: source.to_string(),
            })?;

        let recipe = self
            .catalogue
            .recipe(&row.package)
            .ok_or_else(|| Error::NoRecipe {
                service: row.id.clone(),
                package: row.package.clone(),
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

        let context = Context {
            etc: self.paths.etc().join(service.as_str()),

            // The row wins, and the fallback is `data/<package>/<instance>` rather than
            // `data/<service-id>`: two instances of one server are `mariadb@main` and
            // `mariadb@legacy`, and a directory named after the *package* is what makes them
            // siblings in a listing rather than two unrelated names.
            data: row.data_dir.map_or_else(
                || {
                    self.paths
                        .data()
                        .join(&row.package)
                        .join(&row.instance_name)
                },
                PathBuf::from,
            ),
            run: self.paths.run().to_path_buf(),
            logs: self.paths.service_logs(&service),
            package: row.package,
            version: row.version,
            install_path: PathBuf::from(row.install_path),
            port,
            bind: row.bind_addr,
            settings,
            service,
        };

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

        Ok(Generated { spec, files })
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

    /// A home with a database, a package row and one service row.
    async fn home(overrides: &str) -> (tempfile::TempDir, Generator) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let paths = Paths::new(directory.path().to_path_buf(), &PathOverrides::default());
        let store = Store::open(paths.database_file())
            .await
            .expect("a database");

        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES ('fakeservice', '1.0.0', '/packages/fakeservice', '2026-08-15T00:00:00Z',
                     'https://example', 'ab')",
        )
        .execute(store.pool())
        .await
        .expect("a package row");

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state, port,
                                   config_overrides_json)
             VALUES ('fakeservice@main', (SELECT id FROM packages WHERE name = 'fakeservice'),
                     'main', 'stopped', 4321, ?)",
        )
        .bind(overrides)
        .execute(store.pool())
        .await
        .expect("a services row");

        let catalogue = Catalogue::builtin().with(Arc::new(Fake));

        (directory, Generator::new(paths, store, catalogue))
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
}
