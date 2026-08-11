//! Turning a request body into an answer: batches, notifications, dispatch, and panic containment.
//!
//! The whole of `POST /rpc` is here, and it is deliberately separate from [`super::http`]: this
//! module never sees a header, a status code or a socket, so everything it does can be tested by
//! handing it a slice of bytes.

use std::sync::Arc;

use mixengine_proto::rpc::{self, Id, Request, Response, RpcCode, RpcError};
use mixengine_proto::{DaemonStatus, DaemonVersion, Error, ErrorCode, Uptime};
use serde_json::Value;
use tracing::Instrument as _;

use super::Api;

/// Answer a `POST /rpc` body.
///
/// `None` means *write nothing back*: the body held only notifications, and the spec is explicit
/// that a server returns nothing for those. The caller turns that into `204 No Content` rather than
/// into an empty `200`, because an empty body is not valid JSON and a client that parses every
/// response would choke on one.
pub(super) async fn answer(api: &Arc<Api>, body: &[u8]) -> Option<Vec<u8>> {
    let payload: Value = match serde_json::from_slice(body) {
        Ok(payload) => payload,

        // The one failure where the id is genuinely unknowable, and the only place a `null` id is
        // correct. Note it is still an HTTP 200: the transport worked, the call did not.
        Err(error) => {
            let response = Response::failure(
                None,
                RpcError::at(
                    RpcCode::PARSE_ERROR,
                    Error::new(
                        ErrorCode::InvalidArgument,
                        format!("the request body is not JSON: {error}"),
                    )
                    .with_hint(
                        "`POST /rpc` takes one JSON-RPC 2.0 request object, or an array of them",
                    ),
                ),
            );

            return Some(encode(&response));
        }
    };

    match payload {
        Value::Array(calls) => {
            // An empty batch is the one array the spec singles out: there is nothing to answer, so
            // a server that returned `[]` would be answering a request it never received.
            if calls.is_empty() {
                let response = Response::failure(
                    None,
                    RpcError::at(
                        RpcCode::INVALID_REQUEST,
                        Error::new(ErrorCode::InvalidArgument, "the batch is empty"),
                    ),
                );

                return Some(encode(&response));
            }

            let mut answers = Vec::with_capacity(calls.len());

            // Sequential rather than concurrent. Running a batch in parallel would be free today —
            // no handler touches shared state — and would stop being free the moment one does,
            // silently, in whichever phase adds it. The spec promises nothing about order, so the
            // conservative reading costs a client nothing.
            for call in calls {
                if let Some(answer) = dispatch(api, call).await {
                    answers.push(answer);
                }
            }

            (!answers.is_empty()).then(|| encode(&answers))
        }

        single => dispatch(api, single).await.as_ref().map(encode),
    }
}

