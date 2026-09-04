//! Checking whether a newer MixEngine exists, and putting one on this machine — roadmap task
//! **T88**.
//!
//! # The whole sequence happens here, and `mix` prompts and reconnects
//!
//! `mixengine-cli` may depend on `mixengine-platform` and `mixengine-proto` and on nothing else —
//! `workspace_layering.rs` enforces it — and verifying a signature, unpacking an archive and
//! swapping files are all `mixengine-core`'s. So the client's whole part in an update is asking,
//! waiting for the endpoint to go quiet, and starting the new daemon, which is `Autostart::run` and
//! has been there since T9.
//!
//! # Silent on failure, and that is a requirement rather than a shrug
//!
//! `.claude/features/updates.md`: *an offline machine must never see an error, and never a slower
//! startup*. Both background callers here — the check at start and the 24 h clock — log at `debug!`
//! and change nothing when the network is not there, and `mixengine_core::index::Client` keeps the
//! last document it verified rather than losing it. What a person who *asked* gets is different:
//! `mix self-update` reports the transport failure, because they are standing there.
//!
//! # What this module deliberately cannot do
//!
//! Elevate. Nothing here has an elevation path and nothing here ever will: an updater that could ask
//! for root would be the local privilege-escalation vector `.claude/features/updates.md` calls the
//! single most important rule on the page. A copy of MixEngine installed where this account cannot
//! write is refused in words, before a byte is downloaded — `mixengine_core::updates::placement`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;

use mixengine_core::install::{Installer, Watcher};
use mixengine_core::paths::Paths;
use mixengine_core::store::Store;
use mixengine_core::updates::{self, Feed};
use mixengine_proto::{
    Error, ErrorCode, ServiceId, Timestamp, UpdateApplied, UpdateDecision, UpdateOffer,
    UpdatePlacement, UpdateRelease, UpdateStatus,
};

use crate::error::ToWire as _;

/// Where the update feed comes from, and what verifies it.
///
/// The index's mechanism verbatim — `--update-url` requires `--update-key`, and neither is read
/// below `main`. A URL that could move while the key could not would be a setting that can only ever
/// fail, since nobody but us can sign with our key.
#[derive(Debug, Clone)]
pub(crate) struct FeedSource {
    /// Where `latest.json` is.
    pub(crate) url: String,

    /// The minisign public key it is checked against.
    pub(crate) public_key: String,
}

impl Default for FeedSource {
    fn default() -> Self {
        Self {
            url: updates::DEFAULT_URL.to_owned(),
            public_key: updates::PUBLIC_KEY.to_owned(),
        }
    }
}

/// The last feed this daemon verified.
#[derive(Debug, Clone)]
struct Checked {
    /// What it said.
    feed: Feed,

    /// When this daemon read it.
    at: Timestamp,

    /// Whether that reading came out of a cache the daemon could not refresh.
    stale: bool,
}

/// A payload that is on disk and has been run once.
#[derive(Debug)]
pub(crate) struct Staged {
    /// Where the install's binaries are.
    pub(crate) directory: PathBuf,

    /// Where the payload was unpacked.
    pub(crate) staged: PathBuf,

    /// Executable name to its path inside the payload.
    pub(crate) provides: BTreeMap<String, String>,

    /// The version the feed says this payload is.
    pub(crate) to: String,
}

/// Everything `update.*` needs.
#[derive(Debug)]
pub(crate) struct Updates {
    /// Where the staging directory goes, and where the partial download lives.
    paths: Paths,

    /// Where skip, later and the restore records are kept.
    store: Store,

    /// The verified feed, cached under `cache/`.
    client: updates::Client,

    /// The download pipeline, with its partial downloads in the same place.
    installer: Installer,

    /// Whether this copy of MixEngine may replace itself, read once at start.
    ///
    /// **Once and not per call**: it is a property of how MixEngine was installed, the answer cannot
    /// change while this daemon runs without somebody having moved its binaries underneath it, and
    /// a probe on every `daemon.status` would be a file created and removed on every status line.
    placement: updates::Placement,

