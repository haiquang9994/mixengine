//! Turning a project that already works into a manifest.
//!
//! Roadmap task **T77**. The whole difficulty is in one sentence of the feature doc: capture *what
//! is actually in use*, **not the global defaults**, and never data, credentials or absolute paths.
//! Each of the rules below is that sentence applied to one thing this home knows.
//!
//! # Where each key comes from
//!
//! | Key | Read from |
//! |---|---|
//! | `[runtimes]` | [`crate::resolve`], keeping only what the project or its manifest decided (D4a) |
//! | `[site]` | the project's one `sites` row, its `site_domains`, and nothing about ports |
//! | `[[services]]` | `site_service_links`, minus the front end and minus the pool itself |
//! | `[[services]] database`, `user` | the project's own `mixengine.toml` (D3) |
//! | `[php] extensions` | the *choices* on the PHP the pool runs, which are already deviations (D2) |
//!
//! # What never comes out of here
//!
//! `root_path` and every other absolute path; passwords, keyring entries and database contents;
//! `http_port` and `https_port`, which are properties of this machine's front end; LAN sharing,
//! which is something a person turns on for a machine and a moment; and `[scaffold]`, because
//! capture does not invent a command to run on somebody else's computer.
//!
//! The test that holds this is written against the **rendered string** rather than against the
//! struct: asserting on fields would only prove that the ones we remembered are empty.

use std::collections::BTreeMap;

use mixengine_proto::{RuntimeKind, RuntimeSource, ServiceId, SiteKind, VersionConstraint};

use crate::blueprints::manifest::{
    BlueprintManifest, BlueprintService, BlueprintSite, Header, PER_PROJECT, Php, Provenance,
    SCHEMA,
};
use crate::projects::ProjectRecord;
use crate::{Error, Result, Store, manifest, resolve, services, sites};

/// The token a project's own name is replaced by.
const TOKEN: &str = "{project}";

/// Packages that serve every site on the machine rather than belonging to one project.
///
/// Left out of `[[services]]` because they belong to whoever receives the blueprint: a machine has
/// one front end, and a blueprint that asked for a second would be describing this home rather than
/// the project captured from it.
const FRONT_ENDS: [&str; 2] = ["caddy", "nginx"];

/// The package a php-fpm pool is an instance of.
///
/// Also left out, and for a different reason: the pool is already said by `[runtimes] php` together
/// with `kind = "php-fpm"`, so a `[[services]]` entry for it would be the same fact written twice —
/// and the second copy would carry this machine's instance name.
const POOL: &str = "php-fpm";

/// Everything a capture needs that is not in the database.
#[derive(Debug, Clone, Copy)]
pub struct Asked<'a> {
    /// The project to read.
    pub project: &'a ProjectRecord,

    /// The blueprint's name.
    pub name: &'a str,

    /// What it is for.
    pub description: &'a str,

    /// `windows`, `macos` or `linux` — from the platform layer, never from a `cfg!` in this crate.
    pub os: &'a str,

    /// This build's version.
    pub version: &'a str,

    /// The moment, as the caller reads the clock. Passed in so a test can have one.
    pub created_at: &'a str,
}