/// One call out of a body, answered — or not, if it was a notification.
async fn dispatch(api: &Arc<Api>, call: Value) -> Option<Response> {
    // Read out before the call is decoded, because a request that fails to decode still has to be
    // answered *to the id it claimed*: a client matching answers to calls has no other way to know
    // which of its five requests was the malformed one.
    let claimed = call.get("id");

    // A notification is a request with no `id` **member**, which is not the same thing as one whose
    // id is `null`: the spec discourages the latter but nowhere lets it mean "answer nothing", so a
    // client that sends one and waits would wait forever. The two are indistinguishable after
    // decoding — `Option<Id>` reads an absent member and a null one alike — so the distinction is
    // taken here, from the JSON, and never from `Request::is_notification`.
    let wants_an_answer = claimed.is_some();
    let id = claimed
        .cloned()
        .and_then(|id| serde_json::from_value::<Id>(id).ok());

    let request: Request = match serde_json::from_value(call) {
        Ok(request) => request,

        Err(error) => {
            return Some(Response::failure(
                id,
                RpcError::at(
                    RpcCode::INVALID_REQUEST,
                    Error::new(
                        ErrorCode::InvalidArgument,
                        format!("not a JSON-RPC request: {error}"),
                    )
                    .with_hint(
                        "a request is `{\"jsonrpc\":\"2.0\",\"method\":\"…\",\"id\":1}`, with \
                         `params` where the method takes them",
                    ),
                ),
            ));
        }
    };

    // The id is rendered into the span up front rather than recorded onto an `Empty` field
    // afterwards: `tracing-subscriber` formats a span's fields once and *appends* what is recorded
    // later, so the second way prints `id=3 id=3`. Each of the three cases is a value and not a
    // missing field — "this call wanted no answer" is worth being able to see in the log, and so is
    // the odd client that asked to be answered to `null`.
    let called = match (&id, wants_an_answer) {
        (Some(id), _) => id.to_string(),
        (None, true) => "null".to_owned(),
        (None, false) => "-".to_owned(),
    };
    let span = tracing::info_span!("rpc", method = %request.method, id = %called);

    let outcome = call_method(Arc::clone(api), request.method, request.params)
        .instrument(span)
        .await;

    // A notification is answered by silence even when it failed. The client asked not to be told,
    // and the daemon's own log is where that failure goes — which is why `call_method` logs it
    // rather than leaving it to whoever builds the response.
    if !wants_an_answer {
        return None;
    }

    // `id` is `None` only for the client that spelled its own id `null`, and that is what it gets
    // back: an answer echoes the id it was given rather than improving on it.
    Some(match outcome {
        Ok(result) => Response::success(id, result),
        Err(failure) => Response::failure(id, failure.into_rpc()),
    })
}

/// Run one method, containing anything it does to itself.
///
/// The call happens inside a spawned task purely so that a panic becomes a value. A handler that
/// panics must not take the daemon with it — `.claude/standards/rust.md` is explicit that a panic
/// here kills every managed service — and the connection alone is not enough to sacrifice either:
/// the client would see a dropped socket and have no idea whether its request had been carried out.
/// `panic = "abort"` in the release profile would defeat this, which is why the workspace manifest
/// says so out loud.
async fn call_method(
    api: Arc<Api>,
    method: String,
    params: Option<Value>,
) -> Result<Value, Failure> {
    let named = method.clone();

    let outcome = tokio::spawn(
        async move {
            match method.as_str() {
                rpc::method::DAEMON_STATUS => {
                    no_params(params.as_ref())?;
                    encode_result(&api.status())
                }

                rpc::method::DAEMON_VERSION => {
                    no_params(params.as_ref())?;
                    encode_result(&api.version())
                }

                // Not shipped, and the only way to prove the containment above does anything: a
                // handler that panics has to be a real handler, because catching a panic raised
                // anywhere else would prove something about the test and not about the dispatcher.
                #[cfg(test)]
                "daemon.__panic" => panic!("a handler that panics, on purpose"),

                unknown => Err(Failure {
                    code: RpcCode::METHOD_NOT_FOUND,
                    error: Error::new(
                        ErrorCode::NotFound,
                        format!("this daemon has no method `{unknown}`"),
                    )
                    .with_hint(
                        "the client and the daemon are probably from different releases — \
                         `daemon.version` says which one this is",
                    ),
                }),
            }
        }
        // The span belongs to the request, and the task the work runs in is not the task the span
        // was opened on, so it is attached explicitly rather than inherited.
        .in_current_span(),
    )
    .await;

    match outcome {
        Ok(Ok(result)) => {
            tracing::debug!("answered");
            Ok(result)
        }

        Ok(Err(failure)) => {
            // `warn` and not `error`: the caller has been told, in a message written for them, and
            // most of what lands here is a request that was simply wrong. The line exists so the
            // daemon's log can answer "what was this client doing" later.
            tracing::warn!(code = %failure.error.code, error = %failure.error.message, "refused");
            Err(failure)
        }

        Err(join) => {
            // The panic message itself has already gone to the log through the panic hook, and it
            // is not repeated to the client: a backtrace is not something a user can act on, and
            // the daemon's log is where it belongs.
            tracing::error!(
                panicked = join.is_panic(),
                "a request handler did not finish — answering `internal` and staying up"
            );

            Err(Failure {
                code: RpcCode::INTERNAL_ERROR,
                error: Error::new(
                    ErrorCode::Internal,
                    format!("`{named}` failed in a way it does not account for"),
                )
                .with_hint(
                    "this is a bug in MixEngine — `logs/daemon.log` has the detail a report needs",
                ),
            })
        }
    }
}

