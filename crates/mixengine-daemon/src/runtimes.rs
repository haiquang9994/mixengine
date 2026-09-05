//! `runtime.*`: what the index offers, what this machine has, and the job that turns one into the
//! other.
//!
//! **This is the job system's first producer** (T23). Everything under it existed before this file
//! did and none of it could be reached: T20 verifies a signed index nobody asked, T21 installs an
//! artifact nobody named, T22 runs jobs nobody starts. What is added here is a method in front of
//! each — which is why the wiring is an `impl` and not an adapter, since
//! [`mixengine_core::install::Watcher`] was shaped after [`JobHandle`] on
//! purpose.
//!
//! # Five of the six methods answer inline, and one returns a job
//!
//! The split is the download and nothing else. `.claude/architecture/daemon-and-ipc.md` says a long
//! operation returns a job rather than holding a call open; removing a directory, reading a table
//! and moving a default are none of them long, and making every one of them return a job would make
//! a client learn a second protocol to hear an answer that was ready before it asked.
//!
//! `resolve` (T24) is the newest of the five and the only one a shim will *not* use: it calls
//! [`mixengine_core::resolve`] in-process instead, because a `php` that needs a running daemon to
//! start is a `php` that stops working when the daemon does. What the method is for is every client
//! that is already talking to one — `mix`, the GUI panel — and the answer is the same either way,
//! which is the whole reason the order lives in `core` rather than here.
//!
//! # An install that is already running is answered with the job that is running it
//!
//! Rather than started twice or refused. Two `runtime.install` calls for one version is what two
//! terminals or a double-clicked button produce, and the second is asking for the same outcome —
//! but the two would share a `.part` file in `cache/downloads/`, named after the artifact's hash and
//! appended to by both, which is a download that can only fail its checksum. So the second call is
//! handed the first call's [`JobSummary`], which is what it would have been handed if it had asked a
//! moment earlier.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use mixengine_core::index::{self, Index, Package, Selection, Target};
use mixengine_core::install::Installer;
use mixengine_core::{Paths, Store, paths, resolve, runtimes};
use mixengine_proto::{
    Error, ErrorCode, Execution, JobId, JobKind, JobSummary, PackageVersion, ResolvedRuntime,
    RuntimeCatalogue, RuntimeFilter, RuntimeKind, RuntimeList, RuntimeQuestion, RuntimeRelease,
    RuntimeRemoval, RuntimeSummary, RuntimeTarget, RuntimeUninstall, ServiceState, Timestamp,
    VersionConstraint, rpc,
};

use crate::error::ToWire as _;
use crate::jobs::{JobHandle, Jobs};

/// Where the package index is read from, and which key it has to be signed by.
///
/// **Both, or neither.** A team hosting its own mirror cannot sign with our private key, so an index
/// URL that could be pointed elsewhere while the key stayed compiled in would be a setting that can
/// only ever fail — which is why `.claude/operations/runtime-packaging.md` promises the pair and not
/// the URL alone.
///
/// Overriding them is trusting a different publisher, and that is a decision only somebody who
/// already controls how this daemon starts can make: the values arrive as arguments to `mixengined`,
/// from the command line or its environment, and nothing below `main` reads either.
#[derive(Debug, Clone)]
pub(crate) struct IndexSource {
    /// The document's URL.
    pub(crate) url: String,

    /// The base64 minisign public key every fetch is verified against.
    pub(crate) public_key: String,
}

impl IndexSource {
    /// Where the *extension registry* is, given where the package index is — roadmap task **T81**.
    ///
    /// **Derived rather than a second setting.** The two documents are published side by side under
    /// one tag and verified with one key, so a mirror that serves the index serves this beside it;
    /// a `--registry-url` of its own would be a second thing to point somewhere and a second way to
    /// point only half of a mirror. The key is the same value for the same reason it is the same
    /// key: an extension has the package index's blast radius exactly.
    ///
    /// The last path segment is replaced, so `https://mirror/mixengine/index.json` becomes
    /// `https://mirror/mixengine/extensions.json`; a URL with nothing to replace gets the name
    /// appended.
    pub(crate) fn registry_url(&self) -> String {
        match self.url.rsplit_once('/') {
            Some((base, _)) => {
                format!("{base}/{}", mixengine_core::extensions::registry::FILE_NAME)
            }
            None => format!(
                "{}/{}",
                self.url,
                mixengine_core::extensions::registry::FILE_NAME
            ),
        }
    }
}