/// Write down what a project is made of.
///
/// # Errors
///
/// [`Error::ProjectHasSeveralSites`] for a project a single `[site]` cannot describe;
/// [`Error::Database`] when a table cannot be read, and [`Error::Manifest`] when the project's own
/// `mixengine.toml` does not parse.
pub async fn capture(store: &Store, asked: &Asked<'_>) -> Result<BlueprintManifest> {
    let project = asked.project;
    let mut sites = sites::records(store, Some(project.id)).await?;

    if sites.len() > 1 {
        return Err(Error::ProjectHasSeveralSites {
            project: project.name.clone(),
            domains: sites
                .iter()
                .map(|site| {
                    site.domains
                        .first()
                        .cloned()
                        .unwrap_or_else(|| String::from("(no domain)"))
                })
                .collect(),
        });
    }

    let site = sites.pop();
    let declared = manifest::read(&manifest::at(&project.root))?;

    let pool = site.as_ref().and_then(|site| match &site.kind {
        SiteKind::PhpFpm { pool } => pool.clone(),
        _ => None,
    });

    let php_version = match &pool {
        Some(pool) => services::version(store, pool).await?,
        None => None,
    };

    Ok(BlueprintManifest {
        schema: SCHEMA,
        blueprint: Header {
            name: asked.name.to_owned(),
            description: asked.description.to_owned(),
            created_at: asked.created_at.to_owned(),
            created_on: Provenance {
                os: asked.os.to_owned(),
                version: asked.version.to_owned(),
            },
        },
        runtimes: runtimes(store, project, php_version.as_ref()).await?,
        site: site.as_ref().map(|site| BlueprintSite {
            // The pool is dropped: which pool a site uses is a fact about the machine it was
            // created on, and the receiving machine decides its own.
            kind: match &site.kind {
                SiteKind::PhpFpm { .. } => SiteKind::PhpFpm { pool: None },
                other => other.clone(),
            },
            doc_root: site.doc_root.clone(),
            https: site.https_enabled,
            domain_pattern: site
                .domains
                .first()
                .map(|domain| tokenised_domain(domain, &project.name))
                .unwrap_or_default(),
            aliases: site
                .domains
                .iter()
                .skip(1)
                .map(|domain| tokenised_domain(domain, &project.name))
                .collect(),
        }),
        services: linked(
            store,
            project,
            site.as_ref()
                .map(|site| site.services.as_slice())
                .unwrap_or_default(),
            pool.as_ref(),
            declared.as_ref(),
        )
        .await?,
        php: match php_version.as_ref() {
            Some(version) => extensions(store, version).await?,
            None => None,
        },
        // **Never.** Capture does not invent a command to execute on somebody else's machine.
        scaffold: None,
    })
}

/// The languages this project asked for, resolved to the exact versions it is running on.
///
/// **D4a**: a kind whose answer came from [`RuntimeSource::Default`] was decided by this machine
/// rather than by the project, and writing it down would put this home's default into a file meant
/// for somebody else's. PHP is the exception and not by special-casing — it comes from the
/// `runtime_installs` row behind the pool the site names, which is a fact about the site.
async fn runtimes(
    store: &Store,
    project: &ProjectRecord,
    php: Option<&mixengine_proto::PackageVersion>,
) -> Result<BTreeMap<RuntimeKind, VersionConstraint>> {
    let mut captured = BTreeMap::new();

    for kind in RuntimeKind::ALL {
        let question = resolve::Question {
            kind,
            cwd: Some(&project.root),
            explicit: None,
        };

        match resolve::runtime(store, &question).await {
            Ok(resolved) if resolved.source != RuntimeSource::Default => {
                if let Ok(constraint) = VersionConstraint::parse(resolved.runtime.version.as_str())
                {
                    captured.insert(kind, constraint);
                }
            }
            Ok(_) => {}
            // A language nothing declares and nothing installs is not a language this project uses.
            // Every other failure is a database that cannot be read, and is worth raising.
            Err(Error::RuntimeUnresolved { .. } | Error::NoDefaultRuntime { .. }) => {}
            Err(other) => return Err(other),
        }
    }

    if let Some(version) = php
        && let Ok(constraint) = VersionConstraint::parse(version.as_str())
    {
        captured.insert(RuntimeKind::Php, constraint);
    }

    Ok(captured)
}

/// The services the site declares, as a blueprint states them.
async fn linked(
    store: &Store,
    project: &ProjectRecord,
    declared: &[ServiceId],
    pool: Option<&ServiceId>,
    manifest: Option<&manifest::Manifest>,
) -> Result<Vec<BlueprintService>> {
    let mut captured = Vec::new();

    for service in declared {
        if FRONT_ENDS.contains(&service.name()) || service.name() == POOL {
            continue;
        }
        if Some(service) == pool {
            continue;
        }

        let named = manifest.and_then(|manifest| {
            manifest
                .services
                .iter()
                .find(|entry| entry.name == service.name())
        });

        captured.push(BlueprintService {
            name: service.name().to_owned(),
            version: services::version(store, service)
                .await?
                .and_then(|version| VersionConstraint::parse(version.as_str()).ok()),
            instance: service
                .instance()
                .map(|instance| match instance == project.name {
                    // **The trap D4 exists for.** A dedicated instance copied by name would make the
                    // next project plug into this one's database server.
                    true => PER_PROJECT.to_owned(),
                    false => instance.to_owned(),
                }),
            database: named
                .and_then(|entry| entry.database.as_deref())
                .map(|database| tokenised_value(database, &project.name)),
            user: named
                .and_then(|entry| entry.user.as_deref())
                .map(|user| tokenised_value(user, &project.name)),
        });
    }

    Ok(captured)
}

