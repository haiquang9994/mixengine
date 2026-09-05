//! The connection to the daemon: dial it, start one if there is none, ask it things.
//!
//! One connection per run of `mix`, and every call on it. Keep-alive is `hyper`'s and costs nothing
//! to use, where a connection per call would pay for a handshake and — on Windows — for a pipe
//! instance, twice, to answer one command.
//!
//! **The protocol is checked before anything is believed.** `daemon.version` is the first call, as
//! [`DaemonVersion`] says it should be: a daemon from another release may answer `daemon.status`
//! with a shape this build cannot decode, and finding that out by failing to parse the answer would
//! tell the user "the daemon said something unreadable" when the truth is "these two binaries are
//! from different releases". One extra round trip over a local socket is microseconds, and it is
//! the difference between a diagnosis and a shrug.

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1::SendRequest;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::{Connection, Endpoint};
use mixengine_proto::rpc::{self, Id, ResponseOutcome};
use mixengine_proto::{DaemonVersion, Error, ErrorCode, PROTOCOL_VERSION, flatten};
use serde_json::Value;

use crate::autostart::Autostart;
use crate::error::to_wire;

/// A daemon, connected and known to speak this protocol.
#[derive(Debug)]
pub(crate) struct Client {
    sender: SendRequest<Full<Bytes>>,

    /// The last id sent. Ids are per connection and start at one, so a response echoing the wrong
    /// one is a bug rather than a coincidence.
    calls: i64,

    /// What the handshake learned, kept so a command can report the daemon's build without asking
    /// twice.
    daemon: DaemonVersion,
}

impl Client {
    /// Dial the daemon for this home, starting one if `autostart` allows it.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::PreconditionFailed`] when nothing is listening and starting one was not
    /// permitted, and when the daemon that answered speaks another protocol; whatever
    /// [`Autostart::run`] says when starting one was permitted and failed; [`ErrorCode::Io`] for a
    /// transport that is there and will not talk.
    pub(crate) async fn connect(
        endpoint: &Endpoint,
        autostart: Option<&Autostart>,
    ) -> Result<Self, Error> {
        let connection = match Connection::connect(endpoint).await {
            Ok(connection) => connection,

            Err(error) if is_absent(&error) => {
                let Some(autostart) = autostart else {
                    return Err(nothing_listening(endpoint));
                };

                autostart.run()?;

                // Once, and with no backoff loop: `--detach` returns only after the endpoint
                // answered *it*, so a second refusal is a daemon that came up and went away again
                // rather than one that needs more time.
                Connection::connect(endpoint).await.map_err(|error| {
                    if is_absent(&error) {
                        started_and_gone(endpoint)
                    } else {
                        to_wire(&error)
                    }
                })?
            }

            Err(error) => return Err(to_wire(&error)),
        };

        let (mut sender, driver) = hyper::client::conn::http1::handshake(TokioIo::new(connection))
            .await
            .map_err(|error| transport(&error))?;

        // The driver owns the socket and has to be polled for any request to make progress. It ends
        // on its own when the connection closes, and the runtime is torn down when `mix` exits, so
        // nothing waits on the handle.
        tokio::spawn(async move {
            let _ = driver.await;
        });

        // The handshake happens before there is a `Client`, which is what makes "connected" and
        // "speaks our protocol" one state rather than two: nothing can hold one of these and still
        // have to remember to check.
        let mut calls = 0;
        let daemon = handshake(&mut sender, &mut calls).await?;

        Ok(Self {
            sender,
            calls,
            daemon,
        })
    }

    /// Wait until nothing answers on `endpoint`, or until the budget runs out.
    ///
    /// **The one thing `mix` waits for that is not an answer.** `mix uninstall` removes the home as
    /// its very last act *after* this process has gone, so the client cannot read back what is left
    /// until it has — and reading a moment early would report every path as left behind on a machine
    /// where nothing was (the T87 design, D9).
    ///
    /// Answers whether it is actually gone, so a caller can say which of the two happened rather
    /// than assuming.
    pub(crate) async fn gone(endpoint: &Endpoint, within: std::time::Duration) -> bool {
        /// Short enough that an ordinary shutdown is not waited out, long enough that a slow one is
        /// not polled a hundred times.
        const STEP: std::time::Duration = std::time::Duration::from_millis(100);

        let deadline = tokio::time::Instant::now() + within;

        loop {
            // Any failure to connect is what this is asking about: a daemon that is on its way down
            // stops answering before its process ends, and either way it is no longer holding the
            // home open.
            if Connection::connect(endpoint).await.is_err() {
                return true;
            }

            if tokio::time::Instant::now() + STEP >= deadline {
                return false;
            }

            tokio::time::sleep(STEP).await;
        }
    }