    /// The event stream, for [`DaemonEvent::UpdateAvailable`](mixengine_proto::DaemonEvent).
    events: crate::api::Events,

    /// The last feed read, and how.
    last: Mutex<Option<Checked>>,

    /// The versions this daemon has already announced.
    ///
    /// `certs::renewal`'s `newly` rule: a producer reports a change and not a heartbeat, and a check
    /// that runs every 24 h for a month must not spend a client's stream allowance restating one
    /// fact.
    announced: Mutex<BTreeSet<String>>,
}

/// A [`Watcher`] that reports to nobody.
///
/// **`update.apply` is not a job**, and that is argued rather than assumed: a job whose completion
/// is the daemon exiting is a job nothing can ever observe finishing, and `mix`'s HTTP client sets
/// no request timeout, so a call that takes two minutes cannot fail for being long. What a payload
/// of this size needs is its size printed before the fetch starts, which is what the consent prompt
/// already prints.
struct Quiet;

impl Watcher for Quiet {
    async fn report(&self, percent: u8, message: &str) {
        tracing::debug!(percent, message, "updating");
    }

    fn is_cancelled(&self) -> bool {
        false
    }
}

impl Updates {
    /// Point a feed client and an installer at `source`, caching under the home's `cache/`.
    ///
    /// # Errors
    ///
    /// The wire error of a public key that is not one, or of an HTTP client that cannot be built.
    /// Both fail the daemon's start rather than the first call, on `runtimes::Fetcher`'s reasoning:
    /// a daemon that can never verify an update should say so while somebody is watching.
    pub(crate) fn new(
        paths: &Paths,
        store: &Store,
        source: &FeedSource,
        daemon_exe: Option<&std::path::Path>,
        events: crate::api::Events,
    ) -> Result<std::sync::Arc<Self>, Error> {
        let placement = match daemon_exe {
            Some(exe) => updates::placement::of(
                exe,
                std::env::var_os(APPIMAGE)
                    .filter(|value| !value.is_empty())
                    .as_deref(),
            ),
            // A daemon whose own path the operating system will not name. Refused in words rather
            // than assumed writable: this is the one field an update is not allowed to guess at.
            None => updates::Placement::Managed {
                directory: PathBuf::new(),
                because:
                    "this operating system will not say where this daemon's own binary is, so \
                          MixEngine cannot tell whether it may replace it"
                        .to_owned(),
            },
        };

        Ok(std::sync::Arc::new(Self {
            paths: paths.clone(),
            store: store.clone(),
            client: updates::Client::with(&source.url, &source.public_key, paths.cache())
                .map_err(|error| error.to_wire())?,
            installer: Installer::new(paths.cache()).map_err(|error| error.to_wire())?,
            placement,
            events,
            last: Mutex::new(None),
            announced: Mutex::new(BTreeSet::new()),
        }))
    }

    /// The one line `daemon.status` carries, or [`None`].
    ///
    /// Every failure is [`None`]: a settings row that will not read and a daemon that has not
    /// checked mean the same thing to a client, which is that there is nothing to show.
    pub(crate) async fn offer(&self) -> Option<UpdateOffer> {
        let checked = self.last.lock().ok()?.clone()?;
        let decision = self.decision(&checked.feed).await;

        decision.offered.then(|| UpdateOffer {
            version: checked.feed.version.clone(),
            published_at: checked.feed.published_at.to_string(),
        })
    }

    /// `update.status` — what this daemon knows, without going to the network.
    pub(crate) async fn status(&self, will_restart: Vec<ServiceId>) -> UpdateStatus {
        let current = env!("CARGO_PKG_VERSION").to_owned();
        let placement = placement(&self.placement);

        let Some(checked) = self.last.lock().ok().and_then(|last| last.clone()) else {
            return UpdateStatus {
                current,
                available: None,
                offered: false,
                because: None,
                checked_at: None,
                stale: false,
                placement,
                will_restart,
            };
        };

        let decision = self.decision(&checked.feed).await;

        UpdateStatus {
            current,
            available: Some(release(&checked.feed)),
            offered: decision.offered,
            because: decision.because,
            checked_at: Some(checked.at),
            stale: checked.stale,
            placement,
            will_restart,
        }
    }