impl Default for IndexSource {
    /// What MixEngine publishes, verified by the key compiled into this binary.
    fn default() -> Self {
        Self {
            url: index::DEFAULT_URL.to_owned(),
            public_key: index::PUBLIC_KEY.to_owned(),
        }
    }
}

/// The index and the download pipeline, shared by everything that installs anything.
///
/// **One per daemon, and not one per namespace.** `runtime.*` and `package.*` both read the same
/// signed document and both write into the same `cache/`, so two clients would be two processes
/// worth of refresh racing over one `index.json` and two installers sharing one `downloads/`. The
/// pair is built once, where the public key is checked, and handed to both.
#[derive(Debug)]
pub(crate) struct Fetcher {
    /// The verified package index, cached under `cache/`.
    pub(crate) index: index::Client,

    /// The download pipeline, with its partial downloads in the same place.
    pub(crate) installer: Installer,
}

impl Fetcher {
    /// Point an index client and an installer at `source`, caching under the home's `cache/`.
    ///
    /// # Errors
    ///
    /// The wire error of a public key that is not one, or of an HTTP client that cannot be built —
    /// both of which mean a broken build or an unusable `--index-key`, and both of which fail the
    /// daemon's start rather than the first call: a daemon that cannot install anything should say
    /// so while somebody is looking at it.
    pub(crate) fn new(paths: &Paths, source: &IndexSource) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            index: index::Client::with(&source.url, &source.public_key, paths.cache())
                .map_err(|error| error.to_wire())?,
            installer: Installer::new(paths.cache()).map_err(|error| error.to_wire())?,
        }))
    }
}

/// Everything `runtime.*` needs, and the only thing that starts an install.
#[derive(Debug)]
pub(crate) struct Runtimes {
    /// Where a runtime lands.
    paths: Paths,

    /// Where the row goes.
    store: Store,

    /// What turns the work into a job, and the only thing that can end one.
    jobs: Arc<Jobs>,

    /// The index that offers versions, and the pipeline that downloads them.
    fetcher: Arc<Fetcher>,

    /// What a pool created by an install is registered with, so a request can start it — **T72a**.
    ///
    /// Held for one call: `activate::hold_all` needs it, because an activator that accepts a
    /// connection has to be able to make the service run.
    services: Arc<crate::services::Registry>,

    /// The installs this daemon is running, by what they are installing.
    ///
    /// A `tokio` mutex rather than a `std` one because it is held across the `await` that starts the
    /// job — which is the whole point: the check for "is this already running" and the row that
    /// makes it so have to be one decision, or two callers arriving together both find nothing and
    /// both start.
    running: tokio::sync::Mutex<BTreeMap<(RuntimeKind, PackageVersion), JobId>>,
}

impl Runtimes {
    /// Point the runtime methods at an index and the pipeline that downloads from it.
    pub(crate) fn new(
        paths: &Paths,
        store: &Store,
        jobs: Arc<Jobs>,
        fetcher: Arc<Fetcher>,
        services: Arc<crate::services::Registry>,
    ) -> Arc<Self> {
        Arc::new(Self {
            paths: paths.clone(),
            store: store.clone(),
            jobs,
            fetcher,
            services,
            running: tokio::sync::Mutex::new(BTreeMap::new()),
        })
    }

    /// `runtime.list_installed` — what is on this machine.
    ///
    /// # Errors
    ///
    /// The wire error of a table that could not be read.
    pub(crate) async fn list_installed(
        &self,
        filter: &RuntimeFilter,
    ) -> Result<RuntimeList, Error> {
        let runtimes = runtimes::records(&self.store, filter.kind)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(RuntimeList { runtimes })
    }

