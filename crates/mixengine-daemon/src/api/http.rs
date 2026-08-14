//! The HTTP/1.1 half: routing, limits, and the connection each client gets.
//!
//! HTTP is what `.claude/architecture/daemon-and-ipc.md` chose over a bespoke frame format, and the
//! reason is visible here — streaming, back-pressure and body limits are all `hyper`'s, and the CLI,
//! the GUI and any future extension get a client library for free instead of a hand-written framer
//! each.
//!
//! **The HTTP status is about the envelope, never about the call.** A JSON-RPC method that fails
//! comes back `200` with an `error` member, because the request *was* delivered, parsed and
//! answered. Mixing the two would make a client branch in two places on one outcome, and would make
//! `not_found` on a site indistinguishable from `/rpc` having been typed as `/rcp`. The statuses
//! this module does use — `204`, `400`, `404`, `405`, `413` — are all about the envelope.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{
    BodyExt as _, Full, LengthLimitError, Limited, StreamBody, combinators::BoxBody,
};
use hyper::body::{Bytes, Frame as BodyFrame, Incoming};
use hyper::header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use mixengine_platform::ipc;
use mixengine_proto::{Error, ErrorCode};
use tracing::Instrument as _;

use super::{Api, rpc};

/// The largest `POST /rpc` body the daemon will read.
///
/// Every request in this API is a method name and a handful of scalars; the biggest thing a client
/// ever sends is a blueprint (roadmap task T78), measured in kilobytes. The limit is here so that a
/// client sending an endless body meets a `413` instead of the daemon's memory meeting the
/// machine's.
const MAX_BODY: usize = 1024 * 1024;

/// How long a client may take to finish sending its headers.
///
/// `.claude/standards/rust.md` calls a missing timeout on anything touching a socket a review
/// blocker, and this is the one that matters here: a connection that opens and then says nothing
/// would otherwise hold a task for as long as the daemon runs. The body is bounded by [`MAX_BODY`]
/// rather than by a clock, because a slow client is not a fault and a large one is already refused.
const HEADER_TIMEOUT: Duration = Duration::from_secs(30);

/// What every route answers with.
///
/// One boxed type rather than two concrete ones, because the two shapes are genuinely different —
/// `/rpc` and `/health` know their whole answer before they write a byte, `/events` never knows its
/// whole answer at all — and a route that had to name which of them it produced would leak that
/// difference into every signature between here and the handler.
type ResponseBody = BoxBody<Bytes, Infallible>;

/// Serve one client until it goes away.
///
/// Every connection is already known to belong to this account — the peer check in
/// [`mixengine_platform::ipc`] happened before this was called — so there is no authentication here
/// and nothing for a request to be authorised against: every client of this daemon is the user, and
/// the user may do everything.
pub(crate) async fn serve_connection(api: Arc<Api>, connection: ipc::Connection) {
    let service = service_fn(move |request| route(Arc::clone(&api), request));

    // HTTP/1.1 only. HTTP/2 buys multiplexing over a link with real latency, and this one is a
    // socket on the same machine; a second protocol to keep working on three operating systems is
    // not free.
    let mut builder = hyper::server::conn::http1::Builder::new();
    // The timer has to be handed over explicitly. hyper is runtime-agnostic and owns no clock, so a
    // timeout set without one is not ignored — it panics on the first connection, which is how this
    // was found rather than shipped.
    builder.timer(TokioTimer::new());
    builder.header_read_timeout(HEADER_TIMEOUT);

    // `TokioIo` adapts between tokio's `AsyncRead`/`AsyncWrite` and hyper's own — the whole reason
    // `hyper-util` is a dependency.
    if let Err(error) = builder
        .serve_connection(TokioIo::new(connection), service)
        .await
    {
        // Nothing the daemon can act on, and the commonest cause is a client that closed the
        // window: every `/events` connection ends this way. `debug`, so the log of a healthy daemon
        // is not a list of clients that quit.
        tracing::debug!(%error, "a client connection ended");
    }
}