    /// Read the published feed, and remember what it said.
    ///
    /// **Never an error to the caller when there is a cached document**, which is
    /// `index::Client`'s own rule: an unreachable server, a signature that does not verify and a
    /// feed offered from before the one held all mean the same thing to this call, and the last
    /// document verified is still the last document verified.
    ///
    /// # Errors
    ///
    /// The wire error of a fetch that failed with nothing cached to fall back to. The background
    /// callers swallow it; `mix self-update` prints it, because somebody asked.
    pub(crate) async fn check(
        &self,
        force: bool,
        will_restart: Vec<ServiceId>,
    ) -> Result<UpdateStatus, Error> {
        let catalogue = match force {
            true => self.client.refresh().await,
            false => self.client.catalogue().await,
        }
        .map_err(|error| error.to_wire())?;

        let version = catalogue.index.version.clone();
        let published_at = catalogue.index.published_at.to_string();
        let decision = self.decision(&catalogue.index).await;

        if let Ok(mut last) = self.last.lock() {
            *last = Some(Checked {
                feed: catalogue.index,
                at: Timestamp::from_system_time(std::time::SystemTime::now()),
                stale: catalogue.freshness.is_stale(),
            });
        }

        if decision.offered && self.newly(&version) {
            tracing::info!(%version, "a newer MixEngine has been published");

            self.events
                .publish(mixengine_proto::DaemonEvent::UpdateAvailable {
                    version,
                    published_at,
                });
        }

        Ok(self.status(will_restart).await)
    }

    /// `update.decide` — remember *skip this version* or *remind me later*.
    ///
    /// # Errors
    ///
    /// The wire error of a settings row that could not be written.
    pub(crate) async fn decide(
        &self,
        version: &str,
        decision: UpdateDecision,
        will_restart: Vec<ServiceId>,
    ) -> Result<UpdateStatus, Error> {
        match decision {
            UpdateDecision::Skip => {
                updates::records::set(&self.store, updates::records::SKIPPED_VERSION, &version)
                    .await
                    .map_err(|error| error.to_wire())?;
            }

            UpdateDecision::Later => {
                let due = Timestamp(
                    Timestamp::from_system_time(std::time::SystemTime::now()).0
                        + updates::records::LATER_SECONDS * 1_000,
                );

                updates::records::set(&self.store, updates::records::REMIND_AFTER, &due)
                    .await
                    .map_err(|error| error.to_wire())?;
            }
        }

        Ok(self.status(will_restart).await)
    }

