//! `GET /logs/{id}` against a real `mixengined` supervising a real, talkative process.
//!
//! Roadmap task **T16b**, and the half its unit tests cannot reach. That a ring hands over a tail and
//! a subscription under one lock is provable in one process; that a person watching a service sees
//! what it printed before they connected *and* what it prints afterwards, over a socket, from a
//! daemon that is also supervising it, is not. The seam between the two is the whole feature — see
//! `.claude/decisions/0009-logs-travel-on-their-own-stream.md`.
//!
//! **The service is a `fakeservice` row**, rendered into a spec by the daemon's own generator (T30)
//! through a recipe compiled into debug builds only, so these are ignored in a release build for the
//! reason `crates/mixengine-cli/tests/service.rs` states: a release daemon has nothing that can run
//! one, and the tests would fail on a service that never starts rather than on what they assert.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_proto::LogFrame;
use mixengine_testkit::{Home, Service};

/// How long a stream is given to say something before the test gives up on it.
///
/// Every wait here is on a `fakeservice` printing every 50 ms, so this is a ceiling and not a pause:
/// the reads below return as soon as a line arrives. Generous because CI runners are shared.
const PATIENCE: Duration = Duration::from_secs(20);

/// A home with a daemon in it, and a row for a service that says it is ready and then keeps
/// talking.
///
/// Fifty milliseconds between lines: every wait in this file is on the next one arriving, so this
/// decides how long they take rather than whether they pass.
async fn running(id: &str) -> (Home, Daemon) {
    let home = Home::new();
    let daemon = Daemon::start(&home);
    home.wait_until_listening().await;

    // The migrations that create the schema run when the daemon opens the home, so there is nothing
    // to insert a row into until it is listening. The async half of `declare`, because `Home`'s own
    // is blocking and these tests are already inside a runtime.
    mixengine_testkit::create(
        home.endpoint(),
        &home.database_file(),
        &[Service::new(id).log_every(50)],
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

/// One connection to the daemon, for a request whose answer is read as it arrives.
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

    /// Call a JSON-RPC method and hand back its `result`.
    async fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1,
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/rpc")
            .header(HOST, "mixengine")
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(
                serde_json::to_vec(&body).expect("a request serialises"),
            )))
            .expect("a well formed request");

        // Waited for rather than assumed: `hyper`'s dispatcher allows one request through before
        // it has said it wants one, and every request after that only once the connection task has
        // been polled since the last response. This suite reuses a connection, so sending straight
        // away raced that task and failed with `canceled: connection was not ready` — rarely, and
        // more often on the machine with the least to spare, which is CI.
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
        let answer: serde_json::Value =
            serde_json::from_slice(&bytes).expect("a JSON-RPC response");

        assert!(answer.get("error").is_none(), "{method}: {answer}");

        answer["result"].clone()
    }

    /// Open `GET /logs/…` and read it as it arrives.
    async fn logs(&mut self, query: &str) -> Stream {
        let request = Request::builder()
            .method(Method::GET)
            .uri(query)
            .header(HOST, "mixengine")
            .body(Full::new(Bytes::new()))
            .expect("a well formed request");

        // Waited for rather than assumed: `hyper`'s dispatcher allows one request through before
        // it has said it wants one, and every request after that only once the connection task has
        // been polled since the last response. This suite reuses a connection, so sending straight
        // away raced that task and failed with `canceled: connection was not ready` — rarely, and
        // more often on the machine with the least to spare, which is CI.
        self.sender
            .ready()
            .await
            .expect("the connection is still open");

        let response = self.sender.send_request(request).await.expect("an answer");

        assert_eq!(response.status(), StatusCode::OK, "{query}");
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .expect("a content type"),
            "text/event-stream"
        );

        Stream {
            body: response.into_body(),
            buffer: Vec::new(),
        }
    }
}

/// The response body of `GET /logs/…`, framed the way the daemon frames it.
struct Stream {
    body: hyper::body::Incoming,
    buffer: Vec<u8>,
}

impl Stream {
    /// The next frame, or `None` once the daemon has ended the stream.
    async fn next(&mut self) -> Option<LogFrame> {
        loop {
            while let Some(end) = self.buffer.windows(2).position(|window| window == b"\n\n") {
                let block: Vec<u8> = self.buffer.drain(..end + 2).collect();

                if let Some(data) = block
                    .split(|&byte| byte == b'\n')
                    .find_map(|line| line.strip_prefix(b"data: "))
                {
                    return Some(
                        serde_json::from_slice(data).expect("a frame this build can read"),
                    );
                }
            }

            let frame = self.body.frame().await?.expect("a readable body");

            if let Some(data) = frame.data_ref() {
                self.buffer.extend_from_slice(data);
            }
        }
    }

    /// Every frame until one carries `wanted`, or a panic naming what did arrive.
    async fn until(&mut self, wanted: &str) -> Vec<LogFrame> {
        let mut seen = Vec::new();

        let found = tokio::time::timeout(PATIENCE, async {
            while let Some(frame) = self.next().await {
                let matched = text(&frame).is_some_and(|text| text.contains(wanted));

                seen.push(frame);

                if matched {
                    return true;
                }
            }

            false
        })
        .await;

        assert!(
            found.unwrap_or(false),
            "nothing on this stream carried {wanted:?} — what arrived was {:?}",
            seen.iter().filter_map(text).collect::<Vec<_>>()
        );

        seen
    }
}

