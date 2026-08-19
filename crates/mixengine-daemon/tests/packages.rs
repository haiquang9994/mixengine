//! `package.*` against a real `mixengined`, a real signed index and a real archive.
//!
//! Roadmap task **T31a**, and [`tests/runtimes.rs`](runtimes.rs)' shape one namespace across: a
//! [`MockRegistry`] serves a document it signs over a loopback socket, the daemon is pointed at both
//! with `--index-url` and `--index-key`, and nothing here touches the network.
//!
//! **What is published is `fakeservice`**, because a debug build has a recipe for it and the archive
//! can be a real executable that needs no server behind it. The index also publishes a `redis`,
//! which this build has no recipe for — that is what makes the catalogue filter mean something,
//! since a fixture offering only what is runnable could not tell a filter that works from one that
//! does nothing.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_testkit::{FakePackage, Home, MockRegistry, Packed, Packing};
use serde_json::{Value, json};

/// How long a job is given to finish before the test gives up on it.
const PATIENCE: Duration = Duration::from_secs(60);

/// The version this suite installs.
const VERSION: &str = "1.0.0";

/// The package it installs, which is the one a debug build has a recipe for.
const PACKAGE: &str = "fakeservice";

/// A package the index publishes and this build cannot run.
const UNRUNNABLE: &str = "redis";

/// The name the archive publishes its executable under, with the suffix this OS needs to spawn it.
fn program_name() -> String {
    format!("fakeservice{}", std::env::consts::EXE_SUFFIX)
}

/// A registry serving one `fakeservice`, a home, and a daemon pointed at both.
struct Fixture {
    home: Home,
    /// Held rather than read: dropping it would stop the server the daemon downloads from.
    _registry: MockRegistry,
    _daemon: Daemon,
    packed: Packed,
}