    /// What `daemon.version` said during the handshake.
    pub(crate) fn daemon(&self) -> &DaemonVersion {
        &self.daemon
    }

    /// Open one of the daemon's streams and read it as it arrives.
    ///
    /// **Not a call**, and deliberately on the same connection: `GET /logs/{id}?follow=1` answers
    /// for as long as the client keeps reading, so a body collected the way [`Client::call`]
    /// collects one would never return. What comes back is the response body, framed by
    /// [`Stream`].
    ///
    /// # Errors
    ///
    /// Whatever the daemon refused with — a status other than `200` is the envelope failing, and its
    /// body is the plain wire error, exactly as it is for a call. [`ErrorCode::Io`] for a connection
    /// that failed before the headers arrived.
    pub(crate) async fn stream(&mut self, path: &str) -> Result<Stream, Error> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(HOST, "mixengine")
            .body(Full::new(Bytes::new()))
            .expect("a request built from a checked path and an empty body is well formed");

        // **Waited for rather than assumed, and the difference is a real intermittent failure.**
        // `hyper`'s dispatcher allows exactly one request to be handed over before it has said it wants
        // one — see `can_send` in `hyper::client::dispatch` — so the *first* request on a connection
        // always goes through and every one after it goes through only once the connection task has
        // been polled again since the last response. A client that sent straight away raced the task
        // that drives the socket: on a loaded machine the request was refused before it was written,
        // with `canceled: connection was not ready`, which reads like a daemon that hung up.
        self.sender
            .ready()
            .await
            .map_err(|error| transport(&error))?;

        let response = self
            .sender
            .send_request(request)
            .await
            .map_err(|error| transport(&error))?;

        let status = response.status();

        if status != StatusCode::OK {
            // Bounded by the daemon's own error body, which is one small JSON object: this is the
            // failure path, and a stream that never opened has nothing else to send.
            let body = response
                .into_body()
                .collect()
                .await
                .map_err(|error| transport(&error))?
                .to_bytes();

            return Err(envelope(status, &body));
        }

        Ok(Stream {
            body: response.into_body(),
            buffer: Vec::new(),
        })
    }

    /// Call a method and hand back its result, undecoded.
    ///
    /// A [`Value`] rather than a generic return, because only the caller knows what a given method
    /// answers with — and because the one piece of decoding every command shares, the protocol
    /// check, has already happened.
    ///
    /// # Errors
    ///
    /// Whatever the daemon refused with, translated back into the error the rest of MixEngine
    /// speaks — the code that travelled in `error.data.code` is the one a caller branches on.
    /// [`ErrorCode::Io`] when the answer never arrived, [`ErrorCode::Internal`] when it arrived
    /// malformed.
    pub(crate) async fn call(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, Error> {
        call(&mut self.sender, &mut self.calls, method, params).await
    }
}

/// One of the daemon's streams, read a message at a time.
///
/// **The framing is Server-Sent Events, and this reads the little of it the daemon writes**: one
/// `data:` line per message, a blank line between messages, and comment lines beginning with `:`
/// that exist so an idle connection stays distinguishable from a dead one. A whole SSE parser would
/// be answering questions — event types, ids, retry hints — that
/// `.claude/architecture/daemon-and-ipc.md` settled by not using any of them.
#[derive(Debug)]
pub(crate) struct Stream {
    body: hyper::body::Incoming,

    /// What has arrived and is not yet a whole message. A chunk boundary falls wherever the socket
    /// put it, so a message is routinely split across two of them.
    buffer: Vec<u8>,
}

impl Stream {
    /// The next message, or [`None`] once the daemon has ended the stream.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Io`] for a connection that failed mid-stream, [`ErrorCode::Internal`] for a
    /// message this build cannot decode — which, after the protocol handshake, is a bug rather than
    /// a version difference.
    pub(crate) async fn next<T: serde::de::DeserializeOwned>(
        &mut self,
    ) -> Result<Option<T>, Error> {
        loop {
            if let Some(message) = self.take_message()? {
                return Ok(Some(message));
            }

            let Some(frame) = self.body.frame().await else {
                return Ok(None);
            };

            let frame = frame.map_err(|error| transport(&error))?;

            if let Some(data) = frame.data_ref() {
                self.buffer.extend_from_slice(data);
            }
        }
    }