    /// Everything an apply does before anything is stopped: check, refuse, download, verify, unpack,
    /// smoke-test.
    ///
    /// **In that order and before the stop** — the T88 design, D5. Taken literally,
    /// `.claude/features/updates.md`'s *"stop → download → verify → install"* would leave a
    /// developer's database down for the length of a download, on a connection nobody promised
    /// anything about, to gain nothing: a download that fails after the stop has cost an outage, and
    /// one that succeeds could have happened while everything was still up.
    ///
    /// # Errors
    ///
    /// `precondition_failed` when `version` is no longer what the feed offers or when this copy of
    /// MixEngine is one a package manager installed; and whatever the download, the checksum, the
    /// unpacking or the smoke test reported. Every one of them leaves the installed binaries
    /// untouched.
    pub(crate) async fn stage(&self, version: &str) -> Result<Staged, Error> {
        let checked = self
            .last
            .lock()
            .ok()
            .and_then(|last| last.clone())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::PreconditionFailed,
                    "this daemon has not read the update feed yet",
                )
                .with_hint("`mix self-update --check` reads it")
            })?;

        if checked.feed.version != version {
            return Err(mixengine_core::Error::UpdateNotOffered {
                asked: version.to_owned(),
                offered: Some(checked.feed.version.clone()),
            }
            .to_wire());
        }

        let updates::Placement::SelfUpdatable { directory } = &self.placement else {
            let updates::Placement::Managed { directory, because } = &self.placement else {
                unreachable!("Placement has two variants and the other one is matched above")
            };

            return Err(mixengine_core::Error::UpdateNotWritable {
                directory: directory.clone(),
                because: because.clone(),
            }
            .to_wire());
        };

        let (os, arch) = host()?;
        let artifact = checked
            .feed
            .artifact(os, arch)
            .ok_or_else(|| {
                mixengine_core::Error::UpdateUnavailable {
                    os: format!("{os:?}").to_lowercase(),
                    arch: format!("{arch:?}").to_lowercase(),
                }
                .to_wire()
            })?
            .clone();

        let into = self.staging_for(version);
        let staged = updates::apply::stage(&self.installer, &artifact, &into, &Quiet)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(Staged {
            directory: directory.clone(),
            staged,
            provides: artifact.provides,
            to: checked.feed.version,
        })
    }

    /// Write down what is about to happen, before the binaries move.
    ///
    /// Read by the daemon that comes up next — or, when the swap fails, by the rollback in this
    /// same process. That is why it is written before rather than after: both paths read it, and one
    /// of them never gets to a line that ran later.
    ///
    /// # Errors
    ///
    /// The wire error of a settings row that could not be written.
    pub(crate) async fn remember(&self, to: &str, restore: &[ServiceId]) -> Result<(), Error> {
        let applied = updates::records::Applied {
            from: env!("CARGO_PKG_VERSION").to_owned(),
            to: to.to_owned(),
            at: Timestamp::from_system_time(std::time::SystemTime::now()),
        };
        let restore: Vec<String> = restore.iter().map(|id| id.as_str().to_owned()).collect();

        updates::records::set(&self.store, updates::records::APPLIED, &applied)
            .await
            .map_err(|error| error.to_wire())?;
        updates::records::set(&self.store, updates::records::RESTORE, &restore)
            .await
            .map_err(|error| error.to_wire())
    }

    /// Undo an update that got as far as the stop and no further.
    ///
    /// **The half of the rollback that `swap` cannot do.** `swap` puts every binary it moved back
    /// under its own name; what it cannot put back is the *stop* that preceded it, and nothing else
    /// on the machine intends to. So this starts the services again and forgets the records — and it
    /// deliberately does **not** run the version check [`Updates::restore_after_update`] runs: this
    /// daemon is still the version it was, nothing was installed, and marking the release as skipped
    /// over a full disk would refuse it for ever.
    ///
    /// Failures are logged and not returned: the caller is already reporting the failure that
    /// brought it here, and a second one about the tidying up would replace the diagnosis with the
    /// housekeeping.
    pub(crate) async fn roll_back(&self, services: &crate::services::Registry) {
        let restore: Option<Vec<String>> = read(&self.store, updates::records::RESTORE).await;

        for key in [updates::records::APPLIED, updates::records::RESTORE] {
            if let Err(error) = updates::records::clear(&self.store, key).await {
                tracing::warn!(key, %error, "an abandoned update's record could not be removed");
            }
        }

        if let Some(restore) = restore {
            self.start_again(services, &restore).await;
        }
    }

    /// The pass a daemon makes at start when the one before it replaced these binaries.
    ///
    /// Four things, in this order:
    ///
    /// 1. **Read both records and delete them**, before either is acted on. A record that survived
    ///    being read is replayed by every later start — so somebody who updates, stops MariaDB
    ///    because they are done with it, and reboots would get MariaDB back for ever.
    /// 2. **Compare this build's own version with what the feed said the payload was.** This is the
    ///    first moment anything can answer that honestly, because the running binary is the only
    ///    thing that knows what it is. A mismatch writes `updates.skipped_version` and warns, so a
    ///    mislabelled release costs one pointless update instead of being offered every 24 h for
    ///    ever.
    /// 3. **Remove the `.old` files.** A daemon that is answering has proved they are not needed —
    ///    which is exactly the condition for discarding the only way back. On Windows the
    ///    `mix.exe.old` still held open by the `mix` that ran the update is left for the start after
    ///    this one.
    /// 4. **Start what was running.** In the reverse of the order it was stopped in, dependencies
    ///    first, through the same graph `service.start` uses — a client that read the list and
    ///    issued the calls itself would be deciding an order, which `CLAUDE.md` forbids.
    pub(crate) async fn restore_after_update(
        &self,
        services: &crate::services::Registry,
        replaced: &[String],
    ) {
        let applied: Option<updates::records::Applied> =
            read(&self.store, updates::records::APPLIED).await;
        let restore: Option<Vec<String>> = read(&self.store, updates::records::RESTORE).await;

        if applied.is_none() && restore.is_none() {
            return;
        }

        for key in [updates::records::APPLIED, updates::records::RESTORE] {
            if let Err(error) = updates::records::clear(&self.store, key).await {
                tracing::warn!(key, %error, "an update's record could not be removed after it was read");
            }
        }

        if let Some(applied) = &applied {
            let running = env!("CARGO_PKG_VERSION");

            if applied.to == running {
                tracing::info!(from = %applied.from, to = %applied.to, "this daemon is the update");
            } else {
                tracing::warn!(
                    expected = %applied.to,
                    running,
                    "the release that was installed is not the version its feed declared; it will \
                     not be offered again"
                );

                if let Err(error) = updates::records::set(
                    &self.store,
                    updates::records::SKIPPED_VERSION,
                    &applied.to,
                )
                .await
                {
                    tracing::warn!(%error, "a mislabelled release could not be marked as skipped");
                }
            }
        }

        if let updates::Placement::SelfUpdatable { directory } = &self.placement {
            let discarded = updates::apply::discard_old(directory, replaced);
            tracing::debug!(discarded, "removed what an update kept as its way back");
        }

        let Some(restore) = restore else {
            return;
        };

        self.start_again(services, &restore).await;
    }

    /// Start the services an update stopped, dependencies first.
    async fn start_again(&self, services: &crate::services::Registry, restore: &[String]) {
        let wanted: Vec<ServiceId> = restore
            .iter()
            .filter_map(|id| ServiceId::parse(id).ok())
            .collect();

        if wanted.is_empty() {
            return;
        }

        let graph = match services.graph().await {
            Ok(graph) => graph,
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "this home's services could not be read, so nothing an update stopped was \
                     started again"
                );
                return;
            }
        };

        let plan = match graph.start_plan(wanted.iter()) {
            Ok(plan) => plan,
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "no order could be worked out for the services an update stopped"
                );
                return;
            }
        };

        let walk = services.start(&graph, &plan).await;

        tracing::info!(
            started = walk.reached.len(),
            wanted = wanted.len(),
            refused = walk.failed.as_ref().map(|(id, _)| id.as_str()),
            "started the services an update stopped"
        );
    }

    /// Where a payload is unpacked. One directory per version, under `cache/`.
    fn staging_for(&self, version: &str) -> PathBuf {
        // Not `join`ed from the feed's string without thought: a version out of a document is a
        // path component here, so it goes through the same validation every runtime version does,
        // and anything that will not parse lands in one fixed directory instead.
        let component = mixengine_proto::PackageVersion::parse(version).map_or_else(
            |_| "unnamed".to_owned(),
            |version| version.as_str().to_owned(),
        );

        self.paths.cache().join(STAGING_DIR).join(component)
    }

    /// Whether this version is one nobody has been told about yet.
    fn newly(&self, version: &str) -> bool {
        self.announced
            .lock()
            .is_ok_and(|mut announced| announced.insert(version.to_owned()))
    }

    /// Whether a feed's release is offered here, and if not, why.
    async fn decision(&self, feed: &Feed) -> updates::Decision {
        let skipped: Option<String> = read(&self.store, updates::records::SKIPPED_VERSION).await;
        let remind_after: Option<Timestamp> =
            read(&self.store, updates::records::REMIND_AFTER).await;
        let has_build = host().is_ok_and(|(os, arch)| feed.artifact(os, arch).is_some());

        updates::offer::decide(
            env!("CARGO_PKG_VERSION"),
            &feed.version,
            has_build,
            skipped.as_deref(),
            remind_after,
            Timestamp::from_system_time(std::time::SystemTime::now()),
        )
    }
}

