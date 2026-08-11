//! The API, against a real `mixengined` on a real endpoint.
//!
//! Not a component test with the router called directly: `mixengine-daemon` is a binary crate with
//! no library target, so an integration test cannot reach inside it — and reaching inside is what
//! would be worth avoiding anyway. What is proved here is the part the unit tests next to the code
//! cannot reach: that a daemon started the way a user starts one binds the endpoint its home
//! implies, speaks HTTP over a socket that is not a network socket, and answers each route the way
//! `.claude/architecture/daemon-and-ipc.md` says it does.
//!
//! Every test gets its own `MIXENGINE_HOME` in a `TempDir` **passed as `--home`** — rule 2 in
//! `.claude/standards/testing.md`: the environment is process-global, and two of these running at
//! once under `cargo test` would rewrite each other's home. Nothing here touches the network; a
//! Unix socket and a named pipe are neither.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{ALLOW, CONTENT_TYPE};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_core::Paths;
use mixengine_core::config::PathOverrides;
use mixengine_platform::ipc::{Connection, Endpoint};
use serde_json::Value;
use tempfile::TempDir;

/// How long a freshly spawned daemon is given to bind its endpoint.
///
/// Generous, because the first start of a daemon creates its home, runs the migrations and opens
/// SQLite — and because a loaded CI runner is the machine this has to be reliable on. It is a
/// ceiling and not a wait: the poll below returns the moment the endpoint answers.
const STARTUP: Duration = Duration::from_secs(30);

/// A `mixengined` running against a throwaway home, killed when the test ends.
struct Daemon {
    child: Child,
    home: TempDir,
    endpoint: Endpoint,
}

impl Daemon {
    /// Start one and wait until it answers.
    async fn start() -> Self {
        let home = tempfile::tempdir().expect("a temporary home");
        let paths = Paths::new(home.path().to_owned(), &PathOverrides::default());
        let endpoint = Endpoint::in_run_dir(paths.run()).expect("an endpoint for this home");

        let child = Command::new(env!("CARGO_BIN_EXE_mixengined"))
            .arg("--foreground")
            .arg("--home")
            .arg(home.path())
            // Silenced rather than inherited: a passing test should print nothing, and the daemon's
            // own `logs/daemon.log` inside the home is a better record than interleaved stderr —
            // `wait_until_listening` reads it when a start goes wrong.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the daemon binary runs");

        let daemon = Self {
            child,
            home,
            endpoint,
        };

        daemon.wait_until_listening().await;
        daemon
    }

    /// Poll the endpoint until something is behind it.
    ///
    /// Dialling rather than watching for the socket file: on Unix the file exists a moment before
    /// `accept` is running, and on Windows there is no file to watch at all.
    async fn wait_until_listening(&self) {
        let deadline = tokio::time::Instant::now() + STARTUP;

        while tokio::time::Instant::now() < deadline {
            if Connection::connect(&self.endpoint).await.is_ok() {
                return;
            }

            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        panic!(
            "the daemon did not start listening on {} within {STARTUP:?}\n--- daemon.log ---\n{}",
            self.endpoint,
            std::fs::read_to_string(self.home.path().join("logs").join("daemon.log"))
                .unwrap_or_else(|error| format!("(unreadable: {error})"))
        );
    }

    /// The home this daemon was started with.
    fn home(&self) -> &Path {
        self.home.path()
    }

    /// Send one request over its own connection.
    ///
    /// A connection per request rather than a pooled client. Keep-alive is worth having in `mix`
    /// and is `hyper`'s to provide; here it would only mean one test's connection state could
    /// affect the next assertion in the same test.
    async fn send(&self, request: Request<Full<Bytes>>) -> Response<hyper::body::Incoming> {
        let connection = Connection::connect(&self.endpoint)
            .await
            .expect("the daemon is listening");

        let (mut sender, driver) = hyper::client::conn::http1::handshake(TokioIo::new(connection))
            .await
            .expect("the daemon speaks HTTP/1.1");

        // The driver owns the socket and must be polled for the request to make progress. It ends
        // on its own when the response is done or the connection closes, so it is not awaited.
        tokio::spawn(driver);

        sender
            .send_request(request)
            .await
            .expect("the daemon answers")
    }

    /// A `GET`, answered.
    async fn get(&self, path: &str) -> Response<hyper::body::Incoming> {
        self.send(build(Method::GET, path, Bytes::new())).await
    }

    /// A `HEAD`, answered.
    async fn head(&self, path: &str) -> Response<hyper::body::Incoming> {
        self.send(build(Method::HEAD, path, Bytes::new())).await
    }

    /// A JSON-RPC call, answered and decoded. Panics if the daemon answered no body at all.
    async fn rpc(&self, body: &str) -> Value {
        let response = self
            .send(build(Method::POST, "/rpc", Bytes::from(body.to_owned())))
            .await;

        assert_eq!(response.status(), StatusCode::OK, "for {body}");
        json(response).await
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Killed rather than asked to stop: `daemon.shutdown` is T9's, and an interrupt cannot be
        // delivered to a child portably. The endpoint goes with the process on Windows and with the
        // `TempDir` on Unix, so nothing survives either way.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A request with the one header HTTP/1.1 makes mandatory.
///
/// There is no host to name — the endpoint is a socket, not an address — so the value is a constant
/// the daemon never reads. It is sent because a request without it is not a valid HTTP/1.1 request,
/// and a client that omitted it would be relying on the server not to care.
fn build(method: Method, path: &str, body: Bytes) -> Request<Full<Bytes>> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(hyper::header::HOST, "mixengine")
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(body))
        .expect("a well-formed request")
}

/// The body of a response, as JSON.
async fn json(response: Response<hyper::body::Incoming>) -> Value {
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the whole body arrives")
        .to_bytes();

    serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "the daemon answers JSON: {error}\n{}",
            String::from_utf8_lossy(&body)
        )
    })
}