/// Which handler a request belongs to.
///
/// Matched on method *and* path, so `GET /rpc` is a `405` naming what it should have been rather
/// than a `404` claiming the route does not exist.
async fn route(
    api: Arc<Api>,
    request: Request<Incoming>,
) -> Result<Response<ResponseBody>, Infallible> {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();

    // Instrumented rather than entered: a span guard held across an `.await` would stay open while
    // this task is parked and attach itself to whatever ran next on that worker.
    let span = tracing::debug_span!("http", method = %method, path = %path);

    Ok(handle(api, request, &method, &path).instrument(span).await)
}

/// One request, once its method and path are known.
async fn handle(
    api: Arc<Api>,
    request: Request<Incoming>,
    method: &Method,
    path: &str,
) -> Response<ResponseBody> {
    match (method, path) {
        (&Method::POST, "/rpc") => post_rpc(&api, request).await,

        // `HEAD` alongside `GET` because HTTP says a server that answers one answers the other, and
        // because it is what a liveness probe reaches for. hyper writes the headers and drops the
        // body for it, so there is nothing to do here beyond letting it through.
        (&Method::GET | &Method::HEAD, "/health") => json(StatusCode::OK, &api.health()),

        // Not `HEAD`: this route's whole answer *is* its body, and a `HEAD` on it would subscribe a
        // client to a stream it can never read.
        (&Method::GET, "/events") => events(&api),

        // Routes that exist, but not for this verb. `Allow` is required on a `405` and is what
        // turns "no" into "here is what would have worked".
        (_, "/rpc") => not_allowed("POST"),
        (_, "/health") => not_allowed("GET, HEAD"),
        (_, "/events") => not_allowed("GET"),

        // `/logs/{service_id}` is in the architecture and arrives with the first service that has
        // any (roadmap task T14). Until then it is honestly not here.
        _ => problem(
            StatusCode::NOT_FOUND,
            Error::new(
                ErrorCode::NotFound,
                format!("no such endpoint: {method} {path}"),
            )
            .with_hint("this daemon serves `POST /rpc`, `GET /health` and `GET /events`"),
        ),
    }
}

/// `POST /rpc`.
async fn post_rpc(api: &Arc<Api>, request: Request<Incoming>) -> Response<ResponseBody> {
    let body = match Limited::new(request.into_body(), MAX_BODY).collect().await {
        Ok(collected) => collected.to_bytes(),

        // `Limited` reports the cap and a connection that died mid-body through the same boxed
        // error, and the two deserve different sentences: one is a client that must send less, the
        // other is a client that is no longer there. Told apart by the type rather than guessed at,
        // because a `413` naming a limit nobody hit sends whoever reads it looking for the wrong
        // thing. The answer is written either way — a write to a connection that is already gone
        // fails and is logged as a connection that ended.
        Err(error) => {
            return if error.downcast_ref::<LengthLimitError>().is_some() {
                problem(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    Error::new(
                        ErrorCode::InvalidArgument,
                        format!("the request body is larger than {MAX_BODY} bytes"),
                    )
                    .with_hint("`POST /rpc` takes one call or a batch — send a smaller batch"),
                )
            } else {
                problem(
                    StatusCode::BAD_REQUEST,
                    Error::new(
                        ErrorCode::InvalidArgument,
                        format!("the request body could not be read: {error}"),
                    ),
                )
            };
        }
    };

    match rpc::answer(api, &body).await {
        Some(answer) => json_bytes(StatusCode::OK, answer),

        // Notifications only. The spec says a server returns nothing, and `204` is how HTTP says
        // exactly that — an empty `200` would hand a client zero bytes where it expects JSON.
        None => {
            let mut response = Response::new(full(Bytes::new()));
            *response.status_mut() = StatusCode::NO_CONTENT;
            response
        }
    }
}