/// The environment variable a running AppImage sets, and the one thing that identifies one.
const APPIMAGE: &str = "APPIMAGE";

/// Where payloads are unpacked, under the home's `cache/`.
const STAGING_DIR: &str = "updates";

/// Read one record, treating every failure as absent.
///
/// The whole of this module's error policy for the `settings` table in one place: a row that cannot
/// be read means what an absent one means — go and ask again — and no update question is worth
/// failing a `daemon.status` over.
async fn read<T: serde::de::DeserializeOwned>(store: &Store, key: &str) -> Option<T> {
    match updates::records::get(store, key).await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(key, %error, "an update record could not be read");
            None
        }
    }
}

/// This machine's pair, as the feed spells it.
///
/// # Errors
///
/// `unsupported` on an operating system or architecture this product does not publish for, which is
/// a build nobody made rather than a machine anybody has.
fn host() -> Result<(mixengine_core::index::Os, mixengine_core::index::Arch), Error> {
    let os = mixengine_core::index::Os::host();
    let arch = mixengine_core::index::Arch::host();

    match (os, arch) {
        (Some(os), Some(arch)) => Ok((os, arch)),
        _ => Err(Error::new(
            ErrorCode::UnsupportedPlatform,
            format!(
                "MixEngine publishes no builds for {}/{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        )),
    }
}

/// The wire shape of a placement.
fn placement(placement: &updates::Placement) -> UpdatePlacement {
    match placement {
        updates::Placement::SelfUpdatable { directory } => UpdatePlacement::SelfUpdatable {
            directory: directory.display().to_string(),
        },
        updates::Placement::Managed { directory, because } => UpdatePlacement::Managed {
            directory: directory.display().to_string(),
            because: because.clone(),
        },
    }
}

/// The wire shape of a release, sized for *this* machine.
///
/// `size` is the payload this machine would download and not the largest one published: it is shown
/// in a consent prompt, and a number from another architecture would be a number about somebody
/// else's download.
fn release(feed: &Feed) -> UpdateRelease {
    let size = host()
        .ok()
        .and_then(|(os, arch)| feed.artifact(os, arch))
        .map_or(0, |artifact| artifact.size);

    UpdateRelease {
        version: feed.version.clone(),
        published_at: feed.published_at.to_string(),
        notes: feed.notes.clone(),
        notes_url: feed.notes_url.clone(),
        size,
    }
}

/// What `update.apply` answers, built from what the swap actually did.
pub(crate) fn applied(
    staged: &Staged,
    swapped: &mixengine_core::updates::Swapped,
    restarting: Vec<ServiceId>,
) -> UpdateApplied {
    UpdateApplied {
        from: env!("CARGO_PKG_VERSION").to_owned(),
        to: staged.to.clone(),
        directory: staged.directory.display().to_string(),
        replaced: swapped.replaced.clone(),
        kept: swapped.kept.clone(),
        restarting,
    }
}

/// Read the feed at start, then again every `every`, until `shutdown` — roadmap task **T88**.
///
/// **The first tick is thrown away**, on `certs::renewal`'s reasoning: [`tokio::time::interval`]
/// completes its first immediately, and the start check a few lines below has just run.
///
/// **The start check uses the cache and the clock does not**, which is the whole difference between
/// them: a daemon restarted ten times in an hour should make one request, and a daemon that has been
/// up for a day should make one. So the first is `catalogue()` and every one after it is `refresh()`.
///
/// **Spawned and never awaited.** A start that waited on a network read would be a start that an
/// offline machine pays for, which `.claude/features/updates.md` forbids in as many words.
pub(crate) fn start(
    updates: std::sync::Arc<Updates>,
    every: std::time::Duration,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        // Silent, and `debug!` rather than `warn!`: this is the path an offline machine takes every
        // time, and a warning per day about a laptop that was on a train is how a log stops being
        // read.
        if let Err(error) = updates.check(false, Vec::new()).await {
            tracing::debug!(%error, "the update feed could not be read at start");
        }

        let mut ticker = tokio::time::interval(every);
        ticker.tick().await;

        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = ticker.tick() => {}
            }

            if let Err(error) = updates.check(true, Vec::new()).await {
                tracing::debug!(%error, "the update feed could not be read");
            }
        }
    });
}
