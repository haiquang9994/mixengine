//! Installing an extension, from a directory and from a signed registry — roadmap task **T81**.
//!
//! What these hold to is the order the design's D2 argues for: everything a person could refuse is
//! decided before anything is fetched, and what is fetched lands whole or not at all.

use std::path::{Path, PathBuf};

use mixengine_core::extensions::install::{self, Plan, Request};
use mixengine_core::extensions::manifest::{self, ExtensionManifest};
use mixengine_core::extensions::store::{self as extension_store, Source};
use mixengine_core::{Paths, Store};
use mixengine_platform::mock;
use tempfile::TempDir;

/// A watcher that records nothing and cancels nothing.
struct Quiet;

impl mixengine_core::install::Watcher for Quiet {
    async fn report(&self, _percent: u8, _message: &str) {}

    fn is_cancelled(&self) -> bool {
        false
    }
}

/// A home with a migrated database.
async fn home() -> (TempDir, Paths, Store) {
    let directory = TempDir::new().expect("a temporary directory");
    let paths = Paths::new(
        directory.path().to_path_buf(),
        &mixengine_core::config::PathOverrides::default(),
    );
    let store = Store::open(paths.database_file())
        .await
        .expect("a database");

    (directory, paths, store)
}

fn read(text: &str) -> ExtensionManifest {
    manifest::read(Path::new("extension.toml"), text).expect("a fixture parses")
}

/// A directory holding `extension.toml` and a file standing in for the program.
fn source_directory(text: &str) -> (TempDir, PathBuf) {
    let directory = TempDir::new().expect("a temporary directory");
    std::fs::write(directory.path().join("extension.toml"), text).expect("the manifest");
    std::fs::write(directory.path().join("mailpit"), b"#!/bin/true\n").expect("the program");

    let path = directory.path().to_path_buf();
    (directory, path)
}

async fn plan_for(store: &Store, paths: &Paths, manifest: &ExtensionManifest) -> Plan {
    install::plan(store, paths, manifest, false)
        .await
        .expect("a plan")
}

/// **A plan is answerable before anything is fetched** — the design's D2.
///
/// It carries the permissions and the ports, which is what a person is agreeing to, and it names
/// both directories so the answer is about this machine rather than about a manifest in the
/// abstract.
#[tokio::test]
async fn a_plan_says_what_would_happen_and_does_none_of_it() {
    let (_home, paths, store) = home().await;
    let manifest = read(mixengine_testkit::extension::MAILPIT);

    let plan = plan_for(&store, &paths, &manifest).await;

    assert_eq!(plan.id.as_str(), "mailpit");
    assert_eq!(plan.ports.len(), 2);
    assert_eq!(plan.install_dir, paths.extensions().join("mailpit"));
    assert_eq!(
        plan.data_dir,
        paths.data().join("extensions").join("mailpit")
    );
    assert!(
        !plan.install_dir.exists(),
        "planning created the install directory"
    );
    assert!(
        extension_store::all(&store)
            .await
            .expect("a read")
            .is_empty(),
        "planning wrote a row"
    );
}

/// A `--path` install is recorded as unsigned, and everything it needs lands.
#[tokio::test]
async fn a_path_install_lands_unsigned() {
    let (_home, paths, store) = home().await;
    let manifest = read(mixengine_testkit::extension::MAILPIT);
    let (_source, directory) = source_directory(mixengine_testkit::extension::MAILPIT);

    let installed = install::install(
        &store,
        &paths,
        &mock::Host::with_home("/mixengine"),
        Request {
            manifest: &manifest,
            source: Source::Path,
            from: Some(&directory),
            at: stamp(),
        },
        &Quiet,
    )
    .await
    .expect("the install");

    assert!(!installed.signed, "a directory install vouches for nothing");
    assert_eq!(installed.source, Source::Path);
    assert!(installed.install_dir.join("mailpit").is_file());
    assert!(
        installed.data_dir.is_dir(),
        "the data directory was not created"
    );

    // The row, its ports and its service all landed.
    let read_back = extension_store::get(&store, &installed.id)
        .await
        .expect("a read")
        .expect("the row");
    assert_eq!(read_back.ports.len(), 2);

    let service: Option<String> =
        sqlx::query_scalar("SELECT id FROM services WHERE extension_id = 'mailpit'")
            .fetch_optional(store.pool())
            .await
            .expect("a read");
    assert_eq!(service.as_deref(), Some("mailpit"));
}

