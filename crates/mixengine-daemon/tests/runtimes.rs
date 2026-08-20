//! `runtime.*` against a real `mixengined`, a real signed index and a real archive.
//!
//! Roadmap task **T23**, and the half no unit test can reach. That a table can be read, that an
//! archive can be unpacked and that a job can be run are each provable in one process; that asking a
//! daemon over a socket for a version it has never heard of *ends with PHP on disk, a row describing
//! it, and a job that says so* is the seam, and the seam is the feature.
//!
//! **Nothing here touches the network.** [`MockRegistry`] serves a document it signs with a keypair
//! it generated, over a loopback socket, and the daemon is pointed at both with `--index-url` and
//! `--index-key` — which is also the only end-to-end proof that those two flags do what they say.
//! What is installed is a [`FakePackage`] containing the `fakeservice` binary under the name the
//! index publishes it as, so the post-install check spawns something that really runs.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_testkit::{FakePackage, Home, MockRegistry, Packed, Packing, declare};
use serde_json::{Value, json};

/// How long a job is given to finish before the test gives up on it.
///
/// Every install here is a few kilobytes over loopback, so this is a ceiling and not a pause — but
/// the post-install check spawns a freshly written executable, and a CI runner scanning one for the
/// first time is the slow part rather than the download.
const PATIENCE: Duration = Duration::from_secs(60);

/// The version this suite installs, and one it does not.
const VERSION: &str = "8.3.33";

/// The name the archive publishes its executable under, with the suffix this OS needs to spawn it.
///
/// Windows resolves an extensionless name by appending `.exe`, and a fixture that relied on that
/// would be testing the loader rather than the install.
fn program_name() -> String {
    format!("bin/php{}", std::env::consts::EXE_SUFFIX)
}

/// A registry serving one PHP, a home, and a daemon pointed at both.
struct Fixture {
    home: Home,
    /// Held rather than read: dropping it would stop the server the daemon downloads from.
    _registry: MockRegistry,
    _daemon: Daemon,
    packed: Packed,
}

impl Fixture {
    /// Publish one version of PHP and start a daemon that can see it.
    async fn start() -> Self {
        // `.zip` on Windows and `.tar.zst` elsewhere, which is what the publishing pipeline
        // produces for each — the point being that the daemon unpacks what its own platform is
        // actually served rather than whichever format a fixture found convenient.
        let packing = match cfg!(windows) {
            true => Packing::Zip,
            false => Packing::TarZst,
        };
        let packed = FakePackage::new(packing)
            .executable(&program_name())
            .file("php.ini-production", b"; nothing")
            .build(&format!("php-{VERSION}"));

        let registry = MockRegistry::start(&json!({
            "schema": 1,
            "generated_at": "2026-08-14T06:55:12Z",
            "packages": [],
        }))
        .await;

        let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
        registry.publish(&index(&packed, &url));

        let home = Home::new();
        let daemon = Daemon::start(&home, &registry);
        home.wait_until_listening().await;

        Self {
            home,
            _registry: registry,
            _daemon: daemon,
            packed,
        }
    }

    async fn client(&self) -> Client {
        Client::connect(&self.home).await
    }

    /// Where the daemon would have put this version.
    fn installed_at(&self, version: &str) -> std::path::PathBuf {
        self.home.path().join("runtimes").join("php").join(version)
    }
}

/// An index offering one PHP for this machine, and one for a machine this is not.
///
/// The second entry is what makes `list_available` mean anything: a version published only for
/// another operating system must not be offered here, and a fixture with one artifact could not tell
/// a filter that works from one that does nothing.
fn index(packed: &Packed, url: &str) -> Value {
    let elsewhere = match cfg!(target_os = "linux") {
        true => "macos",
        false => "linux",
    };

    json!({
        "schema": 1,
        "generated_at": "2026-08-14T06:55:12Z",
        "packages": [
            {
                "kind": "php",
                "version": VERSION,
                "channel": "stable",
                "eol": "2027-12-31",
                "artifacts": [{
                    "os": os(),
                    "arch": arch(),
                    "url": url,
                    "sha256": packed.sha256,
                    "size": packed.size(),
                    "provides": { "php": program_name(), sapi(): program_name() },
                }],
            },
            {
                "kind": "php",
                "version": "9.9.9",
                "channel": "stable",
                "artifacts": [{
                    "os": elsewhere,
                    "arch": "x86_64",
                    "url": "https://example.invalid/php-9.9.9.tar.zst",
                    "sha256": "00",
                    "size": 1,
                    "provides": { "php": "bin/php" },
                }],
            },
        ],
    })
}