#[tokio::test]
async fn health_is_answerable_and_says_which_protocol_this_daemon_speaks() {
    let daemon = Daemon::start().await;
    let response = daemon.get("/health").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).map(|v| v.as_bytes()),
        Some(&b"application/json"[..])
    );

    let health = json(response).await;
    assert_eq!(health["ok"], true);
    assert_eq!(health["protocol"], 1);
    assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn health_answers_a_head_with_the_headers_and_no_body() {
    let daemon = Daemon::start().await;
    let response = daemon.head("/health").await;

    // What a liveness probe reaches for, and what HTTP expects of anything that answers `GET`.
    // hyper writes the headers and drops the body, so this also pins that we are not relying on a
    // handler to remember it.
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).map(|v| v.as_bytes()),
        Some(&b"application/json"[..])
    );
    assert!(
        response
            .into_body()
            .collect()
            .await
            .expect("the empty body arrives")
            .to_bytes()
            .is_empty()
    );
}

#[tokio::test]
async fn a_request_that_spelled_its_id_null_is_answered_rather_than_ignored() {
    let daemon = Daemon::start().await;

    // A notification is a request with no `id` *member*. `"id":null` has one, the spec nowhere lets
    // it mean silence, and a client that sent it is waiting — so it is answered, to the id it gave.
    let answer = daemon
        .rpc(r#"{"jsonrpc":"2.0","method":"daemon.version","id":null}"#)
        .await;

    assert_eq!(answer["result"]["protocol"], 1);
    assert!(answer["id"].is_null(), "{answer}");
}

#[tokio::test]
async fn status_describes_the_home_the_daemon_was_actually_started_with() {
    let daemon = Daemon::start().await;
    let answer = daemon
        .rpc(r#"{"jsonrpc":"2.0","method":"daemon.status","id":1}"#)
        .await;

    let status = &answer["result"];
    assert_eq!(answer["id"], 1);
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["protocol"], 1);

    // The point of the field: somebody looking at a daemon they did not expect to be talking to.
    // Compared through `Path`, because the daemon prints the home it resolved and a temporary
    // directory on macOS arrives as `/var/…` where `/private/var/…` was asked for.
    let reported = Path::new(status["home"].as_str().expect("home is a string"));
    assert!(
        reported.ends_with(
            daemon
                .home()
                .file_name()
                .expect("the temporary home has a name")
        ),
        "{reported:?} is not the home the daemon was started with ({:?})",
        daemon.home()
    );

    assert_eq!(status["endpoint"], daemon.endpoint.to_string());
    assert!(
        status["pid"].as_u64().is_some_and(|pid| pid > 0),
        "{status}"
    );
    assert!(
        status["database"]
            .as_str()
            .is_some_and(|path| path.ends_with("mixengine.db")),
        "{status}"
    );
    assert!(status["started_at"].as_i64().is_some(), "{status}");
    assert!(status["uptime"].as_u64().is_some(), "{status}");
}

#[tokio::test]
async fn a_batch_comes_back_as_an_array_and_a_notification_is_left_out_of_it() {
    let daemon = Daemon::start().await;
    let answers = daemon
        .rpc(
            r#"[{"jsonrpc":"2.0","method":"daemon.version","id":1},
                {"jsonrpc":"2.0","method":"daemon.status"},
                {"jsonrpc":"2.0","method":"nope.nope","id":3}]"#,
        )
        .await;

    let answers = answers.as_array().expect("a batch is answered by an array");
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0]["result"]["protocol"], 1);
    assert_eq!(answers[1]["error"]["code"], -32601);
}