/// What somebody turned on for the PHP this project's pool runs.
///
/// **Only the deviations, and only the ones turned on** (D2). The set a build enables by itself is
/// the receiving machine's business, and turning something *off* there is not this project's
/// requirement.
async fn extensions(
    store: &Store,
    version: &mixengine_proto::PackageVersion,
) -> Result<Option<Php>> {
    let state = crate::runtimes::extensions::state(store, RuntimeKind::Php, version).await?;

    let mut enabled: Vec<String> = state
        .choices
        .iter()
        .filter(|(_, wanted)| **wanted)
        .map(|(name, _)| name.clone())
        .collect();

    enabled.sort();

    Ok((!enabled.is_empty()).then_some(Php {
        extensions: enabled,
    }))
}

/// `blog.test` for project `blog` becomes `{project}.test`; `shop-staging.test` stays as it is.
///
/// **Substitution, never invention** (D4). The comparison is per label rather than per substring,
/// so a project called `e` does not turn every `e` in a domain into a token — and a domain that
/// does not contain the project's name keeps its literal spelling, which the plan then reports as a
/// domain conflict rather than guessing at a pattern that would break on the second machine.
fn tokenised_domain(domain: &str, project: &str) -> String {
    domain
        .split('.')
        .map(|label| match label == project {
            true => TOKEN,
            false => label,
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// The whole value, or nothing: a database called `blog` for project `blog` is `{project}`.
fn tokenised_value(value: &str, project: &str) -> String {
    match value == project {
        true => TOKEN.to_owned(),
        false => value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mixengine_proto::Timestamp;

    use crate::blueprints::manifest::render;
    use crate::projects::Registration;

    /// A home with one project, whose root exists so a manifest can be written into it.
    async fn home(name: &str) -> (tempfile::TempDir, Store, ProjectRecord) {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&temp.path().join("mixengine.db"))
            .await
            .expect("a database");

        let root = temp.path().join(name);
        std::fs::create_dir_all(root.join("public")).expect("a project directory");

        let project = crate::projects::create(
            &store,
            &Registration {
                name: name.to_owned(),
                root,
                pins: BTreeMap::new(),
            },
            Timestamp::from_system_time(std::time::SystemTime::UNIX_EPOCH),
        )
        .await
        .expect("a project");

        (temp, store, project)
    }

    /// An installed PHP, a pool on it, and the row that says the pool loads `xdebug`.
    async fn a_php_pool(store: &Store, version: &str, choices: &str) {
        sqlx::query(
            r#"INSERT INTO runtime_installs
                   (id, kind, version, channel, install_path, installed_at, size_bytes, source_url,
                    sha256, extension_choices_json, extensions_json)
               VALUES (1, 'php', ?1, 'stable', '/runtimes/php', '2026-09-01T00:00:00Z', 1,
                       'https://example.invalid/php', 'ab', ?2,
                       '{"shared":["redis","xdebug","mongodb"],"enabled":["redis"],"compiled_in":[]}')"#,
        )
        .bind(version)
        .bind(choices)
        .execute(store.pool())
        .await
        .expect("a runtime install");

        sqlx::query(
            "INSERT INTO services (id, runtime_install_id, instance_name, state, port)
             VALUES (?1, 1, ?2, 'stopped', 9000)",
        )
        .bind(format!("php-fpm@{version}"))
        .bind(version)
        .execute(store.pool())
        .await
        .expect("a pool row");
    }

    /// A package instance, so a site has something to link to.
    async fn a_package(store: &Store, id: i64, name: &str, version: &str, instance: &str) {
        sqlx::query(
            "INSERT INTO packages (id, name, version, install_path, installed_at, source_url, sha256)
             VALUES (?1, ?2, ?3, '/packages/x', '2026-09-01T00:00:00Z', 'https://example.invalid/p', 'ab')",
        )
        .bind(id)
        .bind(name)
        .bind(version)
        .execute(store.pool())
        .await
        .expect("a package");

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state, port, config_overrides_json)
             VALUES (?1, ?2, ?3, 'stopped', 3306, '{\"password\":\"hunter2\"}')",
        )
        .bind(format!("{name}@{instance}"))
        .bind(id)
        .bind(instance)
        .execute(store.pool())
        .await
        .expect("a service row");
    }

    fn asked<'a>(project: &'a ProjectRecord, name: &'a str) -> Asked<'a> {
        Asked {
            project,
            name,
            description: "",
            os: "linux",
            version: "0.1.0",
            created_at: "2026-09-01T09:00:00Z",
        }
    }

    async fn a_site(store: &Store, project: &ProjectRecord, domains: &[&str], links: &[&str]) {
        let pool = ServiceId::parse("php-fpm@8.2.23").expect("an id");

        crate::sites::create(
            store,
            &crate::sites::NewSite {
                project_id: project.id,
                doc_root: "public".to_owned(),
                kind: SiteKind::PhpFpm {
                    pool: Some(pool.clone()),
                },
                https_enabled: true,
                domains: domains.iter().map(|domain| (*domain).to_owned()).collect(),
                services: links
                    .iter()
                    .map(|id| ServiceId::parse(*id).expect("an id"))
                    .collect(),
            },
        )
        .await
        .expect("a site");
    }

    /// The shape of a capture: what the project uses, with `{project}` where its own name was.
    #[tokio::test]
    async fn a_capture_says_what_the_project_uses_and_nothing_about_this_machine() {
        let (_temp, store, project) = home("blog").await;
        a_php_pool(&store, "8.2.23", r#"{"xdebug":true,"mongodb":false}"#).await;
        a_package(&store, 1, "mariadb", "11.4.3", "main").await;
        std::fs::write(
            crate::manifest::at(&project.root),
            "[[services]]\nname = \"mariadb\"\ndatabase = \"blog\"\nuser = \"blog\"\n",
        )
        .expect("a project manifest");
        a_site(
            &store,
            &project,
            &["blog.test", "api.blog.test"],
            &["mariadb@main"],
        )
        .await;

        let manifest = capture(&store, &asked(&project, "blog-stack"))
            .await
            .expect("a capture");

        let site = manifest.site.as_ref().expect("a site");
        assert_eq!(site.domain_pattern, "{project}.test");
        assert_eq!(site.aliases, vec!["api.{project}.test".to_owned()]);
        assert_eq!(site.doc_root, "public");
        assert_eq!(site.kind, SiteKind::PhpFpm { pool: None });

        assert_eq!(manifest.services.len(), 1);
        assert_eq!(manifest.services[0].name, "mariadb");
        assert_eq!(manifest.services[0].instance.as_deref(), Some("main"));
        assert_eq!(manifest.services[0].database.as_deref(), Some("{project}"));
        assert_eq!(manifest.services[0].user.as_deref(), Some("{project}"));

        assert_eq!(
            manifest
                .runtimes
                .get(&RuntimeKind::Php)
                .map(VersionConstraint::as_str),
            Some("8.2.23"),
            "php comes from the pool the site names"
        );

        assert_eq!(
            manifest.php.as_ref().map(|php| php.extensions.clone()),
            Some(vec!["xdebug".to_owned()]),
            "only the deviations, and only the ones turned on"
        );

        assert!(
            manifest.scaffold.is_none(),
            "capture never writes a scaffold"
        );
    }

    /// **D4, and the trap it exists for.** A dedicated instance copied by name would make the next
    /// project plug into this one's database server.
    #[tokio::test]
    async fn a_dedicated_instance_is_captured_as_per_project_rather_than_by_name() {
        let (_temp, store, project) = home("blog").await;
        a_php_pool(&store, "8.2.23", "{}").await;
        a_package(&store, 1, "redis", "7.2.5", "blog").await;
        a_site(&store, &project, &["blog.test"], &["redis@blog"]).await;

        let manifest = capture(&store, &asked(&project, "blog-stack"))
            .await
            .expect("a capture");

        assert_eq!(manifest.services[0].instance.as_deref(), Some(PER_PROJECT));
    }

    /// **D5.** Two sites in one project would be silently reduced to one, so it is refused instead —
    /// and the refusal names them, because "this project has two sites" sends somebody hunting.
    #[tokio::test]
    async fn a_project_with_two_sites_is_refused_and_both_are_named() {
        let (_temp, store, project) = home("blog").await;
        a_php_pool(&store, "8.2.23", "{}").await;
        a_site(&store, &project, &["blog.test"], &[]).await;
        a_site(&store, &project, &["shop.test"], &[]).await;

        let error = capture(&store, &asked(&project, "blog-stack"))
            .await
            .expect_err("it refuses");

        let message = error.to_string();
        assert!(message.contains("blog.test"), "{message}");
        assert!(message.contains("shop.test"), "{message}");
    }

    /// **D4a.** A version this machine's default decided is this machine's, not the project's.
    #[tokio::test]
    async fn a_runtime_that_only_the_global_default_named_is_not_captured() {
        let (_temp, store, project) = home("blog").await;

        sqlx::query(
            r#"INSERT INTO runtime_installs
                   (id, kind, version, channel, install_path, installed_at, size_bytes, source_url,
                    sha256, is_default)
               VALUES (1, 'node', '22.8.0', 'stable', '/runtimes/node', '2026-09-01T00:00:00Z', 1,
                       'https://example.invalid/node', 'ab', 1)"#,
        )
        .execute(store.pool())
        .await
        .expect("a default node");

        let manifest = capture(&store, &asked(&project, "blog-stack"))
            .await
            .expect("a capture");

        assert!(
            manifest.runtimes.is_empty(),
            "{:?} came from this machine, not from the project",
            manifest.runtimes
        );
    }

    /// A domain that does not contain the project's name keeps its literal spelling: guessing a
    /// pattern is what would break on the second machine.
    #[tokio::test]
    async fn a_domain_that_does_not_carry_the_project_name_is_left_alone() {
        let (_temp, store, project) = home("blog").await;
        a_php_pool(&store, "8.2.23", "{}").await;
        a_site(&store, &project, &["shop-staging.test"], &[]).await;

        let manifest = capture(&store, &asked(&project, "blog-stack"))
            .await
            .expect("a capture");

        assert_eq!(
            manifest.site.expect("a site").domain_pattern,
            "shop-staging.test"
        );
    }

    /// **D6, and the test the whole task is measured by.** Written against the rendered string,
    /// because asserting on the struct would only prove that the fields we remembered are empty.
    #[tokio::test]
    async fn nothing_forbidden_reaches_the_rendered_manifest() {
        let (temp, store, project) = home("blog").await;
        a_php_pool(&store, "8.2.23", r#"{"xdebug":true}"#).await;
        a_package(&store, 1, "mariadb", "11.4.3", "main").await;
        a_site(&store, &project, &["blog.test"], &["mariadb@main"]).await;

        let manifest = capture(&store, &asked(&project, "blog-stack"))
            .await
            .expect("a capture");
        let rendered = render(&manifest);

        let home_directory = temp.path().display().to_string();
        assert!(
            !rendered.contains(&home_directory),
            "the home directory is in it:\n{rendered}"
        );

        for forbidden in [
            "hunter2",
            "password",
            "secret",
            "token",
            "3306",
            "9000",
            "MIXENGINE_HOME",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "{forbidden} reached the manifest:\n{rendered}"
            );
        }

        // An absolute path in either of this machine's spellings.
        assert!(!rendered.contains(":\\"), "{rendered}");
        assert!(
            !rendered.lines().any(|line| line.contains(" = \"/")),
            "{rendered}"
        );
    }
}