/// The SAPI a real PHP publishes on this system — roadmap task T32.
///
/// **One and not both**, which is the shape of the index rather than a convenience: a Unix PHP has
/// `php-fpm` and a Windows one has `php-cgi`, and publishing both here would give the php-fpm recipe
/// a `php-fpm` on Windows to run `--test` with, against a fixture that is not php-fpm. The pool the
/// install creates has to be able to build a spec, or every `service.*` call after it fails.
fn sapi() -> &'static str {
    if cfg!(windows) { "php-cgi" } else { "php-fpm" }
}

/// What the index calls the system these tests are running on.
fn os() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        other => other,
    }
}

/// And its architecture.
fn arch() -> &'static str {
    std::env::consts::ARCH
}

/// The daemon process, killed when the test ends however it ends.
struct Daemon(Child);

impl Daemon {
    fn start(home: &Home, registry: &MockRegistry) -> Self {
        Self(
            Command::new(env!("CARGO_BIN_EXE_mixengined"))
                .arg("--home")
                .arg(home.path())
                // Passed as arguments rather than through the environment, per rule 2 in
                // `.claude/standards/testing.md`: two of these running at once under `cargo test`
                // would otherwise be pointed at each other's registry.
                .arg("--index-url")
                .arg(registry.url())
                .arg("--index-key")
                .arg(registry.public_key())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the daemon binary runs"),
        )
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Killed rather than asked: a test that failed halfway must not leave a process holding the
        // temporary home open, which on Windows would make the directory unremovable.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One connection to the daemon.
struct Client {
    sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
}

impl Client {
    async fn connect(home: &Home) -> Self {
        let connection = Connection::connect(home.endpoint())
            .await
            .expect("the daemon is listening");

        let (sender, driver) = hyper::client::conn::http1::handshake(TokioIo::new(connection))
            .await
            .expect("the daemon speaks HTTP/1.1");

        tokio::spawn(driver);

        Self { sender }
    }

    /// Call a method and hand back its `result`, insisting it succeeded.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let answer = self.ask(method, params).await;
        assert!(answer.get("error").is_none(), "{method}: {answer}");

        answer["result"].clone()
    }

    /// Call a method and hand back its `error`, insisting it did not succeed.
    async fn refuse(&mut self, method: &str, params: Value) -> Value {
        let answer = self.ask(method, params).await;
        assert!(answer.get("result").is_none(), "{method}: {answer}");

        answer["error"].clone()
    }

    async fn ask(&mut self, method: &str, params: Value) -> Value {
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": 1 });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/rpc")
            .header(HOST, "mixengine")
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(
                serde_json::to_vec(&body).expect("a request serialises"),
            )))
            .expect("a well formed request");

        let response = self.sender.send_request(request).await.expect("an answer");
        assert_eq!(response.status(), StatusCode::OK, "{method}");

        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a whole body")
            .to_bytes();

        serde_json::from_slice(&bytes).expect("a JSON-RPC response")
    }

    /// Install a version and wait for the job it produced to end.
    async fn install(&mut self, version: &str) -> Value {
        let started = self
            .call(
                "runtime.install",
                json!({"kind": "php", "version": version}),
            )
            .await;

        assert_eq!(
            started["state"], "running",
            "an install is answered as accepted, not as finished: {started}"
        );

        self.finished(started["id"].clone()).await
    }

    /// Wait for a job to end, and answer with it as it ended.
    async fn finished(&mut self, job: Value) -> Value {
        let deadline = tokio::time::Instant::now() + PATIENCE;

        loop {
            let waited = self
                .call("job.wait", json!({"job": job, "timeout": 2_000}))
                .await;

            if waited["state"] != "running" {
                return waited;
            }

            assert!(
                tokio::time::Instant::now() < deadline,
                "the install never finished: {waited}"
            );
        }
    }
}

