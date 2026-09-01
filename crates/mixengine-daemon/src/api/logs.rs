//! `GET /logs/{service_id}` — one service's output, and nobody else's.
//!
//! **The whole log surface, and deliberately not an event.** The event stream is a bounded broadcast
//! sized for state changes, shared by every client; a service in debug mode prints more in a second
//! than it holds, and putting output on it would cost every connected client exactly the transitions
//! it opened that stream for. See
//! `.claude/decisions/0009-logs-travel-on-their-own-stream.md`, which is also why there is no
//! `service.logs` method beside this: a JSON-RPC call cannot stream, and `?tail=N` with no `follow`
//! *is* the snapshot such a method would have been.
//!
//! **One connection, one service, and the back-pressure is per connection.** hyper polls this body
//! as fast as the client drains it, so a slow reader slows its own stream and nothing else — not
//! another client's log, and not anybody's state. A reader slow enough to fall behind the service
//! itself misses lines and is told how many, because a hole nobody mentions is worse than one that
//! is named.
//!
//! **The tail and the follow are one request.** A client that asked for the last lines and then
//! subscribed would lose whatever was printed in between, or see it twice, with no way to tell
//! which; [`ServiceLog::read`](crate::services::logs::ServiceLog::read) hands over both under one
//! lock so there is no in between. Roadmap task **T16b**.

use std::convert::Infallible;
use std::sync::Arc;

use http_body_util::{BodyExt as _, StreamBody};
use hyper::body::{Bytes, Frame as BodyFrame};
use hyper::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use hyper::{Response, StatusCode};
use mixengine_proto::{Error, ErrorCode, JobId, LogFrame, LogSubject, ServiceId};
use tokio::sync::broadcast;

use super::Api;
use super::events;
use super::http::{ResponseBody, full, problem};
use crate::error::ToWire as _;
use crate::services::logs;

/// How many lines are served when a client does not say.
///
/// The same order as the ring behind it, so the ordinary `mix service logs caddy` shows the recent
/// past rather than a screenful chosen by whoever typed the command. A client that wants the whole
/// ring asks for it.
const DEFAULT_TAIL: usize = 200;

/// What a client asked for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    /// Whose output — a service's, or a job's (roadmap task **T78a**, its design's D13).
    subject: LogSubject,

    /// How many of the lines already printed to begin with. Zero is a client that only wants what
    /// happens from now on.
    tail: usize,

    /// Whether the connection stays open afterwards.
    follow: bool,
}

impl Ask {
    /// Read one from the path and query of a request.
    ///
    /// **Two segments always** — roadmap task **T78a**, its design's D13: `/logs/service/<id>` and
    /// `/logs/job/<id>`, so nothing has to decide whether a first segment is a package name or the
    /// word `job`. The one-segment form T16b shipped is refused with the two that replaced it,
    /// because serving it as a guess would be serving somebody else's log.
    ///
    /// **The id is parsed with the daemon's own rule** — `ServiceId::parse`, or a job number — so a
    /// name this API would refuse anywhere else is refused here too, and with the same sentence. A
    /// query parameter that is not a number is an error rather than a default: a client that typed
    /// `tail=all` meant something, and quietly serving 200 lines would look like it worked.
    pub(crate) fn parse(path: &str, query: Option<&str>) -> Result<Self, Error> {
        let malformed = || {
            Error::new(
                ErrorCode::InvalidArgument,
                "this endpoint reads one subject's output and needs to be told which",
            )
            .with_hint("GET /logs/service/<service-id> or GET /logs/job/<job-id>")
        };

        let rest = path.strip_prefix("/logs/").ok_or_else(malformed)?;
        let (kind, id) = rest.split_once('/').ok_or_else(malformed)?;

        if id.is_empty() {
            return Err(malformed());
        }

        let subject = match kind {
            "service" => LogSubject::Service {
                id: ServiceId::parse(id).map_err(|error| {
                    Error::new(ErrorCode::InvalidArgument, error.to_string())
                        .with_hint("service ids are what `mix service list` prints")
                })?,
            },

            "job" => {
                let number: i64 = id.parse().map_err(|_| {
                    Error::new(
                        ErrorCode::InvalidArgument,
                        format!("{id} is not a job number"),
                    )
                    .with_hint("`mix job list` prints the ones this daemon knows")
                })?;

                LogSubject::Job { id: JobId(number) }
            }

            _ => return Err(malformed()),
        };

        let mut tail = DEFAULT_TAIL;
        let mut follow = false;

        for (key, value) in query
            .into_iter()
            .flat_map(|query| query.split('&'))
            .filter_map(|pair| match pair.split_once('=') {
                Some((key, value)) => Some((key, value)),
                None if pair.is_empty() => None,
                // A bare `?follow` is the shape a person types, and refusing it would be pedantry.
                None => Some((pair, "1")),
            })
        {
            match key {
                "tail" => {
                    tail = value.parse().map_err(|_| {
                        Error::new(
                            ErrorCode::InvalidArgument,
                            format!("tail={value} is not a number of lines"),
                        )
                    })?;
                }

                "follow" => follow = matches!(value, "1" | "true" | "yes"),

                // Ignored rather than refused: a client from a later release may send something
                // this build has no opinion about, and the useful behaviour is to serve the log.
                _ => {}
            }
        }

        Ok(Self {
            subject,
            tail,
            follow,
        })
    }
}

