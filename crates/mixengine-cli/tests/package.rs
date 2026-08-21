//! `mix package` and `mix service create|delete` against a real daemon and a real signed index.
//!
//! Roadmap task **T31a**'s client half. What the daemon's own `tests/packages.rs` proves is that the
//! methods do what they say; what is proved here is the part that is only true of `mix` — that the
//! arguments a person types reach the right method, that an install waits by default and ends with
//! an exit status a shell can branch on, that `--json` emits exactly one object, and that the human
//! rendering says the sentence a person needs: which data directory a delete kept.
//!
//! **Multi-threaded on purpose**, for `tests/runtime.rs`' reason: `MockRegistry` is serving the
//! daemon that `mix` is talking to, in this same process, while `mix` runs as a blocking child.

mod harness;

use harness::{Home, json, stdout};
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::{Value, json as document};

/// The version this suite installs.
const VERSION: &str = "1.0.0";

/// The package it installs, which is the one a debug build has a recipe for.
const PACKAGE: &str = "fakeservice";

/// The name the archive publishes its executable under, with the suffix this OS needs to spawn it.
fn program_name() -> String {
    format!("fakeservice{}", std::env::consts::EXE_SUFFIX)
}

/// A home, a daemon in it, and a registry offering one package the daemon can actually install.
struct Fixture {
    home: Home,
    _registry: MockRegistry,
    _daemon: harness::Daemon,
}

impl Fixture {
    async fn start() -> Self {
        let packing = match cfg!(windows) {
            true => Packing::Zip,
            false => Packing::TarZst,
        };
        let packed = FakePackage::new(packing)
            .executable(&program_name())
            .build(&format!("{PACKAGE}-{VERSION}"));

        let registry = MockRegistry::start(&document!({
            "schema": 1, "generated_at": "2026-08-19T06:55:12Z", "packages": []
        }))
        .await;

        let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
        registry.publish(&index(&packed, &url));

        let home = Home::new();
        let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());

        Self {
            home,
            _registry: registry,
            _daemon: daemon,
        }
    }
}

/// An index offering exactly one version, for this machine.
fn index(packed: &Packed, url: &str) -> Value {
    document!({
        "schema": 1,
        "generated_at": "2026-08-19T06:55:12Z",
        "packages": [{
            "kind": PACKAGE,
            "version": VERSION,
            "channel": "stable",
            "artifacts": [{
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "url": url,
                "sha256": packed.sha256,
                "size": packed.size(),
                "provides": { "fakeservice": program_name() },
            }],
        }],
    })
}

/// The sequence a person actually types, in the order they type it.
#[tokio::test(flavor = "multi_thread")]
async fn a_package_is_listed_installed_made_into_a_service_and_removed_from_the_command_line() {
    let fixture = Fixture::start().await;
    let home = &fixture.home;

    let empty = stdout(&home.mix(&["package", "list"]));
    assert!(
        empty.contains("no packages are installed"),
        "an empty home says so rather than printing a heading with nothing under it: {empty}"
    );

    let available = json(&home.mix(&["package", "available", "--json"]));
    assert_eq!(available["packages"][0]["package"], PACKAGE);
    assert_eq!(available["packages"][0]["installed"], false);

    // **Waits by default**, which is what makes `mix package install … && …` a sentence about the
    // package being there rather than about a download having been accepted.
    let installed = json(&home.mix(&["package", "install", PACKAGE, VERSION, "--json"]));
    assert_eq!(installed["state"], "succeeded", "{installed}");
    assert_eq!(installed["kind"], "package.install");
    assert_eq!(installed["outcome"]["result"]["version"], VERSION);

    let listed = stdout(&home.mix(&["package", "list"]));
    assert!(listed.contains(PACKAGE), "{listed}");
    assert!(
        listed
            .lines()
            .next()
            .is_some_and(|heading| heading.contains("SERVICES")),
        "what is holding a package is a column rather than a footnote: {listed}"
    );

    // And the package becomes a service.
    let created = json(&home.mix(&["service", "create", "fakeservice@main", VERSION, "--json"]));
    assert_eq!(created["service"]["id"], "fakeservice@main");
    assert_eq!(created["service"]["state"], "stopped", "{created}");

    let held = home.mix(&["package", "uninstall", PACKAGE, VERSION]);
    assert!(
        !held.status.success(),
        "a package a service is an instance of is not one to remove: {held:?}"
    );

    // The human rendering of a delete names the directory it kept, which is the whole reason the
    // answer is not just the service.
    let deleted = stdout(&home.mix(&["service", "delete", "fakeservice@main"]));
    assert!(deleted.contains("deleted fakeservice@main"), "{deleted}");

    let removed = json(&home.mix(&["package", "uninstall", PACKAGE, VERSION, "--json"]));
    assert_eq!(removed["removed"]["version"], VERSION);
}

/// An install that cannot be done fails the command rather than only the job.
#[tokio::test(flavor = "multi_thread")]
async fn an_install_the_index_cannot_satisfy_exits_non_zero_and_says_why() {
    let fixture = Fixture::start().await;

    let attempted = fixture.home.mix(&["package", "install", PACKAGE, "9.9.9"]);

    assert!(!attempted.status.success(), "{attempted:?}");

    let said = stdout(&attempted);
    assert!(
        said.contains("failed") && said.contains("does not publish"),
        "the job's own failure is rendered where the answer goes: {said}"
    );
}

/// A package this build has no recipe for is refused with the list of the ones it has.
#[tokio::test(flavor = "multi_thread")]
async fn a_package_this_build_cannot_run_is_refused_with_what_it_can() {
    let fixture = Fixture::start().await;

    let refused = fixture
        .home
        .mix(&["package", "install", "meilisearch", VERSION]);

    assert!(!refused.status.success());

    let complaint = format!(
        "{}{}",
        stdout(&refused),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        complaint.contains("meilisearch") && complaint.contains("caddy"),
        "it names what was asked for and what exists: {complaint}"
    );
}

/// A create with no package installed is a missing step, and the hint is the step.
#[tokio::test(flavor = "multi_thread")]
async fn creating_a_service_before_installing_its_package_names_the_install() {
    let fixture = Fixture::start().await;

    let refused = fixture
        .home
        .mix(&["service", "create", "fakeservice@main", VERSION]);

    assert!(!refused.status.success());

    let complaint = format!(
        "{}{}",
        stdout(&refused),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        complaint.contains("package install"),
        "the hint is the command that would fix it: {complaint}"
    );
}