/// The whole of T23 in one test, because the whole of T23 is one sequence: a version is offered,
/// installed, listed, made the default and removed — and the file on disk agrees at every step.
#[tokio::test]
async fn a_version_the_index_offers_is_installed_listed_chosen_and_removed() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    // Offered, and not yet here.
    let available = client.call("runtime.list_available", json!({})).await;
    let offered = &available["runtimes"];
    assert_eq!(
        offered.as_array().map(Vec::len),
        Some(1),
        "the version published only for another system is not offered here: {available}"
    );
    assert_eq!(offered[0]["version"], VERSION);
    assert_eq!(offered[0]["installed"], false);
    assert_eq!(offered[0]["eol"], "2027-12-31");
    assert_eq!(offered[0]["bytes"], fixture.packed.size());
    assert_eq!(
        available["stale"], false,
        "the registry answered, so nothing came out of a cache"
    );

    assert_eq!(
        client.call("runtime.list_installed", json!({})).await["runtimes"],
        json!([]),
        "a home that has installed nothing lists nothing rather than failing"
    );

    // Installed.
    let installed = client.install(VERSION).await;
    assert_eq!(installed["state"], "succeeded", "{installed}");

    let runtime = &installed["outcome"]["result"];
    assert_eq!(runtime["kind"], "php");
    assert_eq!(runtime["version"], VERSION);
    assert_eq!(runtime["channel"], "stable");
    assert_eq!(
        runtime["default"], true,
        "the first version of a kind becomes its default, or `php` resolves to nothing"
    );

    let on_disk = fixture.installed_at(VERSION);
    assert!(
        on_disk.join(program_name()).is_file(),
        "the archive was unpacked into {}",
        on_disk.display()
    );
    assert_eq!(
        runtime["path"],
        on_disk.display().to_string(),
        "the row names the directory that was actually renamed into place"
    );

    // Listed, and no longer offered as something to install.
    let list = client.call("runtime.list_installed", json!({})).await;
    assert_eq!(list["runtimes"][0]["version"], VERSION);
    assert_eq!(list["runtimes"][0]["default"], true);

    let available = client.call("runtime.list_available", json!({})).await;
    assert_eq!(
        available["runtimes"][0]["installed"], true,
        "which of the two lists a version is on is composed by the daemon, not by a client"
    );

    // A second install of the same version is refused rather than overwriting it.
    let refused = client
        .refuse(
            "runtime.install",
            json!({"kind": "php", "version": VERSION}),
        )
        .await;
    assert_eq!(refused["data"]["code"], "already_exists", "{refused}");

    // Made the default again, which changes nothing and is not an error.
    let chosen = client
        .call(
            "runtime.set_default",
            json!({"kind": "php", "version": VERSION}),
        )
        .await;
    assert_eq!(chosen["default"], true);

    // Removed, directory and row together.
    let removal = client
        .call(
            "runtime.uninstall",
            json!({"kind": "php", "version": VERSION}),
        )
        .await;
    assert_eq!(removal["removed"]["version"], VERSION);
    assert_eq!(
        removal["default_cleared"], true,
        "it was the default, and nothing is promoted in its place"
    );
    assert!(
        !on_disk.exists(),
        "{} is still there after an uninstall",
        on_disk.display()
    );
    assert_eq!(
        client.call("runtime.list_installed", json!({})).await["runtimes"],
        json!([])
    );
}