/// A method that takes no arguments, checking that it was called that way.
///
/// Lenient about how "nothing" is spelled: absent, `null`, `[]` and `{}` all mean the same thing,
/// and clients differ on which they send. Anything else is refused, because a client that passed
/// arguments believes they did something.
fn no_params(params: Option<&Value>) -> Result<(), Failure> {
    let empty = match params {
        None | Some(Value::Null) => true,
        Some(Value::Array(items)) => items.is_empty(),
        Some(Value::Object(fields)) => fields.is_empty(),
        Some(_) => false,
    };

    if empty {
        Ok(())
    } else {
        Err(Failure {
            code: RpcCode::INVALID_PARAMS,
            error: Error::new(
                ErrorCode::InvalidArgument,
                "this method takes no parameters",
            ),
        })
    }
}

/// A handler's return value, as JSON.
///
/// Serialising a type we defined can only fail on something like a map with non-string keys, which
/// no `mixengine-proto` type has — but it is a `Result` all the same rather than an `unwrap`, on
/// the same principle as the panic containment above: nothing in the RPC layer panics.
fn encode_result<T: serde::Serialize>(value: &T) -> Result<Value, Failure> {
    serde_json::to_value(value).map_err(|error| Failure {
        code: RpcCode::INTERNAL_ERROR,
        error: Error::new(
            ErrorCode::Internal,
            format!("the answer could not be encoded: {error}"),
        ),
    })
}

/// A failed call, before it is either dropped (a notification) or written into a [`Response`].
///
/// Carries the JSON-RPC integer *and* the MixEngine error because the two are chosen at different
/// moments: the integer says at which stage the call stopped, the error says what a person should
/// do about it.
#[derive(Debug)]
struct Failure {
    code: RpcCode,
    error: Error,
}

impl Failure {
    fn into_rpc(self) -> RpcError {
        RpcError::at(self.code, self.error)
    }
}

impl Api {
    /// `daemon.status` — every fact this build actually has.
    fn status(&self) -> DaemonStatus {
        DaemonStatus {
            version: self.version.to_owned(),
            protocol: self.protocol,
            pid: self.pid,
            home: self.home.clone(),
            endpoint: self.endpoint.clone(),
            database: self.database.clone(),
            started_at: self.started.at(),
            uptime: Uptime::from_duration(self.started.elapsed()),
        }
    }

    /// `daemon.version` — the handshake, cheap enough to answer while everything else is still
    /// coming up.
    fn version(&self) -> DaemonVersion {
        DaemonVersion {
            version: self.version.to_owned(),
            protocol: self.protocol,
        }
    }
}

