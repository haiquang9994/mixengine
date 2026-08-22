//! `project.*` against a real `mixengined` over a real socket.
//!
//! Roadmap task **T39**. What the unit tests next to `core::projects` prove is that the rows and the
//! walk are right; what is proved here is the part only a daemon can be wrong about — that a create
//! that names only a directory picks up the manifest lying in it, that a manifest pin is reported as
//! outranking a contradicting row, and that the same directory under two spellings is one project.
//!
//! No registry and no index: nothing here installs anything, so the daemon needs neither.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_testkit::Home;
use serde_json::{Value, json};

/// A home with a daemon in it, killed when the test ends.
struct Fixture {
    home: Home,
    _daemon: Daemon,
}

impl Fixture {
    async fn start() -> Self {
        let home = Home::new();
        let daemon = Daemon::start(&home);
        home.wait_until_listening().await;

        Self {
            home,
            _daemon: daemon,
        }
    }

    async fn client(&self) -> Client {
        Client::connect(&self.home).await
    }
}

struct Daemon(Child);

impl Daemon {
    fn start(home: &Home) -> Self {
        Self(
            Command::new(env!("CARGO_BIN_EXE_mixengined"))
                .arg("--home")
                .arg(home.path())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the daemon binary runs"),
        )
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Killed rather than asked, on `tests/runtimes.rs`' reasoning: a test that failed halfway
        // must not leave a process holding the temporary home open.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One connection to the daemon. The same three helpers `tests/runtimes.rs` carries.
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

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let answer = self.ask(method, params).await;
        assert!(answer.get("error").is_none(), "{method}: {answer}");
        answer["result"].clone()
    }

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

        self.sender
            .ready()
            .await
            .expect("the connection is still open");

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
}

/// A directory to register, and its manifest when it has one.
fn repository(body: Option<&str>) -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("mixengine-project")
        .tempdir()
        .expect("a temporary directory");

    if let Some(body) = body {
        std::fs::write(directory.path().join("mixengine.toml"), body).expect("a manifest");
    }

    directory
}

fn as_string(path: &Path) -> String {
    path.display().to_string()
}

/// The whole life of a project, in the order somebody lives it.
#[tokio::test]
async fn a_project_is_created_listed_shown_changed_and_forgotten() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(None);

    let empty = client.call("project.list", Value::Null).await;
    assert_eq!(empty["projects"], json!([]));

    let created = client
        .call(
            "project.create",
            json!({"root": as_string(repository.path()), "name": "blog"}),
        )
        .await;
    assert_eq!(created["project"]["name"], "blog");
    assert_eq!(created["pins"], json!([]));
    assert!(
        created["project"]["manifest"].is_null(),
        "there is no manifest in that directory: {created}"
    );

    // Addressed by any directory inside it, which is what a shell has.
    let inside = repository.path().join("public");
    std::fs::create_dir(&inside).expect("a directory");
    let shown = client
        .call(
            "project.show",
            json!({"project": {"path": as_string(&inside)}}),
        )
        .await;
    assert_eq!(shown["project"]["name"], "blog");

    let changed = client
        .call(
            "project.update",
            json!({
                "project": {"name": "blog"},
                "name": "weblog",
                "pins": {"php": "^8.3"},
            }),
        )
        .await;
    assert_eq!(changed["project"]["name"], "weblog");
    assert_eq!(changed["pins"][0]["constraint"], "^8.3");
    assert_eq!(changed["pins"][0]["source"]["from"], "registered");
    assert!(
        changed["pins"][0]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("runtime install php")),
        "nothing is installed, so the pin says what would satisfy it: {changed}"
    );

    let removed = client
        .call("project.delete", json!({"project": {"name": "weblog"}}))
        .await;
    assert_eq!(removed["removed"]["name"], "weblog");
    assert!(
        Path::new(removed["root_kept"].as_str().expect("a path")).is_dir(),
        "the directory is kept, and named: {removed}"
    );

    let gone = client
        .refuse("project.show", json!({"project": {"name": "weblog"}}))
        .await;
    assert_eq!(gone["data"]["code"], "not_found", "{gone}");
}

/// **The import.** A create that names only a directory takes the name and the pins out of the
/// manifest a colleague checked in — no flag, no second method.
#[tokio::test]
async fn a_create_that_names_only_a_directory_reads_the_manifest_in_it() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(Some(
        "[project]\nname = \"shop\"\n\n[runtimes]\nphp = \"^8.3\"\n",
    ));

    let created = client
        .call(
            "project.create",
            json!({"root": as_string(repository.path())}),
        )
        .await;

    assert_eq!(created["project"]["name"], "shop");
    assert_eq!(created["pins"][0]["kind"], "php");
    assert_eq!(created["pins"][0]["constraint"], "^8.3");
    assert_eq!(
        created["pins"][0]["source"]["from"], "manifest",
        "the file it came from is what decides, so that is what is reported: {created}"
    );
    assert!(
        created["project"]["manifest"]
            .as_str()
            .is_some_and(|path| path.ends_with("mixengine.toml")),
        "{created}"
    );
}

