//! A share ending on its own, against a real `mixengined` — roadmap task **T76**.
//!
//! What the unit tests beside `sites::revoke` prove is that a reading becomes the right decision.
//! What is proved here is the part only a daemon can be wrong about: that the loop runs on its own
//! timer at all, that it takes `site.unshare`'s road rather than a second copy of it, and that the
//! row it leaves behind is empty.
//!
//! **`check_seconds = 1`**, so a deadline of one second is over in a few periods rather than in
//! half a minute. That the key exists at all is why this suite can exist — the T76 design, D1.
//!
//! The `Daemon` and `Client` here are `tests/renewal.rs`' — which are `tests/sites.rs`' before that
//! — because a third spelling of a JSON-RPC client is a third thing to keep in step.

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
        // Killed rather than asked, on `tests/sites.rs`' reasoning: a test that failed halfway must
        // not leave a process holding the temporary home open.
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

    async fn call(&mut self, method: &str, params: Value) -> Value {
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

        let answer: Value = serde_json::from_slice(&bytes).expect("a JSON-RPC response");
        assert!(answer.get("error").is_none(), "{method}: {answer}");
        answer["result"].clone()
    }

    /// The same call, for a request that is expected to be refused.
    async fn refusal(&mut self, method: &str, params: Value) -> Value {
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

        self.sender.ready().await.expect("the connection is open");

        let response = self.sender.send_request(request).await.expect("an answer");

        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a whole body")
            .to_bytes();

        let answer: Value = serde_json::from_slice(&bytes).expect("a JSON-RPC response");

        answer["error"].clone()
    }
}

/// A directory to register a project against.
fn repository() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("mixengine-project")
        .tempdir()
        .expect("a temporary directory")
}