/// The job is the whole reason an install is not answered inline, so what it reports on the way is
/// part of the contract rather than decoration.
#[tokio::test]
async fn an_install_reports_where_it_has_got_to_and_is_listed_as_a_job() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let installed = client.install(VERSION).await;

    assert_eq!(
        installed["kind"], "runtime.install",
        "a job's kind is the method that produced it"
    );
    assert_eq!(installed["percent"], 100);
    assert_eq!(installed["outcome"]["ending"], "succeeded");

    let jobs = client.call("job.list", json!({})).await;
    assert_eq!(jobs["jobs"][0]["id"], installed["id"]);
    assert_eq!(jobs["jobs"][0]["kind"], "runtime.install");
}

/// Three disappointments the index client answers `None` to alike, told apart — because they send
/// whoever reads the message to three different places.
#[tokio::test]
async fn a_version_the_index_does_not_publish_for_this_machine_says_which_of_the_two_it_is() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    // Published, but not for this machine: a fact about upstream rather than a bug here.
    let elsewhere = client
        .call(
            "runtime.install",
            json!({"kind": "php", "version": "9.9.9"}),
        )
        .await;
    let ended = client.finished(elsewhere["id"].clone()).await;
    assert_eq!(ended["state"], "failed", "{ended}");
    assert_eq!(
        ended["outcome"]["error"]["code"], "unsupported_platform",
        "{ended}"
    );

    // Not published at all.
    let nowhere = client
        .call(
            "runtime.install",
            json!({"kind": "php", "version": "1.2.3"}),
        )
        .await;
    let ended = client.finished(nowhere["id"].clone()).await;
    assert_eq!(ended["outcome"]["error"]["code"], "not_found", "{ended}");

    assert!(
        !fixture.installed_at("9.9.9").exists() && !fixture.installed_at("1.2.3").exists(),
        "nothing was created for a version that was never downloaded"
    );
}

/// A version that cannot be a directory name is refused by the wire type, before any of this is
/// reached — which is what makes `runtimes/<kind>/<version>` a join rather than an escaping problem.
#[tokio::test]
async fn a_version_that_could_leave_its_own_directory_is_refused_by_the_parameters() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    for version in ["../../escape", "..", ""] {
        let refused = client
            .refuse(
                "runtime.install",
                json!({"kind": "php", "version": version}),
            )
            .await;

        assert_eq!(
            refused["data"]["code"], "invalid_argument",
            "{version:?}: {refused}"
        );
        assert_eq!(
            refused["code"], -32602,
            "refused as parameters rather than by the method: {refused}"
        );
    }
}

/// Removing something that is not there names it rather than failing obscurely, and the hint sends
/// somebody to the command that would have listed it.
#[tokio::test]
async fn uninstalling_something_that_was_never_installed_says_what_was_looked_for() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let refused = client
        .refuse(
            "runtime.uninstall",
            json!({"kind": "php", "version": VERSION}),
        )
        .await;

    assert_eq!(refused["data"]["code"], "not_found", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("php 8.3.33")),
        "{refused}"
    );
    assert!(
        refused["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("mix runtime list")),
        "{refused}"
    );
}