    /// `runtime.list_available` — what the index offers for this machine, and what is already here.
    ///
    /// **Composed here rather than by the client**, which is the rule in `CLAUDE.md`: whether a
    /// listed version is installed is a fact about two lists, and leaving a client to cross-reference
    /// them would be two clients able to disagree about what "installed" means.
    ///
    /// # Errors
    ///
    /// The wire error of an index that could not be obtained *at all* — a fetch that fails while a
    /// cached index exists is answered from the cache, with [`RuntimeCatalogue::stale`] set.
    pub(crate) async fn list_available(
        &self,
        filter: &RuntimeFilter,
    ) -> Result<RuntimeCatalogue, Error> {
        let catalogue = self
            .fetcher
            .index
            .catalogue()
            .await
            .map_err(|error| error.to_wire())?;
        let installed = runtimes::records(&self.store, filter.kind)
            .await
            .map_err(|error| error.to_wire())?;

        let wanted: &[RuntimeKind] = match &filter.kind {
            Some(kind) => std::slice::from_ref(kind),
            None => &RuntimeKind::ALL,
        };

        let mut runtimes = Vec::new();
        for kind in wanted.iter().copied() {
            for package in catalogue.index.installable(kind.as_str()) {
                // An index that offers a version this build could not make a directory for is one
                // whose entry is skipped rather than one that fails the listing: the other versions
                // are still installable, and the alternative is a home that can list nothing because
                // of one malformed row in a document nobody here controls.
                let Ok(version) = PackageVersion::parse(package.version.clone()) else {
                    tracing::warn!(
                        kind = kind.as_str(),
                        version = package.version,
                        "the package index offers a version this build cannot use as a directory \
                         name; skipping it"
                    );
                    continue;
                };

                let chosen = catalogue.index.artifact(kind.as_str(), &package.version);

                runtimes.push(RuntimeRelease {
                    installed: installed
                        .iter()
                        .any(|have| have.kind == kind && have.version == version),
                    kind,
                    version,
                    channel: package.channel.into(),
                    eol: package.eol.clone(),
                    bytes: chosen.map_or(0, |chosen| chosen.artifact.size),
                    execution: chosen.map(|chosen| chosen.execution),
                });
            }
        }

        Ok(RuntimeCatalogue {
            runtimes,
            stale: catalogue.freshness.is_stale(),
        })
    }