/// A project, and one static site under it.
async fn a_site(client: &mut Client, repository: &Path) {
    client
        .call(
            "project.create",
            json!({"root": repository.display().to_string(), "name": "blog"}),
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
}

/// An interface this machine can share on, by name.
///
/// **Named rather than left to the daemon's default, and read rather than guessed.** `site.share`
/// refuses to choose where more than one interface is up — the T74 design, D5 — and the machine
/// that wrote this suite has four: Wi-Fi, a Tailscale adapter and two Hyper-V switches. A CI runner
/// is the same kind of machine. So the name comes from the same enumeration the daemon will make,
/// in the same process's view of the same host, and the test is about revocation rather than about
/// how many adapters somebody happens to have.
fn a_shareable_interface() -> String {
    mixengine_platform::host()
        .network()
        .interfaces()
        .expect("this machine can be asked about its own interfaces")
        .into_iter()
        .find(|interface| !interface.loopback)
        .expect("this machine has a network to share on")
        .name
}

/// Whether this site is still shared, asked the way any client would ask.
async fn still_shared(client: &mut Client, domain: &str) -> bool {
    !client
        .call("site.show", json!({"site": {"domain": domain}}))
        .await["site"]["sharing"]
        .is_null()
}

/// Poll until the share is gone, or give up.
///
/// A deadline of its own rather than the suite's, so a loop that never runs — or a mutex that
/// deadlocked, which is the failure the T76 design names as the one to watch — fails in seconds
/// with a message instead of hanging until the harness gives up on the whole binary.
async fn unshared_within(client: &mut Client, domain: &str, seconds: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);

    while std::time::Instant::now() < deadline {
        if !still_shared(client, domain).await {
            return true;
        }

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    false
}

/// **A share with a deadline ends without anybody ending it** — the whole of T76 in one assertion.
///
/// An expiry is not debounced (the T76 design, D2), so one pass after the deadline is enough.
#[tokio::test]
async fn a_share_with_a_deadline_ends_by_itself() {
    let home = Home::configured("[sharing]\ncheck_seconds = 1\n");
    let _daemon = Daemon::start(&home);
    home.wait_until_listening().await;

    let repository = repository();
    let mut client = Client::connect(&home).await;
    a_site(&mut client, repository.path()).await;

    let shared = client
        .call(
            "site.share",
            json!({
                "site": {"domain": "blog.test"},
                "interface": a_shareable_interface(),
                "for_seconds": 1,
            }),
        )
        .await;

    assert!(
        shared["until"].is_number(),
        "the answer carries the deadline it was given: {shared}"
    );

    assert!(
        unshared_within(&mut client, "blog.test", 30).await,
        "the share should have ended on its own: {}",
        home.daemon_log()
    );

    // **The road it took, not merely the row it left behind.** T74's ordering lives in `unshare`,
    // and this line is what says the loop went through it rather than writing the row itself.
    //
    // **Waited for rather than read.** The row is cleared inside `unshare`, and the line is written
    // after it returns — so a client watching `site.show` sees the share end *before* the log says
    // why. Reading the file at that moment passes about half the time, which is the worst kind of
    // test there is; this one failed on its second run.
    home.wait_until_daemon_log_says("no longer shared on the local network")
        .await;

    let log = home.daemon_log();
    assert!(
        log.contains("the length it was shared for has run out"),
        "the reason is said, not only the fact: {log}"
    );
}

/// **A deadline that passed while the daemon was down is acted on at the next pass**, which is what
/// a laptop closed overnight produces — and what a loop watching only for *changes* would miss for
/// ever.
///
/// The daemon is killed rather than asked to stop, on `tests/lifecycle.rs`' reasoning: what is under
/// test is what the *next* start does.
#[tokio::test]
async fn a_deadline_that_passed_while_the_daemon_was_down_is_acted_on() {
    let home = Home::configured("[sharing]\ncheck_seconds = 1\n");
    let repository = repository();

    {
        let _daemon = Daemon::start(&home);
        home.wait_until_listening().await;

        let mut client = Client::connect(&home).await;
        a_site(&mut client, repository.path()).await;

        client
            .call(
                "site.share",
                json!({
                    "site": {"domain": "blog.test"},
                    "interface": a_shareable_interface(),
                    "for_seconds": 2,
                }),
            )
            .await;

        assert!(still_shared(&mut client, "blog.test").await);
    }

    home.wait_until_gone().await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let _second = Daemon::start(&home);
    home.wait_until_listening().await;
    let mut client = Client::connect(&home).await;

    assert!(
        unshared_within(&mut client, "blog.test", 30).await,
        "a daemon that starts after the deadline must end the share: {}",
        home.daemon_log()
    );
}

/// **A `--for` that has already run out is refused rather than honoured** — the T76 design, D6,
/// end to end.
///
/// The unit test beside `sites::sharing` proves the arithmetic; this proves that a client asking
/// for it gets the refusal rather than a URL, and that the site is left exactly as it was.
#[tokio::test]
async fn a_length_shorter_than_the_share_has_lasted_is_refused() {
    // A period long enough that nothing expires under this test: what is being asserted is the
    // refusal, and a loop ending the share mid-way would assert it by accident.
    let home = Home::configured("[sharing]\ncheck_seconds = 3600\n");
    let _daemon = Daemon::start(&home);
    home.wait_until_listening().await;

    let repository = repository();
    let mut client = Client::connect(&home).await;
    a_site(&mut client, repository.path()).await;

    client
        .call(
            "site.share",
            json!({
                "site": {"domain": "blog.test"},
                "interface": a_shareable_interface(),
            }),
        )
        .await;

    // Two seconds of sharing, then ask for one.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let refusal = client
        .refusal(
            "site.share",
            json!({
                "site": {"domain": "blog.test"},
                "interface": a_shareable_interface(),
                "for_seconds": 1,
            }),
        )
        .await;

    assert!(!refusal.is_null(), "a deadline in the past is refused");
    assert!(
        still_shared(&mut client, "blog.test").await,
        "a refused share leaves the site exactly as it was"
    );
}