/// **T24 over the wire**, and the whole of the order it settles: the same directory answers
/// differently as each source appears above the last, and every answer says which one decided it.
///
/// Only the manifest and the default are exercised end to end here — a project record needs
/// `project.create`, which is Phase 4's, and `core::resolve`'s own tests write that row by hand.
#[tokio::test]
async fn which_version_a_directory_uses_is_answered_by_the_source_that_decided_it() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    // Nothing installed at all: the one question this method cannot answer, and the code the
    // feature spec names for it.
    let refused = client
        .refuse("runtime.resolve", json!({"kind": "php"}))
        .await;
    assert_eq!(refused["data"]["code"], "dependency_missing", "{refused}");

    client.install(VERSION).await;

    // 4 — the default, which the first install became.
    let resolved = client.call("runtime.resolve", json!({"kind": "php"})).await;
    assert_eq!(resolved["runtime"]["version"], VERSION);
    assert_eq!(resolved["source"]["from"], "default", "{resolved}");
    assert!(
        resolved.get("constraint").is_none(),
        "a default names no constraint: {resolved}"
    );

    // 2 — a `mixengine.toml` above the directory the question is asked from.
    let project = tempfile::tempdir().expect("a temporary directory");
    let public = project.path().join("public");
    std::fs::create_dir(&public).expect("a directory");
    std::fs::write(
        project.path().join("mixengine.toml"),
        "[project]\nname = \"blog\"\n\n[runtimes]\nphp = \"^8.3\"\n",
    )
    .expect("a manifest");

    let resolved = client
        .call(
            "runtime.resolve",
            json!({"kind": "php", "cwd": public.display().to_string()}),
        )
        .await;
    assert_eq!(resolved["runtime"]["version"], VERSION);
    assert_eq!(resolved["source"]["from"], "manifest", "{resolved}");
    assert!(
        resolved["source"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("mixengine.toml")),
        "the file that decided it is named, because that is what somebody would go and edit: \
         {resolved}"
    );
    assert_eq!(resolved["constraint"], "^8.3");

    // 1 — what the caller was told, which beats the file.
    let resolved = client
        .call(
            "runtime.resolve",
            json!({"kind": "php", "cwd": public.display().to_string(), "version": VERSION}),
        )
        .await;
    assert_eq!(resolved["source"]["from"], "explicit", "{resolved}");
}

/// A pin nothing installed satisfies is the failure people will actually meet — after a `git clone`
/// of a repository that asks for a version this machine has never had — so what it *says* is the
/// feature.
#[tokio::test]
async fn a_pin_this_machine_cannot_satisfy_says_what_to_install() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    client.install(VERSION).await;

    let project = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(
        project.path().join("mixengine.toml"),
        "[runtimes]\nphp = \"8.1.30\"\n",
    )
    .expect("a manifest");

    let refused = client
        .refuse(
            "runtime.resolve",
            json!({"kind": "php", "cwd": project.path().display().to_string()}),
        )
        .await;

    assert_eq!(refused["data"]["code"], "dependency_missing", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("8.1.30") && message.contains("mixengine.toml")),
        "the message names both the version and the file that asked for it: {refused}"
    );
    assert!(
        refused["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("mix runtime install php 8.1.30")),
        "an exact pin becomes the exact command: {refused}"
    );

    // A range cannot, because the version that would satisfy it is one nobody has published yet as
    // far as this machine knows.
    std::fs::write(
        project.path().join("mixengine.toml"),
        "[runtimes]\nphp = \"^9.0\"\n",
    )
    .expect("a manifest");

    let refused = client
        .refuse(
            "runtime.resolve",
            json!({"kind": "php", "cwd": project.path().display().to_string()}),
        )
        .await;
    assert!(
        refused["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("mix runtime available")),
        "{refused}"
    );
}

/// Two ways a caller can ask something a daemon cannot answer, told apart from a version that is
/// simply not here: both are the *client's* mistake and neither is `dependency_missing`.
#[tokio::test]
async fn a_question_a_daemon_cannot_make_sense_of_is_refused_as_a_bad_argument() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    client.install(VERSION).await;

    // A relative directory would be walked from wherever `mixengined` was started.
    let refused = client
        .refuse(
            "runtime.resolve",
            json!({"kind": "php", "cwd": "blog/public"}),
        )
        .await;
    assert_eq!(refused["data"]["code"], "invalid_argument", "{refused}");

    // A manifest that does not parse, which is the user's file rather than their machine.
    let project = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(
        project.path().join("mixengine.toml"),
        "[runtimes]\nphhp = \"8.3\"\n",
    )
    .expect("a manifest");

    let refused = client
        .refuse(
            "runtime.resolve",
            json!({"kind": "php", "cwd": project.path().display().to_string()}),
        )
        .await;
    assert_eq!(refused["data"]["code"], "invalid_argument", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("mixengine.toml")),
        "{refused}"
    );

    // And a constraint that is not one is refused by the parameters, before any of this is reached.
    let refused = client
        .refuse("runtime.resolve", json!({"kind": "php", "version": "~8.3"}))
        .await;
    assert_eq!(refused["code"], -32602, "{refused}");
}
/// An installed PHP arrives with the pool that serves its sites, and nobody asked for one.
///
/// **The post-install hook, seen from the outside** — roadmap task T32.
/// `.claude/features/runtime-versions.md` decided this before there was a pool to create: a PHP
/// without one is a language no site can be served by, so `runtime.install` makes the record and
/// `service.create` refuses to. The other half of the pair is here too, because the two are one
/// promise: the runtime cannot be removed while its pool is a row, and removing it takes the row.
#[tokio::test]
async fn an_installed_php_arrives_with_its_pool_and_leaves_without_it() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    client.install(VERSION).await;

    let pool = format!("php-fpm@{VERSION}");

    let listed = client.call("service.list", json!({})).await;
    assert!(
        listed["services"]
            .as_array()
            .is_some_and(|services| services.iter().any(|service| service["id"] == pool)),
        "the install created no pool: {listed}"
    );

    // And `service.create` will not write a second one by hand: the row it would need points at a
    // `runtime_installs` row this call has no way to name.
    let refused = client
        .refuse("service.create", json!({ "id": pool, "version": VERSION }))
        .await;
    assert_eq!(refused["data"]["code"], "invalid_argument", "{refused}");
    assert!(
        refused["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("mix runtime install php")),
        "the refusal names the command that does work: {refused}"
    );

    // Removing the runtime removes the pool with it — the row before the directory, so that a
    // `services` row never outlives the install it points at.
    client
        .call(
            "runtime.uninstall",
            json!({"kind": "php", "version": VERSION}),
        )
        .await;

    let listed = client.call("service.list", json!({})).await;
    assert!(
        listed["services"]
            .as_array()
            .is_some_and(|services| services.iter().all(|service| service["id"] != pool)),
        "the pool outlived the PHP it ran out of: {listed}"
    );
}

