//! `site.*` against a real `mixengined` over a real socket.
//!
//! Roadmap task **T39a**. What the unit tests next to `core::sites` prove is that the rows are
//! right; what is proved here is the part only a daemon can be wrong about — that a create with
//! nothing but a project named builds a site out of a colleague's manifest, that the fall-through
//! reaches the right default when it does not, and that deleting a project takes its sites with it.
//!
//! No registry and no index, on `tests/projects.rs`' reasoning: nothing here installs anything. The
//! refusals that need a *declared service* are in `tests/packages.rs`, where a fixture can declare
//! one against a package it has actually installed.

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

/// The whole life of a site, in the order somebody lives it.
#[tokio::test]
async fn a_site_is_created_listed_shown_changed_and_forgotten() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(None);
    std::fs::create_dir(repository.path().join("public")).expect("a doc root");

    client
        .call(
            "project.create",
            json!({"root": as_string(repository.path()), "name": "blog"}),
        )
        .await;

    let empty = client.call("site.list", Value::Null).await;
    assert_eq!(empty["sites"], json!([]), "and no parameters is a question");

    let created = client
        .call(
            "site.create",
            json!({
                "project": {"name": "blog"},
                "domains": ["blog.test", "www.blog.test"],
                "doc_root": "public",
                "kind": {"kind": "static"},
            }),
        )
        .await;

    assert_eq!(created["site"]["site"]["domain"], "blog.test");
    assert_eq!(created["site"]["site"]["state"], "enabled");
    assert_eq!(created["site"]["doc_root_exists"], true);
    assert_eq!(
        created["site"]["domains"],
        json!(["blog.test", "www.blog.test"]),
        "ordered, head first"
    );

    // Reached from inside the project, which is what a shell has.
    let inside = repository.path().join("public");
    let shown = client
        .call("site.show", json!({"site": {"path": as_string(&inside)}}))
        .await;
    assert_eq!(shown["site"]["domain"], "blog.test");

    // And by an alias, which the unique index makes unambiguous.
    let by_alias = client
        .call("site.show", json!({"site": {"domain": "www.blog.test"}}))
        .await;
    assert_eq!(by_alias["site"]["domain"], "blog.test");

    // A replacement removes an alias, which a merge could not.
    let changed = client
        .call(
            "site.update",
            json!({"site": {"domain": "blog.test"}, "domains": ["blog.test"], "state": "disabled"}),
        )
        .await;
    assert_eq!(changed["domains"], json!(["blog.test"]));
    assert_eq!(changed["site"]["state"], "disabled");

    let removed = client
        .call("site.delete", json!({"site": {"domain": "blog.test"}}))
        .await;
    assert_eq!(removed["domains_released"], json!(["blog.test"]));
    assert!(
        removed["doc_root_kept"]
            .as_str()
            .expect("a path")
            .ends_with("public"),
        "{removed}"
    );
}

/// **D7.** `site.create { project }` and nothing else builds the site the manifest describes.
#[tokio::test]
async fn a_create_that_names_only_a_project_is_the_import() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(Some(
        "[project]\nname = \"shop\"\n\n[site]\ndomain = \"shop.test\"\n\
         aliases = [\"api.shop.test\"]\ndoc_root = \"web\"\nkind = \"reverse-proxy\"\n\
         upstream = \"http://127.0.0.1:5173\"\nhttps = false\n",
    ));

    client
        .call(
            "project.create",
            json!({"root": as_string(repository.path())}),
        )
        .await;

    let created = client
        .call("site.create", json!({"project": {"name": "shop"}}))
        .await;

    assert_eq!(created["site"]["site"]["domain"], "shop.test");
    assert_eq!(
        created["site"]["domains"],
        json!(["shop.test", "api.shop.test"])
    );
    assert_eq!(created["site"]["site"]["doc_root"], "web");
    assert_eq!(created["site"]["site"]["kind"]["kind"], "reverse-proxy");
    assert_eq!(
        created["site"]["site"]["kind"]["upstream"],
        "http://127.0.0.1:5173"
    );
    assert_eq!(created["site"]["site"]["https"], false);
    assert_eq!(
        created["site"]["doc_root_exists"], false,
        "a doc root built by `npm run build` is reported, never refused — D2"
    );
}

/// **D10.** With no manifest and no argument, the domain comes from the project's name.
#[tokio::test]
async fn a_site_with_nothing_named_takes_its_projects_name_and_test() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(None);

    client
        .call(
            "project.create",
            json!({"root": as_string(repository.path()), "name": "My Shop"}),
        )
        .await;

    let created = client
        .call(
            "site.create",
            json!({"project": {"name": "My Shop"}, "kind": {"kind": "static"}}),
        )
        .await;

    assert_eq!(created["site"]["site"]["domain"], "my-shop.test");
}

/// One domain, one site — and the refusal names the site holding it rather than an index.
#[tokio::test]
async fn a_domain_another_site_owns_is_refused_by_name() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let first = repository(None);
    let second = repository(None);

    for (root, name) in [(&first, "blog"), (&second, "shop")] {
        client
            .call(
                "project.create",
                json!({"root": as_string(root.path()), "name": name}),
            )
            .await;
    }

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

    let refused = client
        .refuse(
            "site.create",
            json!({
                "project": {"name": "shop"},
                "domains": ["blog.test"],
                "kind": {"kind": "static"},
            }),
        )
        .await;

    assert!(
        refused["message"]
            .as_str()
            .expect("a message")
            .contains("blog.test"),
        "{refused}"
    );
}

/// The TLD table, at the door a person actually reaches it through.
#[tokio::test]
async fn the_tld_policy_is_the_same_one_the_feature_document_states() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let repository = repository(None);

    client
        .call(
            "project.create",
            json!({"root": as_string(repository.path()), "name": "blog"}),
        )
        .await;

    let create = |domain: &str, risky: bool| {
        json!({
            "project": {"name": "blog"},
            "domains": [domain],
            "kind": {"kind": "static"},
            "accept_risky_tld": risky,
        })
    };

    for public in ["blog.dev", "blog.app"] {
        let refused = client.refuse("site.create", create(public, false)).await;
        assert!(
            refused["data"]["hint"]
                .as_str()
                .unwrap_or_default()
                .contains("test"),
            "{refused}"
        );
    }

    client
        .refuse("site.create", create("blog.local", false))
        .await;
    client.call("site.create", create("blog.local", true)).await;
}

/// The cascade, through the API rather than through SQL: forgetting a project forgets its sites.
#[tokio::test]
async fn forgetting_a_project_forgets_its_sites_and_frees_their_domains() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let first = repository(None);
    let second = repository(None);

    client
        .call(
            "project.create",
            json!({"root": as_string(first.path()), "name": "blog"}),
        )
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

    client
        .call("project.delete", json!({"project": {"name": "blog"}}))
        .await;

    assert_eq!(
        client.call("site.list", Value::Null).await["sites"],
        json!([])
    );

    // And the domain is genuinely free, which is the half a cascade can silently not do.
    client
        .call(
            "project.create",
            json!({"root": as_string(second.path()), "name": "shop"}),
        )
        .await;
    client
        .call(
            "site.create",
            json!({
                "project": {"name": "shop"},
                "domains": ["blog.test"],
                "kind": {"kind": "static"},
            }),
        )
        .await;
}