/// `GET /events`.
fn events(api: &Arc<Api>) -> Response<ResponseBody> {
    let subscription = api.events().subscribe();
    let shutdown = api.shutdown().token().clone();

    // An unfolding stream rather than a task writing into a queue: hyper polls this exactly as fast
    // as the client reads it, so a client that stops reading stops the stream instead of filling a
    // buffer behind it. That back-pressure is one of the reasons HTTP was chosen at all.
    //
    // The root token is the other way it ends. Nothing else would: a subscription with no events to
    // deliver sits in a fifteen-second heartbeat, so a shutting-down daemon with a GUI attached
    // would wait out its whole grace period on a stream neither end has anything more to say on.
    // Ending the body is also the honest thing to tell the client — the daemon is going away, and
    // an empty stream that stays open would look like one that simply has no news.
    let frames = futures_util::stream::unfold(
        (subscription, shutdown),
        |(mut subscription, shutdown)| async move {
            let frame = tokio::select! {
                () = shutdown.cancelled() => None,
                frame = subscription.next_or_heartbeat() => frame,
            }?;

            let data = BodyFrame::data(Bytes::from(frame.encode()));

            Some((Ok::<_, Infallible>(data), (subscription, shutdown)))
        },
    );

    let mut response = Response::new(StreamBody::new(frames).boxed());

    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    // Nothing between the two ends caches a local socket, but a client library may decide to on its
    // own, and a cached event stream is a client that never learns anything again.
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));

    response
}

/// A `405` naming the verb that would have worked.
fn not_allowed(allow: &'static str) -> Response<ResponseBody> {
    let mut response = problem(
        StatusCode::METHOD_NOT_ALLOWED,
        Error::new(
            ErrorCode::InvalidArgument,
            "wrong HTTP method for this endpoint",
        )
        .with_hint(format!("use {allow}")),
    );

    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));

    response
}

/// A failure of the envelope rather than of a call: a route that is not there, a body too large.
///
/// Deliberately **not** a JSON-RPC response. There is no `id` to answer and no method that ran, so
/// framing it as one would hand a client an answer to a call it never made. It is the plain
/// [`Error`] shape instead, which every client already knows how to render.
fn problem(status: StatusCode, error: Error) -> Response<ResponseBody> {
    json(status, &error)
}

/// A JSON body at a given status.
fn json<T: serde::Serialize>(status: StatusCode, value: &T) -> Response<ResponseBody> {
    match serde_json::to_vec(value) {
        Ok(body) => json_bytes(status, body),

        // Unreachable — every value here is a `mixengine-proto` type. Written out rather than
        // unwrapped because a panic in the API layer is the one thing the dispatcher next door goes
        // to some length to contain, and reintroducing one here would be odd.
        Err(error) => {
            tracing::error!(%error, "a response could not be encoded");

            json_bytes(
                StatusCode::INTERNAL_SERVER_ERROR,
                br#"{"code":"internal","message":"the answer could not be encoded"}"#.to_vec(),
            )
        }
    }
}

/// A JSON body that is already bytes.
///
/// Built by mutating a `Response` rather than through `Builder`, whose `body` returns a `Result`
/// that could only fail on a status or header this function did not itself write.
fn json_bytes(status: StatusCode, body: Vec<u8>) -> Response<ResponseBody> {
    let mut response = Response::new(full(Bytes::from(body)));

    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    response
}

/// A body that is complete before it is written.
fn full(bytes: Bytes) -> ResponseBody {
    Full::new(bytes).boxed()
}

impl Api {
    /// The body of `GET /health`.
    ///
    /// Here rather than beside the JSON-RPC handlers next door, because `/health` is not a method:
    /// it has no `id`, no `params`, and it exists so a client can decide whether to autostart a
    /// daemon before it knows anything else about it. Its shape is a route's, so it lives with the
    /// routes.
    fn health(&self) -> mixengine_proto::Health {
        mixengine_proto::Health {
            ok: true,
            version: self.version.to_owned(),
            protocol: self.protocol,
        }
    }
}
