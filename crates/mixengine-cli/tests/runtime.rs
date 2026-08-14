//! `mix runtime` and `mix job` against a real daemon, a real signed index and a real archive.
//!
//! Roadmap task **T23**'s client half. What the daemon's own `tests/runtimes.rs` proves is that the
//! methods do what they say; what is proved here is the part that is only true of `mix` — that the
//! two arguments a person types reach the right method, that an install *waits* by default and ends
//! with an exit status a shell can branch on, and that both renderings say what happened.
//!
//! **Multi-threaded on purpose.** `MockRegistry` is serving the daemon that `mix` is talking to, in
//! this same process, while `mix` runs to completion as a blocking child — so the server and the
//! wait cannot share a thread. Nothing here touches the network: a loopback socket and a named pipe
//! are neither.

mod harness;

use harness::{Home, json, stdout};
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::{Value, json as document};

/// The version this suite installs.
const VERSION: &str = "8.3.33";

/// The name the archive publishes its executable under, with the suffix this OS needs to spawn it.
fn program_name() -> String {
    format!("bin/php{}", std::env::consts::EXE_SUFFIX)
}

/// A home, a daemon in it, and a registry offering one PHP the daemon can actually install.
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
            .build(&format!("php-{VERSION}"));

        let registry = MockRegistry::start(&document!({
            "schema": 1, "generated_at": "2026-08-14T06:55:12Z", "packages": []
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
        "generated_at": "2026-08-14T06:55:12Z",
        "packages": [{
            "kind": "php",
            "version": VERSION,
            "channel": "stable",
            "eol": "2027-12-31",
            "artifacts": [{
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "url": url,
                "sha256": packed.sha256,
                "size": packed.size(),
                "provides": { "php": program_name() },
            }],
        }],
    })
}

/// The sequence a person actually types, in the order they type it.
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_is_listed_installed_chosen_and_removed_from_the_command_line() {
    let fixture = Fixture::start().await;
    let home = &fixture.home;

    let available = json(&home.mix(&["runtime", "available", "--json"]));
    assert_eq!(available["runtimes"][0]["version"], VERSION);
    assert_eq!(available["runtimes"][0]["installed"], false);

    // **Waits by default**, which is what makes `mix runtime install … && …` a sentence about PHP
    // being there rather than about a download having been accepted.
    let installed = json(&home.mix(&["runtime", "install", "php", VERSION, "--json"]));
    assert_eq!(installed["state"], "succeeded", "{installed}");
    assert_eq!(installed["kind"], "runtime.install");
    assert_eq!(installed["outcome"]["result"]["version"], VERSION);

    let listed = stdout(&home.mix(&["runtime", "list"]));
    assert!(listed.contains("php"), "{listed}");
    assert!(listed.contains(VERSION), "{listed}");
    assert!(
        listed
            .lines()
            .next()
            .is_some_and(|heading| heading.contains("DEFAULT")),
        "the default is a column of its own rather than a footnote: {listed}"
    );

    // The job the install produced is one `mix job list` can find afterwards, which is the whole
    // reason the two namespaces landed together.
    let jobs = json(&home.mix(&["job", "list", "--json"]));
    assert_eq!(jobs["jobs"][0]["kind"], "runtime.install");
    assert_eq!(jobs["jobs"][0]["id"], installed["id"]);

    let waited = home.mix(&["job", "wait", &jobs["jobs"][0]["id"].to_string(), "--json"]);
    assert!(
        waited.status.success(),
        "waiting for a job that succeeded exits zero"
    );

    let removed = json(&home.mix(&["runtime", "uninstall", "php", VERSION, "--json"]));
    assert_eq!(removed["removed"]["version"], VERSION);
    assert_eq!(
        removed["default_cleared"], true,
        "it was the only version, so its kind is left with none"
    );

    let empty = stdout(&home.mix(&["runtime", "list"]));
    assert!(
        empty.contains("no runtimes are installed"),
        "an empty home says so rather than printing a heading with nothing under it: {empty}"
    );
}