    /// The newest release the index publishes for this machine that satisfies `wanted`.
    ///
    /// **Roadmap task T78, its design's D9.** A plan holds a *constraint* on purpose — it reads this
    /// home's tables and never the index — and turning one into a release is a question only the
    /// index can answer. An apply asks it before it writes anything, so that a constraint nothing
    /// satisfies fails while there is still nothing to take back.
    ///
    /// # Errors
    ///
    /// `not_found` when the index publishes nothing for this machine that satisfies the constraint,
    /// and the wire error of an index that could not be obtained at all.
    pub(crate) async fn newest_satisfying(
        &self,
        kind: RuntimeKind,
        wanted: &VersionConstraint,
    ) -> Result<PackageVersion, Error> {
        let catalogue = self
            .fetcher
            .index
            .catalogue()
            .await
            .map_err(|error| error.to_wire())?;

        catalogue
            .index
            .installable(kind.as_str())
            .filter_map(|package| PackageVersion::parse(package.version.clone()).ok())
            .filter(|version| wanted.matches(version))
            .max_by(PackageVersion::cmp_precedence)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    format!(
                        "the package index publishes no {} for this machine that satisfies {}",
                        kind.as_str(),
                        wanted.as_str()
                    ),
                )
                .with_hint(format!(
                    "`mix runtime available --kind {}` lists what it does publish",
                    kind.as_str()
                ))
            })
    }

    /// `runtime.install` — start the download, and answer with the job doing it.
    ///
    /// Two things are decided before a job exists, because both are answers a caller should have
    /// immediately rather than through a job that fails a moment later: a version that is already
    /// installed, and a version this daemon is already installing.
    ///
    /// # Errors
    ///
    /// `already_exists` when it is installed, and the wire error of a row that could not be read or
    /// a job that could not be started.
    pub(crate) async fn install(
        self: &Arc<Self>,
        target: &RuntimeTarget,
    ) -> Result<JobSummary, Error> {
        // Held across the whole of this, so that "is one running" and "start one" are one decision.
        let mut running = self.running.lock().await;

        let key = (target.kind, target.version.clone());
        if let Some(job) = running.get(&key).copied() {
            tracing::debug!(
                kind = target.kind.as_str(),
                version = target.version.as_str(),
                %job,
                "an install of this version is already running; answering with its job"
            );
            return self.jobs.status(job).await;
        }

        match runtimes::record(&self.store, target.kind, &target.version).await {
            Ok(_) => {
                return Err(mixengine_core::Error::AlreadyRecorded {
                    kind: target.kind,
                    version: target.version.clone(),
                }
                .to_wire());
            }
            Err(mixengine_core::Error::NotFound { .. }) => {}
            Err(error) => return Err(error.to_wire()),
        }

        let kind = JobKind::parse(rpc::method::RUNTIME_INSTALL)
            .expect("`runtime.install` is a method name, which is what a job kind is");

        let runtimes = Arc::clone(self);
        let target = target.clone();
        let started = self
            .jobs
            .begin(&kind, move |handle| async move {
                let outcome = runtimes.perform(&target, &handle).await;

                // Released here rather than by the caller: this future is what owns the install, and
                // it ends by being cancelled as well as by returning. It cannot run ahead of the
                // insert below — the caller holds this same lock until after it — so a job that
                // finishes in the instant it is spawned still leaves the map empty rather than
                // removing a key that has not been added.
                runtimes
                    .running
                    .lock()
                    .await
                    .remove(&(target.kind, target.version));

                outcome
            })
            .await?;

        running.insert(key, started.id);

        Ok(started)
    }

    /// The work behind an install: look it up, fetch it, write it down.
    ///
    /// The three steps are in this order for a reason each: the lookup is refused before a byte is
    /// downloaded, the download is a transaction whose commit is a rename
    /// ([`mixengine_core::install`]), and the row is written **after** that rename — so a failure
    /// anywhere leaves either nothing or a directory with no row, and never a row describing a
    /// runtime that is not there.
    pub(crate) async fn perform(
        &self,
        target: &RuntimeTarget,
        handle: &JobHandle,
    ) -> Result<serde_json::Value, Error> {
        let (kind, version) = (target.kind, &target.version);
        tracing::info!(job = %handle.id(), kind = kind.as_str(), version = version.as_str(), "installing a runtime");

        handle.progress(0, "reading the package index").await;
        let catalogue = self
            .fetcher
            .index
            .catalogue()
            .await
            .map_err(|error| error.to_wire())?;
        let (package, selection) = offered(
            &catalogue.index,
            kind.as_str(),
            version.as_str(),
            &format!("mix runtime available --kind {kind}"),
        )?;

        if let Some(notice) = emulation_notice(kind.as_str(), version.as_str(), selection) {
            handle.progress(0, &notice).await;
        }

        let into = runtimes::directory(&self.paths, kind, version);
        if let Some(parent) = into.parent() {
            paths::create_dir(parent).map_err(|error| error.to_wire())?;
        }

        let smoke = runtimes::smoke_test(kind);
        let installed = self
            .fetcher
            .installer
            .install(
                selection.artifact,
                &into,
                Some(&smoke),
                mixengine_core::install::NotAnArchive::Refuse,
                handle,
            )
            .await
            .map_err(|error| error.to_wire())?;

        let record = runtimes::remember(
            &self.store,
            &runtimes::Installation {
                kind,
                version: version.clone(),
                channel: package.channel.into(),
                path: installed.path.clone(),
                bytes: installed.bytes,
                url: selection.artifact.url.clone(),
                sha256: selection.artifact.sha256.clone(),
                // Recorded because the shim reads it, months later and with nothing to ask: which
                // file inside the directory is `php` is the publisher's layout, not ours.
                provides: selection.artifact.provides.clone(),
                // The other half of what the index knows and the daemon would otherwise consult
                // once and forget. See migration 0005.
                extension_dir: selection.artifact.extension_dir.clone(),
                extensions: selection.artifact.extensions.clone(),
            },
            Timestamp::from_system_time(SystemTime::now()),
        )
        .await;

        let summary = match record {
            Ok(summary) => summary,

            // **The one place the ordering is undone rather than kept.** A directory with no row is
            // survivable in general — it is invisible and costs disk — but this is the moment we
            // know one exists, and leaving it would make the retry that fixes everything else fail
            // with `already installed` instead. Best-effort: if it cannot be removed either, the
            // error the caller gets is still the one that explains why nothing was installed.
            Err(error) => {
                if let Err(cleanup) = runtimes::discard(&installed.path).await {
                    tracing::warn!(
                        path = %installed.path.display(),
                        %cleanup,
                        "a runtime whose row could not be written could not be removed either"
                    );
                }
                return Err(error.to_wire());
            }
        };

        // **After the row and never before it**, because the pool points at that row: this is the
        // post-install hook `.claude/features/runtime-versions.md` describes, and it is the same
        // idempotent call the daemon makes at boot. A failure here is reported and does not undo the
        // install — a PHP with no pool is a PHP the next boot gives one to, where an install rolled
        // back for it would be eighty megabytes thrown away over a row.
        match mixengine_core::services::pools::ensure(
            &self.store,
            mixengine_platform::host().as_ref(),
            &crate::services::catalogue(),
        )
        .await
        {
            Ok(created) if created.is_empty() => {}
            Ok(created) => {
                tracing::info!(pools = ?created, "the new runtime was given its service")
            }
            Err(error) => tracing::warn!(
                kind = kind.as_str(),
                version = version.as_str(),
                %error,
                "this runtime was installed but could not be given its service; the next daemon \
                 start will try again"
            ),
        }

        // **And the address that lets a request start that pool again** — roadmap task **T72a**.
        //
        // The same two idempotent repairs the daemon makes at boot, in the same order, because a
        // pool that has just been created has neither: `activation::ensure` gives it the port a
        // Windows activator listens on, and `hold_all` binds the address a site file is about to
        // name. Without them the activator arrives only at the next daemon start — and until then a
        // pool that idle-stops leaves its site answering 502, which is a failure a user would have
        // to guess a restart out of.
        //
        // **Found by measurement rather than by reading**: this hole was T70's, and it stayed
        // invisible until T72a's cold path put a real request through a pool the sweeper had
        // stopped.
        //
        // Reported and not fatal, on the pool hook's reasoning above.
        let host = mixengine_platform::host();

        if let Err(error) = mixengine_core::services::activation::ensure(
            &self.store,
            host.as_ref(),
            &crate::services::catalogue(),
        )
        .await
        {
            tracing::warn!(
                %error,
                "a new pool could not be given an activation port; the next daemon start will try \
                 again"
            );
        }

        match crate::services::activate::hold_all(
            Arc::clone(&self.services),
            &self.paths,
            &self.store,
            host.as_ref(),
        )
        .await
        {
            Ok(held) if held.is_empty() => {}
            Ok(held) => tracing::info!(services = ?held, "a request can now start these services"),
            Err(error) => tracing::warn!(
                %error,
                "a new pool cannot be started by a request until this daemon is restarted"
            ),
        }

        // The ini set this build can load, rendered before anything runs out of it — reported rather
        // than fatal, on the pool hook's reasoning above: a PHP with no `conf.d` is one the next
        // daemon start gives one to, where an install rolled back for it would be eighty megabytes
        // thrown away over a file.
        match mixengine_core::runtimes::extensions::state(&self.store, kind, version).await {
            Ok(state) => {
                if let Err(error) =
                    mixengine_core::runtimes::extensions::render(&self.paths, &state).await
                {
                    tracing::warn!(
                        %error,
                        "this runtime was installed but its conf.d could not be written"
                    );
                }
            }
            Err(error) => tracing::warn!(
                %error,
                "this runtime was installed but its extensions could not be read back"
            ),
        }

        serde_json::to_value(&summary).map_err(|error| {
            Error::new(
                ErrorCode::Internal,
                format!("what the install produced could not be encoded: {error}"),
            )
        })
    }

    /// `runtime.uninstall` — remove the directory, then the row.
    ///
    /// **In that order**, which is [`mixengine_core::runtimes`]' rule read backwards: a directory
    /// that could not be removed leaves a row that still describes it, and asking again repeats
    /// exactly this. The reverse would leave a runtime on disk that nothing knows about.
    ///
    /// **Two refusals, and `force` crosses exactly one of them.** A project whose pin this removal
    /// would leave with no answer is a statement about the future — the next `cd` into that
    /// directory fails with a message naming the install that fixes it — so somebody who has been
    /// shown the projects and typed `--force` has made a decision they are entitled to make. A
    /// running php-fpm pool is a fact about the present, and no flag buys a live process with no
    /// files under it. That asymmetry is decided here rather than by the schema: a **stopped** pool
    /// is deleted along with the runtime, deliberately, so the `ON DELETE RESTRICT` on
    /// `services.runtime_install_id` is never reached.
    ///
    /// **Pools, plural, since roadmap task T82a.** A `web-app` extension owns a second pool on the
    /// same installed PHP as the shared one, so every sentence above holds for a set: any of them
    /// running refuses the whole removal, and all the stopped ones go with the runtime.
    ///
    /// # Errors
    ///
    /// `not_found` when it is not installed; `precondition_failed` when a registered project pins
    /// it and `force` was not asked for, and when the pool that runs out of it has not been
    /// stopped; and the wire error of a directory that could not be removed — on Windows, most
    /// often a process still running out of it.
    pub(crate) async fn uninstall(
        &self,
        asked: &RuntimeUninstall,
    ) -> Result<RuntimeRemoval, Error> {
        let target = &asked.target;

        let removed = runtimes::record(&self.store, target.kind, &target.version)
            .await
            .map_err(|error| error.to_wire())?;

        // **Every pool this runtime is running, not one of them** — roadmap task **T82a**, its
        // design's D6. A `web-app` extension owns a second pool on the same install, and seeing one
        // of two would delete the shared one, leave the extension's, and then fail on the
        // `ON DELETE RESTRICT` with a message about a foreign key.
        let pools =
            mixengine_core::services::pools::of_runtime(&self.store, target.kind, &target.version)
                .await
                .map_err(|error| error.to_wire())?;

        // Cheaper than the pool check below and asked first for that reason: two reads of tables
        // this home owns, against a `services` row and the state of a process.
        if !asked.force {
            let broken =
                mixengine_core::projects::pins_broken_by(&self.store, target.kind, &target.version)
                    .await
                    .map_err(|error| error.to_wire())?;

            // **An extension's site frozen on this PHP joins the refusal** — roadmap task **T81b**,
            // the design's D9. A pin is a promise about a project's future; a web-app's pool is a
            // fact about what is served now, and losing it silently was the state before this line.
            let mut frozen = Vec::new();

            for pool in &pools {
                frozen.extend(
                    mixengine_core::sites::frozen_on(&self.store, pool)
                        .await
                        .map_err(|error| error.to_wire())?,
                );
            }

            if !broken.is_empty() || !frozen.is_empty() {
                let named = broken
                    .iter()
                    .map(|pin| format!("{} ({})", pin.project, pin.constraint))
                    .chain(frozen.iter().map(|id| format!("{id} (extension)")))
                    .collect::<Vec<_>>()
                    .join(", ");

                return Err(Error::new(
                    ErrorCode::PreconditionFailed,
                    format!(
                        "removing {} {} would leave nothing for {named}",
                        target.kind, target.version
                    ),
                )
                .with_hint(
                    "install another version that answers the pin, change the pin, or `--force`                      to remove it anyway",
                ));
            }
        }

        // A PHP whose pool is running is a PHP something is serving sites out of, and removing the
        // directory under it would leave a process with no files and a row naming a runtime that is
        // gone.
        // **Every one of them is asked before any of them is deleted** — roadmap task **T82a**. A
        // walk that deleted as it went would leave a home with the shared pool gone and the
        // extension's still running, over a refusal that was going to be raised anyway.
        for service in &pools {
            let record = mixengine_core::services::record(&self.store, service)
                .await
                .map_err(|error| error.to_wire())?;

            if !matches!(record.state, ServiceState::Stopped | ServiceState::Failed) {
                return Err(Error::new(
                    ErrorCode::PreconditionFailed,
                    format!("{service} is {}", record.state.as_str()),
                )
                .with_hint(format!("`mix service stop {service}` first")));
            }
        }

        for service in &pools {
            // The row goes before the directory, which is the reverse of the rule the directory
            // follows — and is right for the same reason: a `services` row whose runtime is gone is
            // a row every `service.*` call fails on, where a directory with no row is invisible.
            mixengine_core::services::delete(&self.store, service)
                .await
                .map_err(|error| error.to_wire())?;

            tracing::info!(%service, "a pool was removed with the runtime it ran out of");
        }

        runtimes::discard(Path::new(&removed.path))
            .await
            .map_err(|error| error.to_wire())?;

        // The second directory an uninstall owns, beside the pool's `etc/<service-id>/`.
        if let Err(error) =
            mixengine_core::runtimes::extensions::discard(&self.paths, target.kind, &target.version)
                .await
        {
            tracing::warn!(
                %error,
                "the runtime is gone and its generated conf.d could not be removed"
            );
        }

        let default_cleared = runtimes::forget(&self.store, target.kind, &target.version)
            .await
            .map_err(|error| error.to_wire())?;

        tracing::info!(
            kind = target.kind.as_str(),
            version = target.version.as_str(),
            default_cleared,
            "a runtime was uninstalled"
        );

        Ok(RuntimeRemoval {
            removed,
            default_cleared,
        })
    }

    /// `runtime.set_default` — make one installed version the one its kind resolves to.
    ///
    /// # Errors
    ///
    /// `not_found` when that version is not installed, and the wire error of a row that could not be
    /// written.
    pub(crate) async fn set_default(
        &self,
        target: &RuntimeTarget,
    ) -> Result<RuntimeSummary, Error> {
        runtimes::set_default(&self.store, target.kind, &target.version)
            .await
            .map_err(|error| error.to_wire())
    }

    /// `runtime.resolve` — which installed version this directory uses, and why that one.
    ///
    /// **Every step of the order happens here** ([`mixengine_core::resolve`]), including the two
    /// that read the filesystem: a client that walked for its own `mixengine.toml` would be a client
    /// deciding something, and two of them walking differently is exactly the disagreement this
    /// method exists to make impossible. What a caller supplies is the pair the daemon cannot know —
    /// the directory the user is in, and what their flag or `MIXENGINE_PHP` said.
    ///
    /// # Errors
    ///
    /// `dependency_missing` when nothing installed satisfies the question, with the command that
    /// would fix it in the hint; `invalid_argument` for a relative directory or a `mixengine.toml`
    /// that does not parse; and the wire error of a table that could not be read.
    pub(crate) async fn resolve(
        &self,
        question: &RuntimeQuestion,
    ) -> Result<ResolvedRuntime, Error> {
        let cwd = question.cwd.as_deref().map(Path::new);

        resolve::runtime(
            &self.store,
            &resolve::Question {
                kind: question.kind,
                cwd,
                explicit: question.version.as_ref(),
            },
        )
        .await
        .map_err(|error| error.to_wire())
    }
}

