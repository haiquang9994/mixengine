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

/// Drop everything a daemon's own start-up producers put in the queue.
///
/// **This did not have to exist until T49a**, and what it is working around is the feature rather
/// than a test defect. A started daemon asks for what first-run setup needs — the hosts block, the
/// resolver, the port grant, and now the trust-store install — so "the queue is empty" stopped being
/// something a fresh home says on a machine that has a trust store, which is every machine in CI.
///
/// The two tests below need an *actually* empty queue to mean what they say, so they empty it rather
/// than filtering: a grant refused because the queue is empty is a different claim from a grant
/// refused because the queue holds nothing of a particular kind.
async fn empty_the_queue(client: &mut Client) {
    loop {
        let status = client.call("elevation.status", json!({})).await;
        let pending = status["pending"].as_array().expect("a list").clone();

        let Some(first) = pending.first() else {
            return;
        };

        client
            .call("elevation.drop", json!({ "op": first["id"].clone() }))
            .await;
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

/// The two places a client would look agree about what is waiting.
///
/// **This used to assert a fresh home had nothing waiting, and T49a made that false — correctly.**
/// `.claude/architecture/security-model.md` promises one elevation prompt at first run, and the
/// producers that fill it run at start; a fresh home on a machine with a trust store therefore has
/// exactly that install waiting, and by this project's own definition — "not zero means degraded" —
/// is degraded until somebody grants it. `crates/mixengine-daemon/src/elevation.rs` says so in its
/// header: a fresh install nobody ever grants stays degraded, and that is the correct behaviour.
///
/// What is still worth asserting, and what this now asserts, is that the count and the list are the
/// same answer. A client renders one from the other.
#[tokio::test]
async fn both_views_of_the_queue_agree_on_a_fresh_home() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let status = client.call("elevation.status", json!({})).await;
    let waiting = status["pending"].as_array().expect("a list").len();

    let daemon = client.call("daemon.status", json!(null)).await;

    assert_eq!(daemon["elevation"]["pending"], waiting, "{status}");

    // And nothing a start-up producer enqueued is a duplicate of another: the queue deduplicates on
    // `dedupe_key`, and two rows for one question would each be rendered on the one screen whose job
    // is to say what is about to happen.
    let mut kinds: Vec<&str> = status["pending"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|row| row["op"]["op"].as_str())
        .collect();
    let before = kinds.len();
    kinds.sort_unstable();
    kinds.dedup();

    assert_eq!(
        kinds.len(),
        before,
        "one question is queued twice: {status}"
    );
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

    // Found by kind rather than by index: a started daemon's own producers are in this queue too,
    // and which position they take is not what this test is about.
    let row = status["pending"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|row| row["op"]["op"] == "probe")
        .unwrap_or_else(|| panic!("the row this test enqueued is not in the queue: {status}"))
        .clone();

    assert_eq!(row["description"], PrivilegedOp::Probe {}.describe());
    assert_eq!(row["requested_at"], 1_760_000_000_000_i64);

    // D6: `daemon.status` carries the count, so `mix status` needs no second round trip.
    let daemon = client.call("daemon.status", json!(null)).await;
    assert_eq!(
        daemon["elevation"]["pending"],
        status["pending"].as_array().expect("a list").len()
    );
}

/// The other way out of a degraded mode. Without it a decline would be a trap.
#[tokio::test]
async fn dropping_empties_the_queue_and_clears_the_degraded_mode() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    // Whatever this daemon's start asked for goes first, so that "empties the queue" below is a
    // claim about the drop and not about what happened to be left.
    empty_the_queue(&mut client).await;
    fixture.enqueue(&PrivilegedOp::Probe {}, 1).await;

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

    // A started daemon's own producers fill the queue, and this test is about an empty one — so it
    // empties it. Without this the call would reach a prompt, which is the one thing this suite
    // must never do.
    empty_the_queue(&mut client).await;

    let error = client.refuse("elevation.grant", json!(null)).await;

    assert_eq!(error["data"]["code"], "precondition_failed");
}