/// Answer one.
///
/// The order of the two questions matters. **Whether the subject exists is asked first**, so that a
/// mistyped id is a `404` naming it rather than an empty stream that looks like a quiet service —
/// which is the failure a person would sit and wait through. A job is asked of the `jobs` table for
/// the same reason (roadmap task **T78a**).
pub(crate) async fn respond(api: &Arc<Api>, ask: Ask) -> Response<ResponseBody> {
    match &ask.subject {
        LogSubject::Service { id } => match api.services().graph().await {
            Ok(graph) if graph.spec(id).is_none() => {
                return problem(
                    StatusCode::NOT_FOUND,
                    Error::new(
                        ErrorCode::NotFound,
                        format!("no service is declared as {id}"),
                    )
                    .with_hint("`mix service list` prints the ones that are"),
                );
            }

            Ok(_) => {}

            // The declarations could not be read at all — a spec source that failed, a set that is
            // not a graph. The same answer `service.list` gives, because it is the same failure.
            Err(error) => return problem(StatusCode::INTERNAL_SERVER_ERROR, error.to_wire()),
        },

        LogSubject::Job { id } => {
            if let Err(error) = api.jobs.status(*id).await {
                return problem(StatusCode::NOT_FOUND, error);
            }
        }
    }

    let log = api.services().logs().reading(&ask.subject);
    let (recent, subscription) = log.read(ask.tail);

    // **The file answers only where the daemon has nothing of its own**, and the two are never
    // stitched: a ring with anything in it belongs to a daemon that has been watching this service,
    // which is the better answer, and joining them would mean guessing where one ends in a file the
    // service is still appending to.
    //
    // **A job has no such file** (D13): what a daemon did not keep in memory is gone, because a
    // directory per job on a machine that never prunes the `jobs` table would be growth nothing
    // bounds. What survives a scaffold instead is the last of its output, quoted into the step.
    let recent = match (&ask.subject, recent.is_empty() && ask.tail > 0) {
        (LogSubject::Service { id }, true) => {
            let directory = api.paths().service_logs(id);

            // Reading the end of a file that may be ten megabytes is not work for a runtime thread
            // with connections to serve — `.claude/standards/rust.md` on anything that blocks.
            tokio::task::spawn_blocking(move || logs::historic(&directory, ask.tail))
                .await
                .unwrap_or_default()
        }

        _ => recent,
    };

    if ask.follow {
        following(api, recent, subscription)
    } else {
        snapshot(recent)
    }
}

/// Everything asked for, in one body that ends.
fn snapshot(recent: Vec<LogFrame>) -> Response<ResponseBody> {
    let mut body = Vec::new();

    for frame in recent {
        body.extend_from_slice(&encode(&frame));
    }

    stream_headers(Response::new(full(Bytes::from(body))))
}

/// Everything asked for, and then everything that happens.
fn following(
    api: &Arc<Api>,
    recent: Vec<LogFrame>,
    subscription: broadcast::Receiver<LogFrame>,
) -> Response<ResponseBody> {
    let shutdown = api.shutdown().token().clone();

    // An unfolding stream rather than a task writing into a queue, for the reason `GET /events` is
    // one: hyper polls it exactly as fast as the client reads, so a client that stops reading stops
    // *this* stream instead of filling a buffer behind it.
    //
    // The root token is the other way it ends. A follow on a quiet service is otherwise nothing but
    // a heartbeat, and a shutting-down daemon would wait out its whole grace period on a connection
    // neither end has anything more to say on.
    let frames = futures_util::stream::unfold(
        (recent.into_iter(), subscription, shutdown),
        |(mut recent, mut subscription, shutdown)| async move {
            let bytes = match recent.next() {
                // The tail, before anything is awaited: it was taken under the same lock as the
                // subscription, so nothing can arrive in between and these are still first.
                Some(frame) => encode(&frame),

                None => tokio::select! {
                    () = shutdown.cancelled() => None,
                    frame = next(&mut subscription) => frame,
                }?,
            };

            let data = BodyFrame::data(Bytes::from(bytes));

            Some((Ok::<_, Infallible>(data), (recent, subscription, shutdown)))
        },
    );

    stream_headers(Response::new(StreamBody::new(frames).boxed()))
}

