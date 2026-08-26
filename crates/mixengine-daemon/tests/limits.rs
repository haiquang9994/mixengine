//! `service.limits` and `service.set_limits` against a real `mixengined` over a real socket.
//!
//! Roadmap task **T68**, and the half no unit test reaches. That a job object or a cgroup accepts a
//! ceiling is provable in one process — `mixengine-platform`'s own tests do it. What is only
//! provable here is the seam: that a limit written by a *method* arrives at a process that is
//! **already running**, which is D7's whole promise, and that what comes back describes what this
//! machine will do with the number rather than the number alone.
//!
//! **The service is a `fakeservice` row**, rendered by the daemon's own generator through a recipe
//! compiled into debug builds only — so these are ignored in a release build for the reason
//! `tests/logs.rs` states: a release daemon has nothing that can run one, and the tests would fail
//! on a service that never starts rather than on what they assert.

use std::process::{Child, Command, Stdio};

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_proto::rpc::method;
use mixengine_testkit::{Home, Service};
use serde_json::{Value, json};

/// The service every test here caps.
const SERVICE: &str = "fakeservice@main";

/// Reading a limit and reading what the machine does with it is one call.
///
/// **Because neither is worth having alone.** A `memory_mb` of 512 means one thing where it is a
/// commit charge enforced by a failed allocation, another where it is charged pages enforced by the
/// OOM killer, and a third where it is stored and enforced by nothing at all. The T68 design, D2.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn limits_come_back_with_what_this_machine_will_enforce() {
    let (home, _daemon) = declared().await;
    let mut client = Client::connect(&home).await;

    let report = client
        .call(method::SERVICE_LIMITS, json!({ "service": SERVICE }))
        .await;

    assert_eq!(report["service"], SERVICE, "{report}");
    assert_eq!(report["limits"]["cpu_percent"], Value::Null, "{report}");
    assert_eq!(report["limits"]["memory_mb"], Value::Null, "{report}");
    assert_eq!(report["limits"]["priority"], "normal", "{report}");

    // What this machine says is this machine's business — a runner without cgroup delegation is a
    // legitimate answer. What must be there is the vocabulary a client renders with.
    assert!(
        report["support"]["cores"].as_u64().unwrap_or(0) >= 1,
        "{report}"
    );
    assert!(report["support"]["memory"]["kind"].is_string(), "{report}");
    assert!(report["support"]["memory_measure"].is_string(), "{report}");
}

/// D8: the whole value, never a delta.
///
/// Setting `cpu_percent` alone clears a memory ceiling that was there, and that is **specified**
/// rather than accidental: the alternative is a three-way patch value in every reader of limits, or
/// a read-modify-write in a client, which is business logic a client may not hold and which T46
/// refused by name when it added `domain.add` and `domain.remove`.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn setting_limits_replaces_every_field_rather_than_merging() {
    let (home, _daemon) = declared().await;
    let mut client = Client::connect(&home).await;

    let first = client
        .call(
            method::SERVICE_SET_LIMITS,
            json!({ "service": SERVICE, "limits": { "memory_mb": 512 } }),
        )
        .await;
    assert_eq!(first["limits"]["memory_mb"], 512, "{first}");

    let second = client
        .call(
            method::SERVICE_SET_LIMITS,
            json!({ "service": SERVICE, "limits": { "cpu_percent": 50 } }),
        )
        .await;

    assert_eq!(second["limits"]["cpu_percent"], 50, "{second}");
    assert_eq!(
        second["limits"]["memory_mb"],
        Value::Null,
        "the second call named every field, and the one it did not name is now unset: {second}",
    );
}

/// The write survives the call, which is what makes it a *setting* rather than an answer.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn what_was_set_is_what_is_read_back() {
    let (home, _daemon) = declared().await;
    let mut client = Client::connect(&home).await;

    client
        .call(
            method::SERVICE_SET_LIMITS,
            json!({
                "service": SERVICE,
                "limits": { "cpu_percent": 25, "memory_mb": 256, "priority": "background" },
            }),
        )
        .await;

    let read = client
        .call(method::SERVICE_LIMITS, json!({ "service": SERVICE }))
        .await;

    assert_eq!(read["limits"]["cpu_percent"], 25, "{read}");
    assert_eq!(read["limits"]["memory_mb"], 256, "{read}");
    assert_eq!(read["limits"]["priority"], "background", "{read}");
}

