//! `elevation.*` against a real `mixengined` over a real socket. Roadmap task **T40b**.
//!
//! **Nothing here calls `elevation.grant` successfully, and that is deliberate.** These tests spawn a
//! real daemon, which uses the real `Host` — a successful grant would be a real UAC dialog on the
//! machine running `cargo test`. The assertions about what a grant raises belong to the unit tests
//! beside `src/elevation.rs`, which can inject `mock::Host` and this suite cannot. Stated here so a
//! later reader does not "fix" the gap.
//!
//! Most rows here are put in the queue by opening the daemon's own database and calling
//! `mixengine_core::elevation::enqueue`, which is the honest way to prove the claim the table
//! exists for: the queue is on disk, so a daemon that did not write a row still reports it. The
//! producer T41 added is driven end to end by one test of its own, below — a site is created over
//! the socket, and an operation is found waiting that nothing in this file wrote.

use std::process::{Child, Command, Stdio};

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_core::Store;
use mixengine_platform::ipc::Connection;
use mixengine_proto::Timestamp;
use mixengine_proto::privileged::PrivilegedOp;
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

    /// Put one operation in the daemon's queue, from outside the daemon.
    async fn enqueue(&self, op: &PrivilegedOp, at: i64) {
        let store = Store::open(&self.home.database_file())
            .await
            .expect("the daemon's database is readable");

        mixengine_core::elevation::enqueue(&store, op, Timestamp(at))
            .await
            .expect("the row is written");

        store.close().await;
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
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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

/// A fresh home has nothing waiting, and says so in both places a client would look.
#[tokio::test]
async fn a_fresh_home_is_not_degraded() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let status = client.call("elevation.status", json!({})).await;
    assert_eq!(status["pending"], json!([]));

    let daemon = client.call("daemon.status", json!(null)).await;
    assert_eq!(daemon["elevation"]["pending"], 0);
}

/// The claim the table exists for: a row this daemon did not write is still this daemon's queue, and
/// what a client is told about it is the operation's own description.
#[tokio::test]
async fn what_is_waiting_is_reported_with_what_it_will_change() {
    let fixture = Fixture::start().await;
    fixture
        .enqueue(&PrivilegedOp::Probe {}, 1_760_000_000_000)
        .await;

    let mut client = fixture.client().await;
    let status = client.call("elevation.status", json!({})).await;

    assert_eq!(status["pending"].as_array().unwrap().len(), 1);
    assert_eq!(status["pending"][0]["op"]["op"], "probe");
    assert_eq!(
        status["pending"][0]["description"],
        PrivilegedOp::Probe {}.describe()
    );
    assert_eq!(status["pending"][0]["requested_at"], 1_760_000_000_000_i64);

    // D6: `daemon.status` carries the count, so `mix status` needs no second round trip.
    let daemon = client.call("daemon.status", json!(null)).await;
    assert_eq!(daemon["elevation"]["pending"], 1);
}

/// The other way out of a degraded mode. Without it a decline would be a trap.
#[tokio::test]
async fn dropping_empties_the_queue_and_clears_the_degraded_mode() {
    let fixture = Fixture::start().await;
    fixture.enqueue(&PrivilegedOp::Probe {}, 1).await;

    let mut client = fixture.client().await;
    let waiting = client.call("elevation.status", json!({})).await;
    let id = waiting["pending"][0]["id"].clone();

    let left = client.call("elevation.drop", json!({ "op": id })).await;
    assert_eq!(left["pending"], json!([]));

    let daemon = client.call("daemon.status", json!(null)).await;
    assert_eq!(daemon["elevation"]["pending"], 0);
}

/// D1's stated consequence, over the wire: an empty queue is not something to raise a prompt about.
/// This is the one call to `elevation.grant` in this suite, and it is the one that cannot reach a
/// prompt — there is nothing to ask for.
#[tokio::test]
async fn granting_an_empty_queue_is_refused_before_any_prompt() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let error = client.refuse("elevation.grant", json!(null)).await;

    assert_eq!(error["data"]["code"], "precondition_failed");
}

/// D9's whole fixture: the helper is found beside `current_exe()` and nowhere else, so moving the
/// binary is the entire setup. A workspace build never produces a daemon without a helper next to
/// it, which is why this test has to make one.
#[tokio::test]
async fn a_daemon_with_no_helper_beside_it_says_nothing_can_be_granted() {
    let elsewhere = tempfile::tempdir().expect("a directory with one binary in it");
    let moved = elsewhere
        .path()
        .join(format!("mixengined{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_mixengined"), &moved).expect("the daemon is copied alone");

    let home = Home::new();
    let mut child = Command::new(&moved)
        .arg("--home")
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the copied daemon runs");
    home.wait_until_listening().await;

    let store = Store::open(&home.database_file())
        .await
        .expect("its database");
    mixengine_core::elevation::enqueue(&store, &PrivilegedOp::Probe {}, Timestamp(1))
        .await
        .expect("the row is written");
    store.close().await;

    let mut client = Client::connect(&home).await;

    let status = client.call("elevation.status", json!({})).await;
    assert_eq!(status["can_prompt"], false);
    assert!(status.get("helper").is_none(), "{status}");
    assert!(
        status["reason"]
            .as_str()
            .unwrap()
            .contains("mixengine-elevate"),
        "{status}"
    );

    let error = client.refuse("elevation.grant", json!(null)).await;
    assert_eq!(error["data"]["code"], "dependency_missing");

    let _ = child.kill();
    let _ = child.wait();
}

/// **The test T64's notes said would replace the fixture.** A site is created over a real socket,
/// and an operation is found waiting that nothing in the test wrote.
#[tokio::test]
async fn creating_a_site_puts_a_hosts_change_in_the_queue() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    // Unique to this run: the daemon compares against the machine's real hosts file.
    let domain = format!("t41-{}.test", std::process::id());
    let root = fixture.home.path().join("project");
    std::fs::create_dir_all(&root).expect("a directory for the project");
    let path = root.display().to_string();

    client
        .call("project.create", json!({ "root": path, "name": "t41" }))
        .await;

    client
        .call(
            "site.create",
            json!({
                "project": { "path": path },
                "domains": [domain],
                "kind": { "kind": "static" }
            }),
        )
        .await;

    let status = client.call("elevation.status", json!({})).await;
    let pending = status["pending"].as_array().expect("a list");

    assert_eq!(pending.len(), 1, "{status}");
    assert_eq!(pending[0]["op"]["op"], "hosts-apply", "{status}");
    assert!(
        pending[0]["description"]
            .as_str()
            .is_some_and(|said| said.contains(&domain)),
        "the screen names the domain it will write: {status}"
    );
}