/// The package and the artifact the index offers for this kind and version, or the reason it
/// offers neither.
///
/// `listing` is the command whoever reads the message should run next, which differs by namespace:
/// a runtime is listed by `mix runtime available` and a service package by `mix package available`.
///
/// **Three disappointments, told apart**, where [`Index::artifact`] deliberately answers [`None`] to
/// all three: a kind the index has nothing for, a version it does not publish, and a version it
/// publishes for other systems only. They send whoever reads the message to three different places —
/// a typo, a version list, and the fact that upstream ships no build for this machine — and the last
/// one is the one that would otherwise look like a bug in MixEngine.
pub(crate) fn offered<'a>(
    index: &'a Index,
    kind: &str,
    version: &str,
    listing: &str,
) -> Result<(&'a Package, Selection<'a>), Error> {
    let Some(package) = index
        .packages
        .iter()
        .find(|package| package.kind == kind && package.version == version)
    else {
        return Err(Error::new(
            ErrorCode::NotFound,
            format!("the package index does not publish {kind} {version}"),
        )
        .with_hint(format!("`{listing}` lists every version it does publish")));
    };

    // Read off the target triple this daemon was compiled for rather than asked of the running
    // machine, which is also the answer the caller wants: an x86_64 build running under emulation
    // should install x86_64 artifacts, because that is what it can execute. Since **T92** the
    // reverse holds too and is `Target::runnable`'s business — an ARM64 Windows daemon may install
    // an x86_64 artifact, because that machine can execute one and upstream builds it nothing else.
    let Some(target) = Target::host() else {
        return Err(Error::new(
            ErrorCode::UnsupportedPlatform,
            format!(
                "this build of MixEngine runs on a system the package index has no vocabulary \
                 for ({} {})",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        ));
    };

    package
        .select(target)
        .map(|selection| (package, selection))
        .ok_or_else(|| {
            Error::new(
                ErrorCode::UnsupportedPlatform,
                format!("{kind} {version} is not published for this machine"),
            )
            .with_hint(
                "upstream does not build every version for every system — \
                 `mix runtime available` only lists what this one can run",
            )
        })
}

/// What an install says before it downloads a build this machine cannot run natively.
///
/// **A progress line rather than a column in a table**, because it is a fact about *this* install at
/// the moment it happens; the listing marks the same versions before anybody commits to one. The
/// alternative — remembering it in `runtime_installs` — is a migration and a response member for
/// something re-derivable from the index at any time.
///
/// [`None`] for the native case, so a caller writes no branch of its own. See
/// [ADR 0023](../../../.claude/decisions/0023-an-arm64-windows-machine-runs-the-x86_64-build.md),
/// whose whole rule is that this is automatic and never silent.
pub(crate) fn emulation_notice(
    what: &str,
    version: &str,
    selection: Selection<'_>,
) -> Option<String> {
    (selection.execution == Execution::Emulated).then(|| {
        format!(
            "no {} build of {what} {version} is published, so the {} one is being installed and \
             this system will run it under emulation",
            std::env::consts::ARCH,
            selection.artifact.arch.as_str(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One package, published for one target — enough to hand [`emulation_notice`] a selection.
    fn published_for(os: index::Os, arch: index::Arch) -> Package {
        serde_json::from_value(serde_json::json!({
            "kind": "php", "version": "8.3.33", "channel": "stable",
            "artifacts": [{
                "os": os.as_str(), "arch": arch.as_str(),
                "url": "https://example.invalid/php.zip",
                "sha256": "00", "size": 1,
                "provides": { "php": "php.exe" }
            }]
        }))
        .expect("the published shape parses")
    }

    /// The notice is the whole of what an emulated install owes a person — roadmap task **T92**.
    ///
    /// It is composed here rather than asserted through a job, because what a job carries is a
    /// string and this is the only place that decides what the string says.
    #[test]
    fn an_emulated_install_says_what_it_is_about_to_do() {
        let package = published_for(index::Os::Windows, index::Arch::X86_64);

        let native = package
            .select(Target::new(index::Os::Windows, index::Arch::X86_64))
            .expect("its own build");
        assert_eq!(
            emulation_notice("php", "8.3.33", native),
            None,
            "a native install has nothing to explain"
        );

        let emulated = package
            .select(Target::new(index::Os::Windows, index::Arch::Aarch64))
            .expect("the x86_64 build is what that machine can run");
        let said = emulation_notice("php", "8.3.33", emulated).expect("an emulated one does");

        assert!(
            said.contains("php 8.3.33"),
            "it names what is being installed: {said}"
        );
        assert!(said.contains("x86_64"), "and which build: {said}");
        assert!(
            said.contains("emulation"),
            "and that this system will emulate it: {said}"
        );
    }

    /// **A mirror is one setting, not two** — roadmap task **T81**. The registry sits beside the
    /// index it was published with, so pointing at a mirror points at both.
    #[test]
    fn the_registry_is_the_index_url_with_the_other_document_s_name() {
        let source = IndexSource {
            url: "https://mirror.example/mixengine/index.json".to_owned(),
            public_key: "irrelevant".to_owned(),
        };

        assert_eq!(
            source.registry_url(),
            "https://mirror.example/mixengine/extensions.json"
        );
    }

    /// And what MixEngine publishes is what the compiled-in constant says, rather than something
    /// derived that happens to agree with it today.
    #[test]
    fn the_default_pair_is_what_this_build_publishes() {
        assert_eq!(
            IndexSource::default().registry_url(),
            mixengine_core::extensions::registry::DEFAULT_URL
        );
    }
}