/// D7: a running service is re-capped without being restarted.
///
/// **The pid is what proves it.** A daemon that restarted the service to apply a limit would answer
/// correctly and still be wrong — the promise is that there is no moment in which the service is
/// down because somebody changed a number.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn a_limit_reaches_a_service_that_is_already_running() {
    let (home, _daemon) = declared().await;
    let mut client = Client::connect(&home).await;

    client
        .call(
            method::SERVICE_START,
            json!({ "service": SERVICE, "wait": true }),
        )
        .await;

    let before = client
        .call(method::SERVICE_STATUS, json!({ "service": SERVICE }))
        .await;
    assert_eq!(before["state"], "running", "{before}");

    client
        .call(
            method::SERVICE_SET_LIMITS,
            json!({ "service": SERVICE, "limits": { "memory_mb": 512 } }),
        )
        .await;

    let after = client
        .call(method::SERVICE_STATUS, json!({ "service": SERVICE }))
        .await;

    assert_eq!(after["state"], "running", "{after}");
    assert_eq!(
        after["pid"], before["pid"],
        "same process, new ceiling: {before} then {after}",
    );
}

/// D10: the cores rule is the daemon's, because the number it is measured against is the machine's.
///
/// `mixengine-proto` has no host and must not grow one, so this is the one refusal in T68 that
/// cannot live beside the type it is about.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn a_cpu_percent_above_the_whole_machine_is_refused() {
    let (home, _daemon) = declared().await;
    let mut client = Client::connect(&home).await;

    // 255% of one core needs three cores. A machine with three or more would accept it, so the
    // ceiling this asserts against is read from the same place the daemon reads it.
    let support = client
        .call(method::SERVICE_LIMITS, json!({ "service": SERVICE }))
        .await;
    let cores = support["support"]["cores"].as_u64().unwrap_or(1);

    let asked = json!({ "service": SERVICE, "limits": { "cpu_percent": 255 } });

    // **Both sides are asserted, because which one this machine takes is not this test's to choose.**
    // `cpu_percent` is a `u8`, so the largest value anybody can express is 255 — which is below the
    // ceiling on any machine with three cores or more. The refusal is therefore a guard for small
    // machines (a one-core VM, a constrained container) and is unreachable on a laptop, and a test
    // that only checked the refusal would silently assert nothing on almost every runner.
    if cores >= 3 {
        let report = client.call(method::SERVICE_SET_LIMITS, asked).await;

        assert_eq!(
            report["limits"]["cpu_percent"],
            255,
            "{cores} cores is {} percent of one core, so 255 is under the ceiling: {report}",
            cores * 100,
        );

        return;
    }

    let error = client.refuse(method::SERVICE_SET_LIMITS, asked).await;

    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("core"),
        "the refusal names what it was measured against: {error}",
    );
}

/// D10: zero is refused, because it is wrong on every machine there will ever be.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn a_zero_memory_limit_is_refused() {
    let (home, _daemon) = declared().await;
    let mut client = Client::connect(&home).await;

    let error = client
        .refuse(
            method::SERVICE_SET_LIMITS,
            json!({ "service": SERVICE, "limits": { "memory_mb": 0 } }),
        )
        .await;

    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("allocate nothing"),
        "{error}",
    );
}

/// D10: a limit this system cannot enforce is **stored**, not refused.
///
/// This is macOS's case, and it runs on all three systems because what it asserts is the rule rather
/// than the platform: refusing here would mean a blueprint written for three systems fails to apply
/// on one, which is a worse product than a stored limit and a report that says it does nothing.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn a_limit_is_stored_even_where_this_machine_enforces_nothing() {
    let (home, _daemon) = declared().await;
    let mut client = Client::connect(&home).await;

    let report = client
        .call(
            method::SERVICE_SET_LIMITS,
            json!({ "service": SERVICE, "limits": { "memory_mb": 512 } }),
        )
        .await;

    assert_eq!(report["limits"]["memory_mb"], 512, "stored: {report}");

    // And whatever this machine does about it is said in the same breath, every time.
    assert!(report["support"]["memory"]["kind"].is_string(), "{report}");
}

/// A limit for a service nothing declares is refused before anything is written.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn a_limit_for_a_service_that_does_not_exist_is_refused() {
    let (home, _daemon) = declared().await;
    let mut client = Client::connect(&home).await;

    let error = client
        .refuse(
            method::SERVICE_SET_LIMITS,
            json!({ "service": "nothing@here", "limits": { "memory_mb": 512 } }),
        )
        .await;

    assert!(error["message"].is_string(), "{error}");
}

/// A home with a daemon in it, and one declared `fakeservice`.
async fn declared() -> (Home, Daemon) {
    let home = Home::new();
    let daemon = Daemon::start(&home);
    home.wait_until_listening().await;

    // The migrations run when the daemon opens the home, so there is nothing to write a row into
    // until it is listening — `logs.rs`'s reasoning, and its helper.
    mixengine_testkit::create(
        home.endpoint(),
        &home.database_file(),
        &[Service::new(SERVICE).log_every(50)],
    )
    .await;

    (home, daemon)
}

/// The daemon process, killed when the test ends however it ends.
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

        // Waited for rather than assumed: `hyper`'s dispatcher allows one request through before it
        // has said it wants one, and every request after that only once the connection task has been
        // polled since the last response. This suite reuses a connection — see `logs.rs`, where the
        // race this avoids was found.
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