impl Fixture {
    /// Publish one version of one package and start a daemon that can see it.
    async fn start() -> Self {
        // `.zip` on Windows and `.tar.zst` elsewhere, which is what the publishing pipeline produces
        // for each — the point being that the daemon unpacks what its own platform is served.
        let packing = match cfg!(windows) {
            true => Packing::Zip,
            false => Packing::TarZst,
        };
        let packed = FakePackage::new(packing)
            .executable(&program_name())
            .build(&format!("{PACKAGE}-{VERSION}"));

        let registry = MockRegistry::start(&json!({
            "schema": 1,
            "generated_at": "2026-08-19T06:55:12Z",
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

    /// A fixture whose package is already installed, for the tests about services.
    async fn started_with_package() -> Self {
        let fixture = Self::start().await;
        let installed = fixture.client().await.install(VERSION).await;
        assert_eq!(installed["state"], "succeeded", "{installed}");

        fixture
    }

    /// Where a service's generated configuration goes.
    fn etc_for(&self, service: &str) -> std::path::PathBuf {
        self.home.path().join("etc").join(service)
    }

    /// Where the daemon would have put this version.
    fn installed_at(&self, version: &str) -> std::path::PathBuf {
        self.home
            .path()
            .join("packages")
            .join(PACKAGE)
            .join(version)
    }
}

/// An index offering one `fakeservice` for this machine, and one package this build cannot run.
fn index(packed: &Packed, url: &str) -> Value {
    let artifacts = json!([{
        "os": os(),
        "arch": arch(),
        "url": url,
        "sha256": packed.sha256,
        "size": packed.size(),
        "provides": { "fakeservice": program_name() },
    }]);

    json!({
        "schema": 1,
        "generated_at": "2026-08-19T06:55:12Z",
        "packages": [
            {
                "kind": PACKAGE,
                "version": VERSION,
                "channel": "stable",
                "artifacts": artifacts,
            },
            {
                // Published, installable for this machine, and still not offered: this build has no
                // recipe for it, so a download would end in a directory nothing could start.
                "kind": UNRUNNABLE,
                "version": "8.0.0",
                "channel": "stable",
                "artifacts": artifacts,
            },
        ],
    })
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
                "package.install",
                json!({"package": PACKAGE, "version": version}),
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

/// The whole of the package half of T31a in one test, because it is one sequence: a version is
/// offered, installed, listed and removed — and the directory on disk agrees at every step.
#[tokio::test]
async fn a_service_package_is_offered_installed_listed_and_removed() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    // Offered, and not yet here.
    let available = client.call("package.list_available", json!({})).await;
    let offered = &available["packages"];
    assert_eq!(
        offered.as_array().map(Vec::len),
        Some(1),
        "the package this build has no recipe for is not offered: {available}"
    );
    assert_eq!(offered[0]["package"], PACKAGE);
    assert_eq!(offered[0]["version"], VERSION);
    assert_eq!(offered[0]["installed"], false);
    assert_eq!(offered[0]["bytes"], fixture.packed.size());
    assert_eq!(
        available["stale"], false,
        "the registry answered, so nothing came out of a cache"
    );

    assert_eq!(
        client.call("package.list", json!({})).await["packages"],
        json!([]),
        "a home that has installed nothing lists nothing rather than failing"
    );

    // Installed.
    let installed = client.install(VERSION).await;
    assert_eq!(installed["state"], "succeeded", "{installed}");

    let package = &installed["outcome"]["result"];
    assert_eq!(package["package"], PACKAGE);
    assert_eq!(package["version"], VERSION);
    assert_eq!(
        package["services"],
        json!([]),
        "nothing can be an instance of a version installed a moment ago"
    );
    assert!(
        fixture.installed_at(VERSION).is_dir(),
        "the archive was unpacked where the row says it is"
    );

    // Listed, and no longer offered as something to install.
    let list = client.call("package.list", json!({})).await;
    assert_eq!(list["packages"].as_array().map(Vec::len), Some(1), "{list}");
    assert_eq!(list["packages"][0]["package"], PACKAGE);

    let available = client.call("package.list_available", json!({})).await;
    assert_eq!(
        available["packages"][0]["installed"], true,
        "the daemon composes this rather than leaving a client to cross-reference: {available}"
    );

    // Removed, directory and row together.
    let removal = client
        .call(
            "package.uninstall",
            json!({"package": PACKAGE, "version": VERSION}),
        )
        .await;
    assert_eq!(removal["removed"]["version"], VERSION);
    assert!(
        !fixture.installed_at(VERSION).exists(),
        "the directory goes with the row"
    );
    assert_eq!(
        client.call("package.list", json!({})).await["packages"],
        json!([])
    );
}

/// A kind this build has no recipe for is refused at install, not at create — nobody spends a
/// download on a directory MixEngine could never start anything out of.
#[tokio::test]
async fn a_package_this_build_cannot_run_is_refused_with_what_it_can() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let error = client
        .refuse(
            "package.install",
            json!({"package": UNRUNNABLE, "version": "8.0.0"}),
        )
        .await;

    assert_eq!(error["data"]["code"], "invalid_argument", "{error}");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains(UNRUNNABLE)),
        "it names what was asked for: {error}"
    );
    assert!(
        error["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("caddy")),
        "and what exists instead: {error}"
    );
}

/// The same rule seen from the listing side, filtered rather than surveyed.
#[tokio::test]
async fn a_filter_naming_something_unrunnable_is_refused_rather_than_answered_empty() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let error = client
        .refuse("package.list_available", json!({"package": UNRUNNABLE}))
        .await;

    assert_eq!(error["data"]["code"], "invalid_argument", "{error}");
}

/// Installing the same version twice is two terminals, and the second is asking for the outcome the
/// first already reached.
#[tokio::test]
async fn installing_a_version_that_is_already_here_says_so_rather_than_downloading_it_again() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    assert_eq!(client.install(VERSION).await["state"], "succeeded");

    let error = client
        .refuse(
            "package.install",
            json!({"package": PACKAGE, "version": VERSION}),
        )
        .await;

    assert_eq!(error["data"]["code"], "already_exists", "{error}");
}

/// The whole point of the task: an installed package becomes a service a person can start.
#[tokio::test]
async fn an_installed_package_becomes_a_service_and_can_be_deleted_again() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    let created = client
        .call(
            "service.create",
            json!({"id": "fakeservice@main", "version": VERSION}),
        )
        .await;
    assert_eq!(created["id"], "fakeservice@main");
    assert_eq!(created["state"], "stopped", "{created}");
    assert!(
        fixture.etc_for("fakeservice@main").is_dir(),
        "a create renders before it answers"
    );

    let list = client.call("service.list", Value::Null).await;
    assert_eq!(
        list["services"].as_array().map(Vec::len),
        Some(1),
        "a created service is a declared service: {list}"
    );

    // And the package it is an instance of is no longer one that can be removed.
    let held = client
        .refuse(
            "package.uninstall",
            json!({"package": PACKAGE, "version": VERSION}),
        )
        .await;
    assert_eq!(held["data"]["code"], "precondition_failed", "{held}");
    assert!(
        held["message"]
            .as_str()
            .is_some_and(|message| message.contains("fakeservice@main")),
        "it names what holds it: {held}"
    );

    let removal = client
        .call("service.delete", json!({"service": "fakeservice@main"}))
        .await;
    assert_eq!(removal["removed"]["id"], "fakeservice@main");
    assert!(
        !fixture.etc_for("fakeservice@main").exists(),
        "generated configuration is disposable and goes with the row"
    );
    assert_eq!(
        client.call("service.list", Value::Null).await["services"],
        json!([])
    );

    // Which frees the package.
    let removed = client
        .call(
            "package.uninstall",
            json!({"package": PACKAGE, "version": VERSION}),
        )
        .await;
    assert_eq!(removed["removed"]["version"], VERSION);
}

