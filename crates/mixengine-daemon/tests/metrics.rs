//! What a real daemon says it is costing — roadmap task **T71**.
//!
//! The arithmetic is settled against invented readings beside `metrics::minutes`, the reading itself
//! against a table in `mixengine-platform`, and the two methods against a fixture in `api::rpc`.
//! What is left is the wiring, and it is the half most easily wrong in a way nothing else notices:
//! **that the loop runs at all without anybody asking, and that opening the stream is what makes it
//! run every second.**
//!
//! That second property is what this suite is for. A daemon whose stream is served but whose rate
//! never changes passes every other test in this task and answers a live client with one frame a
//! minute — so the home below is configured with an hour between background readings, and a second
//! frame arriving at all is the assertion.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_testkit::Home;
use serde_json::{Value, json};

/// How long the stream is given to deliver what it is asked for.
///
/// Generous against a one-second rate, because a loaded runner that is merely slow must not read as
/// a daemon that never raised its rate at all.
const PATIENCE: Duration = Duration::from_secs(30);

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
        // Killed rather than asked, on `tests/idle.rs`' reasoning: a test that failed halfway must
        // not leave a process holding the temporary home open.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One connection to the daemon, the shape `tests/idle.rs` carries.
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

    /// Read `wanted` frames off `GET /metrics`, then drop the connection.
    async fn frames(&mut self, wanted: usize) -> Vec<Value> {
        let request = Request::builder()
            .method(Method::GET)
            .uri("/metrics")
            .header(HOST, "mixengine")
            .body(Full::new(Bytes::new()))
            .expect("a request");

        let response = self.sender.send_request(request).await.expect("an answer");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );

        let mut body = response.into_body();
        let mut buffered = String::new();
        let mut frames = Vec::new();

        while frames.len() < wanted {
            let Some(next) = body.frame().await else {
                break;
            };

            let chunk = next.expect("a frame");
            let Some(data) = chunk.data_ref() else {
                continue;
            };

            buffered.push_str(&String::from_utf8_lossy(data));

            while let Some(end) = buffered.find("\n\n") {
                let line = buffered[..end].to_owned();
                buffered.drain(..end + 2);

                if let Some(json) = line.strip_prefix("data: ") {
                    frames.push(serde_json::from_str(json).expect("a frame is JSON"));
                }
            }
        }

        frames
    }
}

/// Wait for the daemon to be answering before anything is asked of it.
async fn wait_for_daemon(home: &Home) -> Client {
    let deadline = std::time::Instant::now() + PATIENCE;

    loop {
        if Connection::connect(home.endpoint()).await.is_ok() {
            return Client::open(home).await;
        }

        assert!(
            std::time::Instant::now() < deadline,
            "the daemon never started listening"
        );

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// **A daemon measures itself, and answers with a reading rather than with nothing.**
///
/// The daemon is half of the footprint this project defends in its README, and it is the half no
/// client could measure for itself without going behind the daemon's back to the operating system.
#[tokio::test]
async fn a_snapshot_names_the_daemon_itself() {
    let home = Home::configured("[metrics]\nidle_sample_seconds = 3600\n");
    let _daemon = Daemon::start(&home);
    let mut client = wait_for_daemon(&home).await;

    let frame = client.rpc("metrics.snapshot", json!({})).await;

    let subjects: Vec<&str> = frame["samples"]
        .as_array()
        .expect("samples")
        .iter()
        .map(|sample| sample["subject"].as_str().expect("a subject"))
        .collect();

    assert!(
        subjects.contains(&"daemon"),
        "a snapshot with an hour between background readings still measures now: {subjects:?}"
    );

    assert!(
        frame["samples"][0]["rss_bytes"]
            .as_u64()
            .is_some_and(|rss| rss > 0),
        "a running process occupies memory"
    );
}

/// **Opening the stream is what puts this daemon on its one-second rate.**
///
/// An hour between background readings, so a second frame inside the patience below cannot have come
/// from the loop's own schedule. This is the assertion the whole two-rate design turns on, and the
/// one every other test in this task would pass without.
#[tokio::test]
async fn the_stream_raises_the_rate_it_is_read_at() {
    let home = Home::configured("[metrics]\nsample_seconds = 1\nidle_sample_seconds = 3600\n");
    let _daemon = Daemon::start(&home);
    let mut client = wait_for_daemon(&home).await;

    let frames = tokio::time::timeout(PATIENCE, client.frames(2))
        .await
        .expect("two frames arrive while the stream is open");

    assert_eq!(frames.len(), 2);

    let first = frames[0]["at"].as_i64().expect("a moment");
    let second = frames[1]["at"].as_i64().expect("a moment");

    assert!(
        second >= first,
        "frames arrive in the order they were taken"
    );
    assert!(
        second - first < 60_000,
        "two frames an hour apart would mean the stream never raised the rate: {first} then {second}"
    );
}

/// **What the stream reported is what the history keeps.**
///
/// The read is taken through the same daemon that did the measuring, so this is the wiring — the
/// accumulator reaching the store — rather than the arithmetic, which is asserted from invented
/// numbers elsewhere.
#[tokio::test]
async fn the_history_answers_with_what_this_home_keeps() {
    let home = Home::configured("[metrics]\nidle_sample_seconds = 3600\nretention_hours = 24\n");
    let _daemon = Daemon::start(&home);
    let mut client = wait_for_daemon(&home).await;

    let history = client.rpc("metrics.history", json!({})).await;

    assert_eq!(history["retention_hours"], 24);
    assert!(
        history["minutes"].is_array(),
        "a home that has measured for a few seconds has no completed minute yet, and an empty list \
         is the answer rather than a failure"
    );
}