    /// The first whole message in the buffer, if there is one yet.
    fn take_message<T: serde::de::DeserializeOwned>(&mut self) -> Result<Option<T>, Error> {
        while let Some(end) = find(&self.buffer, b"\n\n") {
            let block: Vec<u8> = self.buffer.drain(..end + 2).collect();

            let Some(data) = block
                .split(|&byte| byte == b'\n')
                .find_map(|line| line.strip_prefix(b"data: "))
            else {
                // A heartbeat, which is every fifteen seconds of a stream with nothing to say.
                continue;
            };

            return serde_json::from_slice(data).map(Some).map_err(|error| {
                Error::new(
                    ErrorCode::Internal,
                    format!("the daemon sent a message mix cannot read: {error}"),
                )
            });
        }

        Ok(None)
    }
}

/// Where `needle` begins in `haystack`.
fn find(haystack: &[u8], needle: &[u8; 2]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Ask which protocol a connected daemon speaks, and refuse to go on if it is not ours.
async fn handshake(
    sender: &mut SendRequest<Full<Bytes>>,
    calls: &mut i64,
) -> Result<DaemonVersion, Error> {
    let method = rpc::method::DAEMON_VERSION;
    let result = call(sender, calls, method, None).await?;

    let version: DaemonVersion = serde_json::from_value(result).map_err(|error| {
        Error::new(
            ErrorCode::Internal,
            format!("the daemon's answer to {method} is not a version: {error}"),
        )
    })?;

    if version.protocol != PROTOCOL_VERSION {
        return Err(Error::new(
            ErrorCode::PreconditionFailed,
            format!(
                "this daemon speaks protocol {} and `mix` speaks {PROTOCOL_VERSION} — mixengined \
                 {} and mix {} are not from the same release",
                version.protocol,
                version.version,
                env!("CARGO_PKG_VERSION")
            ),
        )
        .with_hint(
            "upgrade both halves of MixEngine, then stop the daemon so the new build replaces the \
             running one",
        ));
    }

    Ok(version)
}

/// One call on a connection, before or after there is a [`Client`] around it.
///
/// Free rather than a method because the handshake runs on a connection that is not yet a client.
async fn call(
    sender: &mut SendRequest<Full<Bytes>>,
    calls: &mut i64,
    method: &str,
    params: Option<Value>,
) -> Result<Value, Error> {
    *calls += 1;
    let id = Id::Number(*calls);

    let body = serde_json::to_vec(&rpc::Request::new(method, params, id.clone()))
        .expect("a JSON-RPC request built from proto types always serialises");

    let request = Request::builder()
        .method(Method::POST)
        .uri("/rpc")
        // There is no host to name — the endpoint is a socket, not an address — but HTTP/1.1 makes
        // the header mandatory, and a client that left it out would be relying on the server not to
        // care.
        .header(HOST, "mixengine")
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .expect("a request built from a constant path and a serialised body is well formed");

    // **Waited for rather than assumed, and the difference is a real intermittent failure.**
    // `hyper`'s dispatcher allows exactly one request to be handed over before it has said it wants
    // one — see `can_send` in `hyper::client::dispatch` — so the *first* request on a connection
    // always goes through and every one after it goes through only once the connection task has
    // been polled again since the last response. A client that sent straight away raced the task
    // that drives the socket: on a loaded machine the request was refused before it was written,
    // with `canceled: connection was not ready`, which reads like a daemon that hung up.
    sender.ready().await.map_err(|error| transport(&error))?;

    let response = sender
        .send_request(request)
        .await
        .map_err(|error| transport(&error))?;

    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|error| transport(&error))?
        .to_bytes();

    // The rule the daemon's HTTP layer is built on, read from this side: the status describes the
    // envelope and the JSON-RPC error describes the call, so anything but `200` means the request
    // never became a call and the body is the plain error shape rather than a response.
    if status != StatusCode::OK {
        return Err(envelope(status, &body));
    }

    let response: rpc::Response = serde_json::from_slice(&body).map_err(|error| {
        Error::new(
            ErrorCode::Internal,
            format!("the daemon's answer to {method} is not a JSON-RPC response: {error}"),
        )
    })?;

    // An answer to a call nobody made. Worth a sentence of its own rather than being decoded
    // anyway: on one connection with one call in flight there is no benign explanation, and
    // reporting the result as if it belonged to this method would hide it.
    if response.id.as_ref() != Some(&id) {
        return Err(Error::new(
            ErrorCode::Internal,
            format!(
                "the daemon answered {method} with id {} instead of {id}",
                response
                    .id
                    .map_or_else(|| "null".to_owned(), |id| id.to_string())
            ),
        ));
    }

    match response.outcome {
        ResponseOutcome::Success { result } => Ok(result),
        ResponseOutcome::Failure { error } => Err(error.into_error()),
    }
}

