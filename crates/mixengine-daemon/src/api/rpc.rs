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
    BlueprintApply, BlueprintCapture, BlueprintImport, BundleReport, CaRotateQuery, CaStatus,
    CaStatusQuery, CaUninstallQuery, CertIssue, CertStatusQuery, DaemonShutdown, DaemonStatus,
    DaemonVersion, DatabaseClientQuery, DatabaseCreate, DatabaseOpen, DiagnosticsBundle,
    DoctorRepair, DomainAdd, DomainRemove, DomainStatusQuery, ElevationDrop, Enforcement, Error,
    ErrorCode, ExtensionChoice, ExtensionInspect, ExtensionInstall, ExtensionPlanRequest,
    ExtensionTarget, ExtensionUninstall, IdleReport, IdleSource, JobFilter, JobKind, JobList,
    JobQuery, JobSummary, JobWait, LimitSupport, MemoryWatchdog, MetricsFrame, MetricsHistory,
    MetricsHistoryQuery, PackageFilter, PackageTarget, ProjectCreate, ProjectQuery, ProjectUpdate,
    ResourceLimits, RuntimeFilter, RuntimeQuestion, RuntimeTarget, RuntimeUninstall, ServiceCreate,
    ServiceDelete, ServiceFailure, ServiceId, ServiceIdleSet, ServiceLimitsReport,
    ServiceLimitsSet, ServiceList, ServiceQuery, ServiceSpec, ServiceSummary, ServiceTarget,
    ServiceWalk, SiteCreate, SiteListQuery, SiteQuery, SiteShare, SiteUpdate, UninstallQuery,
    UpdateApplied, UpdateApply, UpdateCheck, UpdateDecide, UpdateStatus, Uptime,
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
                    encode_result(&api.status().await.map_err(refused)?)
                }

                rpc::method::DAEMON_VERSION => {
                    no_params(params.as_ref())?;
                    encode_result(&api.version())
                }

                rpc::method::DAEMON_SHUTDOWN => {
                    no_params(params.as_ref())?;
                    encode_result(&api.daemon_shutdown().await)
                }

                rpc::method::UPDATE_STATUS => {
                    no_params(params.as_ref())?;
                    encode_result(&api.update_status().await)
                }

                rpc::method::UPDATE_CHECK => {
                    let check: UpdateCheck = arguments(params)?;
                    encode_result(&api.update_check(check).await.map_err(refused)?)
                }

                rpc::method::UPDATE_DECIDE => {
                    let decide: UpdateDecide = arguments(params)?;
                    encode_result(&api.update_decide(decide).await.map_err(refused)?)
                }

                rpc::method::UPDATE_APPLY => {
                    let apply: UpdateApply = arguments(params)?;
                    encode_result(&api.update_apply(apply).await.map_err(refused)?)
                }

                rpc::method::DAEMON_UNINSTALL_PLAN => {
                    let query: UninstallQuery = arguments(params)?;
                    encode_result(&api.uninstall.plan(&query).await.map_err(refused)?)
                }

                rpc::method::DAEMON_UNINSTALL => {
                    let query: UninstallQuery = arguments(params)?;
                    encode_result(&api.uninstall_now(query).await.map_err(refused)?)
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
                    let asked: RuntimeUninstall = arguments(params)?;
                    encode_result(&api.runtimes.uninstall(&asked).await.map_err(refused)?)
                }

                rpc::method::RUNTIME_SET_DEFAULT => {
                    let target: RuntimeTarget = arguments(params)?;
                    encode_result(&api.runtimes.set_default(&target).await.map_err(refused)?)
                }

                rpc::method::RUNTIME_LIST_EXTENSIONS => {
                    let target: RuntimeTarget = arguments(params)?;
                    encode_result(&api.php_extensions.list(&target).await.map_err(refused)?)
                }

                rpc::method::RUNTIME_SET_EXTENSION => {
                    let choice: ExtensionChoice = arguments(params)?;
                    encode_result(&api.php_extensions.set(&choice).await.map_err(refused)?)
                }

                rpc::method::PACKAGE_LIST => {
                    let filter: PackageFilter = arguments(params)?;
                    encode_result(&api.packages.list(&filter).await.map_err(refused)?)
                }

                rpc::method::PACKAGE_LIST_AVAILABLE => {
                    let filter: PackageFilter = arguments(params)?;
                    encode_result(
                        &api.packages
                            .list_available(&filter)
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::PACKAGE_INSTALL => {
                    let target: PackageTarget = arguments(params)?;
                    encode_result(&api.packages.install(&target).await.map_err(refused)?)
                }

                rpc::method::PACKAGE_UNINSTALL => {
                    let target: PackageTarget = arguments(params)?;
                    encode_result(&api.packages.uninstall(&target).await.map_err(refused)?)
                }

                rpc::method::PROJECT_CREATE => {
                    let create: ProjectCreate = arguments(params)?;
                    encode_result(&api.projects.create(&create).await.map_err(refused)?)
                }

                rpc::method::PROJECT_LIST => {
                    no_params(params.as_ref())?;
                    encode_result(&api.projects.list().await.map_err(refused)?)
                }

                rpc::method::PROJECT_SHOW => {
                    let query: ProjectQuery = arguments(params)?;
                    encode_result(&api.projects.show(&query).await.map_err(refused)?)
                }

                rpc::method::PROJECT_UPDATE => {
                    let update: ProjectUpdate = arguments(params)?;
                    encode_result(&api.projects.update(&update).await.map_err(refused)?)
                }

                rpc::method::PROJECT_DELETE => {
                    let query: ProjectQuery = arguments(params)?;
                    encode_result(&api.projects.delete(&query).await.map_err(refused)?)
                }

                rpc::method::PROJECT_EXPORT => {
                    let query: ProjectQuery = arguments(params)?;
                    encode_result(&api.projects.export(&query).await.map_err(refused)?)
                }

                rpc::method::SITE_CREATE => {
                    let create: SiteCreate = arguments(params)?;
                    encode_result(&api.sites.create(&create).await.map_err(refused)?)
                }

                // `arguments` already answers "nothing" with an empty object, and every field of
                // `SiteListQuery` has a default — so `mix site list` with no project named is a
                // whole request without a second helper beside it.
                rpc::method::SITE_LIST => {
                    let query: SiteListQuery = arguments(params)?;
                    encode_result(&api.sites.list(&query).await.map_err(refused)?)
                }

                rpc::method::SITE_SHOW => {
                    let query: SiteQuery = arguments(params)?;
                    encode_result(&api.sites.show(&query).await.map_err(refused)?)
                }

                rpc::method::SITE_UPDATE => {
                    let update: SiteUpdate = arguments(params)?;
                    encode_result(&api.sites.update(&update).await.map_err(refused)?)
                }

                rpc::method::SITE_SHARE => {
                    let request: SiteShare = arguments(params)?;
                    encode_result(
                        &api.sites
                            .share(
                                &request.site,
                                request.interface.as_deref(),
                                request.for_seconds,
                                mixengine_proto::Timestamp::from_system_time(
                                    std::time::SystemTime::now(),
                                ),
                            )
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::SITE_UNSHARE => {
                    let query: SiteQuery = arguments(params)?;
                    api.sites.unshare(&query.site).await.map_err(refused)?;
                    encode_result(&())
                }

                rpc::method::SITE_START => {
                    let query: SiteQuery = arguments(params)?;
                    encode_result(&api.sites.start(&query).await.map_err(refused)?)
                }

                rpc::method::SITE_STOP => {
                    let query: SiteQuery = arguments(params)?;
                    encode_result(&api.sites.stop(&query).await.map_err(refused)?)
                }

                rpc::method::SITE_DELETE => {
                    let query: SiteQuery = arguments(params)?;
                    encode_result(&api.sites.delete(&query).await.map_err(refused)?)
                }

                rpc::method::DAEMON_DOCTOR => encode_result(&api.doctor.report().await),

                rpc::method::BLUEPRINT_CAPTURE => {
                    let capture: BlueprintCapture = arguments(params)?;
                    encode_result(&api.blueprints.capture(&capture).await.map_err(refused)?)
                }

                rpc::method::BLUEPRINT_IMPORT => {
                    let asked: BlueprintImport = arguments(params)?;
                    encode_result(&api.blueprints.import(&asked).await.map_err(refused)?)
                }

                rpc::method::BLUEPRINT_LIST => {
                    no_params(params.as_ref())?;
                    encode_result(&api.blueprints.list().await.map_err(refused)?)
                }

                rpc::method::BLUEPRINT_APPLY => {
                    let apply: BlueprintApply = arguments(params)?;
                    encode_result(&api.blueprint_apply(&apply).await.map_err(refused)?)
                }

                rpc::method::EXTENSION_INSPECT => {
                    let asked: ExtensionInspect = arguments(params)?;
                    encode_result(&api.extensions.inspect(&asked).map_err(refused)?)
                }

                rpc::method::EXTENSION_LIST => {
                    no_params(params.as_ref())?;
                    encode_result(&api.extensions.list().await.map_err(refused)?)
                }

                rpc::method::EXTENSION_AVAILABLE => {
                    no_params(params.as_ref())?;
                    encode_result(&api.extensions.available().await.map_err(refused)?)
                }

                rpc::method::EXTENSION_PLAN => {
                    let asked: ExtensionPlanRequest = arguments(params)?;
                    encode_result(&api.extensions.plan(&asked).await.map_err(refused)?)
                }

                rpc::method::EXTENSION_INSTALL => {
                    let asked: ExtensionInstall = arguments(params)?;
                    encode_result(&api.extensions.install(&asked).await.map_err(refused)?)
                }

                rpc::method::EXTENSION_UNINSTALL => {
                    let asked: ExtensionUninstall = arguments(params)?;
                    encode_result(&api.extensions.uninstall(&asked).await.map_err(refused)?)
                }

                // **The same walk `service.start` takes** — the T81 design's D11. An extension is
                // what somebody installed and its `ServiceId` is an implementation detail of that,
                // so this resolves the one to the other and adds no supervision of its own.
                rpc::method::EXTENSION_START => {
                    let asked: ExtensionTarget = arguments(params)?;
                    let service = api
                        .extensions
                        .service_of(&asked.id)
                        .await
                        .map_err(refused)?;

                    encode_result(
                        &api.service_start(&ServiceTarget {
                            service: Some(service),
                            wait: true,
                        })
                        .await
                        .map_err(refused)?,
                    )
                }

                rpc::method::EXTENSION_STOP => {
                    let asked: ExtensionTarget = arguments(params)?;
                    let service = api
                        .extensions
                        .service_of(&asked.id)
                        .await
                        .map_err(refused)?;

                    encode_result(
                        &api.service_stop(&ServiceTarget {
                            service: Some(service),
                            wait: true,
                        })
                        .await
                        .map_err(refused)?,
                    )
                }

                rpc::method::DAEMON_DOCTOR_REPAIR => {
                    let asked: DoctorRepair = arguments(params)?;
                    encode_result(&api.repairs.run(&asked).await)
                }

                // Through `arguments` rather than `no_params`: `deny_unknown_fields` is what refuses
                // a misspelled option, and an option that bounds what goes into an archive is one a
                // caller must not believe they set.
                rpc::method::DAEMON_BUNDLE => {
                    let _: DiagnosticsBundle = arguments(params)?;
                    encode_result(&api.bundle().await.map_err(refused)?)
                }

                // **The reference is resolved here rather than inside `Certificates`** — T50. That
                // is where `expect` already lives, and a `Certificates` able to resolve one would
                // have to hold the `Sites` that in turn holds it.
                rpc::method::CERT_ISSUE => {
                    let issue: CertIssue = arguments(params)?;

                    let site = match issue.site.as_ref() {
                        Some(reference) => {
                            Some(api.sites.expect(reference).await.map_err(refused)?.0)
                        }
                        None => None,
                    };

                    encode_result(&api.certificates.issue(site).await.map_err(refused)?)
                }

                // **The one route in this file that opens a socket** — roadmap task T53. It
                // asks this home's own front end what it is serving, which is the only question
                // here whose answer is not read out of a file or a row.
                rpc::method::CERT_STATUS => {
                    let query: CertStatusQuery = arguments(params)?;

                    let site = match query.site.as_ref() {
                        Some(reference) => {
                            Some(api.sites.expect(reference).await.map_err(refused)?.0)
                        }
                        None => None,
                    };

                    let port = api.services.front_end_tls_port().await;

                    encode_result(
                        &api.certificates
                            .site_status(site, port)
                            .await
                            .map_err(refused)?,
                    )
                }

                // Through `arguments` rather than `no_params`, for the reason above it:
                // `deny_unknown_fields` is what refuses a misspelled option instead of quietly
                // handing back the default.
                rpc::method::CERT_CA_STATUS => {
                    let _: CaStatusQuery = arguments(params)?;
                    encode_result(&api.ca_status().await.map_err(refused)?)
                }

                // **The two routes in this file that take something away** — roadmap task T54.
                // Jobs, because they wait for an elevation prompt, and a prompt is not something an
                // RPC can block on: the person may take a minute, or walk away.
                rpc::method::CERT_CA_ROTATE => {
                    let _: CaRotateQuery = arguments(params)?;

                    encode_result(
                        &crate::certs::authority::rotate(
                            api.certificates.clone(),
                            Arc::clone(&api.elevation),
                            Arc::clone(&api.services),
                            &api.jobs,
                        )
                        .await
                        .map_err(refused)?,
                    )
                }

                rpc::method::CERT_CA_UNINSTALL => {
                    let _: CaUninstallQuery = arguments(params)?;

                    encode_result(
                        &crate::certs::authority::uninstall(
                            api.certificates.clone(),
                            Arc::clone(&api.elevation),
                            &api.jobs,
                        )
                        .await
                        .map_err(refused)?,
                    )
                }

                rpc::method::DOMAIN_ADD => {
                    let add: DomainAdd = arguments(params)?;
                    encode_result(&api.domains.add(&add).await.map_err(refused)?)
                }

                rpc::method::DOMAIN_REMOVE => {
                    let remove: DomainRemove = arguments(params)?;
                    encode_result(&api.domains.remove(&remove).await.map_err(refused)?)
                }

                rpc::method::DOMAIN_DNS_STATUS => {
                    let query: DomainStatusQuery = arguments(params)?;
                    encode_result(&api.domains.status(&query).await.map_err(refused)?)
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

                // The second capability that writes outside the home, beside `path.*` above and on
                // its rule: only ever when somebody asks. Blocking for those three's reason too — a
                // tool to run and a file to write.
                rpc::method::AUTOSTART_STATUS => {
                    no_params(params.as_ref())?;
                    let autostart = Arc::clone(&api.autostart);
                    encode_result(
                        &on_a_blocking_thread(move || autostart.status())
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::AUTOSTART_ENABLE => {
                    no_params(params.as_ref())?;
                    let autostart = Arc::clone(&api.autostart);
                    encode_result(
                        &on_a_blocking_thread(move || autostart.enable())
                            .await
                            .map_err(refused)?,
                    )
                }

                rpc::method::AUTOSTART_DISABLE => {
                    no_params(params.as_ref())?;
                    let autostart = Arc::clone(&api.autostart);
                    encode_result(
                        &on_a_blocking_thread(move || autostart.disable())
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

                rpc::method::DATABASE_CREATE => {
                    let create: DatabaseCreate = arguments(params)?;
                    encode_result(&api.databases.create(&create).await.map_err(refused)?)
                }

                rpc::method::DATABASE_CLIENT => {
                    let asked: DatabaseClientQuery = arguments(params)?;
                    encode_result(&api.databases.client(&asked).await.map_err(refused)?)
                }

                rpc::method::DATABASE_OPEN => {
                    let asked: DatabaseOpen = arguments(params)?;
                    encode_result(&api.databases.open(&asked).await.map_err(refused)?)
                }

                rpc::method::SERVICE_CREATE => {
                    let create: ServiceCreate = arguments(params)?;
                    encode_result(&api.service_create(&create).await.map_err(refused)?)
                }

                rpc::method::SERVICE_LIMITS => {
                    let target: ServiceTarget = arguments(params)?;
                    encode_result(&api.service_limits(&target).await.map_err(refused)?)
                }

                rpc::method::SERVICE_SET_LIMITS => {
                    let asked: ServiceLimitsSet = arguments(params)?;
                    encode_result(&api.service_set_limits(&asked).await.map_err(refused)?)
                }

                rpc::method::SERVICE_IDLE => {
                    let target: ServiceTarget = arguments(params)?;
                    encode_result(&api.service_idle(&target).await.map_err(refused)?)
                }

                rpc::method::SERVICE_SET_IDLE => {
                    let asked: ServiceIdleSet = arguments(params)?;
                    encode_result(&api.service_set_idle(&asked).await.map_err(refused)?)
                }

                rpc::method::SERVICE_DELETE => {
                    let asked: ServiceDelete = arguments(params)?;
                    encode_result(
                        &api.service_delete(&asked.target.service, asked.force)
                            .await
                            .map_err(refused)?,
                    )
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

                rpc::method::METRICS_SNAPSHOT => {
                    no_params(params.as_ref())?;
                    encode_result(&api.metrics_snapshot().await.map_err(refused)?)
                }

                rpc::method::METRICS_HISTORY => {
                    let query: MetricsHistoryQuery = arguments(params)?;
                    encode_result(&api.metrics_history(&query).await.map_err(refused)?)
                }

                rpc::method::JOB_CANCEL => {
                    let query: JobQuery = arguments(params)?;
                    encode_result(&api.jobs.cancel(query.job).await.map_err(refused)?)
                }

                rpc::method::ELEVATION_STATUS => {
                    no_params(params.as_ref())?;
                    encode_result(&api.elevation.status().await.map_err(refused)?)
                }

                rpc::method::ELEVATION_GRANT => {
                    no_params(params.as_ref())?;
                    encode_result(&api.elevation.grant().await.map_err(refused)?)
                }

                rpc::method::ELEVATION_DROP => {
                    let asked: ElevationDrop = arguments(params)?;
                    encode_result(&api.elevation.drop_pending(&asked).await.map_err(refused)?)
                }

                rpc::method::ELEVATION_UPGRADE => {
                    no_params(params.as_ref())?;
                    encode_result(
                        &crate::helper::upgrade(&api.elevation, &api.updates, api.paths())
                            .await
                            .map_err(refused)?,
                    )
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
            // The panic message itself has already gone to the log through the panic hook — which
            // is `crate::crash`'s, installed at `main`, and which also left a report in
            // `logs/crashes/`. Until T91 built it there was no hook at all and this sentence was
            // true of nothing: the default one writes to stderr, and a `--detach`ed daemon's stderr
            // is the null device. It is still not repeated to the client — a backtrace is not
            // something a user can act on, and the daemon's log is where it belongs.
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
pub(crate) async fn on_a_blocking_thread<T, F>(work: F) -> Result<T, Error>
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
    ///
    /// **Fallible since T40b**, and `daemon.version` is what stays infallible: how many operations
    /// are waiting is a read of the queue, and there is no honest number to report when that read
    /// fails. Reporting zero would be the stale-clear failure D6 exists to prevent — a healthy
    /// machine that is missing its hosts entries — and the handshake a client makes before it trusts
    /// anything here is `daemon.version`, which touches nothing.
    ///
    /// # Errors
    ///
    /// The wire error of a queue that could not be read.
    async fn status(&self) -> Result<DaemonStatus, Error> {
        Ok(DaemonStatus {
            version: self.version.to_owned(),
            protocol: self.protocol,
            pid: self.pid,
            home: self.home.clone(),
            endpoint: self.endpoint.clone(),
            database: self.database.clone(),
            started_at: self.started.at(),
            uptime: Uptime::from_duration(self.started.elapsed()),
            // Always `Some` from a daemon that has the members at all — they are `Option` for the
            // wire, so an older daemon's answer decodes, and never to say "this build did not
            // manage to find out". A queue that cannot be read fails the call above.
            elevation: Some(self.elevation.summary().await?),
            dns: Some(self.dns.status()),
            update: self.updates.offer().await,
        })
    }

    /// What this home's certificate authority is (T48).
    ///
    /// Reads, and never makes: the making happens once at start, so that T49's trust-store install
    /// falls inside the same first-run elevation batch as the resolver and the port grant. A method
    /// that generated on demand would move that install behind a prompt of its own.
    async fn ca_status(&self) -> Result<CaStatus, Error> {
        self.certificates.status().await
    }

    /// `daemon.bundle` — one diagnostics archive, written into this home — roadmap task **T93**.
    ///
    /// **The status is read here and handed over, its failure included.** `status` is fallible and a
    /// bundle is wanted exactly when things are failing, so a queue that would not answer becomes an
    /// omission inside the archive rather than the end of the call.
    ///
    /// # Errors
    ///
    /// The wire error of an archive that could not be written.
    async fn bundle(&self) -> Result<BundleReport, Error> {
        let report = self.doctor.report().await;
        let status = self.status().await;

        self.bundles.take(&report, status, self.version()).await
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

    /// `update.status` — what this daemon knows about updating itself, touching no network.
    async fn update_status(&self) -> UpdateStatus {
        self.updates.status(self.running_services().await).await
    }

    /// `update.check` — read the published feed now.
    ///
    /// # Errors
    ///
    /// The transport failure of a machine that is offline with nothing cached to fall back to.
    /// **Which the background callers never see**, because they never call this: they call
    /// `Updates::check` directly and swallow it. This is the path somebody typed a command on.
    async fn update_check(&self, check: UpdateCheck) -> Result<UpdateStatus, Error> {
        self.updates
            .check(check.force, self.running_services().await)
            .await
    }

    /// `update.decide` — remember *skip this version* or *remind me later*.
    async fn update_decide(&self, decide: UpdateDecide) -> Result<UpdateStatus, Error> {
        self.updates
            .decide(
                &decide.version,
                decide.decision,
                self.running_services().await,
            )
            .await
    }

    /// `update.apply` — install it, and then stop.
    ///
    /// **The order is download → verify → unpack → smoke → stop → swap → answer → exit**, and the
    /// first four happening before the stop is this task's one departure from
    /// `.claude/features/updates.md`'s wording (the T88 design, D5): a download that fails after the
    /// stop has cost an outage, and one that succeeds could have happened while everything was still
    /// up. What is down is the swap and the restart, which is seconds.
    ///
    /// **A swap that fails leaves a working daemon**, which is why the [`Going`](super::Going) guard
    /// is taken *after* the swap and not before it. The rollback puts the binaries back, starts the
    /// services the stop stopped — nothing else on the machine intends to — clears the records, and
    /// returns the failure. This daemon is then the daemon it was before the attempt.
    ///
    /// **A client that goes away mid-apply does not stop the apply**, on `daemon_shutdown`'s rule
    /// and for its reason: abandoning the work between the stop and the swap would leave a home with
    /// everything down and half its binaries renamed. Before the stop there is nothing to protect —
    /// an abandoned download leaves a `.part` file, which is what the next attempt resumes from.
    ///
    /// # Errors
    ///
    /// `precondition_failed` when the version asked for is not the one offered, or when this copy of
    /// MixEngine was installed by something that is not MixEngine; and whatever the download, the
    /// checksum, the unpacking, the smoke test or the swap reported.
    async fn update_apply(&self, apply: UpdateApply) -> Result<UpdateApplied, Error> {
        // Everything that can refuse, before anything is stopped.
        let staged = self.updates.stage(&apply.version).await?;

        let stopped = self.stop_everything().await.map_err(|error| {
            // The services are still running: `stop_everything` builds the plan before it walks it,
            // and this is the failure to build one.
            tracing::warn!(%error, "an update could not work out the order to stop services in");
            error
        })?;

        self.updates.remember(&staged.to, &stopped.reached).await?;

        let swapped = match mixengine_core::updates::apply::swap(
            &staged.staged,
            &staged.provides,
            &staged.directory,
        ) {
            Ok(swapped) => swapped,

            Err(error) => {
                tracing::warn!(
                    %error,
                    "an update was rolled back; starting again what it stopped"
                );

                // The binaries are already back — `swap` unwinds its own renames. What is left is
                // the stop, which nothing else on this machine is going to undo.
                self.updates.roll_back(&self.services).await;

                return Err(error.to_wire());
            }
        };

        // **Taken here and not earlier**, which is the whole of the paragraph above: from this line
        // on there is no way of leaving this daemon running, and every path that could still have
        // failed is behind it.
        let _going = self.shutdown.begun();

        tracing::info!(
            from = env!("CARGO_PKG_VERSION"),
            to = %staged.to,
            replaced = ?swapped.replaced,
            kept = ?swapped.kept,
            restarting = stopped.reached.len(),
            "this daemon has been replaced and is stopping so the new one can start"
        );

        Ok(crate::updates::applied(&staged, &swapped, stopped.reached))
    }

    /// The services an update would stop and start again.
    ///
    /// **What is running now**, which is what the stop walk will reach. Reported so a consent prompt
    /// can say *"3 services will be stopped and started again"* — `.claude/features/updates.md`'s
    /// *"never update while a supervised service is under load without asking"* in the only form
    /// that rule can take once consent is always required.
    async fn running_services(&self) -> Vec<ServiceId> {
        self.services.supervised().into_iter().collect()
    }

    /// `daemon.uninstall` — one job, and this daemon's own end when the home goes with it.
    ///
    /// **The guard that ends the daemon is taken after the job, not around it.** A grant nobody
    /// answers keeps the job open for as long as the dialog is on the screen, and a `Going` held
    /// across that wait would end the daemon the moment the wait gave up — with the prompt still on
    /// the person's screen. So this waits for the job to finish, and only then asks whether anything
    /// was armed: a declined grant arms nothing, the daemon stays up, and the same command works when
    /// the person is ready (the T87 design, D9).
    async fn uninstall_now(&self, query: UninstallQuery) -> Result<JobSummary, Error> {
        let uninstall = Arc::clone(&self.uninstall);
        let started = self
            .jobs
            .begin(
                &JobKind::parse(rpc::method::DAEMON_UNINSTALL).expect("a valid kind"),
                move |handle| async move {
                    let report = uninstall.run(&query, &handle).await?;

                    serde_json::to_value(report).map_err(|error| {
                        Error::new(
                            ErrorCode::Internal,
                            format!("an uninstall report could not be encoded: {error}"),
                        )
                    })
                },
            )
            .await?;

        let jobs = Arc::clone(&self.jobs);
        let armed = Arc::clone(&self.armed);
        let token = self.shutdown.token().clone();
        let id = started.id;

        tokio::spawn(async move {
            // In long steps rather than one deadline, because what this is waiting on has none: the
            // job ends when a person answers a prompt. Every other end of the wait — a job that was
            // cancelled, a row that has gone — leaves nothing armed and therefore ends nothing.
            const STEP: mixengine_proto::Millis = mixengine_proto::Millis(60_000);

            loop {
                match jobs.wait(id, STEP).await {
                    Ok(summary) if summary.state.is_finished() => break,
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(job = %id, %error, "an uninstall could not be followed");
                        return;
                    }
                }
            }

            if armed.is_empty() {
                return;
            }

            tracing::info!(
                "this home has been removed from this machine; the daemon is stopping so its own \
                 directory can go with it"
            );

            token.cancel();
        });

        Ok(started)
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

    /// `metrics.snapshot` — every subject's reading, taken now.
    ///
    /// **A reading rather than the last one taken**, which is what the method's own documentation
    /// promises: between clients this daemon samples once a minute, so the cached tick would answer
    /// a person with a number up to a minute old and would not mention a service that started ten
    /// seconds ago. Reuse of a reading younger than a second happens inside the loop, which is the
    /// only thing that measures.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Internal`] when the sampling loop is gone, which happens only while this daemon
    /// is shutting down — and is worth saying rather than answering with an empty frame that would
    /// read as a machine using nothing.
    async fn metrics_snapshot(&self) -> Result<MetricsFrame, Error> {
        self.metrics.snapshot().await.ok_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                "this daemon is no longer measuring anything",
            )
            .with_hint("it is shutting down; ask the one that starts next")
        })
    }

    /// `metrics.history` — the 24-hour history, one row per subject per minute.
    ///
    /// # Errors
    ///
    /// Whatever [`mixengine_core::metrics::history`] reports when the table cannot be read.
    async fn metrics_history(&self, query: &MetricsHistoryQuery) -> Result<MetricsHistory, Error> {
        mixengine_core::metrics::history(&self.store, query, self.metrics.retention_hours())
            .await
            .map_err(|error| error.to_wire())
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

    /// `service.limits` — what this service may take, and what this machine will do about it.
    ///
    /// **Both halves in one answer**, because neither is worth having alone: a `memory_mb` of 512
    /// means one thing where it is a commit charge enforced by a failed allocation and another where
    /// it is charged pages enforced by the OOM killer — and a third where it is stored and enforced
    /// by nothing. The T68 design, D2.
    async fn service_limits(&self, target: &ServiceTarget) -> Result<ServiceLimitsReport, Error> {
        let id = self.named_service(target.service.as_ref())?;
        let spec = self.spec_of(&id).await?;

        let support = self.elevation.host().resource_control().support();

        let watchdog = watchdog_of(&support, spec.limits(), &spec, self.memory_over_minutes);

        Ok(ServiceLimitsReport {
            service: id,
            limits: spec.limits(),
            support,
            watchdog,
        })
    }

    /// `service.idle` — when this service would be stopped for being unused, and what is stopping
    /// that from happening right now.
    ///
    /// **Four answers rather than one**, which is the whole reason this method exists beside the
    /// setting it reports: no policy, a policy switched off here, a running dependent, and a
    /// keep-warm project all look identical from outside — a service that stays running. Only two
    /// of them are settings anybody can change, and telling a person to go and change the wrong one
    /// is worse than saying nothing. Roadmap task **T69**.
    async fn service_idle(&self, target: &ServiceTarget) -> Result<IdleReport, Error> {
        let id = self.named_service(target.service.as_ref())?;
        let spec = self.spec_of(&id).await?;

        let column = mixengine_core::services::idle_minutes(&self.store, &id)
            .await
            .map_err(|error| error.to_wire())?;

        // The recipe's own default, asked of the same catalogue the generator renders with, so this
        // report and the policy actually in force cannot disagree about what "unset" means.
        let recipe_default = crate::services::catalogue()
            .recipe(id.name())
            .and_then(|recipe| recipe.idle_default());

        Ok(IdleReport {
            source: IdleSource::of(column, recipe_default, spec.idle()),
            policy: spec.idle().cloned(),
            exempt: self.exemptions_for(&id).await,
            service: id,
        })
    }

    /// `service.set_idle` — replace how long this service may look idle before it is stopped.
    ///
    /// **Nothing is applied here, unlike `service.set_limits` beside it.** A limit is a property of
    /// a running process and has to reach one; an idle policy is a statement about a future sweep,
    /// and the next one reads the row. So a service already past its new policy is not stopped by
    /// the call that set it — it is stopped by the sweep that follows, which is also what a person
    /// can watch happen.
    async fn service_set_idle(&self, asked: &ServiceIdleSet) -> Result<IdleReport, Error> {
        let id = asked.service.clone();

        // Refused for a service nothing declares before anything is written, on `set_limits`'
        // reasoning: a setting accepted for a name nobody has is a row nobody can read back.
        let _ = self.spec_of(&id).await?;

        mixengine_core::services::set_idle(&self.store, &id, asked.minutes)
            .await
            .map_err(|error| error.to_wire())?;

        self.service_idle(&ServiceTarget {
            service: Some(id),
            wait: false,
        })
        .await
    }

    /// What is holding `id` open right now, whatever its policy says.
    ///
    /// **Reads the same two things the sweeper reads, through the same function**, so the report
    /// and the decision cannot drift: a client told nothing exempts a service, and a sweeper that
    /// then declines to stop it, would be two answers to one question.
    ///
    /// An empty answer on a failure to read either, which is the one place this differs from the
    /// sweeper: there, an unreadable keep-warm table skips the whole sweep because acting on it
    /// could stop the wrong service; here nothing is being acted on, and a report that failed
    /// entirely because one of its four answers was unavailable would be less useful than one
    /// missing that answer.
    async fn exemptions_for(&self, id: &ServiceId) -> Vec<mixengine_proto::IdleExemption> {
        let Ok(graph) = self.services.graph().await else {
            return Vec::new();
        };

        let Ok(warm) = mixengine_core::projects::kept_warm(&self.store).await else {
            return Vec::new();
        };

        crate::services::idle::exemptions(&graph, id, &self.services.supervised(), &warm)
    }

    /// The one service a `service.limits` call is about, or a refusal naming what is missing.
    ///
    /// **A limit belongs to one service and there is no sensible answer for "all of them"**, which
    /// is `service.status`'s reasoning: a call with no subject is one that was typed wrongly, and
    /// answering it as a list would hide that.
    fn named_service(&self, service: Option<&ServiceId>) -> Result<ServiceId, Error> {
        service.cloned().ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidArgument,
                "which service's limits? name one",
            )
            .with_hint("`mix service limits <service>`")
        })
    }

    /// The declared spec for `id`, which is where a service's limits actually live.
    ///
    /// **The spec and not the `services` row**, although `limits_json` is a column on that row: the
    /// row is the *input* to configuration generation and the spec is what came out of it, so the
    /// spec is what the supervisor will actually apply. Reading the column directly would be a
    /// second path to one answer.
    async fn spec_of(&self, id: &ServiceId) -> Result<ServiceSpec, Error> {
        let graph = self
            .services
            .graph()
            .await
            .map_err(|error| error.to_wire())?;

        graph.spec(id).cloned().ok_or_else(|| {
            mixengine_core::Error::Graph(GraphError::NoSuchService { id: id.clone() }).to_wire()
        })
    }

    /// `service.set_limits` — replace what this service may take, and apply it now.
    ///
    /// Four steps, in an order each of which is why the next one is safe:
    ///
    /// 1. **Refuse what is wrong on any machine.** `ResourceLimits::validate`, beside the type.
    /// 2. **Refuse what is wrong on *this* machine** — a `cpu_percent` above `100 × cores`. Here and
    ///    not in `mixengine-proto`, because the number it is measured against is a property of the
    ///    machine and proto has no host to ask. The T68 design, D10.
    ///
    ///    **It guards small machines and nothing else, and that is worth knowing before reading it
    ///    as more than it is**: `cpu_percent` is a `u8`, so the largest value anybody can express is
    ///    255, which is already below the ceiling on any machine with three cores or more. What this
    ///    catches is a one-core VM or a constrained container being asked for two cores' worth. The
    ///    rule stays because it is correct and costs one comparison — not because it fires often.
    /// 3. **Write the row**, which is what a stopped service's next spawn will read.
    /// 4. **Push it at the runner**, which writes it into the live process. A service that is not
    ///    running skips this and has lost nothing — step 3 already said everything.
    async fn service_set_limits(
        &self,
        asked: &ServiceLimitsSet,
    ) -> Result<ServiceLimitsReport, Error> {
        let id = asked.service.clone();

        // Refused before anything is written, and refused for a service that does not exist before
        // that: a limit accepted for a name nothing declares would be a row nobody could read back.
        // The spec is kept since T71a: whether a watchdog would restart this service is its answer.
        let spec = self.spec_of(&id).await?;

        asked.limits.validate().map_err(|reason| {
            Error::new(ErrorCode::InvalidArgument, reason)
                .with_hint("leave `memory_mb` unset to run this service uncapped")
        })?;

        let support = self.elevation.host().resource_control().support();
        let ceiling = support.cores.saturating_mul(100);

        if let Some(percent) = asked.limits.cpu_percent
            && u32::from(percent) > ceiling
        {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                format!(
                    "`cpu_percent` is a percentage of one core, and this machine has                      {} — so {ceiling} is the whole of it",
                    support.cores
                ),
            )
            .with_hint("a ceiling above the machine's own is not a ceiling"));
        }

        mixengine_core::services::set_limits(&self.store, &id, asked.limits)
            .await
            .map_err(|error| error.to_wire())?;

        self.services.set_limits(&id, asked.limits);

        let watchdog = watchdog_of(&support, asked.limits, &spec, self.memory_over_minutes);

        Ok(ServiceLimitsReport {
            service: id,
            limits: asked.limits,
            support,
            watchdog,
        })
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
    /// [`services::restarted`].
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
        let roots =
            services::restarted(target.service.as_ref(), &down, &self.services.supervised());

        // Cannot fail: every id in it came out of this same graph. Mapped rather than unwrapped all
        // the same, because a panic here would be one bad request taking the daemon with it.
        let up = graph
            .start_plan(roots.iter())
            .map_err(|error| mixengine_core::Error::Graph(error).to_wire())?;

        self.walk(
            target.wait,
            up.flat().cloned().collect(),
            move |services| async move {
                // **The walk itself is `services::stop_then_start`**, shared with the memory
                // watchdog since T71a: what is left here is the reporting this method owes a client
                // and the target semantics only an RPC has.
                (
                    services::stop_then_start(&services, &graph, &down, &up).await,
                    "restart",
                )
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

/// What is watching this service's ceiling, if anything — roadmap task **T71a**.
///
/// **[`None`] on two different machines, and it is the same answer for both**: one whose kernel
/// holds the ceiling itself, and one where this service declared no ceiling at all. In each case
/// there is no loop to describe, and a client drawing a watchdog would be describing something that
/// never runs.
///
/// `restarts` is the *spec's* answer, which is the recipe's: a person may set `memory_mb` on
/// anything, and what happens at the end of the count is a property of the program.
fn watchdog_of(
    support: &LimitSupport,
    limits: ResourceLimits,
    spec: &ServiceSpec,
    after_minutes: u32,
) -> Option<MemoryWatchdog> {
    let watched = limits.memory_mb.is_some() && !matches!(support.memory, Enforcement::Hard { .. });

    watched.then(|| MemoryWatchdog {
        after_minutes,
        restarts: spec.restart_over_memory(),
    })
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

/// One service, as the three readings that know about it describe it.
///
/// `record` is [`None`] for a service that is declared and has no `services` row. That is not a case
/// a finished MixEngine reaches — from T30 a declaration is rendered *from* a row — and it is
/// reported rather than smoothed into `stopped`, because a service that claims to be stopped and
/// then refuses to start explains nothing to whoever declared it.
pub(super) fn summary(
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
        // The row's column, which is where the number was decided — not the spec's list, which is
        // what that number was rendered into. A service with no row has no port to report, exactly
        // as it has no state.
        port: record.and_then(|record| record.port),
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

    use mixengine_proto::rpc::ResponseOutcome;
    use mixengine_proto::{
        AutostartReport, Millis, PathReport, ReadyCheck, ServiceState, StopBehaviour,
    };
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
        matches!(response.outcome, ResponseOutcome::Success { .. })
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

        // Before the registry, which takes it: a start may have a first-run ritual to perform, and
        // that is a job — roadmap task T33.
        let jobs = Arc::new(crate::jobs::Jobs::new(
            &store,
            events.clone(),
            CancellationToken::new(),
        ));

        let services = Arc::new(services::Registry::new(
            &paths,
            &store,
            Arc::new(mixengine_platform::mock::Host::with_home(paths.root())),
            events.clone(),
            specs,
            CancellationToken::new(),
            Arc::clone(&jobs),
        ));

        // Pointed at the published index, which nothing in this file asks anything of: these tests
        // are about dispatch, and every `runtime.*` method's own behaviour is proved against a
        // `MockRegistry` in `tests/runtimes.rs`, where there is a real socket to serve one over.
        // Constructing it here is still worth doing rather than stubbing — it is the one assertion
        // available that a daemon builds one at all without reaching the network to do it.
        let fetcher =
            crate::runtimes::Fetcher::new(&paths, &crate::runtimes::IndexSource::default())
                .expect("the compiled-in index key is a key");
        let runtimes = crate::runtimes::Runtimes::new(
            &paths,
            &store,
            Arc::clone(&jobs),
            Arc::clone(&fetcher),
            Arc::clone(&services),
        );
        let packages = crate::packages::Packages::new(&paths, &store, Arc::clone(&jobs), fetcher);

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

        let autostart = Arc::new(crate::autostart::Autostart::new(
            installed.join(format!("mixengined{}", std::env::consts::EXE_SUFFIX)),
            paths.root().to_path_buf(),
            Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
        ));

        let elevation = crate::elevation::Elevation::new(
            &paths,
            &store,
            events.clone(),
            Arc::clone(&jobs),
            Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
            crate::elevation::Candidates {
                program: installed.join(format!("mixengined{}", std::env::consts::EXE_SUFFIX)),
                // A machine with no installed helper, stated rather than inherited — T85's D5. The
                // machine running these tests may well have one, and every assertion here is about
                // the copy this fixture puts beside its own `mixengined`.
                installed: None,
            },
            Arc::new(crate::dns::Dns::hosts_only_for_tests()),
        );

        let sites = crate::sites::Sites::new(
            &store,
            Arc::clone(&elevation),
            Arc::clone(&services),
            &paths,
            Arc::new(crate::mdns::Mdns::silent_for_tests()),
            crate::api::Events::new(),
        );

        let armed = Arc::new(super::super::Armed::default());

        let api = Arc::new(Api {
            version: "0.1.0",
            protocol: mixengine_proto::PROTOCOL_VERSION,
            pid: 4123,
            // The shipped default, so what these tests read is what a home reads.
            memory_over_minutes: mixengine_core::config::Services::default().memory_over_minutes,
            home: paths.root().display().to_string(),
            endpoint: "/tmp/mixengine/run/mixengined.sock".to_owned(),
            database: paths.database_file().display().to_string(),
            paths: paths.clone(),
            jobs: Arc::clone(&jobs),
            runtimes,
            php_extensions: crate::php_extensions::Extensions::new(
                &paths,
                &store,
                Arc::clone(&services),
            ),
            extensions: Arc::new(crate::extensions::Extensions::new(
                paths.clone(),
                store.clone(),
                Arc::clone(&jobs),
                Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
                mixengine_core::extensions::registry::client(
                    &crate::runtimes::IndexSource::default().registry_url(),
                    &crate::runtimes::IndexSource::default().public_key,
                    paths.cache(),
                )
                .expect("the compiled-in registry key is a key"),
                Arc::clone(&sites),
                Arc::clone(&services),
            )),
            packages,
            projects: crate::projects::Projects::new(&store),
            databases: crate::databases::Databases::new(
                Arc::clone(&services),
                Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
                store.clone(),
            ),
            blueprints: crate::blueprints::Blueprints::new(&store, &paths, "0.1.0"),
            sites: Arc::clone(&sites),
            doctor: crate::doctor::Doctor::new(
                &store,
                Arc::new(crate::dns::Dns::hosts_only_for_tests()),
                Arc::clone(&elevation),
                Arc::clone(&services),
                crate::domains::Domains::new(
                    Arc::clone(&sites),
                    &store,
                    Arc::new(crate::dns::Dns::hosts_only_for_tests()),
                    Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
                ),
                &paths,
                crate::crash::Reports::new(&paths, true),
            ),
            repairs: {
                let doctor = crate::doctor::Doctor::new(
                    &store,
                    Arc::new(crate::dns::Dns::hosts_only_for_tests()),
                    Arc::clone(&elevation),
                    Arc::clone(&services),
                    crate::domains::Domains::new(
                        Arc::clone(&sites),
                        &store,
                        Arc::new(crate::dns::Dns::hosts_only_for_tests()),
                        Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
                    ),
                    &paths,
                    crate::crash::Reports::new(&paths, true),
                );

                crate::repair::Repairs::new(
                    doctor,
                    Arc::clone(&elevation),
                    Arc::clone(&services),
                    &store,
                    &paths,
                )
            },
            bundles: crate::diagnostics::Bundles::new(
                Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
                &paths,
            ),
            armed: Arc::clone(&armed),
            uninstall: crate::uninstall::Uninstall::new(
                &store,
                Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
                crate::uninstall::Doors {
                    dns: Arc::new(crate::dns::Dns::hosts_only_for_tests()),
                    services: Arc::clone(&services),
                    shims: Arc::clone(&shims),
                    autostart: Arc::clone(&autostart),
                    elevation: Arc::clone(&elevation),
                    certificates: crate::certs::Certificates::issuing(
                        &paths,
                        Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
                        store.clone(),
                    ),
                    armed: Arc::clone(&armed),
                },
                &paths,
            ),
            certificates: crate::certs::Certificates::issuing(
                &paths,
                Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
                store.clone(),
            ),
            domains: crate::domains::Domains::new(
                sites,
                &store,
                Arc::new(crate::dns::Dns::hosts_only_for_tests()),
                Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
            ),
            shims,
            autostart,
            // Pointed at a URL nothing answers on, which is the state these tests want: every one
            // of them asks what a daemon that has never read a feed says, and a fixture that could
            // reach the published one would be a test suite with an opinion about the network.
            updates: crate::updates::Updates::new(
                &paths,
                &store,
                &crate::updates::FeedSource {
                    url: "http://127.0.0.1:1/latest.json".to_owned(),
                    public_key: mixengine_core::updates::PUBLIC_KEY.to_owned(),
                },
                Some(&installed.join(format!("mixengined{}", std::env::consts::EXE_SUFFIX))),
                events.clone(),
            )
            .expect("the compiled-in updater key is a key"),
            elevation,
            dns: Arc::new(crate::dns::Dns::hosts_only_for_tests()),
            // A sampler whose loop is never started: these tests answer method calls, and a snapshot
            // taken here would be this test binary's own reading rather than a home's.
            metrics: crate::metrics::sampler::Sampler::new(
                store.clone(),
                Arc::clone(&host) as Arc<dyn mixengine_platform::Host>,
                crate::metrics::watchers::Watchers::new(),
                &mixengine_core::config::Metrics::default(),
            )
            .handle(),
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

    /// A daemon that declares nothing, which is every home with no `services` rows in it.
    async fn undeclared() -> Daemon {
        daemon(Arc::new(fixture::Declared(Vec::new())), &[]).await
    }

    /// One call against a daemon with nothing declared, decoded.
    async fn call(body: &str) -> Value {
        undeclared().await.call(body).await
    }

    #[tokio::test]
    async fn a_history_read_answers_with_what_this_home_keeps() {
        let answer = call(r#"{"jsonrpc":"2.0","method":"metrics.history","id":1}"#).await;

        assert_eq!(answer["result"]["retention_hours"], 24);
        assert_eq!(
            answer["result"]["minutes"].as_array().map(Vec::len),
            Some(0),
            "a home that has measured nothing has no rows, which is an answer and not a failure"
        );
    }

    #[tokio::test]
    async fn a_history_read_refuses_a_subject_it_cannot_read() {
        let answer = call(
            r#"{"jsonrpc":"2.0","method":"metrics.history","params":{"subject":"nonsense"},"id":1}"#,
        )
        .await;

        assert_eq!(answer["error"]["code"], -32602);
        assert_eq!(answer["error"]["data"]["code"], "invalid_argument");
    }

    #[tokio::test]
    async fn a_snapshot_with_no_loop_behind_it_says_so_rather_than_waiting() {
        // The fixture builds a sampler and keeps only its handle, which is the shape a daemon has
        // while it is shutting down. The answer has to be an error: an empty frame would read as a
        // machine using nothing.
        let answer = call(r#"{"jsonrpc":"2.0","method":"metrics.snapshot","id":1}"#).await;

        assert_eq!(answer["error"]["data"]["code"], "internal");
        assert!(
            answer["error"]["data"]["hint"]
                .as_str()
                .is_some_and(|hint| hint.contains("shutting down"))
        );
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

    /// T87. The plan is a read: it answers on a home with nothing in it, it names every row, and it
    /// asks for nothing — a home whose queue is still empty afterwards is the assertion that it
    /// enqueued nothing.
    #[tokio::test]
    async fn the_uninstall_plan_answers_every_row_and_asks_for_nothing() {
        let daemon = undeclared().await;

        let answer = daemon
            .ask(rpc::method::DAEMON_UNINSTALL_PLAN, Value::Null)
            .await;

        let report: mixengine_proto::UninstallReport =
            serde_json::from_value(answer["result"].clone())
                .unwrap_or_else(|error| panic!("{error}: {answer}"));

        // Eleven rows before any relocation, and this fixture relocates nothing.
        assert_eq!(report.items.len(), 11, "{report:?}");

        for item in &report.items {
            assert!(!item.what.is_empty(), "{item:?}");
            assert!(!item.location.is_empty(), "{item:?}");
            assert!(
                !matches!(item.outcome, mixengine_proto::Removal::Removed { .. }),
                "a plan removes nothing: {item:?}"
            );
        }

        // Every id exactly once, which is what a client renders against.
        let mut ids: Vec<mixengine_proto::ResidueId> =
            report.items.iter().map(|item| item.id).collect();
        let counted = ids.len();
        ids.sort_by_key(|id| format!("{id:?}"));
        ids.dedup();
        assert_eq!(ids.len(), counted, "{report:?}");

        let waiting = daemon
            .expect::<mixengine_proto::ElevationStatus>(rpc::method::ELEVATION_STATUS, Value::Null)
            .await;

        assert!(waiting.pending.is_empty(), "the plan enqueued something");
    }

    /// And the home is always a row, said the way the caller asked for it to be — the one
    /// irreversible thing on the list is the one a person must not have to infer.
    #[tokio::test]
    async fn the_plan_says_whether_the_home_is_being_kept() {
        let daemon = undeclared().await;

        for (keep, kept) in [(false, false), (true, true)] {
            let report: mixengine_proto::UninstallReport = daemon
                .expect(
                    rpc::method::DAEMON_UNINSTALL_PLAN,
                    serde_json::json!({ "keep_home": keep }),
                )
                .await;

            let home = report
                .items
                .iter()
                .find(|item| item.id == mixengine_proto::ResidueId::Home)
                .expect("the home is always a row");

            assert_eq!(
                matches!(home.outcome, mixengine_proto::Removal::Kept { .. }),
                kept,
                "{home:?}"
            );
        }
    }

    /// T87. Without `grant`, nothing outside this home is touched and every privileged row that had
    /// something to remove says what it is waiting for. That is the two-call path T64 exists for: a
    /// person reads the batch before they allow it.
    #[tokio::test]
    async fn an_uninstall_that_was_not_asked_to_grant_removes_nothing_privileged() {
        let daemon = undeclared().await;

        let started: JobSummary = daemon
            .expect(
                rpc::method::DAEMON_UNINSTALL,
                serde_json::json!({ "keep_home": true, "grant": false }),
            )
            .await;

        let report = uninstall_result(&daemon, started).await;

        for item in &report.items {
            // The three that need no token may report a removal; nothing else may, because nothing
            // else was applied.
            let unprivileged = matches!(
                item.id,
                mixengine_proto::ResidueId::PathEntry
                    | mixengine_proto::ResidueId::AutostartEntry
                    | mixengine_proto::ResidueId::BrowserTrust
            );

            assert!(
                unprivileged || !matches!(item.outcome, mixengine_proto::Removal::Removed { .. }),
                "nothing that needs the helper may be removed without a grant: {item:?}"
            );
        }
    }

    /// And `keep_home` leaves the home where it is, says so on its row, and arms nothing — which is
    /// what stops this daemon ending itself.
    #[tokio::test]
    async fn keeping_the_home_arms_nothing_and_says_so() {
        let daemon = undeclared().await;

        let started: JobSummary = daemon
            .expect(
                rpc::method::DAEMON_UNINSTALL,
                serde_json::json!({ "keep_home": true, "grant": false }),
            )
            .await;

        let report = uninstall_result(&daemon, started).await;

        let home = report
            .items
            .iter()
            .find(|item| item.id == mixengine_proto::ResidueId::Home)
            .expect("the home is always a row");

        assert!(
            matches!(home.outcome, mixengine_proto::Removal::Kept { .. }),
            "{home:?}"
        );
        assert!(daemon.api.armed.is_empty(), "the home was armed anyway");
        assert!(
            !daemon.api.shutdown.token().is_cancelled(),
            "a home that is being kept is a daemon that keeps running"
        );
    }

    /// A complete uninstall keeps the home when something outside it is still there, and takes it
    /// when nothing is — and never any other combination.
    ///
    /// **Asserted as the relation and not as one outcome**, because which of the two arms this
    /// machine takes is a property of the machine: a workstation carries a privileged helper and an
    /// audit log of its own, and a fresh CI runner carries neither. Both arms are meaningful and
    /// neither is a skip.
    ///
    /// **The regression this pins was found by CI**: the fold that settles each row against the
    /// second reading was applied to *every* row, including the home — which is still `Planned` at
    /// that point, because `arm_the_home` rewrites it two statements later. Settled, the home read
    /// as an operation the helper had been asked about and had not managed, which made the run look
    /// unfinished, which kept the home. Every complete uninstall left the home behind and reported
    /// that it had meant to. Counting the machine's rows and not the home's is what makes this
    /// assertion able to say so.
    #[tokio::test]
    async fn a_complete_uninstall_keeps_the_home_exactly_when_the_machine_is_not_clear() {
        let daemon = undeclared().await;

        let started: JobSummary = daemon
            .expect(
                rpc::method::DAEMON_UNINSTALL,
                serde_json::json!({ "keep_home": false, "grant": false }),
            )
            .await;

        let report = uninstall_result(&daemon, started).await;

        let outstanding = report
            .items
            .iter()
            .filter(|item| {
                !matches!(
                    item.id,
                    mixengine_proto::ResidueId::Home
                        | mixengine_proto::ResidueId::RelocatedDirectory
                )
            })
            .any(|item| {
                matches!(
                    item.outcome,
                    mixengine_proto::Removal::Enqueued { .. }
                        | mixengine_proto::Removal::Failed { .. }
                )
            });

        let home = report
            .items
            .iter()
            .find(|item| item.id == mixengine_proto::ResidueId::Home)
            .expect("the home is always a row");

        match outstanding {
            true => {
                assert!(
                    matches!(home.outcome, mixengine_proto::Removal::Kept { .. }),
                    "something outside this home is still there and the home went anyway: \
                     {home:?}\n{report:?}"
                );
                assert!(daemon.api.armed.is_empty(), "{report:?}");
            }
            false => {
                assert!(
                    matches!(home.outcome, mixengine_proto::Removal::OnExit { .. }),
                    "nothing is outstanding and the home was kept anyway: {home:?}\n{report:?}"
                );
                assert!(
                    !daemon.api.armed.is_empty(),
                    "the home says it is going and nothing was armed to remove it"
                );
            }
        }
    }

    /// The report a `daemon.uninstall` job leaves behind, decoded.
    async fn uninstall_result(
        daemon: &Daemon,
        started: JobSummary,
    ) -> mixengine_proto::UninstallReport {
        let finished: JobSummary = daemon
            .expect(
                rpc::method::JOB_WAIT,
                serde_json::json!({ "job": started.id, "timeout": 30_000 }),
            )
            .await;

        match finished.outcome {
            Some(mixengine_proto::JobOutcome::Succeeded { result }) => {
                serde_json::from_value(result).expect("an uninstall report")
            }
            other => panic!("{other:?}"),
        }
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

    /// The three `autostart.*` methods over the dispatcher — roadmap task **T85b**.
    ///
    /// Against `mock::Host`, for the `path.*` test's reason one step louder: the real
    /// implementations register a logon task, a LaunchAgent or a systemd user unit for the account
    /// running them, and a suite that exercised them would be a `cargo test` that arranges for a
    /// daemon to start at every login of whoever ran it. The three real ones have their own tests
    /// inside `mixengine-platform`, against a scratch name and a directory they create themselves.
    #[tokio::test]
    async fn the_autostart_entry_is_reported_then_registered_and_then_taken_away() {
        let daemon = undeclared().await;

        let before: AutostartReport = daemon
            .expect(rpc::method::AUTOSTART_STATUS, Value::Null)
            .await;
        assert!(!before.enabled, "{before:?}");
        assert!(!before.changed, "a status never claims a write: {before:?}");
        assert!(!before.for_this_home, "{before:?}");
        assert!(before.command.is_empty(), "{before:?}");

        let enabled: AutostartReport = daemon
            .expect(rpc::method::AUTOSTART_ENABLE, Value::Null)
            .await;
        assert!(enabled.enabled && enabled.changed, "{enabled:?}");
        assert!(
            enabled.for_this_home,
            "the entry names this daemon's own home: {enabled:?}"
        );
        assert!(
            enabled.command.iter().any(|word| word == "--home"),
            "{enabled:?}"
        );

        // Idempotent, and it says which of the two it was — the `path.*` distinction, and for its
        // reason.
        let again: AutostartReport = daemon
            .expect(rpc::method::AUTOSTART_ENABLE, Value::Null)
            .await;
        assert!(again.enabled, "{again:?}");
        assert!(!again.changed, "the second enable wrote: {again:?}");

        let disabled: AutostartReport = daemon
            .expect(rpc::method::AUTOSTART_DISABLE, Value::Null)
            .await;
        assert!(!disabled.enabled && disabled.changed, "{disabled:?}");
        assert!(disabled.command.is_empty(), "{disabled:?}");

        // Two enables and a disable. The status is absent for the `path.*` recorder's reason: a
        // read is not a mutation.
        assert_eq!(daemon.host.autostart_operations().len(), 3);

        daemon.quiet().await;
    }

    /// None of the three takes parameters, and a client that sent some is told so.
    #[tokio::test]
    async fn an_autostart_call_with_parameters_is_refused() {
        let answer = call(
            r#"{"jsonrpc":"2.0","method":"autostart.status","params":{"home":"/tmp"},"id":1}"#,
        )
        .await;

        assert_eq!(answer["error"]["data"]["code"], "invalid_argument");
    }

    #[tokio::test]
    async fn an_unknown_method_is_method_not_found_and_says_which_one() {
        // A namespace this build has not reached — `site.create` used to stand here until T39a,
        // `blueprint.apply` until T77 and `extension.install` until T81. Each of them becoming a
        // real method is exactly the drift this test is worth keeping past; what stands here now is
        // the one `desktop-app` integration nobody has written (T83).
        let answer = call(r#"{"jsonrpc":"2.0","method":"extension.open","id":1}"#).await;

        assert_eq!(answer["error"]["code"], -32601);
        assert_eq!(answer["error"]["data"]["code"], "not_found");
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("extension.open")),
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

    /// A daemon with no feed to read answers the question anyway — roadmap task **T88**.
    ///
    /// **What it must not do is fail.** `.claude/features/updates.md` says an offline machine never
    /// sees an error and never a slower startup, and this is the same rule one call along: the
    /// fixture's feed URL answers nothing, and `update.status` still says what version is running,
    /// where it lives, and that it has been offered nothing.
    #[tokio::test]
    async fn update_status_answers_on_a_machine_that_has_never_checked() {
        let daemon = daemon(web_and_db(), &[]).await;

        let status: UpdateStatus = daemon.expect(rpc::method::UPDATE_STATUS, Value::Null).await;

        assert_eq!(status.current, env!("CARGO_PKG_VERSION"));
        assert!(status.available.is_none(), "{status:?}");
        assert!(!status.offered);
        assert!(status.checked_at.is_none(), "{status:?}");
        assert!(!status.stale);

        daemon.quiet().await;
    }

    /// This test binary is not an installed MixEngine, and the fixture's daemon lives in a temporary
    /// home the test owns — so the placement it reports is `self_updatable`, and the assertion worth
    /// making is that the answer is *a placement* rather than a panic on a path nobody installed.
    #[tokio::test]
    async fn update_status_says_where_this_copy_lives() {
        let daemon = daemon(web_and_db(), &[]).await;

        let status: UpdateStatus = daemon.expect(rpc::method::UPDATE_STATUS, Value::Null).await;

        let mixengine_proto::UpdatePlacement::SelfUpdatable { directory } = &status.placement
        else {
            panic!("a directory this test made is one this account can write: {status:?}");
        };
        assert!(directory.contains("installed-beside"), "{directory}");

        daemon.quiet().await;
    }

    /// `update.apply` takes the version the client showed the user. Without that check, a feed that
    /// changed between the prompt and the answer would install something nobody read the notes for —
    /// and on a daemon that has read no feed at all, *every* version is that case.
    #[tokio::test]
    async fn applying_a_version_this_daemon_was_never_offered_is_refused() {
        let daemon = daemon(web_and_db(), &[]).await;

        let answer = daemon
            .ask(
                rpc::method::UPDATE_APPLY,
                serde_json::json!({ "version": "9.9.9" }),
            )
            .await;

        assert_eq!(
            answer["error"]["data"]["code"], "precondition_failed",
            "{answer}"
        );

        daemon.quiet().await;
    }

    /// Skip is remembered, and it is remembered by the daemon rather than by the client — which is
    /// what makes *"declining an update does not re-prompt for that version"* a property of the
    /// product rather than of whichever client somebody happened to use.
    #[tokio::test]
    async fn a_skipped_version_is_written_down_where_the_next_daemon_will_read_it() {
        let daemon = daemon(web_and_db(), &[]).await;

        let _: UpdateStatus = daemon
            .expect(
                rpc::method::UPDATE_DECIDE,
                serde_json::json!({ "version": "9.9.9", "decision": "skip" }),
            )
            .await;

        assert_eq!(
            mixengine_core::updates::records::get::<String>(
                &daemon.api.store,
                mixengine_core::updates::records::SKIPPED_VERSION,
            )
            .await
            .expect("a read"),
            Some("9.9.9".to_owned())
        );

        daemon.quiet().await;
    }

    /// And *later* writes a moment rather than a version, because it is about every version.
    #[tokio::test]
    async fn remind_me_later_writes_a_moment_inside_the_clamp() {
        let daemon = daemon(web_and_db(), &[]).await;
        let now = mixengine_proto::Timestamp::from_system_time(std::time::SystemTime::now());

        let _: UpdateStatus = daemon
            .expect(
                rpc::method::UPDATE_DECIDE,
                serde_json::json!({ "version": "9.9.9", "decision": "later" }),
            )
            .await;

        let due = mixengine_core::updates::records::get::<mixengine_proto::Timestamp>(
            &daemon.api.store,
            mixengine_core::updates::records::REMIND_AFTER,
        )
        .await
        .expect("a read")
        .expect("a moment");

        // Inside the clamp `updates::offer` reads it against, or the reminder would be disbelieved
        // the moment it was written.
        let ahead = due.0 - now.0;
        assert!(ahead > 0, "{ahead}");
        assert!(
            ahead <= mixengine_core::updates::records::REMIND_CLAMP_SECONDS * 1_000,
            "{ahead}"
        );

        daemon.quiet().await;
    }

    /// `daemon.status` carries the offer, and carries nothing when there is none — which is every
    /// daemon that has not managed to read a feed.
    #[tokio::test]
    async fn the_status_line_says_nothing_about_updates_until_there_is_something_to_say() {
        let daemon = daemon(web_and_db(), &[]).await;

        let answer = daemon.ask(rpc::method::DAEMON_STATUS, Value::Null).await;

        assert!(answer["result"].get("update").is_none(), "{answer}");

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

    /// **The code is the failure's own, not one this handler picked** — R8.
    ///
    /// It used to be `internal` for every source failure whatever it was, and the fixture is why:
    /// `SpecSource` returned `anyhow::Error`, the wire mapping downcast to find a
    /// `mixengine_core::Error` behind it, and a fixture that raised a bare string had none to find.
    /// The running daemon never took that path — its source renders through `mixengine-core` and so
    /// always carried a code — so what this test was pinning down was the hole and not the product.
    /// With the trait typed, a package that is not installed answers `not_found` and points at
    /// `mix package list`, which is the sentence `error.rs` has argued for since T30: a user who
    /// misspelled a setting must not be sent to file a bug report.
    #[tokio::test]
    async fn a_source_that_cannot_answer_is_reported_with_the_failure_s_own_code() {
        let daemon = daemon(Arc::new(fixture::Unavailable), &[]).await;

        let answer = daemon.ask(rpc::method::SERVICE_LIST, Value::Null).await;

        assert_eq!(answer["error"]["data"]["code"], "not_found");
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("no such package")),
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

        assert_eq!(why.code, ErrorCode::NotFound);
        assert!(
            why.message.contains("no such package"),
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

    /// A service with no database vocabulary is told so **in those words** — roadmap task T77a.
    ///
    /// Deliberately not `unsupported`: that code means *this operating system cannot*, and every
    /// system this ships to runs the fixture service perfectly well. What has no databases is the
    /// package. The same distinction T77 drew for `blueprint.apply`.
    #[tokio::test]
    async fn creating_a_database_on_a_service_that_has_none_is_refused_by_name() {
        let daemon = daemon(
            Arc::new(fixture::Declared(vec![
                fixture::spec("db").build().expect("a usable spec"),
            ])),
            &["db"],
        )
        .await;

        let answer = daemon
            .ask(
                rpc::method::DATABASE_CREATE,
                serde_json::json!({"service": "db", "database": "blog"}),
            )
            .await;

        assert_eq!(
            answer["error"]["data"]["code"], "invalid_argument",
            "{answer}"
        );
        assert!(
            answer["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("db"),
            "{answer}"
        );

        daemon.quiet().await;
    }

    /// A service this home does not declare is a different miss, and says so: "no such service"
    /// would send somebody looking for a service that is right there, and this is the case where it
    /// genuinely is not.
    #[tokio::test]
    async fn creating_a_database_on_a_service_nothing_declares_is_not_found() {
        let daemon = daemon(Arc::new(fixture::Declared(Vec::new())), &[]).await;

        let answer = daemon
            .ask(
                rpc::method::DATABASE_CREATE,
                serde_json::json!({"service": "mariadb@main", "database": "blog"}),
            )
            .await;

        assert_eq!(answer["error"]["data"]["code"], "not_found", "{answer}");

        daemon.quiet().await;
    }

    /// `database.client` and `database.open` reach their handler, and a service nothing declares
    /// is the same miss to both — roadmap task **T83**. What the methods *do* is proved beside
    /// them, in `crate::databases`, on a mock host that records what it was asked to start.
    #[tokio::test]
    async fn asking_where_a_service_nothing_declares_could_be_opened_is_not_found() {
        let daemon = daemon(Arc::new(fixture::Declared(Vec::new())), &[]).await;

        for method in [rpc::method::DATABASE_CLIENT, rpc::method::DATABASE_OPEN] {
            let answer = daemon
                .ask(method, serde_json::json!({"service": "mariadb@main"}))
                .await;

            assert_eq!(answer["error"]["data"]["code"], "not_found", "{answer}");
        }

        daemon.quiet().await;
    }

    /// A name that could not be a database name is refused **before** anything is started: a
    /// caller's typo should not first cost a database server coming up.
    #[tokio::test]
    async fn a_database_name_that_is_not_one_is_refused_before_anything_starts() {
        let daemon = daemon(Arc::new(fixture::Declared(Vec::new())), &[]).await;

        let answer = daemon
            .ask(
                rpc::method::DATABASE_CREATE,
                serde_json::json!({"service": "mariadb@main", "database": "Blog; DROP"}),
            )
            .await;

        assert_eq!(
            answer["error"]["data"]["code"], "invalid_argument",
            "{answer}"
        );

        daemon.quiet().await;
    }
}