/// Serialise an answer.
///
/// The failure case is unreachable — every type in it is one of ours — and is answered with a
/// hand-written JSON-RPC error rather than an `unwrap`, so that the invariant being wrong costs one
/// bad response instead of the process.
fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_else(|_| {
        br#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"the answer could not be encoded","data":{"code":"internal"}},"id":null}"#.to_vec()
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mixengine_core::Paths;
    use mixengine_core::config::PathOverrides;
    use mixengine_proto::rpc::Outcome;

    use super::*;

    /// Whether a response says the call succeeded.
    fn succeeded(response: &Response) -> bool {
        matches!(response.outcome, Outcome::Success { .. })
    }

    /// An [`Api`] with no daemon under it.
    ///
    /// The RPC layer reads state that was captured at startup and never touches the store or the
    /// listener, which is what lets these tests be unit tests: everything they exercise — batches,
    /// notifications, unknown methods, a panicking handler — is decided before any of that would
    /// come into play.
    fn api() -> Arc<Api> {
        let paths = Paths::new(PathBuf::from("/tmp/mixengine"), &PathOverrides::default());

        Arc::new(Api {
            version: "0.1.0",
            protocol: mixengine_proto::PROTOCOL_VERSION,
            pid: 4123,
            home: paths.root().display().to_string(),
            endpoint: "/tmp/mixengine/run/mixengined.sock".to_owned(),
            database: paths.database_file().display().to_string(),
            started: super::super::Started::now(),
            events: super::super::Events::new(),
        })
    }

    /// One call against a fresh daemon, decoded.
    async fn call(body: &str) -> Value {
        answer_json(&api(), body).await
    }

    #[tokio::test]
    async fn status_answers_with_what_the_daemon_knows_about_itself() {
        let answer = call(r#"{"jsonrpc":"2.0","method":"daemon.status","id":1}"#).await;

        assert_eq!(answer["jsonrpc"], "2.0");
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["result"]["version"], "0.1.0");
        assert_eq!(answer["result"]["pid"], 4123);
        assert_eq!(answer["result"]["protocol"], 1);
        assert!(answer["result"]["endpoint"].is_string());
    }

    #[tokio::test]
    async fn a_string_id_comes_back_as_the_same_string() {
        let answer = call(r#"{"jsonrpc":"2.0","method":"daemon.version","id":"handshake"}"#).await;

        assert_eq!(answer["id"], "handshake");
    }

    #[tokio::test]
    async fn an_unknown_method_is_method_not_found_and_says_which_one() {
        let answer = call(r#"{"jsonrpc":"2.0","method":"site.create","id":1}"#).await;

        assert_eq!(answer["error"]["code"], -32601);
        assert_eq!(answer["error"]["data"]["code"], "not_found");
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("site.create")),
            "{answer}"
        );
    }

    #[tokio::test]
    async fn parameters_to_a_method_that_takes_none_are_refused() {
        let answer =
            call(r#"{"jsonrpc":"2.0","method":"daemon.status","params":{"verbose":true},"id":1}"#)
                .await;

        assert_eq!(answer["error"]["code"], -32602);
        assert_eq!(answer["error"]["data"]["code"], "invalid_argument");
    }

    #[tokio::test]
    async fn the_four_ways_of_spelling_no_parameters_are_all_accepted() {
        for params in [
            r#","params":null"#,
            r#","params":[]"#,
            r#","params":{}"#,
            "",
        ] {
            let body = format!(r#"{{"jsonrpc":"2.0","method":"daemon.version"{params},"id":1}}"#);
            let answer = call(&body).await;

            assert!(answer.get("result").is_some(), "{params}: {answer}");
        }
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_is_a_parse_error_with_a_null_id() {
        let answer = call("not json at all").await;

        assert_eq!(answer["error"]["code"], -32700);
        assert!(answer["id"].is_null(), "{answer}");
    }

    #[tokio::test]
    async fn a_request_that_is_not_one_keeps_the_id_it_claimed() {
        // No `method`, so it never becomes a `Request` — but a client waiting on id 9 still has to
        // be told that id 9 is the one that was wrong.
        let answer = call(r#"{"jsonrpc":"2.0","id":9}"#).await;

        assert_eq!(answer["error"]["code"], -32600);
        assert_eq!(answer["id"], 9);
    }

    #[tokio::test]
    async fn a_notification_is_answered_with_silence() {
        assert!(
            answer(&api(), br#"{"jsonrpc":"2.0","method":"daemon.status"}"#)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_null_id_is_answered_rather_than_mistaken_for_a_notification() {
        // The spec discourages a null id and nowhere lets it mean silence: a request carrying one
        // is still a request, and a client that sent it is still waiting. Answered to the id it
        // gave, which is `null`.
        let answer = call(r#"{"jsonrpc":"2.0","method":"daemon.version","id":null}"#).await;

        assert_eq!(answer["result"]["protocol"], 1);
        assert!(answer["id"].is_null(), "{answer}");
    }

    #[tokio::test]
    async fn a_notification_that_fails_is_still_answered_with_silence() {
        assert!(
            answer(&api(), br#"{"jsonrpc":"2.0","method":"nope.nope"}"#)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_batch_answers_every_call_that_asked_for_one() {
        let answered = answer(
            &api(),
            br#"[
                {"jsonrpc":"2.0","method":"daemon.version","id":1},
                {"jsonrpc":"2.0","method":"daemon.status"},
                {"jsonrpc":"2.0","method":"nope.nope","id":3}
            ]"#,
        )
        .await
        .expect("two of the three asked for an answer");

        let answers: Vec<Response> = serde_json::from_slice(&answered).expect("an array of them");

        assert_eq!(answers.len(), 2, "the notification is not answered");
        assert!(succeeded(&answers[0]));
        assert!(!succeeded(&answers[1]));
        assert_eq!(answers[1].id, Some(Id::Number(3)));
    }

    #[tokio::test]
    async fn a_batch_of_nothing_but_notifications_answers_nothing_at_all() {
        assert!(
            answer(
                &api(),
                br#"[{"jsonrpc":"2.0","method":"daemon.status"},{"jsonrpc":"2.0","method":"daemon.version"}]"#
            )
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn an_empty_batch_is_an_invalid_request_rather_than_an_empty_array() {
        let answer = call("[]").await;

        assert_eq!(answer["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn a_batch_element_that_is_not_an_object_fails_on_its_own() {
        let answered = answer(
            &api(),
            br#"[1, {"jsonrpc":"2.0","method":"daemon.version","id":2}]"#,
        )
        .await
        .expect("both elements produce an answer");

        let answers: Vec<Response> = serde_json::from_slice(&answered).unwrap();

        assert_eq!(answers.len(), 2);
        assert!(!succeeded(&answers[0]));
        assert_eq!(answers[0].id, None, "a bare number has no id to echo");
        assert!(succeeded(&answers[1]), "the sound call is still answered");
    }

    #[tokio::test]
    async fn a_handler_that_panics_answers_internal_and_leaves_the_daemon_up() {
        let api = api();

        let answered = answer(
            &api,
            br#"{"jsonrpc":"2.0","method":"daemon.__panic","id":1}"#,
        )
        .await
        .expect("a panic is still an answer");
        let answer: Value = serde_json::from_slice(&answered).unwrap();

        assert_eq!(answer["error"]["code"], -32603);
        assert_eq!(answer["error"]["data"]["code"], "internal");
        // The panic message is not handed to the client — a backtrace is not something a user can
        // act on, and `logs/daemon.log` already has it.
        assert!(
            !answer["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("on purpose"),
            "{answer}"
        );

        // The whole point: the next request is answered normally.
        let after = answer_json(
            &api,
            r#"{"jsonrpc":"2.0","method":"daemon.version","id":2}"#,
        )
        .await;
        assert_eq!(after["result"]["version"], "0.1.0");
    }

    /// One call against a caller-supplied [`Api`], for the tests that need the same daemon twice.
    async fn answer_json(api: &Arc<Api>, body: &str) -> Value {
        let answered = answer(api, body.as_bytes())
            .await
            .expect("this call expects an answer");

        serde_json::from_slice(&answered).expect("the daemon answers JSON")
    }

    #[tokio::test]
    async fn another_protocol_version_is_an_invalid_request() {
        let answer = call(r#"{"jsonrpc":"1.0","method":"daemon.status","id":1}"#).await;

        assert_eq!(answer["error"]["code"], -32600);
    }
}