/// Whether this failure means "no daemon", as opposed to "a daemon that will not talk to you".
///
/// The two are told apart by the OS error and nowhere else. A Unix socket file whose listener is
/// gone answers `ECONNREFUSED`, and one that was cleaned up is simply not there; a Windows pipe
/// ceases to exist with its last handle, so its name reads as `ERROR_FILE_NOT_FOUND`. Everything
/// else — access denied, a pipe still busy past the platform layer's own retries — is a fact about
/// the machine that starting a second daemon would not change, and is reported rather than worked
/// around.
fn is_absent(error: &mixengine_platform::Error) -> bool {
    matches!(
        error,
        mixengine_platform::Error::Io { source, .. }
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            )
    )
}

/// Nothing is listening, and this run was told not to start anything.
fn nothing_listening(endpoint: &Endpoint) -> Error {
    Error::new(
        ErrorCode::PreconditionFailed,
        format!("no MixEngine daemon is listening on {endpoint}"),
    )
    .with_hint("run `mixengined`, or drop --no-autostart and `mix` will start one itself")
}

/// A daemon reported that it was up, and was gone by the time we dialled it.
fn started_and_gone(endpoint: &Endpoint) -> Error {
    Error::new(
        ErrorCode::Io,
        format!(
            "a daemon reported that it was listening on {endpoint}, and it was not there a moment \
             later"
        ),
    )
    .with_hint(
        "run `mixengined` in the foreground for this home — it exits with the reason, where a \
         detached one only writes it to logs/daemon.log",
    )
}

/// A response that never became a call: the daemon's own envelope statuses, and their error body.
fn envelope(status: StatusCode, body: &[u8]) -> Error {
    // `.claude/architecture/daemon-and-ipc.md`: the body of one of these is the plain wire error and
    // not a JSON-RPC response, because there is no `id` to answer and no method that ran.
    if let Ok(error) = serde_json::from_slice::<Error>(body) {
        return error;
    }

    Error::new(
        ErrorCode::Io,
        format!(
            "the daemon answered {status} to a JSON-RPC call, with a body no MixEngine daemon \
             writes: {}",
            String::from_utf8_lossy(body).trim()
        ),
    )
    .with_hint("something other than mixengined may be answering on this endpoint")
}

/// The connection itself failed, anywhere between the handshake and the last byte of a body.
fn transport(error: &hyper::Error) -> Error {
    Error::new(
        ErrorCode::Io,
        format!("the connection to the daemon failed: {}", flatten(error)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_nobody_is_listening_on_is_a_daemon_that_is_not_running() {
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::ConnectionRefused,
        ] {
            assert!(
                is_absent(&mixengine_platform::Error::Io {
                    action: "connect to",
                    path: std::path::PathBuf::from("run/mixengined.sock"),
                    source: std::io::Error::from(kind),
                }),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn an_endpoint_that_refuses_this_account_is_not_a_daemon_worth_starting() {
        // The distinction that matters: autostarting here would leave the user with a second daemon
        // and the same error, because the endpoint they cannot open belongs to somebody else.
        assert!(!is_absent(&mixengine_platform::Error::Io {
            action: "connect to",
            path: std::path::PathBuf::from("run/mixengined.sock"),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        }));

        assert!(!is_absent(&mixengine_platform::Error::Address {
            address: "run/mixengined.sock".to_owned(),
            reason: "the path is longer than sun_path allows".to_owned(),
        }));
    }

    #[test]
    fn a_daemons_own_error_body_is_reported_the_way_the_daemon_wrote_it() {
        let error = envelope(
            StatusCode::NOT_FOUND,
            br#"{"code":"not_found","message":"no such route: /rcp","hint":"POST to /rpc"}"#,
        );

        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.message, "no such route: /rcp");
        assert_eq!(error.hint.as_deref(), Some("POST to /rpc"));
    }

    #[test]
    fn something_else_answering_on_the_endpoint_is_not_reported_as_a_daemon_failure() {
        let error = envelope(StatusCode::BAD_GATEWAY, b"<html>nginx</html>");

        assert_eq!(error.code, ErrorCode::Io);
        assert!(error.message.contains("nginx"), "{}", error.message);
        assert!(error.hint.is_some());
    }
}