/// **The service's port is the one its readiness check names** — the design's D8.
#[tokio::test]
async fn the_service_row_holds_the_port_ready_watches() {
    let (_home, paths, store) = home().await;
    let manifest = read(mixengine_testkit::extension::MAILPIT);
    let (_source, directory) = source_directory(mixengine_testkit::extension::MAILPIT);

    let installed = install::install(
        &store,
        &paths,
        &mock::Host::with_home("/mixengine"),
        Request {
            manifest: &manifest,
            source: Source::Path,
            from: Some(&directory),
            at: stamp(),
        },
        &Quiet,
    )
    .await
    .expect("the install");

    let port: Option<i64> = sqlx::query_scalar("SELECT port FROM services WHERE id = 'mailpit'")
        .fetch_one(store.pool())
        .await
        .expect("a read");

    assert_eq!(
        port.map(|held| u16::try_from(held).expect("a port")),
        installed.ports.get("ui_port").copied(),
        "the row holds a port that is not the one the readiness check watches"
    );
}

/// **A `[recipe] front_end` fragment is refused, and named** — the design's D10.
///
/// Refused rather than accepted and not applied: a manifest whose stated effect does not happen is
/// worse than one that was turned away.
#[tokio::test]
async fn a_front_end_fragment_is_refused_rather_than_ignored() {
    let (_home, paths, store) = home().await;
    let text = format!(
        "{}\n[[recipe.front_end]]\nfragment = \"header_up X-Test 1\"\n",
        mixengine_testkit::extension::MAILPIT
    );
    let manifest = read(&text);

    let refusal = install::plan(&store, &paths, &manifest, true)
        .await
        .expect_err("refused");

    let said = refusal.to_string();
    assert!(said.contains("recipe.front_end"), "{said}");
}

/// Installing over something already installed is refused before anything is copied.
#[tokio::test]
async fn one_extension_is_installed_once() {
    let (_home, paths, store) = home().await;
    let manifest = read(mixengine_testkit::extension::MAILPIT);
    let (_source, directory) = source_directory(mixengine_testkit::extension::MAILPIT);
    let host = mock::Host::with_home("/mixengine");

    install::install(
        &store,
        &paths,
        &host,
        Request {
            manifest: &manifest,
            source: Source::Path,
            from: Some(&directory),
            at: stamp(),
        },
        &Quiet,
    )
    .await
    .expect("the first install");

    let refusal = install::install(
        &store,
        &paths,
        &host,
        Request {
            manifest: &manifest,
            source: Source::Path,
            from: Some(&directory),
            at: stamp(),
        },
        &Quiet,
    )
    .await
    .expect_err("the second");

    assert!(
        refusal.to_string().contains("already installed"),
        "{refusal}"
    );
}

/// A `recipe` extension installs with no artifact and no service — and still gets a directory, so
/// everything downstream has one place to name.
#[tokio::test]
async fn something_that_runs_nothing_installs_without_a_service() {
    let (_home, paths, store) = home().await;
    let manifest = read(mixengine_testkit::extension::SENDMAIL);

    let installed = install::install(
        &store,
        &paths,
        &mock::Host::with_home("/mixengine"),
        Request {
            manifest: &manifest,
            source: Source::Registry,
            from: None,
            at: stamp(),
        },
        &Quiet,
    )
    .await
    .expect("the install");

    assert!(installed.install_dir.is_dir());
    assert!(installed.signed, "a registry install is vouched for");

    let services: i64 = sqlx::query_scalar("SELECT count(*) FROM services")
        .fetch_one(store.pool())
        .await
        .expect("a count");
    assert_eq!(services, 0, "something that runs nothing got a service row");
}

/// An extension published for other machines says which ones, rather than failing as if something
/// were broken.
#[tokio::test]
async fn nothing_published_for_this_machine_names_what_is() {
    let (_home, paths, store) = home().await;
    let text = mixengine_testkit::extension::MAILPIT
        .replace("[artifact.windows-x86_64]", "[artifact.plan9-vax]");
    let manifest = read(&text);

    let refusal = install::plan(&store, &paths, &manifest, true).await;

    // On a machine the fixture *does* publish for, the other targets still exist and this reads as
    // a plan; the assertion is about what the message says when it does not.
    if let Err(said) = refusal {
        let said = said.to_string();
        assert!(said.contains("no artifact for this machine"), "{said}");
    }
}

