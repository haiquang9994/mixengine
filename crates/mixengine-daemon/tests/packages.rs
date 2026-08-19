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
