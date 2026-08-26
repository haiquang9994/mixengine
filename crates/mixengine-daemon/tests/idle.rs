//! A service nothing is using is stopped, by a real daemon on its own clock — task **T69**.
//!
//! The arithmetic is settled against a mock beside `services::idle`, the reading against a fake
//! server in `mixengine-supervisor`, and the surface against a real socket in the CLI's own suite.
//! What is left is the wiring, and it is the half most easily wrong in a way nothing else notices:
//! that the sweeper runs at all without anybody asking, and that the stop it performs says **why**.
//!
//! **The reason is the assertion this suite exists for.** A sweeper wired to the registry but not to
//! `Registry::stopping_because` — or one that sets the reason *after* it cancels rather than before
//! — stops exactly the right service at exactly the right moment and tells the person who asks that
//! somebody requested it. Every other test in this task would still pass.
//!
//! # Why this suite takes a minute
//!
//! `services.idle_minutes` stores minutes, so the shortest policy anybody can express is one; and a
//! policy is spent in whole sweeps, so the fastest a service can reach its end is one sweep of sixty
//! seconds. That is the floor, and it is the price of the column's unit rather than something the
//! test could be written around. One test pays it, once.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_testkit::{Home, Service};
use serde_json::{Value, json};

/// The service this suite declares and lets go idle.
const SERVICE: &str = "fakeservice@main";

/// How long the whole thing is given before it is called a failure.
///
/// Twice the floor described in this module's own note, so a loaded runner that is merely slow does
/// not read as a sweeper that never ran.
const PATIENCE: Duration = Duration::from_secs(150);

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
        // Killed rather than asked, on `tests/renewal.rs`' reasoning: a test that failed halfway
        // must not leave a process holding the temporary home open.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One connection to the daemon, the shape `tests/renewal.rs` carries.
struct Client {
    sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
}

impl Client {
    async fn open(home: &Home) -> Self {
        let stream = Connection::connect(home.endpoint())
            .await
            .expect("the daemon is listening");

        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .expect("a handshake");

        tokio::spawn(async move {
            let _ = connection.await;
        });

        Self { sender }
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        })
        .to_string();

        let request = Request::builder()
            .method(Method::POST)
            .uri("/rpc")
            .header(HOST, "mixengine")
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .expect("a request");

        let response = self.sender.send_request(request).await.expect("an answer");
        assert_eq!(response.status(), StatusCode::OK, "for {method}");

        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();

        let answered: Value = serde_json::from_slice(&bytes).expect("JSON");
        assert!(answered.get("error").is_none(), "{method}: {answered}");

        answered["result"].clone()
    }
}

/// **A running daemon stops a service nothing is using, and says why.**
///
/// The two assertions are one sentence apart on purpose. That the service reached `stopped` would
/// pass for a sweeper that stopped it for any reason at all — including the wrong one — and the
/// reason on the transition is the only place the difference is visible.
#[tokio::test]
async fn a_running_daemon_stops_a_service_nothing_is_using_and_says_why() {
    // Sixty seconds, so a one-minute policy is one sweep. See this module's own note.
    let home = Home::configured("[services]\nidle_check_seconds = 60\n");
    let _daemon = Daemon::start(&home);
    home.wait_until_listening().await;

    // The `packages` row and the `services` row, through the shipped method — the same helper every
    // supervision suite declares with, rather than a second spelling of `service.create` here.
    mixengine_testkit::create(
        home.endpoint(),
        &home.database_file(),
        &[Service::new(SERVICE)],
    )
    .await;

    let mut client = Client::open(&home).await;

    client
        .rpc("service.start", json!({ "service": SERVICE, "wait": true }))
        .await;

    let running = client
        .rpc("service.status", json!({ "service": SERVICE }))
        .await;
    assert_eq!(running["state"], "running", "{running}");

    // **Set after the service is up**, so the first sweep to see it is also the first sweep that
    // has a policy to spend — a policy written before the start would be counted against a service
    // that was not running yet.
    let report = client
        .rpc(
            "service.set_idle",
            json!({ "service": SERVICE, "minutes": 1 }),
        )
        .await;
    assert_eq!(report["source"], "row", "{report}");
    assert_eq!(
        report["policy"]["after"], 60_000,
        "a one-minute policy: {report}"
    );

    // **Waited for by the log rather than by the event stream**, which was the first shape of this
    // test and is a longer way round: the daemon logs every transition with its reason, so the line
    // that says the service stopped and the line that says why are the same line. A subscriber would
    // add a second mechanism to get wrong between the sweeper and the assertion.
    wait_for(&home, "to=stopped reason=Idle").await;

    // A fresh connection: the daemon answers and closes, so the one that carried the calls above is
    // long gone by the time a minute has passed.
    let stopped = Client::open(&home)
        .await
        .rpc("service.status", json!({ "service": SERVICE }))
        .await;
    assert_eq!(stopped["state"], "stopped", "{stopped}");

    let log = home.daemon_log();

    assert!(
        log.contains("nothing was using this service, so it was stopped"),
        "the sweeper is what stopped it, rather than anything else in the daemon: {log}"
    );
    assert!(
        log.contains("Millis(60000)"),
        "the transition carries the policy it was stopped under: {log}"
    );
}

/// Poll the daemon log until it says `wanted`.
///
/// The testkit's own helper is bounded by its start-up patience, which is far below the minute this
/// test has to wait for. Everything else about it is the same.
async fn wait_for(home: &Home, wanted: &str) {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    while tokio::time::Instant::now() < deadline {
        if home.daemon_log().contains(wanted) {
            return;
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    panic!(
        "waited {PATIENCE:?} and the daemon never said {wanted:?}\n--- daemon.log ---\n{}",
        home.daemon_log()
    );
}