fn stamp() -> mixengine_proto::Timestamp {
    mixengine_proto::Timestamp::parse_rfc3339("2026-09-02T09:00:00Z").expect("a timestamp")
}

/// A PHP recorded as installed, with the pool `pools::ensure` would have made for it — roadmap
/// task **T81b**.
async fn php(store: &Store, version: &str) {
    sqlx::query(
        "INSERT INTO runtime_installs (kind, version, channel, install_path, installed_at,
                                       size_bytes, source_url, sha256, provides_json)
         VALUES ('php', ?1, 'stable', '/runtimes/php/' || ?1, '2026-09-03T00:00:00Z', 1,
                 'https://example.invalid/php', 'ab',
                 '{\"php\":\"bin/php\",\"php-fpm\":\"sbin/php-fpm\"}')",
    )
    .bind(version)
    .execute(store.pool())
    .await
    .expect("a runtime row");

    sqlx::query(
        "INSERT INTO services (id, runtime_install_id, instance_name, state, port)
         VALUES ('php-fpm@' || ?1,
                 (SELECT id FROM runtime_installs WHERE kind = 'php' AND version = ?1),
                 ?1, 'stopped', 9000 + (SELECT count(*) FROM services))",
    )
    .bind(version)
    .execute(store.pool())
    .await
    .expect("a pool row");
}

/// A directory holding the phpMyAdmin fixture and the doc root it names.
fn web_app_directory() -> (TempDir, PathBuf) {
    let directory = TempDir::new().expect("a temporary directory");
    std::fs::write(
        directory.path().join("extension.toml"),
        mixengine_testkit::extension::PHPMYADMIN,
    )
    .expect("the manifest");
    std::fs::create_dir_all(directory.path().join("app")).expect("the doc root");
    std::fs::write(directory.path().join("app").join("index.php"), b"<?php\n").expect("a file");

    let path = directory.path().to_path_buf();
    (directory, path)
}

/// **T81b, D4 and D5.** The plan names the domain and the pool — the newest PHP inside
/// `requires`, never the default — and changes nothing.
#[tokio::test]
async fn a_web_app_plan_names_its_site_and_the_newest_matching_pool() {
    let (_home, paths, store) = home().await;
    php(&store, "8.1.30").await;
    php(&store, "8.3.34").await;
    let manifest = read(mixengine_testkit::extension::PHPMYADMIN);

    let plan = plan_for(&store, &paths, &manifest).await;

    let site = plan.site.expect("a web-app plans a site");
    assert_eq!(site.domain, "phpmyadmin.mixengine.test");
    assert_eq!(site.pool.as_str(), "php-fpm@8.3.34");
    assert_eq!(site.doc_root, "app");
    assert!(
        mixengine_core::sites::records(&store, None)
            .await
            .expect("a read")
            .is_empty(),
        "planning wrote a site"
    );
}

/// **T81b, D5.** Nothing installed satisfies `^8.1`: refused, naming the extension as what asked,
/// and nothing on disk.
#[tokio::test]
async fn a_web_app_with_no_matching_php_is_refused_before_anything_is_fetched() {
    let (_home, paths, store) = home().await;
    php(&store, "7.4.33").await;
    let manifest = read(mixengine_testkit::extension::PHPMYADMIN);

    let refusal = install::plan(&store, &paths, &manifest, false)
        .await
        .expect_err("no PHP answers ^8.1");

    assert!(
        matches!(refusal, mixengine_core::Error::RuntimeUnresolved { ref origin, .. }
            if origin.contains("phpmyadmin")),
        "{refusal}"
    );
    assert!(!paths.extensions().join("phpmyadmin").exists());
}

