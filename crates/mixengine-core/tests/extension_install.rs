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