/// **D1's whole reason.** A row pin the manifest contradicts is inert, and the answer says so
/// rather than leaving somebody reading one version while their shell runs another.
#[tokio::test]
async fn a_manifest_pin_is_reported_as_outranking_the_row_it_contradicts() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(Some("[runtimes]\nphp = \"8.4\"\n"));

    let created = client
        .call(
            "project.create",
            json!({
                "root": as_string(repository.path()),
                "name": "blog",
                "pins": {"php": "8.2"},
            }),
        )
        .await;

    assert_eq!(created["pins"].as_array().expect("pins").len(), 1);
    assert_eq!(created["pins"][0]["constraint"], "8.4");
    assert_eq!(created["pins"][0]["source"]["from"], "manifest");
}

/// One directory is one project, however this filesystem spells it.
#[tokio::test]
async fn the_same_directory_under_two_spellings_is_one_project() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(None);

    client
        .call(
            "project.create",
            json!({"root": as_string(repository.path()), "name": "blog"}),
        )
        .await;

    let refused = client
        .refuse(
            "project.create",
            json!({
                "root": as_string(&mixengine_platform::paths::in_full(repository.path())),
                "name": "other",
            }),
        )
        .await;

    assert_eq!(refused["data"]["code"], "already_exists", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|said| said.contains("blog")),
        "the refusal names the project that has it: {refused}"
    );
}

/// A root that is not a directory this machine can find is the caller's own bug, and is refused
/// before anything is written.
#[tokio::test]
async fn a_root_that_is_relative_or_missing_is_refused() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let elsewhere = repository(None);
    let missing = elsewhere.path().join("not-created-yet");

    for root in ["blog/public".to_owned(), as_string(&missing)] {
        let refused = client
            .refuse("project.create", json!({"root": root, "name": "blog"}))
            .await;

        assert_eq!(
            refused["data"]["code"], "invalid_argument",
            "{root}: {refused}"
        );
    }
}

/// **D10.** An export merges into the file rather than rewriting it, and says which it did.
#[tokio::test]
async fn an_export_writes_the_project_into_the_manifest_and_keeps_the_rest() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(Some("# mine\n[site]\ndomain = \"blog.test\"\n"));

    client
        .call(
            "project.create",
            json!({
                "root": as_string(repository.path()),
                "name": "blog",
                "pins": {"php": "^8.3"},
            }),
        )
        .await;

    let exported = client
        .call("project.export", json!({"project": {"name": "blog"}}))
        .await;

    assert_eq!(exported["created"], false, "the file was already there");
    let written =
        std::fs::read_to_string(exported["path"].as_str().expect("a path")).expect("the manifest");
    assert!(written.contains("# mine"), "{written}");
    assert!(written.contains("[site]"), "{written}");
    assert!(written.contains("name = \"blog\""), "{written}");
    assert!(written.contains("php = \"^8.3\""), "{written}");
}

/// **D9.** An export sends the site, because a file with the runtimes and not the site loses the
/// thing worth sending. A project with two sites writes neither, and says which.
#[tokio::test]
async fn an_export_writes_the_site_and_names_the_ones_it_could_not() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(None);
    let root = as_string(repository.path());

    client
        .call("project.create", json!({"root": root, "name": "blog"}))
        .await;
    client
        .call(
            "site.create",
            json!({
                "project": {"name": "blog"},
                "domains": ["blog.test"],
                "kind": {"kind": "static"},
            }),
        )
        .await;

    let exported = client
        .call("project.export", json!({"project": {"name": "blog"}}))
        .await;
    assert_eq!(
        exported["sites_omitted"],
        Value::Null,
        "one site is written, so nothing is omitted: {exported}"
    );

    let written = std::fs::read_to_string(repository.path().join("mixengine.toml"))
        .expect("the manifest that was written");
    assert!(written.contains("domain = \"blog.test\""), "{written}");
    assert!(written.contains("kind = \"static\""), "{written}");

    // A second site, and the file format's own limit is reported rather than half-honoured.
    client
        .call(
            "site.create",
            json!({
                "project": {"name": "blog"},
                "domains": ["shop.test"],
                "kind": {"kind": "static"},
            }),
        )
        .await;

    let again = client
        .call("project.export", json!({"project": {"name": "blog"}}))
        .await;
    let omitted = again["sites_omitted"].as_array().expect("a list");
    assert_eq!(omitted.len(), 2, "{again}");
}