/// The recipe says how many instances it has, and the id is where a person meets the answer.
#[tokio::test]
async fn a_named_instance_recipe_refuses_an_id_with_no_instance() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    let error = client
        .refuse("service.create", json!({"id": PACKAGE, "version": VERSION}))
        .await;

    assert_eq!(error["data"]["code"], "invalid_argument", "{error}");
    assert!(
        error["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("fakeservice@")),
        "it shows the shape: {error}"
    );
}

/// A version nobody installed is a missing step rather than a mistake, and the hint is the step.
#[tokio::test]
async fn creating_a_service_from_a_package_that_is_not_installed_names_the_install() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let error = client
        .refuse(
            "service.create",
            json!({"id": "fakeservice@main", "version": VERSION}),
        )
        .await;

    assert_eq!(error["data"]["code"], "precondition_failed", "{error}");
    assert!(
        error["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("package install")),
        "{error}"
    );
}

/// One row that cannot be rendered fails the whole declared set, so a bad row left behind would take
/// `service.list` down with it.
#[tokio::test]
async fn a_create_that_cannot_be_rendered_leaves_the_home_as_it_was() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    let error = client
        .refuse(
            "service.create",
            json!({
                "id": "fakeservice@bad",
                "version": VERSION,
                "overrides": {"exit_afterr": 1},
            }),
        )
        .await;
    assert_eq!(error["data"]["code"], "invalid_argument", "{error}");

    let list = client.call("service.list", Value::Null).await;
    assert_eq!(
        list["services"],
        json!([]),
        "the row went with the failure: {list}"
    );
}

/// A delete keeps the data directory, and says which one it kept.
#[tokio::test]
async fn a_delete_keeps_the_data_directory_and_says_so() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    client
        .call(
            "service.create",
            json!({"id": "fakeservice@main", "version": VERSION}),
        )
        .await;

    // Created by hand rather than by starting the service: what is being tested is that a delete
    // leaves a directory alone, and the cheapest way to have one is to make one.
    let data = fixture.home.path().join("data").join(PACKAGE).join("main");
    std::fs::create_dir_all(&data).expect("a data directory in a temporary home");

    let removal = client
        .call("service.delete", json!({"service": "fakeservice@main"}))
        .await;

    let kept = removal["data_kept"]
        .as_str()
        .unwrap_or_else(|| panic!("a data directory is named: {removal}"));
    assert!(kept.ends_with("main"), "{kept}");
    assert!(data.is_dir(), "it is named because it is still there");
}

/// A row deleted out from under a live process would leave the process with nothing describing it.
#[tokio::test]
async fn a_running_service_is_not_deleted_out_from_under_itself() {
    let fixture = Fixture::started_with_package().await;
    let mut client = fixture.client().await;

    client
        .call(
            "service.create",
            json!({"id": "fakeservice@main", "version": VERSION}),
        )
        .await;

    let walk = client
        .call("service.start", json!({"service": "fakeservice@main"}))
        .await;
    assert_eq!(walk["reached"], json!(["fakeservice@main"]), "{walk}");

    let error = client
        .refuse("service.delete", json!({"service": "fakeservice@main"}))
        .await;

    assert_eq!(error["data"]["code"], "precondition_failed", "{error}");
    assert!(
        error["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("service stop")),
        "{error}"
    );

    // And it goes once it is stopped, which is what makes the refusal a step rather than a wall.
    client
        .call("service.stop", json!({"service": "fakeservice@main"}))
        .await;
    client
        .call("service.delete", json!({"service": "fakeservice@main"}))
        .await;
}