/// D9's whole fixture: the helper is found beside `current_exe()`, so moving the binary is the
/// entire setup. A workspace build never produces a daemon without a helper next to it, which is
/// why this test has to make one.
///
/// **T85 gave the question a second half, and this test now covers both.** A daemon with nothing
/// beside it looks where this operating system installs one, so what it answers depends on the
/// machine: on a fresh one — a developer's, and CI's `test` job — there is nothing installed and the
/// old assertion holds unchanged. On a machine where somebody has run the elevated `system` suite,
/// there *is* one, and the honest thing to assert is that the daemon found that copy. Both branches
/// assert something; neither is a skip, which is the shape `mixengine-elevate`'s `audit.rs` already
/// uses for a premise the machine may refuse to hold.
///
/// **The second branch asks for no grant.** On a machine that has a helper the daemon can run, a
/// grant is a real elevation prompt, and this suite must never raise one.
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

    match mixengine_platform::install::helper_path()
        .ok()
        .filter(|path| path.is_file())
    {
        // Nothing installed: the copy beside the program is the only candidate, and there is none.
        None => {
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
        }

        // This machine has one. Either the daemon will run it — and says so — or it refuses it as
        // not an administrator's and says *that*, naming the same path. Both are T85's D5 working;
        // what would be wrong is a status that never mentions the file at all.
        Some(installed) => {
            let named = installed.display().to_string();
            let mentioned = status["helper"].as_str() == Some(named.as_str())
                || status["reason"]
                    .as_str()
                    .is_some_and(|why| why.contains(&named));

            assert!(mentioned, "the installed helper is at {named}: {status}");
        }
    }

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

    let row = pending
        .iter()
        .find(|row| row["op"]["op"] == "hosts-apply")
        .unwrap_or_else(|| panic!("creating a site queued no hosts change: {status}"));

    assert!(
        row["description"]
            .as_str()
            .is_some_and(|said| said.contains(&domain)),
        "the screen names the domain it will write: {status}"
    );
}

/// A started daemon has already asked this machine to trust its authority — roadmap task **T49a**.
///
/// **The ordering claim, proved where it is made.** A unit test calling `require_trust_store` proves
/// the function works; only a daemon started over an empty home proves that something calls it *at
/// start*, which is what puts the install in first-run setup's single grant rather than behind a
/// second prompt when somebody creates the first HTTPS site.
///
/// Nothing has created a site here, so the queue holds what the start put there and nothing else —
/// which is also how this asserts the row is in the **same batch** as the resolver's rather than in
/// one of its own.
#[tokio::test]
async fn a_started_daemon_has_already_asked_to_be_trusted() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;

    let status = client.call("elevation.status", json!({})).await;
    let pending = status["pending"].as_array().expect("a list");

    let install = pending
        .iter()
        .find(|row| row["op"]["op"] == "trust-ca-install");

    // A machine with no trust store MixEngine knows how to write asks for nothing, and that is a
    // supported machine rather than a failure — the T49a design, D7. On the three CI runners there
    // is always one, so this is the case that actually gets exercised.
    let Some(install) = install else {
        let method = mixengine_platform::host()
            .trust_store()
            .method()
            .expect("this machine can say what store it has");

        assert_eq!(
            method,
            mixengine_platform::TrustStoreMethod::None,
            "this machine has a trust store and the start did not ask it to trust anything: {status}"
        );
        return;
    };

    assert!(
        install["description"]
            .as_str()
            .is_some_and(|said| said.contains("certificate authority")),
        "the screen does not say what it is about to trust: {status}"
    );

    // D3: what travels is the certificate, and there is nowhere in it for a key to travel.
    let encoded = install.to_string();
    assert!(!encoded.contains("PRIVATE"), "{encoded}");
    assert!(!encoded.contains("key_pem"), "{encoded}");
}