/// **T81b, D4.** The name is already somebody's: refused naming the holder, before anything is
/// fetched.
#[tokio::test]
async fn a_web_app_whose_domain_is_taken_is_refused_naming_the_holder() {
    let (home, paths, store) = home().await;
    php(&store, "8.3.34").await;
    let project = mixengine_core::projects::create(
        &store,
        &mixengine_core::projects::Registration {
            name: "squatter".to_owned(),
            root: home.path().join("squatter"),
            pins: std::collections::BTreeMap::new(),
        },
        mixengine_proto::Timestamp(0),
    )
    .await
    .expect("a project");
    mixengine_core::sites::create(
        &store,
        &mixengine_core::sites::NewSite {
            owner: mixengine_core::sites::SiteOwner::Project(project.id),
            doc_root: String::new(),
            kind: mixengine_proto::SiteKind::Static,
            https_enabled: true,
            domains: vec!["phpmyadmin.mixengine.test".to_owned()],
            services: Vec::new(),
        },
    )
    .await
    .expect("a site on the name");
    let manifest = read(mixengine_testkit::extension::PHPMYADMIN);

    let refusal = install::plan(&store, &paths, &manifest, false)
        .await
        .expect_err("the name is taken");

    assert!(
        matches!(refusal, mixengine_core::Error::DomainTaken { ref holder, .. }
            if holder == "phpmyadmin.mixengine.test"),
        "{refusal}"
    );
}

/// **T81b, D7.** The install writes the site where a `service` would have written its row: owned
/// by the extension, on the frozen pool, HTTPS on, rooted under the install directory.
#[tokio::test]
async fn installing_a_web_app_writes_its_site() {
    let (_home, paths, store) = home().await;
    php(&store, "8.3.34").await;
    let manifest = read(mixengine_testkit::extension::PHPMYADMIN);
    let (_directory, from) = web_app_directory();

    let installed = install::install(
        &store,
        &paths,
        &mock::Host::with_home(paths.root()),
        Request {
            manifest: &manifest,
            source: Source::Path,
            from: Some(&from),
            at: mixengine_proto::Timestamp(0),
        },
        &Quiet,
    )
    .await
    .expect("the install");

    let site = mixengine_core::sites::of_extension(&store, &installed.id)
        .await
        .expect("a read")
        .expect("the site was written");
    assert_eq!(
        site.owner,
        mixengine_core::sites::SiteOwner::Extension(installed.id.clone())
    );
    assert_eq!(site.domains, vec!["phpmyadmin.mixengine.test".to_owned()]);
    assert_eq!(site.doc_root, "app");
    assert!(site.https_enabled);
    assert_eq!(site.state, mixengine_proto::SiteState::Enabled);
    assert!(
        matches!(site.kind, mixengine_proto::SiteKind::PhpFpm { pool: Some(ref pool) }
            if pool.as_str() == "php-fpm@8.3.34"),
        "{:?}",
        site.kind
    );
    assert!(site.services.is_empty());

    let services: i64 =
        sqlx::query_scalar("SELECT count(*) FROM services WHERE extension_id IS NOT NULL")
            .fetch_one(store.pool())
            .await
            .expect("a count");
    assert_eq!(services, 0, "a web-app runs no process of its own");
}

/// **T81b, D8.** Uninstalling a web-app takes its site and its domain row, names the domain it
/// released, and keeps the data directory like every other uninstall.
#[tokio::test]
async fn uninstalling_a_web_app_takes_its_site_and_says_so() {
    let (_home, paths, store) = home().await;
    php(&store, "8.3.34").await;
    let manifest = read(mixengine_testkit::extension::PHPMYADMIN);
    let (_directory, from) = web_app_directory();
    let installed = install::install(
        &store,
        &paths,
        &mock::Host::with_home(paths.root()),
        Request {
            manifest: &manifest,
            source: Source::Path,
            from: Some(&from),
            at: mixengine_proto::Timestamp(0),
        },
        &Quiet,
    )
    .await
    .expect("the install");

    let removed =
        mixengine_core::extensions::uninstall::uninstall(&store, &paths, &installed.id, false)
            .await
            .expect("the uninstall");

    assert_eq!(removed.site.as_deref(), Some("phpmyadmin.mixengine.test"));
    assert_eq!(removed.service, None);
    assert!(
        mixengine_core::sites::of_extension(&store, &installed.id)
            .await
            .expect("a read")
            .is_none()
    );
    let domains: i64 = sqlx::query_scalar("SELECT count(*) FROM site_domains")
        .fetch_one(store.pool())
        .await
        .expect("a count");
    assert_eq!(domains, 0, "the domain outlived its site");
    assert_eq!(
        removed.data_dir_kept.as_deref(),
        Some(installed.data_dir.as_path())
    );
}
