//! Turning a request body into an answer: batches, notifications, dispatch, and panic containment.
//!
//! The whole of `POST /rpc` is here, and it is deliberately separate from [`super::http`]: this
//! module never sees a header, a status code or a socket, so everything it does can be tested by
//! handing it a slice of bytes.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use mixengine_core::services::{GraphError, Plan, ServiceGraph, ServiceRecord};
use mixengine_proto::rpc::{self, Id, Request, Response, RpcCode, RpcError};
use mixengine_proto::{
    DaemonShutdown, DaemonStatus, DaemonVersion, Error, ErrorCode, JobFilter, JobList, JobQuery,
    JobWait, RuntimeFilter, RuntimeQuestion, RuntimeTarget, ServiceFailure, ServiceId, ServiceList,
    ServiceQuery, ServiceSummary, ServiceTarget, ServiceWalk, Uptime,
};
use serde_json::Value;
use tracing::Instrument as _;

use super::Api;
use crate::error::ToWire as _;
use crate::services;

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

                rpc::method::DAEMON_SHUTDOWN => {
                    no_params(params.as_ref())?;
                    encode_result(&api.daemon_shutdown().await)
                }

                rpc::method::RUNTIME_LIST_AVAILABLE => {
                    let filter: RuntimeFilter = arguments(params)?;
                    encode_result(
                        &api.runtimes
                            .list_available(&filter)
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::RUNTIME_LIST_INSTALLED => {
                    let filter: RuntimeFilter = arguments(params)?;
                    encode_result(
                        &api.runtimes
                            .list_installed(&filter)
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::RUNTIME_INSTALL => {
                    let target: RuntimeTarget = arguments(params)?;
                    encode_result(&api.runtimes.install(&target).await.map_err(refused)?)
                }

                rpc::method::RUNTIME_UNINSTALL => {
                    let target: RuntimeTarget = arguments(params)?;
                    encode_result(&api.runtimes.uninstall(&target).await.map_err(refused)?)
                }

                rpc::method::RUNTIME_SET_DEFAULT => {
                    let target: RuntimeTarget = arguments(params)?;
                    encode_result(&api.runtimes.set_default(&target).await.map_err(refused)?)
                }

                rpc::method::RUNTIME_RESOLVE => {
                    let question: RuntimeQuestion = arguments(params)?;
                    encode_result(&api.runtimes.resolve(&question).await.map_err(refused)?)
                }

                // The three that write a file in the user's home rather than one in ours. Each is a
                // blocking pile of filesystem and registry work, so each runs where blocking work
                // belongs — see [`on_a_blocking_thread`].
                rpc::method::PATH_STATUS => {
                    no_params(params.as_ref())?;
                    let shims = Arc::clone(&api.shims);
                    encode_result(
                        &on_a_blocking_thread(move || shims.status())
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::PATH_INSTALL => {
                    no_params(params.as_ref())?;
                    let shims = Arc::clone(&api.shims);
                    encode_result(
                        &on_a_blocking_thread(move || shims.install())
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::PATH_UNINSTALL => {
                    no_params(params.as_ref())?;
                    let shims = Arc::clone(&api.shims);
                    encode_result(
                        &on_a_blocking_thread(move || shims.uninstall())
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::SERVICE_LIST => {
                    no_params(params.as_ref())?;
                    encode_result(&api.service_list().await.map_err(refused)?)
                }

                rpc::method::SERVICE_STATUS => {
                    let query: ServiceQuery = arguments(params)?;
                    encode_result(&api.service_status(&query.service).await.map_err(refused)?)
                }

                rpc::method::SERVICE_START => {
                    let target: ServiceTarget = arguments(params)?;
                    encode_result(&api.service_start(&target).await.map_err(refused)?)
                }

                rpc::method::SERVICE_STOP => {
                    let target: ServiceTarget = arguments(params)?;
                    encode_result(&api.service_stop(&target).await.map_err(refused)?)
                }

                rpc::method::SERVICE_RESTART => {
                    let target: ServiceTarget = arguments(params)?;
                    encode_result(&api.service_restart(&target).await.map_err(refused)?)
                }

                rpc::method::JOB_LIST => {
                    let filter: JobFilter = arguments(params)?;
                    encode_result(&JobList {
                        jobs: api.jobs.list(&filter).await.map_err(refused)?,
                    })
                }

                rpc::method::JOB_STATUS => {
                    let query: JobQuery = arguments(params)?;
                    encode_result(&api.jobs.status(query.job).await.map_err(refused)?)
                }

                rpc::method::JOB_WAIT => {
                    let wait: JobWait = arguments(params)?;
                    encode_result(
                        &api.jobs
                            .wait(wait.job, wait.timeout)
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::JOB_CANCEL => {
                    let query: JobQuery = arguments(params)?;
                    encode_result(&api.jobs.cancel(query.job).await.map_err(refused)?)
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

/// Run a handler's blocking half where blocking work belongs.
///
/// `.claude/standards/rust.md` keeps anything that waits on a disk off the runtime threads that
/// have connections to serve, and `path.*` is the first *method* here with such a half: nineteen
/// file copies, a directory walk, and on Windows a registry write that broadcasts to every window
/// on the desktop. Every other handler answers from memory or through `sqlx`, which does its own.
///
/// **A panic inside is re-raised rather than turned into an error**, so that the containment in
/// [`call_method`] is the one place a panicking handler is described — a second rendering of the
/// same accident here would be a second sentence for the same bug, differing only in which thread
/// it happened on.
async fn on_a_blocking_thread<T, F>(work: F) -> Result<T, Error>
where
    F: FnOnce() -> Result<T, Error> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(answer) => answer,
        Err(join) => match join.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            // Cancellation, which only happens when the runtime is going down — and a blocking task
            // is not cancellable, so this is unreachable while the daemon is the only thing
            // spawning them. Reported as itself rather than unwrapped, because nothing here panics.
            Err(_) => Err(Error::new(
                ErrorCode::Internal,
                "the work behind this call did not finish".to_owned(),
            )),
        },
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

/// A method's arguments, decoded into the shape that method documents.
///
/// Lenient about "nothing" in the same three spellings [`no_params`] accepts, because a method whose
/// every parameter has a default — `service.start` with no service named is every service — should
/// answer a client that sent `{}`, `null` or nothing at all identically. A type with a required
/// field still refuses all three, which is the point: `service.status` with no subject is a
/// `service.list` that was typed wrongly, and reporting it is better than answering it.
fn arguments<T: serde::de::DeserializeOwned>(params: Option<Value>) -> Result<T, Failure> {
    let given = match params {
        None | Some(Value::Null) => Value::Object(serde_json::Map::new()),
        Some(Value::Array(items)) if items.is_empty() => Value::Object(serde_json::Map::new()),
        Some(value) => value,
    };

    serde_json::from_value(given).map_err(|error| Failure {
        code: RpcCode::INVALID_PARAMS,
        error: Error::new(
            ErrorCode::InvalidArgument,
            format!("these are not this method's parameters: {error}"),
        ),
    })
}

/// A failure MixEngine itself produced: the method ran, and the work did not.
///
/// Everything that reaches this has already been through [`crate::error::ToWire`], where the code
/// and the hint are chosen; all that is added here is the JSON-RPC integer that says the call got as
/// far as running.
fn refused(error: Error) -> Failure {
    Failure {
        code: RpcCode::APPLICATION_ERROR,
        error,
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

    /// `daemon.shutdown` — stop every supervised service in reverse dependency order, then stop.
    ///
    /// **The order is the reason this is a method and not a cancelled token**, which is what a signal
    /// already is. A root token cancelled outright stops every runner at once, and a site that is
    /// still serving requests against a database that has gone is exactly what dependency order
    /// exists to prevent — for the whole set it is only a few hundred milliseconds of difference, and
    /// those are the milliseconds a user reads in a log as `connection refused`.
    ///
    /// **The budget is granted before the plan is built and it is the total**, not each service's:
    /// the specs are the services' own statements about what they need, and dividing what is left
    /// among them is `Runner::grace_for`'s, one level down. Nothing here
    /// takes a grace from the caller — a shutdown budget is a property of the machine, and a client
    /// that could ask for thirty seconds could ask for thirty minutes.
    ///
    /// **Asked a second time, it grants nothing at all** — the same escalation `main.rs` performs
    /// when a second console event arrives during a stop that is still going, and for the same
    /// reason: somebody asking again is somebody who has stopped being willing to wait for the
    /// polite stop. What that reaches is every runner that has not begun its stop, which then goes
    /// straight to the kill; a service already inside a grace period keeps the one it was granted,
    /// exactly as it does on the signal path. Two clients asking at the same instant get the same
    /// treatment, which is the cost of not being able to tell them apart from one person asking
    /// twice — the same cost two Ctrl-Cs in quick succession already carry.
    ///
    /// **Without it one platform has no escalation at all.** Every console control event needs a
    /// console, and a `--detach`ed daemon on Windows — the one `mix` autostarts — has none, so this
    /// method is the only thing that can ask it anything. `Registry::stopping_within` narrows and
    /// never extends, so a second request that passed the configured grace again would be discarded
    /// by its own `min` and change nothing.
    ///
    /// **The token is cancelled after the walk and before this answers**, which is the ordering the
    /// whole method rests on. Cancelling first would stop the services out of order through the
    /// signal path, and answering first would mean writing a response into a connection this daemon
    /// is about to stop waiting for. What the client then sees is the answer, followed by the
    /// connection closing — which *is* the shutdown, not a failure of it.
    ///
    /// **It is cancelled by a guard and not by a statement, because this future is not guaranteed to
    /// reach one.** A client that goes away mid-walk takes the handler with it, and the first thing
    /// done here latches the registry shut for good — so a cancellation that lived on a line at the
    /// end would be skipped on exactly the paths that leave a daemon nobody can start anything with.
    /// See [`Going`](super::Going). Both facts above still hold: it drops after the walk, and the
    /// answer is encoded by the caller.
    ///
    /// **A daemon that cannot say what it declares still stops.** An `Undeclarable` here is a
    /// `extension.toml` somebody is in the middle of editing, and refusing to shut down over it
    /// would leave them with a daemon they can only kill. It is reported, the ordered walk is
    /// skipped, and the cancellation on the way out stops every runner the untidy way — which is
    /// what a signal would have done anyway.
    ///
    /// **Reported to the client and not only to `daemon.log`**, which is what
    /// [`DaemonShutdown::unordered`] is for: the walk that goes out in that case is empty and
    /// complete, and a client reading that alone cannot tell a skipped order from a home with
    /// nothing to stop. What it carries is the wire error [`ToWire`](crate::error::ToWire) already
    /// writes for these same declarations, so `mix daemon stop` and `mix service list` say the same
    /// sentence about the same half-written file rather than two.
    async fn daemon_shutdown(&self) -> DaemonShutdown {
        // Read before anything here latches it, or every shutdown would look like a second one.
        let asked_again = self.services.is_shutting_down();

        // Taken before the registry is latched, so that from the moment this daemon refuses to start
        // anything there is no way of leaving it running.
        let _going = self.shutdown.begun();

        self.services.stopping_within(match asked_again {
            true => std::time::Duration::ZERO,
            false => self.shutdown.grace(),
        });

        let (services, unordered) = match self.stop_everything().await {
            Ok(walk) => (walk, None),

            Err(error) => {
                tracing::warn!(
                    %error,
                    "cannot work out the order to stop services in; stopping them all at once"
                );

                (
                    ServiceWalk {
                        planned: Vec::new(),
                        complete: true,
                        reached: Vec::new(),
                        failed: None,
                        blocked: Vec::new(),
                    },
                    Some(error),
                )
            }
        };

        tracing::info!(
            stopped = services.reached.len(),
            refused = services
                .failed
                .as_ref()
                .map(|failure| failure.service.as_str()),
            ordered = unordered.is_none(),
            // The only record that a stop was cut short, and the sentence somebody reads when they
            // go looking for why a database recovered on its next start.
            hurried = asked_again,
            "a client asked this daemon to stop"
        );

        DaemonShutdown {
            services,
            unordered,
        }
    }

    /// The ordered half of [`Api::daemon_shutdown`]: every declared service, dependents first.
    ///
    /// Separate so that the failure to build a plan is a value the caller can go on from rather than
    /// a `?` that would take the shutdown with it.
    async fn stop_everything(&self) -> Result<ServiceWalk, Error> {
        let graph = self
            .services
            .graph()
            .await
            .map_err(|error| error.to_wire())?;
        let plan = graph.stop_order();
        let planned = plan.flat().cloned().collect();

        Ok(walked(planned, self.services.stop(&plan).await))
    }

    /// `service.list` — every declared service and what it is doing.
    ///
    /// **Three readings composed, and each keeps its own authority.** The *set* comes from the
    /// declarations, so a listing cannot name something `service.start` would not find; the *state*
    /// comes from the `services` row, which is the same value `ServiceStateChanged` announced; and
    /// whether a task is supervising it comes from the registry, which is a different question and
    /// is reported as one. Nothing here re-derives a fact another layer already owns.
    ///
    /// An empty list is a real answer and not a failure: until T30 renders a `services` row into a
    /// runnable spec, this build declares nothing at all.
    async fn service_list(&self) -> Result<ServiceList, Error> {
        let graph = self
            .services
            .graph()
            .await
            .map_err(|error| error.to_wire())?;
        let rows = mixengine_core::services::records(&self.store)
            .await
            .map_err(|error| error.to_wire())?;
        let supervised = self.services.supervised();

        let services = graph
            .ids()
            .map(|id| summary(&graph, id, rows.get(id.as_str()), &supervised))
            .collect();

        Ok(ServiceList { services })
    }

    /// `service.status` — the same sentence about one of them.
    ///
    /// A separate read of the one row rather than a filtered [`Api::service_list`]: the question is
    /// about one service, and answering it by reading every row would make a home with forty of them
    /// pay for thirty-nine it did not ask about.
    async fn service_status(&self, id: &ServiceId) -> Result<ServiceSummary, Error> {
        let graph = self
            .services
            .graph()
            .await
            .map_err(|error| error.to_wire())?;

        if graph.spec(id).is_none() {
            return Err(
                mixengine_core::Error::Graph(GraphError::NoSuchService { id: id.clone() })
                    .to_wire(),
            );
        }

        // A declared service with no row is reported rather than refused — see `summary`.
        let record = match mixengine_core::services::record(&self.store, id).await {
            Ok(record) => Some(record),
            Err(mixengine_core::Error::NotFound { .. }) => None,
            Err(error) => return Err(error.to_wire()),
        };

        Ok(summary(
            &graph,
            id,
            record.as_ref(),
            &self.services.supervised(),
        ))
    }

    /// `service.start` — bring a service up, and everything it depends on with it.
    async fn service_start(&self, target: &ServiceTarget) -> Result<ServiceWalk, Error> {
        let graph = self
            .services
            .graph()
            .await
            .map_err(|error| error.to_wire())?;
        let plan = start_plan(&graph, target.service.as_ref())?;

        self.walk(
            target.wait,
            plan.flat().cloned().collect(),
            move |services| async move {
                let walk = services.start(&graph, &plan).await;

                (walk, "start")
            },
        )
        .await
    }

    /// `service.stop` — take a service down, and everything that depends on it first.
    ///
    /// **A stop can fail**, and since T18 there is exactly one way: a process that outlived a
    /// previous daemon, was adopted by this one, and would not die. What comes back then names it,
    /// with no reason attached — see [`Registry::stop`](crate::services::Registry::stop).
    async fn service_stop(&self, target: &ServiceTarget) -> Result<ServiceWalk, Error> {
        let graph = self
            .services
            .graph()
            .await
            .map_err(|error| error.to_wire())?;
        let plan = stop_plan(&graph, target.service.as_ref())?;

        self.walk(
            target.wait,
            plan.flat().cloned().collect(),
            move |services| async move {
                let walk = services.stop(&plan).await;

                (walk, "stop")
            },
        )
        .await
    }

    /// `service.restart` — take it down, and put back exactly what went down with it.
    ///
    /// **The two halves name different sets, and that asymmetry is the whole of this method.**
    /// Restarting MariaDB stops everything that depends on it, so starting MariaDB again would leave
    /// `php-fpm` where the stop left it: down, on behalf of a user who asked for a restart and got
    /// half of one. So what is started is what the stop *took down*, in start order — which
    /// [`ServiceGraph::start_plan`] computes for a set as readily as for one service, and which also
    /// pulls in anything that set depends on and was not already up.
    ///
    /// **Took down, not covered**: the two differ, and reading the stop plan as the start's would
    /// make a restart of MariaDB start every dependent a user had deliberately stopped. See
    /// [`restarted`].
    ///
    /// What comes back describes the *start*, in the ordinary case where the stop reached everything
    /// it was asked to; the plan reported is then the one that was walked second.
    ///
    /// **When the stop fails, that is what comes back instead, and the start never happens.** The one
    /// way it can (T18) is a survivor this daemon adopted and could not kill — a process still
    /// holding the port and the data directory — and starting the service again on top of it would
    /// put a second one there to collide with the first. A restart that could not take the service
    /// down has not restarted it, and says so.
    async fn service_restart(&self, target: &ServiceTarget) -> Result<ServiceWalk, Error> {
        let graph = self
            .services
            .graph()
            .await
            .map_err(|error| error.to_wire())?;
        let down = stop_plan(&graph, target.service.as_ref())?;

        // Read before anything is stopped, because afterwards nothing is supervised and every
        // service would look like one that had been down all along.
        let roots = restarted(target.service.as_ref(), &down, &self.services.supervised());

        // Cannot fail: every id in it came out of this same graph. Mapped rather than unwrapped all
        // the same, because a panic here would be one bad request taking the daemon with it.
        let up = graph
            .start_plan(roots.iter())
            .map_err(|error| mixengine_core::Error::Graph(error).to_wire())?;

        self.walk(
            target.wait,
            up.flat().cloned().collect(),
            move |services| async move {
                // Reported against the *start* plan, which is what this walk was announced with —
                // and the service that would not stop is always in it, because it was supervised
                // when `restarted` read the set a moment ago.
                if let Some((refused, _)) = services.stop(&down).await.failed {
                    return (
                        services::Walk {
                            failed: Some((refused, None)),
                            ..services::Walk::default()
                        },
                        "restart",
                    );
                }

                (services.start(&graph, &up).await, "restart")
            },
        )
        .await
    }

    /// Run a walk here, or behind the answer — the whole of what [`ServiceTarget::wait`] chooses.
    ///
    /// **A walk that is not waited for is still bounded by the daemon's own life.** It is cancelled
    /// by the root token rather than detached, per the rule in `.claude/standards/rust.md` against a
    /// task that outlives shutdown: the services it started keep their runners, which are the
    /// registry's and were never this task's to hold.
    ///
    /// Its outcome goes to `daemon.log`, because nobody is waiting to be told: a client that asked
    /// not to wait is reading `ServiceStateChanged`, where every move in the walk appears as it
    /// happens — the walk's own summary is the one thing that stream does not carry.
    async fn walk<F, W>(
        &self,
        wait: bool,
        planned: Vec<ServiceId>,
        walking: F,
    ) -> Result<ServiceWalk, Error>
    where
        F: FnOnce(Arc<services::Registry>) -> W + Send + 'static,
        W: Future<Output = (services::Walk, &'static str)> + Send + 'static,
    {
        let services = Arc::clone(&self.services);

        if wait {
            let (walk, _) = walking(services).await;

            return Ok(walked(planned, walk));
        }

        let shutdown = self.shutdown.token().clone();
        let accepted = planned.clone();

        tokio::spawn(async move {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("a walk that was not waited for was cut short by the shutdown");
                }

                (walk, what) = walking(services) => {
                    tracing::info!(
                        %what,
                        reached = walk.reached.len(),
                        failed = walk.failed.as_ref().map(|(id, _)| id.as_str()),
                        blocked = walk.blocked.len(),
                        "a walk nobody was waiting for has finished"
                    );
                }
            }
        });

        Ok(ServiceWalk {
            planned: accepted,
            complete: false,
            reached: Vec::new(),
            failed: None,
            blocked: Vec::new(),
        })
    }
}

/// What must start for `service` to be running, or for everything to be.
fn start_plan(graph: &ServiceGraph, service: Option<&ServiceId>) -> Result<Plan, Error> {
    match service {
        Some(id) => graph
            .start_plan([id])
            .map_err(|error| mixengine_core::Error::Graph(error).to_wire()),
        None => Ok(graph.start_order()),
    }
}

/// The opposite walk — **not** the same one reversed. See [`ServiceGraph::stop_plan`].
fn stop_plan(graph: &ServiceGraph, service: Option<&ServiceId>) -> Result<Plan, Error> {
    match service {
        Some(id) => graph
            .stop_plan([id])
            .map_err(|error| mixengine_core::Error::Graph(error).to_wire()),
        None => Ok(graph.stop_order()),
    }
}

/// What a restart puts back: what was asked for, plus what the stop is about to take down.
///
/// **A stop plan is what the graph says a stop reaches, and not what it finds there.** Half of it
/// can already be down — `mix service stop web` an hour ago, a service never started — and a restart
/// that fed the whole plan back into a start would take that as a request to start them, so
/// restarting a database would silently bring up every site that names it. What was down before is
/// left down.
///
/// The service the caller *named* is the exception, and is restarted whether or not it was running:
/// `restart` on something stopped is a request for it to be running, the same reading `start` gives.
/// With nothing named every declared service is the named one, so `service.restart` with no target
/// stays what it says — restart everything.
///
/// Supervision rather than the row is the test for "was up", because it is the registry's own
/// answer to a question about the registry's own tasks: a service in its fourth restart backoff is
/// not running and is very much still one this daemon is bringing up.
fn restarted(
    named: Option<&ServiceId>,
    down: &Plan,
    supervised: &BTreeSet<ServiceId>,
) -> Vec<ServiceId> {
    down.flat()
        .filter(|id| named.is_none_or(|named| named == *id) || supervised.contains(*id))
        .cloned()
        .collect()
}

/// One service, as the three readings that know about it describe it.
///
/// `record` is [`None`] for a service that is declared and has no `services` row. That is not a case
/// a finished MixEngine reaches — from T30 a declaration is rendered *from* a row — and it is
/// reported rather than smoothed into `stopped`, because a service that claims to be stopped and
/// then refuses to start explains nothing to whoever declared it.
fn summary(
    graph: &ServiceGraph,
    id: &ServiceId,
    record: Option<&ServiceRecord>,
    supervised: &BTreeSet<ServiceId>,
) -> ServiceSummary {
    ServiceSummary {
        id: id.clone(),
        state: record.map(|record| record.state),
        supervised: supervised.contains(id),
        pid: record.and_then(|record| record.pid),
        last_started_at: record.and_then(|record| record.last_started_at),
        last_exit_code: record.and_then(|record| record.last_exit_code),
        // The graph's edges rather than the spec's list: each dependency once, in id order. A
        // service that is in the graph has an entry, so the failure is unreachable — and it is
        // answered with "none declared" rather than a panic, on the same principle as everything
        // else in this module.
        depends_on: graph
            .dependencies_of(id)
            .map(|dependencies| dependencies.iter().cloned().collect())
            .unwrap_or_default(),
    }
}

/// A finished walk, in the shape a client renders.
fn walked(planned: Vec<ServiceId>, walk: services::Walk) -> ServiceWalk {
    ServiceWalk {
        planned,
        complete: true,
        reached: walk.reached,
        failed: walk
            .failed
            .map(|(service, reason)| ServiceFailure { service, reason }),
        blocked: walk.blocked,
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
    use std::time::Instant;

    use mixengine_proto::rpc::Outcome;
    use mixengine_proto::{Millis, PathReport, ReadyCheck, ServiceState, StopBehaviour};
    use mixengine_testkit::{FakeService, Home};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::services::fixture::{self, EVENTUALLY};

    /// The shutdown budget these tests give a daemon.
    ///
    /// Generous, because nothing here is measuring how a budget runs out — that is the runner's
    /// arithmetic and is tested where it lives. What these tests assert is the *order*: services
    /// stopped, then the token cancelled, then the answer.
    const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

    /// Whether a response says the call succeeded.
    fn succeeded(response: &Response) -> bool {
        matches!(response.outcome, Outcome::Success { .. })
    }

    /// A daemon with a home under it: a database, a registry and whatever it declares.
    ///
    /// The `daemon.*` handlers read state captured at startup and touch neither, which is what let
    /// this be a bare struct before T19a. `service.*` reads the `services` rows and the registry, so
    /// the home is a real one and is held here for as long as the test needs it.
    struct Daemon {
        _home: Home,
        api: Arc<Api>,
        services: Arc<services::Registry>,

        /// The same machine the API was built with, so a test can ask what it was told to do.
        ///
        /// Held as the concrete mock rather than as `dyn Host`, because what is worth asserting is
        /// the recording — `path_operations`, `restricted` — and the trait deliberately has no way
        /// to ask for it.
        host: Arc<mixengine_platform::mock::Host>,
    }

    impl Daemon {
        /// One call, decoded.
        async fn call(&self, body: &str) -> Value {
            answer_json(&self.api, body).await
        }

        /// One `service.*` call, built from its parameters.
        async fn ask(&self, method: &str, params: Value) -> Value {
            let body = serde_json::json!({
                "jsonrpc": "2.0", "method": method, "params": params, "id": 1
            });

            self.call(&body.to_string()).await
        }

        /// The result of a call that was expected to succeed, as the type it documents.
        async fn expect<T: serde::de::DeserializeOwned>(&self, method: &str, params: Value) -> T {
            let answer = self.ask(method, params).await;

            serde_json::from_value(answer["result"].clone())
                .unwrap_or_else(|error| panic!("{method} answered {answer}: {error}"))
        }

        /// What `service.status` says this service is doing.
        async fn state(&self, id: &str) -> Option<ServiceState> {
            let summary: ServiceSummary = self
                .expect(
                    rpc::method::SERVICE_STATUS,
                    serde_json::json!({"service": id}),
                )
                .await;

            summary.state
        }

        /// Wait for a service to reach a state, for the calls that answer before it has.
        async fn until(&self, id: &str, state: ServiceState) {
            let deadline = Instant::now() + EVENTUALLY;

            while Instant::now() < deadline {
                if self.state(id).await == Some(state) {
                    return;
                }

                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }

            panic!(
                "{id} never reached {state}, and is {:?}",
                self.state(id).await
            );
        }

        /// Stop everything this daemon is supervising, as `serve` does on its way out.
        ///
        /// Called by every test that started something: a `Home` is a temporary directory, and on
        /// Windows one cannot be removed while a process it holds the log file of is still running.
        async fn quiet(self) {
            self.services.shut_down().await;
        }
    }

    /// A daemon declaring `specs`, with a `services` row for each of `rows`.
    ///
    /// The two lists are separate on purpose: a declaration and a row are different things, and the
    /// one test that gives a service the first without the second is testing exactly that.
    async fn daemon(specs: Arc<dyn services::SpecSource>, rows: &[&str]) -> Daemon {
        let (home, paths, store) = fixture::home(rows).await;
        let events = super::super::Events::new();

        let services = Arc::new(services::Registry::new(
            &paths,
            &store,
            Arc::new(mixengine_platform::mock::Host::with_home(paths.root())),
            events.clone(),
            specs,
            CancellationToken::new(),
        ));

        let jobs = Arc::new(crate::jobs::Jobs::new(
            &store,
            events.clone(),
            CancellationToken::new(),
        ));

        // Pointed at the published index, which nothing in this file asks anything of: these tests
        // are about dispatch, and every `runtime.*` method's own behaviour is proved against a
        // `MockRegistry` in `tests/runtimes.rs`, where there is a real socket to serve one over.
        // Constructing it here is still worth doing rather than stubbing — it is the one assertion
        // available that a daemon builds one at all without reaching the network to do it.
        let runtimes = crate::runtimes::Runtimes::new(
            &paths,
            &store,
            Arc::clone(&jobs),
            &crate::runtimes::IndexSource::default(),
        )
        .expect("the compiled-in index key is a key");

        // A stand-in for the two binaries a release ships side by side. `shims::source` looks for
        // the shim *beside the program that is running*, and the program running these tests is a
        // test binary in `target/debug/deps` — so the pair is made here, inside the home this test
        // owns, and the copies that land in `bin/` are copies of a file with known contents.
        let installed = paths.root().join("installed-beside");
        std::fs::create_dir_all(&installed).expect("a directory in a temporary home");
        std::fs::write(
            installed.join(format!("mixengine-shim{}", std::env::consts::EXE_SUFFIX)),
            b"the shim, as far as a copy is concerned",
        )
        .expect("a file in a temporary home");

        let host = Arc::new(mixengine_platform::mock::Host::with_home(paths.root()));

        let shims = Arc::new(crate::shims::Shims::new(
            &paths,
            installed.join(format!("mixengined{}", std::env::consts::EXE_SUFFIX)),
            Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
        ));

        let api = Arc::new(Api {
            version: "0.1.0",
            protocol: mixengine_proto::PROTOCOL_VERSION,
            pid: 4123,
            home: paths.root().display().to_string(),
            endpoint: "/tmp/mixengine/run/mixengined.sock".to_owned(),
            database: paths.database_file().display().to_string(),
            paths: paths.clone(),
            jobs,
            runtimes,
            shims,
            store,
            services: Arc::clone(&services),
            started: super::super::Started::now(),
            events,
            // Cancelled by `daemon.shutdown` and by nothing else here — there is no accept loop in
            // these tests waiting on it, which is what makes the method's own test able to assert
            // that it was cancelled rather than watch a process exit.
            shutdown: super::super::Shutdown::new(CancellationToken::new(), SHUTDOWN_GRACE),
        });

        Daemon {
            _home: home,
            api,
            services,
            host,
        }
    }

    /// A daemon that declares nothing — this build's own [`services::Undeclared`].
    async fn undeclared() -> Daemon {
        daemon(Arc::new(services::Undeclared), &[]).await
    }

    /// One call against a daemon with nothing declared, decoded.
    async fn call(body: &str) -> Value {
        undeclared().await.call(body).await
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

    /// The three `path.*` methods over the dispatcher — roadmap task **T26**.
    ///
    /// Against `mock::Host`, which is what makes this runnable at all: the real implementations
    /// write a registry value or a file in the person's own home, and a suite that exercised them
    /// would be a `cargo test` that edits the PATH of whoever ran it. The two real ones have their
    /// own tests inside `mixengine-platform`, against a key and a home they create themselves.
    #[tokio::test]
    async fn the_path_is_reported_then_taken_and_then_given_back() {
        let daemon = undeclared().await;

        let before: PathReport = daemon.expect(rpc::method::PATH_STATUS, Value::Null).await;
        assert!(!before.on_path, "{before:?}");
        assert!(before.directory.ends_with("bin"), "{before:?}");

        // The status did not fill `bin/`, and says so rather than listing the table.
        assert!(before.commands.is_empty(), "{before:?}");

        let installed: PathReport = daemon.expect(rpc::method::PATH_INSTALL, Value::Null).await;
        assert!(installed.on_path);
        assert!(installed.places.iter().all(|place| place.changed));
        assert_eq!(
            installed.commands.len(),
            mixengine_core::shims::COMMANDS.len(),
            "{installed:?}"
        );
        assert!(installed.stale.is_empty());

        // Idempotent, and it says which of the two it was — a client that reported a write it did
        // not perform would be indistinguishable from one that did.
        let again: PathReport = daemon.expect(rpc::method::PATH_INSTALL, Value::Null).await;
        assert!(again.on_path);
        assert!(again.places.iter().all(|place| !place.changed), "{again:?}");

        let removed: PathReport = daemon
            .expect(rpc::method::PATH_UNINSTALL, Value::Null)
            .await;
        assert!(!removed.on_path);

        // The shims stay: removing the home is what removes them.
        assert_eq!(
            removed.commands.len(),
            mixengine_core::shims::COMMANDS.len()
        );

        // Two installs and an uninstall. The status is absent, which is the point: a read is not a
        // mutation, and the mock records only what changed the machine or tried to.
        assert_eq!(daemon.host.path_operations().len(), 3);

        daemon.quiet().await;
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
            answer(
                &undeclared().await.api,
                br#"{"jsonrpc":"2.0","method":"daemon.status"}"#
            )
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
            answer(
                &undeclared().await.api,
                br#"{"jsonrpc":"2.0","method":"nope.nope"}"#
            )
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn a_batch_answers_every_call_that_asked_for_one() {
        let answered = answer(
            &undeclared().await.api,
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
                &undeclared().await.api,
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
            &undeclared().await.api,
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
        let api = undeclared().await.api;

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

    // `service.*` — T19a. What these exercise is the surface over T19's registry, which is why they
    // are here and not there: the walk itself has its own tests next door, and what has never been
    // proved before is that a request reaches it and comes back as something a client can render.

    /// Two services, the second depending on the first — the shape every walk below is about.
    fn web_and_db() -> Arc<fixture::Declared> {
        Arc::new(fixture::Declared(vec![
            fixture::spec("db").build().expect("a usable spec"),
            fixture::spec("web")
                .depends_on(fixture::service("db"))
                .build()
                .expect("a usable spec"),
        ]))
    }

    /// One service that ignores a request to stop, with a stop command that never answers.
    ///
    /// A minute of grace it can never use: whatever it actually spends stopping is the grace period
    /// the budget allowed it, which is what makes a stop's *length* the readable thing it is here.
    fn slow_to_stop(id: &str) -> Arc<fixture::Declared> {
        Arc::new(fixture::Declared(vec![
            fixture::spec(id)
                .args(fixture::arguments(&FakeService::new().ignoring_stop()))
                .stop(StopBehaviour::Command {
                    program: FakeService::program(),
                    args: fixture::arguments(&FakeService::new()),
                    grace: Millis::from_secs(60),
                })
                .build()
                .expect("a usable spec"),
        ]))
    }

    #[tokio::test]
    async fn a_home_that_declares_nothing_lists_nothing_rather_than_failing() {
        // The honest answer for this build, and the one the registry, the graph and the walk all
        // handle without a special case: `Undeclared` until T30 renders a row into a spec.
        let list: ServiceList = undeclared()
            .await
            .expect(rpc::method::SERVICE_LIST, Value::Null)
            .await;

        assert!(list.services.is_empty(), "{list:?}");
    }

    #[tokio::test]
    async fn a_listing_names_the_declared_set_with_what_each_row_says() {
        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        let list: ServiceList = daemon.expect(rpc::method::SERVICE_LIST, Value::Null).await;

        let ids: Vec<&str> = list.services.iter().map(|one| one.id.as_str()).collect();
        assert_eq!(
            ids,
            ["db", "web"],
            "in id order, which is a listing's order"
        );

        let web = &list.services[1];
        assert_eq!(web.state, Some(ServiceState::Stopped), "what the row says");
        assert!(!web.supervised, "nothing has been started");
        assert_eq!(web.pid, None);
        assert_eq!(
            web.depends_on,
            vec![fixture::service("db")],
            "the graph's edge, which is what makes a start order explicable"
        );
    }

    #[tokio::test]
    async fn a_service_that_is_declared_and_has_no_row_is_reported_without_a_state() {
        // Not a case a finished MixEngine reaches — from T30 a declaration is rendered *from* a row
        // — and reported rather than smoothed into `stopped`, because a service that claims to be
        // stopped and then refuses to start explains nothing to whoever declared it.
        let daemon = daemon(web_and_db(), &["db"]).await;

        let list: ServiceList = daemon.expect(rpc::method::SERVICE_LIST, Value::Null).await;

        assert_eq!(list.services[0].state, Some(ServiceState::Stopped), "db");
        assert_eq!(list.services[1].state, None, "web, which has no row");
    }

    #[tokio::test]
    async fn the_status_of_a_service_nobody_declared_is_not_found() {
        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        let answer = daemon
            .ask(
                rpc::method::SERVICE_STATUS,
                serde_json::json!({"service": "mailpit"}),
            )
            .await;

        assert_eq!(answer["error"]["data"]["code"], "not_found");
        assert_eq!(
            answer["error"]["code"], -32000,
            "the method ran and the work did not: an application error, not a protocol one"
        );
    }

    #[tokio::test]
    async fn a_status_with_no_service_is_refused_rather_than_answered_as_a_listing() {
        let answer = undeclared()
            .await
            .ask(rpc::method::SERVICE_STATUS, serde_json::json!({}))
            .await;

        assert_eq!(answer["error"]["code"], -32602, "{answer}");
        assert_eq!(answer["error"]["data"]["code"], "invalid_argument");
    }

    #[tokio::test]
    async fn starting_one_service_starts_what_it_depends_on_and_says_what_it_reached() {
        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        let walk: ServiceWalk = daemon
            .expect(
                rpc::method::SERVICE_START,
                serde_json::json!({"service": "web"}),
            )
            .await;

        assert!(walk.complete, "the default is to wait: {walk:?}");
        assert_eq!(
            walk.planned,
            vec![fixture::service("db"), fixture::service("web")],
            "a plan is the transitive set, in the order it was walked"
        );
        assert_eq!(walk.reached, walk.planned, "{walk:?}");
        assert!(walk.failed.is_none(), "{walk:?}");

        let summary: ServiceSummary = daemon
            .expect(
                rpc::method::SERVICE_STATUS,
                serde_json::json!({"service": "db"}),
            )
            .await;
        assert_eq!(summary.state, Some(ServiceState::Running));
        assert!(summary.supervised, "a task is holding it");
        assert!(summary.pid.is_some(), "the process it is running as");

        daemon.quiet().await;
    }

    #[tokio::test]
    async fn a_start_that_is_not_waited_for_answers_with_the_plan_and_walks_behind_it() {
        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        let walk: ServiceWalk = daemon
            .expect(
                rpc::method::SERVICE_START,
                serde_json::json!({"service": "web", "wait": false}),
            )
            .await;

        assert!(!walk.complete, "nothing has happened yet: {walk:?}");
        assert_eq!(walk.planned.len(), 2, "the plan is still what was accepted");
        assert!(
            walk.reached.is_empty() && walk.failed.is_none(),
            "an empty walk, and it says so through `complete` rather than by looking finished"
        );

        // The walk carries on inside the daemon, which is the whole of what this mode promises.
        daemon.until("web", ServiceState::Running).await;

        daemon.quiet().await;
    }

    #[tokio::test]
    async fn a_dependency_that_fails_stops_the_walk_and_blocks_what_needed_it() {
        let never = FakeService::new().never_ready();
        let specs = Arc::new(fixture::Declared(vec![
            fixture::spec("db")
                .args(fixture::arguments(&never))
                .ready(ReadyCheck::LogPattern {
                    regex: mixengine_testkit::service::READY_LINE.to_owned(),
                    timeout: Millis(750),
                })
                .build()
                .expect("a usable spec"),
            fixture::spec("web")
                .depends_on(fixture::service("db"))
                .build()
                .expect("a usable spec"),
        ]));
        let daemon = daemon(specs, &["db", "web"]).await;

        let walk: ServiceWalk = daemon.expect(rpc::method::SERVICE_START, Value::Null).await;

        assert!(walk.reached.is_empty(), "{walk:?}");
        let failed = walk.failed.clone().expect("the walk stopped somewhere");
        assert_eq!(failed.service, fixture::service("db"));
        assert!(
            matches!(
                failed.reason,
                Some(mixengine_proto::StateReason::ReadyTimeout { .. })
            ),
            "the reason the transition carried, not one invented here: {failed:?}"
        );
        assert_eq!(
            walk.blocked,
            vec![fixture::service("web")],
            "fail-fast: what needed it was never spawned"
        );

        // And the row says the same thing, with the edge that broke named on it.
        assert_eq!(daemon.state("web").await, Some(ServiceState::Failed));

        daemon.quiet().await;
    }

    #[tokio::test]
    async fn stopping_a_service_takes_down_what_depends_on_it_first() {
        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        let _: ServiceWalk = daemon.expect(rpc::method::SERVICE_START, Value::Null).await;

        let walk: ServiceWalk = daemon
            .expect(
                rpc::method::SERVICE_STOP,
                serde_json::json!({"service": "db"}),
            )
            .await;

        assert_eq!(
            walk.planned,
            vec![fixture::service("web"), fixture::service("db")],
            "the opposite walk: a site is never left pointed at a database that is going away"
        );
        assert_eq!(walk.reached, walk.planned);
        assert!(
            walk.failed.is_none(),
            "a stop has no state it fails to reach"
        );

        assert_eq!(daemon.state("db").await, Some(ServiceState::Stopped));
        assert_eq!(daemon.state("web").await, Some(ServiceState::Stopped));

        daemon.quiet().await;
    }

    /// The one that says what `restart` means, and the reason it is not stop-then-start-the-same-id.
    #[tokio::test]
    async fn restarting_a_dependency_puts_back_everything_it_took_down() {
        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        let _: ServiceWalk = daemon.expect(rpc::method::SERVICE_START, Value::Null).await;
        let before = daemon
            .expect::<ServiceSummary>(
                rpc::method::SERVICE_STATUS,
                serde_json::json!({"service": "db"}),
            )
            .await
            .pid;

        let walk: ServiceWalk = daemon
            .expect(
                rpc::method::SERVICE_RESTART,
                serde_json::json!({"service": "db"}),
            )
            .await;

        // Stopping `db` takes `web` with it; starting `db` alone would have left it there.
        assert_eq!(
            walk.planned,
            vec![fixture::service("db"), fixture::service("web")],
            "what the stop covered, in start order: {walk:?}"
        );
        assert_eq!(walk.reached, walk.planned, "{walk:?}");

        let after = daemon
            .expect::<ServiceSummary>(
                rpc::method::SERVICE_STATUS,
                serde_json::json!({"service": "db"}),
            )
            .await;

        assert_eq!(after.state, Some(ServiceState::Running));
        assert_ne!(after.pid, before, "a restart is a different process");
        assert_eq!(
            daemon.state("web").await,
            Some(ServiceState::Running),
            "the dependent came back rather than being left where the stop put it"
        );

        daemon.quiet().await;
    }

    /// The other half of what `restart` means: it puts back what it took down, and nothing else.
    #[tokio::test]
    async fn restarting_a_dependency_leaves_a_dependent_that_was_down_where_it_was() {
        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        // Only `db`. A plan for one service pulls in what it depends on, and `web` depends on `db`
        // rather than the other way about, so `web` is deliberately left stopped.
        let _: ServiceWalk = daemon
            .expect(
                rpc::method::SERVICE_START,
                serde_json::json!({"service": "db"}),
            )
            .await;
        assert_eq!(daemon.state("web").await, Some(ServiceState::Stopped));

        let walk: ServiceWalk = daemon
            .expect(
                rpc::method::SERVICE_RESTART,
                serde_json::json!({"service": "db"}),
            )
            .await;

        assert_eq!(
            walk.planned,
            vec![fixture::service("db")],
            "the stop covered `web` and took nothing down there: {walk:?}"
        );
        assert_eq!(
            daemon.state("web").await,
            Some(ServiceState::Stopped),
            "a restart of a dependency is not a request to start its dependents"
        );

        daemon.quiet().await;
    }

    #[tokio::test]
    async fn starting_a_service_that_is_not_declared_is_not_found_rather_than_a_walk() {
        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        let answer = daemon
            .ask(
                rpc::method::SERVICE_START,
                serde_json::json!({"service": "mailpit"}),
            )
            .await;

        assert_eq!(answer["error"]["data"]["code"], "not_found");
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("mailpit")),
            "{answer}"
        );
    }

    #[tokio::test]
    async fn a_source_that_cannot_answer_is_the_daemons_problem_and_not_the_users() {
        let daemon = daemon(Arc::new(fixture::Unavailable), &[]).await;

        let answer = daemon.ask(rpc::method::SERVICE_LIST, Value::Null).await;

        assert_eq!(answer["error"]["data"]["code"], "internal");
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("not installed")),
            "the source's own complaint survives the trip: {answer}"
        );
    }

    /// T9a's whole claim, in the order it makes it: services down, then the token, then the answer.
    #[tokio::test]
    async fn shutting_down_stops_the_services_before_it_cancels_anything() {
        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        let _: ServiceWalk = daemon.expect(rpc::method::SERVICE_START, Value::Null).await;
        assert_eq!(daemon.state("web").await, Some(ServiceState::Running));

        let shutdown: DaemonShutdown = daemon
            .expect(rpc::method::DAEMON_SHUTDOWN, Value::Null)
            .await;

        assert_eq!(
            shutdown.services.planned,
            vec![fixture::service("web"), fixture::service("db")],
            "dependents first — the same walk `service.stop` does, and not the root token's \
             everything-at-once: {shutdown:?}"
        );
        assert_eq!(shutdown.services.reached, shutdown.services.planned);
        assert!(shutdown.services.complete);
        assert!(shutdown.services.failed.is_none(), "{shutdown:?}");
        assert!(
            shutdown.unordered.is_none(),
            "the order was kept, and a note about one that was not is a note nobody may print: \
             {shutdown:?}"
        );

        // The rows are what say the stop was performed rather than merely planned, and they were
        // written before this answer existed: cancelling first and reporting afterwards would have
        // produced the same struct with the services still going.
        assert_eq!(daemon.state("db").await, Some(ServiceState::Stopped));
        assert_eq!(daemon.state("web").await, Some(ServiceState::Stopped));

        assert!(
            daemon.api.shutdown.token().is_cancelled(),
            "the accept loop is what actually ends the process, and this is what tells it to"
        );
    }

    /// **A shutdown that was begun is finished by the daemon, whatever becomes of whoever asked.**
    ///
    /// The handler is a future hyper holds, and hyper is built here with its default `half_close`,
    /// which closes a connection the moment a read sees end of file rather than waiting for a
    /// response nobody is left to receive. So a `mix daemon stop` that is interrupted — Ctrl-C, the
    /// GUI window closing, the client killed — drops this future where it stands. So does a panic
    /// anywhere inside the walk.
    ///
    /// What that used to leave is the worst state this daemon has: `Registry::stopping_within` has
    /// already latched `shutting_down`, which is never cleared, so every `service.start` from that
    /// moment on is refused — and the token that ends the process is cancelled on a line the future
    /// never reached. A live daemon, its services stopped, starting nothing, for as long as nobody
    /// notices.
    #[tokio::test]
    async fn a_shutdown_whose_client_went_away_still_takes_the_daemon_with_it() {
        use std::future::Future as _;

        let daemon = daemon(web_and_db(), &["db", "web"]).await;

        let _: ServiceWalk = daemon.expect(rpc::method::SERVICE_START, Value::Null).await;
        assert_eq!(daemon.state("web").await, Some(ServiceState::Running));

        {
            // Polled by hand rather than raced against a timer: one poll gets past the point where
            // the shutdown has committed itself and cannot get past the first thing it waits on,
            // which makes "dropped in flight" the fact this test rests on rather than a hope about
            // scheduling. `#[tokio::test]` is a current-thread runtime, so nothing else runs while
            // this poll does and the future cannot finish underneath it.
            let mut shutting = std::pin::pin!(daemon.api.daemon_shutdown());
            let mut polling = std::task::Context::from_waker(std::task::Waker::noop());

            assert!(
                shutting.as_mut().poll(&mut polling).is_pending(),
                "the whole shutdown answered in a single poll, so this test drops nothing and \
                 proves nothing"
            );
        }

        assert!(
            daemon.api.shutdown.token().is_cancelled(),
            "the shutdown latched this daemon shut and then left it running: the accept loop is \
             waiting on a token nothing is going to cancel now"
        );

        // The other half of that state, and the reason the first assertion is worth making: the
        // latch is permanent, so a daemon left like this is one that refuses every start it is ever
        // asked for again. Whoever typed `mix daemon stop` reads the refusal, not the cause.
        let refused: ServiceWalk = daemon.expect(rpc::method::SERVICE_START, Value::Null).await;
        assert!(
            refused.failed.is_some(),
            "a daemon that has begun shutting down starts nothing, which is what makes leaving one \
             running the failure it is: {refused:?}"
        );

        // Waited for by hand, because the drop above landed inside `web`'s stop: `stop_one` takes an
        // entry out of the map before it awaits the task, so the runner it cancelled is one nothing
        // is left holding. It still stops — that is what the cancellation did — and this is what
        // gives it the chance to, since `quiet` below can only drain what is still registered and a
        // `Home` is a temporary directory a live process would keep from being removed.
        daemon.until("web", ServiceState::Stopped).await;
        daemon.quiet().await;
    }

    #[tokio::test]
    async fn a_daemon_with_nothing_declared_still_shuts_down() {
        let daemon = undeclared().await;

        let shutdown: DaemonShutdown = daemon
            .expect(rpc::method::DAEMON_SHUTDOWN, Value::Null)
            .await;

        assert!(shutdown.services.planned.is_empty(), "{shutdown:?}");
        assert!(shutdown.services.complete);
        assert!(
            shutdown.unordered.is_none(),
            "nothing to stop is not a failure to work out how to stop it, and the two answer the \
             same empty walk: {shutdown:?}"
        );
        assert!(daemon.api.shutdown.token().is_cancelled());
    }

    /// A declaration nobody can read is a reason to say so, and not a reason to stay running.
    #[tokio::test]
    async fn a_daemon_that_cannot_say_what_it_declares_still_shuts_down() {
        let daemon = daemon(Arc::new(fixture::Unavailable), &[]).await;

        // Not an error: `service.list` answers `internal` for this same source, because there the
        // question *was* about the services. Here it is about the daemon, and refusing would leave
        // whoever is editing an extension with a daemon they can only kill.
        let shutdown: DaemonShutdown = daemon
            .expect(rpc::method::DAEMON_SHUTDOWN, Value::Null)
            .await;

        assert!(
            shutdown.services.planned.is_empty(),
            "no order could be worked out, and the walk says so rather than claiming one: \
             {shutdown:?}"
        );

        // The half that used to be a line in `daemon.log` and nothing else. Without it this answer
        // is the previous test's — an empty, complete walk — and whoever typed `mix daemon stop`
        // would be told the ordering happened when every runner was cancelled at the same moment.
        let why = shutdown
            .unordered
            .as_ref()
            .unwrap_or_else(|| panic!("the skipped order is reported: {shutdown:?}"));

        assert_eq!(why.code, ErrorCode::Internal);
        assert!(
            why.message.contains("not installed"),
            "the source's own complaint, which is what `service.list` would have said about the \
             same declarations: {why:?}"
        );

        assert!(
            daemon.api.shutdown.token().is_cancelled(),
            "what stops the services then is the root token, which is what a signal does anyway"
        );
    }

    #[tokio::test]
    async fn shutting_down_takes_no_parameters() {
        let daemon = undeclared().await;

        // A client that passed a grace period believes it chose one. The budget is the machine's,
        // from `config.toml`, and a method that ignored the argument would be a client quietly not
        // getting what it asked for.
        let answer = daemon
            .ask(
                rpc::method::DAEMON_SHUTDOWN,
                serde_json::json!({"grace_seconds": 30}),
            )
            .await;

        assert_eq!(answer["error"]["data"]["code"], "invalid_argument");
        assert!(!daemon.api.shutdown.token().is_cancelled(), "{answer}");
    }

    /// **Asking again is asking to hurry**, which is what a second signal already means and what a
    /// second `daemon.shutdown` had no way of saying.
    ///
    /// `main.rs` treats a second console event as the person no longer being willing to wait for the
    /// polite stop: it narrows the budget to nothing, so every runner that has not begun its stop
    /// goes straight to the kill. A second request over the API could not do that — `stopping_within`
    /// only ever narrows, and `now + grace` is always *later* than the deadline the first request
    /// set, so the `min` discarded it and the second caller simply queued behind the first.
    ///
    /// **Which left one platform with no way out at all.** A `--detach`ed daemon on Windows has no
    /// console for any of the five events to arrive on — `mix` autostarts exactly that daemon — so
    /// the API is the only thing that can ask it anything, and asking twice did nothing. The escape
    /// from a shutdown wedged on one service was Task Manager.
    ///
    /// The first shutdown is staged as the latch alone rather than as a walk racing this one: what
    /// is under test is what the second request *grants*, and a competing walk would put its own
    /// claims in the way of measuring it.
    ///
    /// **`Command` and not `Signal`, for the reason every other budget test here gives**: Windows
    /// sends no request to stop at all (ADR 0008), so a `Signal` spec spends no grace period there
    /// and this would pass without the escalation existing — green on the one system that needs it
    /// most.
    #[tokio::test]
    async fn a_second_request_to_stop_is_the_hurry_a_second_signal_would_have_been() {
        let daemon = daemon(slow_to_stop("db"), &["db"]).await;

        let _: ServiceWalk = daemon.expect(rpc::method::SERVICE_START, Value::Null).await;
        assert_eq!(daemon.state("db").await, Some(ServiceState::Running));

        // A shutdown already under way, and one whose budget is no help to anybody: ten minutes is
        // the most `config.toml` accepts, so this is the slowest a first request can legitimately be.
        daemon
            .services
            .stopping_within(std::time::Duration::from_secs(600));

        let began = Instant::now();
        let shutdown: DaemonShutdown = daemon
            .expect(rpc::method::DAEMON_SHUTDOWN, Value::Null)
            .await;
        let took = began.elapsed();

        assert_eq!(
            shutdown.services.reached,
            vec![fixture::service("db")],
            "the service still stopped and the answer still says so — an escalation is a shorter \
             stop, not a skipped one: {shutdown:?}"
        );

        // The stop command never returns, so what this measures is the grace period the second
        // request granted. Escalated it is nothing and the service is killed at once; unescalated it
        // is this daemon's own budget less the drain — eight seconds — and the margin between the
        // two is the whole assertion.
        assert!(
            took < std::time::Duration::from_secs(4),
            "the second request to stop waited {took:?} for a service whose stop command never \
             answers; asking again is what says the polite stop is over"
        );

        assert!(daemon.api.shutdown.token().is_cancelled());
    }

    #[tokio::test]
    async fn a_target_says_nothing_and_means_every_declared_service() {
        // The three spellings of "no parameters" that `no_params` accepts, on a method that does
        // take them: a client sending any of them is asking about everything.
        for params in [Value::Null, serde_json::json!({}), serde_json::json!([])] {
            let daemon = undeclared().await;
            let walk: ServiceWalk = daemon.expect(rpc::method::SERVICE_START, params).await;

            assert!(walk.planned.is_empty(), "nothing is declared: {walk:?}");
            assert!(walk.complete);
        }
    }
}