/// The text of a frame, for the two variants that have one.
fn text(frame: &LogFrame) -> Option<&str> {
    match frame {
        LogFrame::Line(line) => Some(line.text.as_str()),
        LogFrame::Historic { text } => Some(text.as_str()),
        _ => None,
    }
}

/// The whole of T16b in one connection: what was printed before, and what is printed after, with
/// nothing lost between them and nothing shown twice.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn a_follow_hands_over_the_tail_and_then_carries_on_from_it() {
    let (home, _daemon) = running("fakeservice@main").await;

    let mut client = Client::connect(&home).await;

    let walk = client
        .call(
            "service.start",
            serde_json::json!({ "service": "fakeservice@main", "wait": true }),
        )
        .await;
    assert_eq!(walk["reached"][0], "fakeservice@main", "{walk}");

    // The service has been printing since before this connection existed, so the readiness line is
    // in the tail rather than on the live half — which is the thing a subscription alone could not
    // deliver.
    let mut stream = client
        .logs("/logs/fakeservice@main?tail=200&follow=1")
        .await;
    let tail = stream.until(mixengine_testkit::service::READY_LINE).await;

    assert!(
        tail.iter().all(|frame| matches!(frame, LogFrame::Line(_))),
        "a daemon that captured this service serves its own lines, not the file's"
    );

    // And the stream carries on into what the service prints from here. Line 25 of a service
    // printing every 50 ms cannot have been in the tail of a connection opened moments after it
    // started: whatever delivers it is the live half.
    let after = stream.until("fakeservice: line 25").await;

    // Nothing is delivered twice, which is the other half of the seam. The readiness line is
    // printed exactly once by the service, so seeing it twice on one connection would mean the tail
    // and the subscription overlapped.
    let announcements = tail
        .iter()
        .chain(after.iter())
        .filter_map(text)
        .filter(|line| line.contains(mixengine_testkit::service::READY_LINE))
        .count();

    assert_eq!(
        announcements, 1,
        "the service announced itself once; this connection showed it {announcements} times"
    );

    // A second connection, because the first one is holding a `follow` open: HTTP/1.1 carries one
    // request at a time, and this is exactly the shape a GUI has — a log panel streaming while the
    // user presses stop.
    Client::connect(&home)
        .await
        .call(
            "service.stop",
            serde_json::json!({ "service": "fakeservice@main", "wait": true }),
        )
        .await;
}

/// A snapshot ends. That is the difference between the two shapes of this endpoint, and a client
/// that could not tell them apart would hang on every `mix service logs` without `--follow`.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn a_tail_without_a_follow_is_a_body_that_finishes() {
    let (home, _daemon) = running("fakeservice@main").await;

    let mut client = Client::connect(&home).await;

    client
        .call(
            "service.start",
            serde_json::json!({ "service": "fakeservice@main", "wait": true }),
        )
        .await;

    let mut stream = client.logs("/logs/fakeservice@main?tail=200").await;
    let mut frames = Vec::new();

    let ended = tokio::time::timeout(PATIENCE, async {
        while let Some(frame) = stream.next().await {
            frames.push(frame);
        }
    })
    .await;

    assert!(ended.is_ok(), "a snapshot ends rather than staying open");
    assert!(
        frames
            .iter()
            .filter_map(text)
            .any(|line| line.contains(mixengine_testkit::service::READY_LINE)),
        "the snapshot carried what the service had printed"
    );

    client
        .call(
            "service.stop",
            serde_json::json!({ "service": "fakeservice@main", "wait": true }),
        )
        .await;
}

/// A service that this daemon never captured still has a log, and it says what it is.
///
/// The case is ordinary rather than exotic: a daemon restarted while a home's services stayed
/// stopped has an empty ring and a `current.log` full of the last run — which is exactly the run
/// somebody is asking about.
#[tokio::test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn output_from_before_this_daemon_comes_from_the_file_and_is_marked_as_such() {
    let (home, _daemon) = running("fakeservice@main").await;

    // Written where the daemon's own capture would have written it, for a service it has not run.
    let directory = home
        .path()
        .join("logs")
        .join("services")
        .join("fakeservice@main");
    std::fs::create_dir_all(&directory).expect("the service's log directory");
    std::fs::write(
        directory.join("current.log"),
        "an older run said this\nand then this\n",
    )
    .expect("the log file is written");

    let mut client = Client::connect(&home).await;
    let mut stream = client.logs("/logs/fakeservice@main?tail=200").await;
    let mut frames = Vec::new();

    tokio::time::timeout(PATIENCE, async {
        while let Some(frame) = stream.next().await {
            frames.push(frame);
        }
    })
    .await
    .expect("a snapshot ends");

    assert_eq!(
        frames.iter().filter_map(text).collect::<Vec<_>>(),
        ["an older run said this", "and then this"]
    );
    assert!(
        frames
            .iter()
            .all(|frame| matches!(frame, LogFrame::Historic { .. })),
        "a line read back from the file has no stream and no timestamp to claim: {frames:?}"
    );
}