/// The next thing to write, or `None` once this stream is over.
///
/// A reader that fell behind is told how many lines it lost and then carries on from what is still
/// buffered — `recv` resets to the oldest message still held, so nothing is skipped twice. The
/// stream ends only when the service's log itself is gone, which happens when the daemon is on its
/// way out.
async fn next(subscription: &mut broadcast::Receiver<LogFrame>) -> Option<Vec<u8>> {
    // Cancel safe, which is what lets the heartbeat below restart it: `recv` either takes a message
    // or leaves it for the next call.
    match tokio::time::timeout(events::HEARTBEAT, subscription.recv()).await {
        Ok(Ok(frame)) => Some(encode(&frame)),

        Ok(Err(broadcast::error::RecvError::Lagged(missed))) => {
            Some(encode(&LogFrame::Gap { missed }))
        }

        Ok(Err(broadcast::error::RecvError::Closed)) => None,

        // Nothing was printed for fifteen seconds, which on a healthy service is most of the time.
        // A comment frame, so that both ends learn about a broken connection while it is idle.
        Err(_elapsed) => Some(events::Frame::Heartbeat.encode()),
    }
}

/// One frame, framed the way SSE frames one.
///
/// The same shape as the event stream's, and for the same reason: the discriminator is inside the
/// JSON object, so a client has one handler rather than one subscription per variant, and a variant
/// added later arrives as an object it can ignore.
fn encode(frame: &LogFrame) -> Vec<u8> {
    match serde_json::to_vec(frame) {
        Ok(json) => {
            let mut framed = Vec::with_capacity(json.len() + 8);
            framed.extend_from_slice(b"data: ");
            framed.extend_from_slice(&json);
            framed.extend_from_slice(b"\n\n");
            framed
        }

        // Unreachable — a `LogFrame` is a string, a tag and a number — and written as a heartbeat
        // rather than as a broken frame, because a truncated `data:` line would desynchronise the
        // parser at the other end for the rest of the connection.
        Err(error) => {
            tracing::error!(%error, "a log line could not be encoded and was dropped");

            events::Frame::Heartbeat.encode()
        }
    }
}

/// What both shapes of this response carry.
fn stream_headers(mut response: Response<ResponseBody>) -> Response<ResponseBody> {
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    // Nothing between the two ends caches a local socket, but a client library may decide to on its
    // own, and a cached log is one that never says anything again.
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(path: &str, query: Option<&str>) -> Ask {
        Ask::parse(path, query).expect("a request this endpoint serves")
    }

    #[test]
    fn a_path_names_the_service_and_the_query_says_how_much_of_it() {
        let asked = ask("/logs/service/caddy", Some("tail=20&follow=1"));

        assert_eq!(
            asked.subject,
            LogSubject::Service {
                id: ServiceId::parse("caddy").expect("an id")
            }
        );
        assert_eq!(asked.tail, 20);
        assert!(asked.follow);
    }

    /// **A job is the second kind of subject** — roadmap task **T78a**, its design's D13.
    #[test]
    fn a_job_has_a_route_of_its_own() {
        let asked = ask("/logs/job/7", Some("follow=1"));

        assert_eq!(asked.subject, LogSubject::Job { id: JobId(7) });
        assert!(asked.follow);
    }

    /// The one-segment form T16b shipped is refused with the two that replaced it, rather than
    /// being read as a guess: serving a guess here would be serving somebody else's log.
    #[test]
    fn the_one_segment_form_is_refused_with_what_replaced_it() {
        let error = Ask::parse("/logs/caddy", None).expect_err("it is refused");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("/logs/service/")),
            "{error:?}"
        );
    }

    /// A job number that is not a number is an error rather than a stream that stays empty.
    #[test]
    fn a_job_that_is_not_a_number_is_refused_by_name() {
        let error = Ask::parse("/logs/job/latest", None).expect_err("it is refused");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_query_that_says_nothing_is_a_snapshot_of_the_recent_past() {
        let asked = ask("/logs/service/caddy", None);

        assert_eq!(asked.tail, DEFAULT_TAIL);
        assert!(!asked.follow, "a follow is asked for, never assumed");
    }

    /// The shape a person types, rather than the shape a client library generates.
    #[test]
    fn a_bare_follow_is_a_follow() {
        assert!(ask("/logs/service/caddy", Some("follow")).follow);
    }

    #[test]
    fn a_tail_that_is_not_a_number_is_refused_rather_than_defaulted() {
        let error =
            Ask::parse("/logs/service/caddy", Some("tail=all")).expect_err("not a line count");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("all"), "{}", error.message);
    }

    #[test]
    fn an_id_this_daemon_would_refuse_anywhere_else_is_refused_here() {
        for path in ["/logs/", "/logs/service/", "/logs/service/Not A Service"] {
            let error = Ask::parse(path, None).expect_err(path);

            assert_eq!(error.code, ErrorCode::InvalidArgument, "{path}");
            assert!(error.hint.is_some(), "{path}");
        }
    }

    /// A parameter from a later release must not stop this build serving the log.
    #[test]
    fn a_query_parameter_this_build_has_no_opinion_about_is_ignored() {
        let asked = ask("/logs/service/caddy", Some("since=yesterday&tail=5"));

        assert_eq!(asked.tail, 5);
    }

    #[test]
    fn a_frame_is_one_data_line_carrying_its_own_type() {
        assert_eq!(
            String::from_utf8(encode(&LogFrame::Gap { missed: 3 })).unwrap(),
            "data: {\"type\":\"gap\",\"missed\":3}\n\n"
        );
    }
}
