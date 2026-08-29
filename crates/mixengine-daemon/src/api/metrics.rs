//! `GET /metrics` — what everything is costing, a frame per tick, while somebody is watching.
//!
//! **Opening this connection is the subscription and closing it is the end of it.** There is no
//! `metrics.subscribe` method, for [ADR
//! 0009](https://github.com/haiquang9994/MixEngine/blob/master/.claude/decisions/0009-logs-travel-on-their-own-stream.md)'s
//! reason and for one more of its own: a subscription ended by a second call would leave a client
//! that crashed sampling this machine every second for as long as the daemon ran. A socket closing
//! cannot be forgotten.
//!
//! **The rate is a consequence of this route, not a setting.** The watch the stream holds is what
//! puts the sampler on its one-second period; it lives in the stream's own state, so it is dropped
//! exactly when the body ends — whether the client closed the window, lost the socket, or the daemon
//! is shutting down.
//!
//! **A reader that falls behind is given the newest frame rather than a resync.** Eight frames of
//! back-pressure, where the event bus holds 1024: for a metric the old value is worth nothing and
//! the next one is a second away, so there is nothing to replay and nothing to ask the client to
//! re-fetch.

use std::convert::Infallible;
use std::sync::Arc;

use http_body_util::{BodyExt as _, StreamBody};
use hyper::Response;
use hyper::body::{Bytes, Frame as BodyFrame};
use hyper::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use mixengine_proto::MetricsFrame;
use tokio::sync::broadcast;

use super::Api;
use super::http::ResponseBody;

/// One frame, as an SSE `data:` line.
///
/// The same shape `GET /events` writes and for the same reason: one `data:` line holding a JSON
/// object, so a client needs one handler rather than one per event name.
fn encode(frame: &MetricsFrame) -> Bytes {
    let json = serde_json::to_string(frame).expect("a proto type always serialises");

    Bytes::from(format!("data: {json}\n\n"))
}

/// Serve the live readings until the client goes away.
pub(super) fn stream(api: &Arc<Api>) -> Response<ResponseBody> {
    let (watch, frames) = api.metrics().stream();
    let shutdown = api.shutdown().token().clone();

    // An unfolding stream rather than a task writing into a queue, exactly as `/events` has it:
    // hyper polls this as fast as the client reads it, so a client that stops reading slows its own
    // stream and nobody else's.
    let body = futures_util::stream::unfold(
        (watch, frames, shutdown),
        |(watch, mut frames, shutdown)| async move {
            let frame = loop {
                let received = tokio::select! {
                    // Ending the body is the honest thing to tell a client whose daemon is going
                    // away, and it is what drops the `Watch`.
                    () = shutdown.cancelled() => return None,
                    received = frames.recv() => received,
                };

                match received {
                    Ok(frame) => break frame,

                    // Behind by more than the channel holds. The next frame is a second away and
                    // the ones it missed describe a moment that has passed, so it simply gets the
                    // newest — there is nothing about a metric worth replaying.
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::debug!(missed, "a metrics reader fell behind");
                    }

                    // The sampler is gone, which can only mean the daemon is on its way out.
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            };

            let data = BodyFrame::data(encode(&frame));

            Some((Ok::<_, Infallible>(data), (watch, frames, shutdown)))
        },
    );

    let mut response = Response::new(StreamBody::new(body).boxed());

    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    // A cached metrics stream is a client that never sees a new number again.
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));

    response
}

#[cfg(test)]
mod tests {
    use mixengine_proto::{MetricsSample, MetricsSubject, Timestamp};

    use super::*;

    #[test]
    fn a_frame_goes_out_as_one_sse_data_line() {
        let encoded = encode(&MetricsFrame {
            at: Timestamp(60_000),
            samples: vec![MetricsSample {
                subject: MetricsSubject::Daemon,
                cpu_percent: None,
                rss_bytes: 1,
                processes: 1,
            }],
        });

        let text = String::from_utf8(encoded.to_vec()).expect("utf-8");

        assert!(text.starts_with("data: {"));
        assert!(text.ends_with("\n\n"), "one blank line ends an SSE frame");
        assert_eq!(text.matches("data: ").count(), 1);
    }
}