#[tokio::test]
async fn a_body_of_nothing_but_notifications_is_answered_with_no_content() {
    let daemon = Daemon::start().await;
    let response = daemon
        .send(build(
            Method::POST,
            "/rpc",
            Bytes::from_static(br#"{"jsonrpc":"2.0","method":"daemon.status"}"#),
        ))
        .await;

    // Not an empty `200`: a client that parses every response would be handed zero bytes where it
    // expects JSON.
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response
            .into_body()
            .collect()
            .await
            .expect("an empty body still arrives")
            .to_bytes()
            .is_empty()
    );
}

#[tokio::test]
async fn a_failing_method_is_still_an_http_200_because_the_request_did_arrive() {
    let daemon = Daemon::start().await;

    // The rule the whole HTTP layer is built on: the status describes the envelope, the JSON-RPC
    // error describes the call. A 4xx here would make `not_found` on a site indistinguishable from
    // `/rpc` having been mistyped.
    let answer = daemon
        .rpc(r#"{"jsonrpc":"2.0","method":"site.create","id":1}"#)
        .await;

    assert_eq!(answer["error"]["code"], -32601);
    assert_eq!(answer["error"]["data"]["code"], "not_found");
}

#[tokio::test]
async fn an_endpoint_that_does_not_exist_is_a_404_in_the_shape_every_client_renders() {
    let daemon = Daemon::start().await;
    let response = daemon.get("/logs/mariadb").await;

    // `/logs/{service_id}` is in the architecture and arrives in T14. Until then it is honestly not
    // here — and the body is the plain error shape rather than a JSON-RPC response, because there
    // is no call to answer.
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let error = json(response).await;
    assert_eq!(error["code"], "not_found");
    assert!(
        error["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("/rpc")),
        "{error}"
    );
}

#[tokio::test]
async fn the_wrong_verb_on_a_real_route_says_which_one_would_have_worked() {
    let daemon = Daemon::start().await;
    let response = daemon.get("/rpc").await;

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response.headers().get(ALLOW).map(|v| v.as_bytes()),
        Some(&b"POST"[..])
    );
}

#[tokio::test]
async fn a_body_larger_than_the_limit_is_refused_rather_than_read() {
    let daemon = Daemon::start().await;

    // Valid JSON, and two megabytes of it: the point is that the daemon stops reading rather than
    // that the content is bad.
    let oversized = format!(
        r#"{{"jsonrpc":"2.0","method":"{}","id":1}}"#,
        "x".repeat(2 * 1024 * 1024)
    );
    let response = daemon
        .send(build(Method::POST, "/rpc", Bytes::from(oversized)))
        .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json(response).await["code"], "invalid_argument");
}

#[tokio::test]
async fn the_event_stream_opens_and_is_held_open_rather_than_closed_at_once() {
    let daemon = Daemon::start().await;
    let response = daemon.get("/events").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).map(|v| v.as_bytes()),
        Some(&b"text/event-stream"[..])
    );

    // Nothing in this build publishes an event yet — the first producer arrives with service state
    // in Phase 1 — so what is provable here is the property that would otherwise be missed: an idle
    // stream stays open instead of ending, which is what makes it a stream rather than an empty
    // response. The frames themselves are pinned by the unit tests in `api::events`.
    let mut body = response.into_body();
    let idle = tokio::time::timeout(Duration::from_millis(500), body.frame()).await;

    assert!(
        idle.is_err(),
        "an idle event stream should stay open, not end: {idle:?}"
    );
}