/// An install that cannot be done fails the command rather than only the job — which is the one
/// thing a script cares about and the one thing a job id alone would not give it.
#[tokio::test(flavor = "multi_thread")]
async fn an_install_the_index_cannot_satisfy_exits_non_zero_and_says_why() {
    let fixture = Fixture::start().await;

    let attempted = fixture.home.mix(&["runtime", "install", "php", "1.2.3"]);

    assert!(!attempted.status.success(), "{attempted:?}");

    let said = stdout(&attempted);
    assert!(
        said.contains("failed") && said.contains("does not publish"),
        "the job's own failure is rendered where the answer goes: {said}"
    );
}

/// The client refuses what it can refuse locally, which is what keeps a typo from starting a daemon
/// and travelling over a socket to be told it is a typo.
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_this_build_does_not_manage_is_refused_before_anything_is_dialled() {
    let fixture = Fixture::start().await;

    let refused = fixture.home.mix(&["runtime", "install", "pph", VERSION]);

    assert!(!refused.status.success());

    let complaint = String::from_utf8_lossy(&refused.stderr);
    assert!(
        complaint.contains("php") && complaint.contains("ruby"),
        "the four it does know are in the message: {complaint}"
    );
}

/// **T24's client half.** What is only true of `mix` here is the pair the daemon cannot know: the
/// directory it was run in, and `MIXENGINE_PHP`.
#[tokio::test(flavor = "multi_thread")]
async fn resolving_sends_the_directory_it_was_run_in_and_the_variable_it_was_given() {
    let fixture = Fixture::start().await;
    let home = &fixture.home;

    home.mix(&["runtime", "install", "php", VERSION, "--json"]);

    let project = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(
        project.path().join("mixengine.toml"),
        "[runtimes]\nphp = \"^8.3\"\n",
    )
    .expect("a manifest");

    // The manifest is found because `mix` sent the directory it was started in — nothing on the
    // command line names it.
    let resolved = json(&home.mix_in(
        project.path(),
        &[],
        &["runtime", "resolve", "php", "--json"],
    ));
    assert_eq!(resolved["runtime"]["version"], VERSION, "{resolved}");
    assert_eq!(resolved["source"]["from"], "manifest", "{resolved}");

    // The variable overrides it, and the name of the variable is the kind's own.
    let resolved = json(&home.mix_in(
        project.path(),
        &[("MIXENGINE_PHP", VERSION)],
        &["runtime", "resolve", "php", "--json"],
    ));
    assert_eq!(resolved["source"]["from"], "explicit", "{resolved}");

    // And the human rendering says which file decided it, because that is the question being asked.
    let said = stdout(&home.mix_in(project.path(), &[], &["runtime", "resolve", "php"]));
    assert!(
        said.contains(VERSION) && said.contains("mixengine.toml"),
        "{said}"
    );
}

/// A variable that quietly did nothing would be the exact failure this command exists to explain,
/// so `mix` refuses it rather than falling through to the next source.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_variable_that_is_not_a_version_is_refused_by_the_client() {
    let fixture = Fixture::start().await;
    let home = &fixture.home;
    let project = tempfile::tempdir().expect("a temporary directory");

    let refused = home.mix_in(
        project.path(),
        &[("MIXENGINE_PHP", "~8.3")],
        &["runtime", "resolve", "php"],
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("MIXENGINE_PHP"),
        "the message names the variable: {refused:?}"
    );

    // Empty is how a shell unsets one for a single command, and means exactly that.
    home.mix(&["runtime", "install", "php", VERSION, "--json"]);
    let resolved = json(&home.mix_in(
        project.path(),
        &[("MIXENGINE_PHP", "")],
        &["runtime", "resolve", "php", "--json"],
    ));
    assert_eq!(resolved["source"]["from"], "default", "{resolved}");
}