/// The two methods, against a runtime the fixture recorded rather than downloaded.
///
/// A row and not an install for the reason [`declare::runtime_with_extensions`] gives: what is being
/// proved here is the state model and the wire shape. That the generated files a real PHP then loads
/// say what this answers is `crates/mixengine-cli/tests/php_extensions.rs`.
#[tokio::test(flavor = "multi_thread")]
async fn extensions_are_listed_with_a_reason_and_turned_round_one_at_a_time() {
    let fixture = Fixture::start().await;
    declare::runtime_with_extensions(&fixture.home.database_file(), "8.4.1").await;
    let mut client = fixture.client().await;

    let target = json!({"kind": "php", "version": "8.4.1"});
    let listed = client.call("runtime.list_extensions", target.clone()).await;
    let of = |name: &str| {
        listed["extensions"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|extension| extension["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} is missing from {listed}"))
    };

    assert_eq!(of("opcache")["linkage"], "static");
    assert_eq!(of("xdebug")["enabled"], false);
    assert_eq!(
        of("xdebug")["source"],
        "build_default",
        "off because this build says so, and nobody has said otherwise yet"
    );

    let changed = client
        .call(
            "runtime.set_extension",
            json!({"kind": "php", "version": "8.4.1", "name": "xdebug", "enabled": true}),
        )
        .await;

    assert_eq!(changed["extension"]["enabled"], true);
    assert_eq!(changed["extension"]["source"], "user");
    assert_eq!(
        changed["pool"], "pool_not_running",
        "nothing was started, so nothing was reloaded and nothing has to be restarted"
    );

    let refused = client
        .refuse(
            "runtime.set_extension",
            json!({"kind": "php", "version": "8.4.1", "name": "opcache", "enabled": false}),
        )
        .await;

    // The domain code, under `data`, where every other refusal in this suite reads it.
    assert_eq!(refused["data"]["code"], "unsupported_platform", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|said| said.contains("compiled into")),
        "a refusal has to say that a different build is what it would take: {refused}"
    );
}
