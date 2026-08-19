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

use mixengine_core::index::{self, Arch, Artifact, Index, Os, Package};
use mixengine_core::install::Installer;
use mixengine_core::{Paths, Store, paths, resolve, runtimes};
use mixengine_proto::{
    Error, ErrorCode, JobId, JobKind, JobSummary, PackageVersion, ResolvedRuntime,
    RuntimeCatalogue, RuntimeFilter, RuntimeKind, RuntimeList, RuntimeQuestion, RuntimeRelease,
    RuntimeRemoval, RuntimeSummary, RuntimeTarget, ServiceState, Timestamp, rpc,
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
    ) -> Arc<Self> {
        Arc::new(Self {
            paths: paths.clone(),
            store: store.clone(),
            jobs,
            fetcher,
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

                let bytes = catalogue
                    .index
                    .artifact(kind.as_str(), &package.version)
                    .map_or(0, |artifact| artifact.size);

                runtimes.push(RuntimeRelease {
                    installed: installed
                        .iter()
                        .any(|have| have.kind == kind && have.version == version),
                    kind,
                    version,
                    channel: package.channel.into(),
                    eol: package.eol.clone(),
                    bytes,
                });
            }
        }

        Ok(RuntimeCatalogue {
            runtimes,
            stale: catalogue.freshness.is_stale(),
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
    async fn perform(
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
        let (package, artifact) = offered(
            &catalogue.index,
            kind.as_str(),
            version.as_str(),
            &format!("mix runtime available --kind {kind}"),
        )?;

        let into = runtimes::directory(&self.paths, kind, version);
        if let Some(parent) = into.parent() {
            paths::create_dir(parent).map_err(|error| error.to_wire())?;
        }

        let smoke = runtimes::smoke_test(kind);
        let installed = self
            .fetcher
            .installer
            .install(artifact, &into, Some(&smoke), handle)
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
                url: artifact.url.clone(),
                sha256: artifact.sha256.clone(),
                // Recorded because the shim reads it, months later and with nothing to ask: which
                // file inside the directory is `php` is the publisher's layout, not ours.
                provides: artifact.provides.clone(),
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
        match mixengine_core::services::pools::ensure(&self.store, &crate::services::catalogue())
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
    /// **A running pool refuses it** — roadmap task T32, and the first refusal this method has ever
    /// been able to make. [runtime-versions.md] promised two, and the other half is still open: a
    /// *project* pinning the version is unchecked because there are no projects until Phase 4.
    ///
    /// [runtime-versions.md]: ../../../.claude/features/runtime-versions.md
    ///
    /// # Errors
    ///
    /// `not_found` when it is not installed, `precondition_failed` when the pool that runs out of it
    /// has not been stopped, and the wire error of a directory that could not be removed — on
    /// Windows, most often a process still running out of it.
    pub(crate) async fn uninstall(&self, target: &RuntimeTarget) -> Result<RuntimeRemoval, Error> {
        let removed = runtimes::record(&self.store, target.kind, &target.version)
            .await
            .map_err(|error| error.to_wire())?;

        // A PHP whose pool is running is a PHP something is serving sites out of, and removing the
        // directory under it would leave a process with no files and a row naming a runtime that is
        // gone.
        if let Some(service) =
            mixengine_core::services::pools::of(&self.store, target.kind, &target.version)
                .await
                .map_err(|error| error.to_wire())?
        {
            let record = mixengine_core::services::record(&self.store, &service)
                .await
                .map_err(|error| error.to_wire())?;

            if !matches!(record.state, ServiceState::Stopped | ServiceState::Failed) {
                return Err(Error::new(
                    ErrorCode::PreconditionFailed,
                    format!("{service} is {}", record.state.as_str()),
                )
                .with_hint(format!("`mix service stop {service}` first")));
            }

            // The row goes before the directory, which is the reverse of the rule the directory
            // follows — and is right for the same reason: a `services` row whose runtime is gone is
            // a row every `service.*` call fails on, where a directory with no row is invisible.
            mixengine_core::services::delete(&self.store, &service)
                .await
                .map_err(|error| error.to_wire())?;

            tracing::info!(%service, "a pool was removed with the runtime it ran out of");
        }

        runtimes::discard(Path::new(&removed.path))
            .await
            .map_err(|error| error.to_wire())?;

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
) -> Result<(&'a Package, &'a Artifact), Error> {
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
    // should install x86_64 artifacts, because that is what it can execute.
    let (Some(os), Some(arch)) = (Os::host(), Arch::host()) else {
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
        .artifacts
        .iter()
        .find(|artifact| artifact.os == os && artifact.arch == arch)
        .map(|artifact| (package, artifact))
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
