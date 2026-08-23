//! The queue of privileged operations this daemon is holding, and the only thing that raises a
//! prompt. Roadmap task **T40b**.
//!
//! **The division is the one [`crate::jobs`] documents, one table across.**
//! [`mixengine_core::elevation`] owns the row, the document and the report and has no loop, no clock
//! and no task; what owns the timing, the cancellation the grant hangs off and the `Events` the
//! batch is announced on is here.
//!
//! **This daemon never raises a prompt on its own initiative.**
//! `.claude/architecture/daemon-and-ipc.md` already carries the rule: *a method that writes outside
//! `MIXENGINE_HOME` is never called on the daemon's own initiative* (T26). Everything the helper
//! will ever do — the hosts file, the trust store, the resolver, a firewall rule — is outside the
//! home by definition; that is why it needs root. So enqueuing and flushing have two different
//! triggers: producers enqueue, and only a client calls `elevation.grant`. A fresh install where
//! nobody ever does is a machine in degraded mode forever, and that is the correct behaviour.
//!
//! **T41's `HostsApply` is the first producer.** [`Elevation::enqueue`] ships with none, which is the
//! deliberate position rather than an oversight — the same one T22 took with the job registry and
//! T19 with the service runner: the alternative is writing the queue twice, once inside the first
//! producer and once properly afterwards.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use mixengine_core::{Paths, Store};
use mixengine_platform::{ElevationSupport, Host};
use mixengine_proto::privileged::{ElevationOutcome, PrivilegedOp};
use mixengine_proto::{
    DaemonEvent, ElevationDrop, ElevationStatus, ElevationSummary, Error, ErrorCode, GrantOutcome,
    JobId, JobKind, JobSummary, Timestamp, rpc,
};

use crate::api::Events;
use crate::error::ToWire as _;

/// The single grant slot — the runtime half of "no code path elevates in a loop".
///
/// Two concurrent grants are two prompts for one queue, which is the defect ADR 0005 names, and
/// refusing the second is the only answer that cannot itself become a loop.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// Nothing is being granted.
    #[default]
    Free,

    /// A grant has been accepted and its job row is not written yet.
    ///
    /// Its own state rather than a placeholder id: the row is written by `Jobs::begin`, and a
    /// `JobId(0)` held in the meantime would be a number a client could be told.
    Reserved,

    /// A grant is running, and this is the job to wait on.
    Running(JobId),
}

/// What this daemon knows that outlives a single grant but not a restart.
#[derive(Debug, Default)]
struct State {
    /// The one grant at a time.
    slot: Slot,

    /// What the most recent one did — in memory, deliberately. See
    /// [`ElevationStatus::last`](mixengine_proto::ElevationStatus).
    last: Option<GrantOutcome>,
}

/// The queue, the machine that can be asked about it, and the only door into a prompt.
#[derive(Debug)]
pub(crate) struct Elevation {
    /// Where the rows live.
    store: Store,

    /// How a batch is announced.
    events: Events,

    /// The registry a grant becomes a job in.
    jobs: Arc<crate::jobs::Jobs>,

    /// The OS: `probe()` for the degraded mode, `run()` for the grant.
    host: Arc<dyn Host>,

    /// `MIXENGINE_HOME`, which every request names and the helper checks the ownership of.
    home: PathBuf,

    /// `<root>/run/elevate` — the parent of every single-use request directory.
    elevate: PathBuf,

    /// The program that is running, which is what the helper is found beside (D9).
    program: PathBuf,

    /// Whether **this daemon** holds an administrative token, read once at construction.
    ///
    /// Reported and not refused — the T40b design, D10. Read once because it cannot change under a
    /// running process, and reading it per request would be a syscall per `mix status`.
    elevated: bool,

    /// Which of the two name mechanisms this home is on — roadmap task **T44**.
    ///
    /// Read by [`require_hosts`](Elevation::require_hosts) and by nothing else here: whether a
    /// managed name resolves through DNS decides whether the hosts file needs to hold it at all.
    dns: Arc<crate::dns::Dns>,

    /// The grant slot and the last outcome.
    state: Mutex<State>,
}

impl Elevation {
    /// A registry with nothing waiting and nothing running.
    pub(crate) fn new(
        paths: &Paths,
        store: &Store,
        events: Events,
        jobs: Arc<crate::jobs::Jobs>,
        host: Arc<dyn Host>,
        program: PathBuf,
        dns: Arc<crate::dns::Dns>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: store.clone(),
            events,
            jobs,
            elevated: mixengine_platform::elevated::is_elevated(),
            host,
            home: paths.root().to_path_buf(),
            elevate: paths.run().join("elevate"),
            program,
            dns,
            state: Mutex::new(State::default()),
        })
    }

    /// Put an operation in the queue, and announce the batch when that changed something.
    ///
    /// [`require_hosts`](Self::require_hosts) is the one caller, and T41 made it the first: the
    /// queue and its event landed with T40b so that T41 would be one operation rather than an
    /// operation plus a mechanism.
    ///
    /// # Errors
    ///
    /// The wire error of a row that could not be written.
    pub(crate) async fn enqueue(&self, op: &PrivilegedOp) -> Result<(), Error> {
        let at = Timestamp::from_system_time(SystemTime::now());

        let announced = mixengine_core::elevation::enqueue(&self.store, op, at)
            .await
            .map_err(|error| error.to_wire())?;

        // `None` is the operation that was already waiting. The machine's needs did not change, so
        // there is nothing to announce — an event per attempt would put a producer's retry loop on a
        // client's screen. See the T40b design, D8.
        if let Some(pending) = announced {
            self.events
                .publish(DaemonEvent::ElevationRequired { pending });
        }

        Ok(())
    }

    /// This machine, for the one other thing in this daemon that reads it — roadmap task **T46**.
    ///
    /// **Reached through here rather than built again.** A `Host` is a handful of trait objects and
    /// a second one is cheap, but two of them are two answers to "what does this machine's hosts
    /// file hold" — and the diagnostic exists to report the answer *this queue acted on*.
    pub(crate) fn host(&self) -> Arc<dyn Host> {
        Arc::clone(&self.host)
    }

    /// Ask for the hosts file to say what this home's sites say it should — roadmap task **T41**.
    ///
    /// **The disk is read before a prompt is spent** (T41 design, D11). A machine that already
    /// agrees needs nothing, and enqueueing anyway would put a row on `mix status` whose only
    /// possible outcome is `AlreadyDone`.
    ///
    /// **Here rather than on `Sites`** because this object already holds the `Host` and already owns
    /// the "is this worth a prompt" question. `Sites` gains one dependency and three call sites —
    /// after a successful `create`, `update` and `delete`, and never before, so a failed create asks
    /// for nothing.
    ///
    /// A read that fails does not stop the operation being queued: the helper is the authority on
    /// what is in that file, and it will refuse with the reason on the screen T64 built.
    ///
    /// **What the block should hold depends on which TLDs this machine routes here** — roadmap task
    /// **T45**, design D6. A TLD the resolver sends to our server is answered by pattern, so a hosts
    /// entry under it adds nothing; a TLD nothing routes has no other mechanism. Asking for a block
    /// without the routed names is also what clears one an earlier, unwired daemon left behind —
    /// skipping the queue instead would leave those stale names resolving to loopback for ever.
    ///
    /// **Per TLD rather than per mode**, which is T45's correction to T44: T44 computed the whole
    /// block from one home-wide `DnsMode`, which was right only while nothing could be wired at all.
    /// Every mechanism there is scopes to one TLD, and `.local` is never routed — so a home with
    /// both `blog.test` and `shop.local` needs a block with exactly one line in it.
    ///
    /// # Errors
    ///
    /// The wire error of a home whose sites cannot be read, or whose row cannot be written.
    pub(crate) async fn require_hosts(&self) -> Result<(), Error> {
        let desired = mixengine_core::hosts::desired(&self.store, &self.dns.wired())
            .await
            .map_err(|error| error.to_wire())?;

        let wanted = PrivilegedOp::hosts_apply(desired);

        // Compared as operations rather than as lists, so the ordering and deduplication are
        // `hosts_apply`'s in both directions and there is one definition of "the same block".
        match self.host.hosts_file().managed() {
            // Not a pattern guard: `present` is a `Vec` and a guard cannot move out of one.
            Ok(present) if PrivilegedOp::hosts_apply(present.clone()) == wanted => Ok(()),
            Ok(_) => self.enqueue(&wanted).await,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "the hosts file cannot be read; asking for permission to write it anyway"
                );

                self.enqueue(&wanted).await
            }
        }
    }

    /// Ask for this machine to send its managed TLDs to this daemon's DNS server — roadmap task
    /// **T45**.
    ///
    /// **Called at every daemon start, beside [`require_port_access`](Self::require_port_access),
    /// and that ordering is what makes M4's promise true.** On a fresh home this queues the
    /// operation *before any site exists*, so the single grant of first-run setup wires the machine;
    /// from then on `site.create` computes a hosts block that already matches the disk, enqueues
    /// nothing and prompts for nothing.
    ///
    /// Asking after the first site is created gets that wrong in a way that is invisible until it is
    /// counted: the block would already hold that site's line, emptying it is a second operation,
    /// and a second operation is a second prompt — which is the acceptance criterion this phase is
    /// measured against.
    ///
    /// **A machine with no DNS server of its own asks for nothing**, because there would be nothing
    /// to route names to; and a machine with no scoped mechanism asks for nothing either, which is
    /// a Linux without systemd and is a mode rather than a failure (the T45 design, D2).
    ///
    /// A probe that fails asks for nothing, as `require_port_access` does and unlike
    /// [`require_hosts`](Self::require_hosts): there the helper is the authority on the file and
    /// will refuse with a reason on the screen T64 built, and here a probe that could not read the
    /// machine has said nothing about what to ask for.
    ///
    /// # Errors
    ///
    /// The wire error of a row that could not be written.
    pub(crate) async fn require_resolver(&self) -> Result<(), Error> {
        let Some(port) = self.dns.wirable_port() else {
            tracing::debug!(
                "no DNS server is answering on a port a resolver could be pointed at, so nothing                  is asked for"
            );
            return Ok(());
        };

        let want: Vec<&str> = mixengine_proto::domains::WIRED_TLDS.to_vec();

        let state = match self.host.resolver().probe(&want, port) {
            Ok(state) => state,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "this machine's resolver cannot be read; asking for nothing"
                );
                return Ok(());
            }
        };

        match state.plan(&want, port) {
            Some(plan) => {
                self.enqueue(&mixengine_proto::privileged::PrivilegedOp::ResolverApply { plan })
                    .await
            }
            None => Ok(()),
        }
    }

    /// The ports a site is reached on. Fixed, per the T42 design, D2: a front end renumbered to 81
    /// is not a front end anybody asked for, and the recipes say so too.
    const ANSWERING: [u16; 2] = [80, 443];

    /// Ask for this machine to let `binary` answer on 80 and 443 — roadmap task **T42**.
    ///
    /// **Called at every daemon start, and that is also the re-probe the roadmap asks for.** A
    /// capability is cleared by any write to the binary, so an update loses it; asking here catches
    /// that, and catches a loss that was not an update, and needs no hook in the updater. What makes
    /// it affordable is that reading the grant back costs one `getxattr` and no privilege at all —
    /// measured, not assumed.
    ///
    /// `None` is a home with no front end: nothing is asked for. **And nothing is ever revoked
    /// here** — the T42 design, D12: on Linux the question needs the binary, which is precisely what
    /// a home with no front end cannot supply, so "no row, therefore withdraw" is a question this
    /// system cannot be asked. Uninstall (T87) is the producer that can.
    ///
    /// A probe that fails asks for nothing, unlike [`require_hosts`](Self::require_hosts): there the
    /// helper is the authority on the file and will refuse with a reason on the screen T64 built,
    /// and here a probe that could not read one attribute has told us nothing about what to ask for.
    ///
    /// # Errors
    ///
    /// The wire error of a row that could not be written.
    pub(crate) async fn require_port_access(&self, binary: Option<&Path>) -> Result<(), Error> {
        let Some(binary) = binary else {
            tracing::debug!("this home has no front end, so nothing needs to answer on 80 or 443");
            return Ok(());
        };

        let state = match self.host.port_access().probe(binary, &Self::ANSWERING) {
            Ok(state) => state,
            Err(error) => {
                tracing::warn!(
                    %error,
                    binary = %binary.display(),
                    "cannot tell whether this machine will let the front end answer on 80 and 443"
                );

                return Ok(());
            }
        };

        if state.granted {
            return Ok(());
        }

        // Derived from the method rather than from a `#[cfg]`, which is the whole reason
        // `PortAccessState` carries one — the T42 design, D1.
        let Some(plan) = state.plan(binary) else {
            return Ok(());
        };

        if let Some(missing) = &state.missing {
            tracing::info!(%missing, "asking for permission to answer on 80 and 443");
        }

        self.enqueue(&PrivilegedOp::PortAccessGrant { plan }).await
    }

    /// `elevation.grant` — spend one prompt on everything that is waiting.
    ///
    /// **A job, and the exception `service.start` earns does not transfer.** What this waits on is a
    /// person reading a dialog: `Elevation::run` blocks with no deadline, and there is no declared
    /// ready timeout to bound it with. So the row exists the moment a client asks, and the work runs
    /// on `spawn_blocking` exactly as the trait's own documentation anticipates.
    ///
    /// **Cancellation is checked before the prompt and after it, and never during.** A cancellation
    /// token cannot close a UAC dialog, and pretending otherwise would report a job as cancelled
    /// while the person at the machine was still looking at a prompt with MixEngine's name on it.
    ///
    /// # Errors
    ///
    /// `precondition_failed` when nothing is waiting — the helper refuses an empty batch outright,
    /// so raising a prompt to discover that would be a dialog for nothing. `dependency_missing` when
    /// there is no helper beside this daemon. `privileged_required`, carrying `probe()`'s reason,
    /// when this machine cannot raise a prompt at all. `conflict`, naming the job already running,
    /// when a grant is in flight.
    pub(crate) async fn grant(self: &Arc<Self>) -> Result<JobSummary, Error> {
        let waiting = mixengine_core::elevation::pending(&self.store)
            .await
            .map_err(|error| error.to_wire())?;

        if waiting.is_empty() {
            return Err(Error::new(
                ErrorCode::PreconditionFailed,
                "nothing is waiting for permission",
            )
            .with_hint("`mix elevation status` lists what would be asked for"));
        }

        // Before the slot is taken, so a machine that cannot prompt is told so without a job row
        // being written and immediately failed.
        let helper =
            mixengine_core::elevation::helper(&self.program).map_err(|error| error.to_wire())?;

        if let Some(reason) = self.reason() {
            return Err(Error::new(
                ErrorCode::PrivilegedRequired,
                format!("this machine cannot raise an elevation prompt: {reason}"),
            ));
        }

        self.reserve()?;

        let elevation = Arc::clone(self);
        let started = self
            .jobs
            .begin(
                &JobKind::parse(rpc::method::ELEVATION_GRANT).expect("a valid kind"),
                move |handle| async move { elevation.flush(&handle, helper, waiting).await },
            )
            .await;

        match started {
            Ok(summary) => {
                // Only while the slot is still `Reserved`: the work may already have finished and
                // released it, and writing the id over a free slot would wedge every later grant.
                let mut state = self
                    .state
                    .lock()
                    .expect("the elevation slot is not held across an await");
                if state.slot == Slot::Reserved {
                    state.slot = Slot::Running(summary.id);
                }

                Ok(summary)
            }
            Err(error) => {
                self.release();
                Err(error)
            }
        }
    }

    /// Take the one grant slot, or say who has it.
    fn reserve(&self) -> Result<(), Error> {
        let mut state = self
            .state
            .lock()
            .expect("the elevation slot is not held across an await");

        match state.slot {
            Slot::Free => {
                state.slot = Slot::Reserved;
                Ok(())
            }
            Slot::Reserved => Err(Error::new(
                ErrorCode::Conflict,
                "a grant is already starting",
            )),
            Slot::Running(job) => Err(Error::new(
                ErrorCode::Conflict,
                format!("job {job} is already asking for permission"),
            )
            .with_hint(format!(
                "`mix job wait {job}` follows the one that is running"
            ))),
        }
    }

    /// Give the slot back.
    fn release(&self) {
        self.state
            .lock()
            .expect("the elevation slot is not held across an await")
            .slot = Slot::Free;
    }

    /// The work: write the batch, raise the one prompt, apply what came back.
    async fn flush(
        &self,
        handle: &crate::jobs::JobHandle,
        helper: PathBuf,
        waiting: Vec<mixengine_proto::PendingOp>,
    ) -> Result<serde_json::Value, Error> {
        // Released however this ends — including through a panic the RPC layer contains, which is
        // the whole reason it is not a line at the bottom.
        let _slot = Released(self);

        if handle.is_cancelled() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "the grant was cancelled before any prompt was raised",
            ));
        }

        // A fresh single-use directory per grant: `response.json`'s existence is the anti-replay
        // check, so a directory that has been answered is finished (T40/D10).
        let directory = self.elevate.join(
            mixengine_platform::generate_secret(16)
                .map_err(|error| mixengine_core::Error::Platform(error).to_wire())?,
        );

        let request = mixengine_core::elevation::write_request(&directory, &self.home, &waiting)
            .map_err(|error| error.to_wire())?;

        handle.progress(20, "asking for permission").await;

        let path = request.path().to_path_buf();
        let machine = Arc::clone(&self.host);
        let raised = tokio::task::spawn_blocking(move || machine.elevation().run(&helper, &path))
            .await
            .map_err(|join| {
                Error::new(
                    ErrorCode::Internal,
                    format!("the elevation prompt could not be waited on: {join}"),
                )
            })?;

        let answer = self.judge(handle, &request, raised, waiting.len()).await;

        // **D8.** The machine may have just been wired, and a daemon that only learned that at its
        // next start would go on writing hosts entries while the user watched their grant do
        // nothing. Unconditional rather than "only if a resolver operation was in the batch": the
        // helper is the authority on what it applied, and a re-read costs one file or one registry
        // key.
        self.dns.reprobe(self.host.as_ref());

        // And the block a hosts-only home accumulated is now redundant, so it is cleared by the
        // grant that made it so rather than by a second prompt a week later. A failure here is
        // logged and not returned: the grant itself succeeded, and reporting it as failed because
        // the follow-up could not be queued would be a worse answer than the truth.
        if let Err(error) = self.require_hosts().await {
            tracing::warn!(%error, "the hosts block could not be reconciled after the grant");
        }

        // On every branch, including the failing ones. The directory is single-use by construction,
        // and leaving one behind would make the next grant's fresh directory the only thing keeping
        // that true — a property worth having in two places rather than one.
        if let Err(error) = std::fs::remove_dir_all(request.directory()) {
            tracing::warn!(
                directory = %request.directory().display(),
                %error,
                "a single-use elevation request directory could not be removed"
            );
        }

        answer
    }

    /// Turn what the prompt answered into a job result, and apply it to the queue.
    async fn judge(
        &self,
        handle: &crate::jobs::JobHandle,
        request: &mixengine_core::elevation::Request,
        raised: mixengine_platform::Result<ElevationOutcome>,
        asked: usize,
    ) -> Result<serde_json::Value, Error> {
        let outcome = raised.map_err(|error| mixengine_core::Error::Platform(error).to_wire())?;

        let (applied, still_pending) = match &outcome {
            // Every row kept, and the job **succeeds**: ADR 0005 says a declined prompt is a normal
            // outcome, never an error, and a failed job would put a red line in `mix job list` for
            // somebody exercising a choice the design offers them.
            ElevationOutcome::Declined => {
                tracing::info!("the elevation prompt was declined; nothing was applied");
                (0, asked)
            }

            // Kept too, but this one is a failure: nothing was asked and nothing can be until the
            // machine changes. The reason is the answer — on Linux it is a command to type.
            ElevationOutcome::Unavailable { reason } => {
                let error = Error::new(
                    ErrorCode::PrivilegedRequired,
                    format!("this machine cannot raise an elevation prompt: {reason}"),
                );
                self.remember(handle, &outcome, 0, asked);
                return Err(error);
            }

            // The helper **ran**. Whether it left a report is the next question, and "no report" is
            // a real state: T40a is explicit that `Completed` does not promise a file.
            ElevationOutcome::Completed => {
                handle.progress(70, "reading what the helper did").await;

                let report = match mixengine_core::elevation::read_report(request) {
                    Ok(report) => report,
                    Err(error) => {
                        self.remember(handle, &outcome, 0, asked);
                        return Err(error.to_wire());
                    }
                };

                let results: Vec<_> = request
                    .ids()
                    .iter()
                    .copied()
                    .zip(report.results.iter().cloned())
                    .collect();

                let settled = mixengine_core::elevation::settle(&self.store, &results)
                    .await
                    .map_err(|error| error.to_wire())?;

                for (id, reason) in &settled.refused {
                    tracing::warn!(
                        %id,
                        reason,
                        "a privileged operation will not succeed as written and was dropped"
                    );
                }

                tracing::info!(
                    applied = settled.applied,
                    kept = settled.kept,
                    refused = settled.refused.len(),
                    elevated = report.elevated,
                    helper = report.elevate_version,
                    "an elevated batch was applied"
                );

                (settled.applied, settled.kept)
            }
        };

        let grant = self.remember(handle, &outcome, applied, still_pending);

        serde_json::to_value(&grant).map_err(|source| {
            Error::new(
                ErrorCode::Internal,
                format!("the grant's outcome could not be encoded: {source}"),
            )
        })
    }

    /// Record what this grant did, for `elevation.status`, and hand it back for the job's result.
    fn remember(
        &self,
        handle: &crate::jobs::JobHandle,
        outcome: &ElevationOutcome,
        applied: usize,
        still_pending: usize,
    ) -> GrantOutcome {
        let grant = GrantOutcome {
            job: handle.id(),
            at: Timestamp::from_system_time(SystemTime::now()),
            outcome: outcome.clone(),
            applied,
            still_pending,
        };

        self.state
            .lock()
            .expect("the elevation slot is not held across an await")
            .last = Some(grant.clone());

        grant
    }

    /// The three facts `daemon.status` carries.
    ///
    /// # Errors
    ///
    /// The wire error of a queue that could not be read.
    pub(crate) async fn summary(&self) -> Result<ElevationSummary, Error> {
        let waiting = mixengine_core::elevation::pending(&self.store)
            .await
            .map_err(|error| error.to_wire())?;

        Ok(ElevationSummary {
            elevated: self.elevated,
            can_prompt: self.reason().is_none(),
            pending: waiting.len(),
        })
    }

    /// `elevation.status` — the screen.
    ///
    /// # Errors
    ///
    /// The wire error of a queue that could not be read.
    pub(crate) async fn status(&self) -> Result<ElevationStatus, Error> {
        let pending = mixengine_core::elevation::pending(&self.store)
            .await
            .map_err(|error| error.to_wire())?;

        let reason = self.reason();
        let last = self
            .state
            .lock()
            .expect("the elevation slot is not held across an await")
            .last
            .clone();

        Ok(ElevationStatus {
            elevated: self.elevated,
            can_prompt: reason.is_none(),
            reason,
            helper: mixengine_core::elevation::helper(&self.program)
                .ok()
                .map(|path| path.display().to_string()),
            pending,
            last,
        })
    }

    /// `elevation.drop` — forget one operation, or all of them, and answer with what is left.
    ///
    /// Answering with the whole [`ElevationStatus`] rather than a count: what a person does next is
    /// look at the list, and a client that had to call again to see it would render a stale one in
    /// between.
    ///
    /// # Errors
    ///
    /// The wire error of a queue that could not be written or read back.
    pub(crate) async fn drop_pending(
        &self,
        asked: &ElevationDrop,
    ) -> Result<ElevationStatus, Error> {
        let removed = mixengine_core::elevation::discard(&self.store, asked.op)
            .await
            .map_err(|error| error.to_wire())?;

        tracing::info!(removed, "pending privileged operations were dropped");

        self.status().await
    }

    /// Why a prompt cannot be raised here, or [`None`] when one can.
    ///
    /// **Two halves, and the sentence differs.** A machine with no authentication agent cannot show
    /// a prompt; a daemon with no `mixengine-elevate` beside it has nothing to show one *for*. Both
    /// leave `can_prompt` false, and only one of them is fixed by installing a polkit agent.
    fn reason(&self) -> Option<String> {
        if let Err(error) = mixengine_core::elevation::helper(&self.program) {
            return Some(error.to_string());
        }

        match self.host.elevation().probe() {
            ElevationSupport::Available => None,
            ElevationSupport::Unavailable { reason } => Some(reason),
        }
    }
}

/// The grant slot, released however the work ends.
///
/// A guard and not a last statement, on `Going`'s reasoning in [`crate::api`]: the future serving a
/// job is dropped where it stands if the daemon shuts down under it, and a panic anywhere inside the
/// flush does the same. A slot left `Running` after either would refuse every later grant for as
/// long as this daemon lives — with no way out short of restarting it.
struct Released<'a>(&'a Elevation);

impl Drop for Released<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mixengine_platform::mock;

    /// A registry over a temporary home, with a machine that accepts every prompt.
    ///
    /// The helper is a **file that exists** and is never run: everything in this module stops at
    /// `mock::Host`, which records the prompt and raises nothing.
    async fn registry(
        machine: mock::Host,
    ) -> (tempfile::TempDir, Arc<Elevation>, Events, Arc<mock::Host>) {
        registry_resolving(machine, crate::dns::Dns::hosts_only_for_tests()).await
    }

    /// The same, for the two tests that care which way this home resolves a name.
    async fn registry_resolving(
        machine: mock::Host,
        dns: crate::dns::Dns,
    ) -> (tempfile::TempDir, Arc<Elevation>, Events, Arc<mock::Host>) {
        let home = tempfile::tempdir().expect("a temporary home");
        let paths = Paths::new(
            home.path().to_path_buf(),
            &mixengine_core::config::PathOverrides::default(),
        );
        std::fs::create_dir_all(paths.run()).expect("the run directory");

        let program = home
            .path()
            .join(format!("mixengined{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&program, b"not run").expect("a program to be found beside");
        std::fs::write(
            home.path()
                .join(format!("mixengine-elevate{}", std::env::consts::EXE_SUFFIX)),
            b"not run",
        )
        .expect("a helper to be found beside it");

        let store = Store::open(&home.path().join("mixengine.db"))
            .await
            .expect("a fresh database migrates");
        let events = Events::new();
        let jobs = Arc::new(crate::jobs::Jobs::new(
            &store,
            events.clone(),
            tokio_util::sync::CancellationToken::new(),
        ));

        let machine = Arc::new(machine);
        let elevation = Elevation::new(
            &paths,
            &store,
            events.clone(),
            jobs,
            Arc::clone(&machine) as Arc<dyn Host>,
            program,
            Arc::new(dns),
        );

        (home, elevation, events, machine)
    }

    /// Wait for a job to end, so a test can assert on the row rather than on a race.
    async fn finished(jobs: &Arc<crate::jobs::Jobs>, job: JobId) -> mixengine_proto::JobSummary {
        jobs.wait(job, mixengine_proto::Millis(5_000))
            .await
            .expect("the job ends")
    }

    /// Three rows where this build has one operation: `Probe` through the shipped path, and two
    /// more written directly — the shape T41 will produce, and enough to prove a batch is a batch.
    async fn three_waiting(elevation: &Arc<Elevation>) {
        elevation.enqueue(&PrivilegedOp::Probe {}).await.unwrap();

        for (key, at) in [("second", 2), ("third", 3)] {
            sqlx::query(
                "INSERT INTO pending_privileged_ops (op, dedupe_key, requested_at)                  VALUES ('{\"op\":\"probe\"}', ?, ?)",
            )
            .bind(key)
            .bind(at)
            .execute(elevation.store.pool())
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn a_machine_with_nothing_waiting_is_not_degraded() {
        let (_home, elevation, _events, _machine) =
            registry(mock::Host::with_home("/tmp/mixengine")).await;

        let summary = elevation.summary().await.expect("the queue is readable");

        assert_eq!(summary.pending, 0);
        assert!(summary.can_prompt);

        let status = elevation.status().await.expect("a status");
        assert!(status.pending.is_empty());
        assert!(status.helper.is_some(), "it is beside the program");
        assert!(status.reason.is_none());
        assert!(status.last.is_none(), "this daemon has granted nothing yet");
    }

    /// D8, from the side that publishes it: the event carries the whole queue, and an enqueue that
    /// changed nothing publishes nothing at all.
    #[tokio::test]
    async fn enqueueing_announces_the_batch_and_a_repeat_announces_nothing() {
        let (_home, elevation, events, _machine) =
            registry(mock::Host::with_home("/tmp/mixengine")).await;
        let mut watching = events.subscribe();

        elevation
            .enqueue(&PrivilegedOp::Probe {})
            .await
            .expect("the row is written");

        let published = watching.next().await.expect("an event");
        let crate::api::events::Frame::Event(DaemonEvent::ElevationRequired { pending }) =
            published
        else {
            panic!("the wrong event: {published:?}")
        };
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].op, PrivilegedOp::Probe {});
        assert!(!pending[0].description.is_empty());

        elevation
            .enqueue(&PrivilegedOp::Probe {})
            .await
            .expect("a repeat is not an error");

        assert_eq!(
            elevation.summary().await.unwrap().pending,
            1,
            "one operation, however many times it was asked for"
        );
    }

    /// The other way out of a degraded mode, and the reason a decline is not a trap.
    #[tokio::test]
    async fn dropping_empties_the_queue_and_answers_with_what_is_left() {
        let (_home, elevation, _events, _machine) =
            registry(mock::Host::with_home("/tmp/mixengine")).await;

        elevation.enqueue(&PrivilegedOp::Probe {}).await.unwrap();
        let waiting = elevation.status().await.unwrap().pending;
        assert_eq!(waiting.len(), 1);

        let left = elevation
            .drop_pending(&ElevationDrop {
                op: Some(waiting[0].id),
            })
            .await
            .expect("the row goes");

        assert!(left.pending.is_empty());
        assert_eq!(elevation.summary().await.unwrap().pending, 0);
    }

    /// D6, and the reason `can_prompt` is not merely `probe()`: a machine with every mechanism in
    /// place and no helper beside the daemon cannot grant anything either, and the sentence a person
    /// needs is different in the two cases.
    #[tokio::test]
    async fn a_machine_that_cannot_prompt_says_which_half_is_missing() {
        let (_home, elevation, _events, _machine) = registry(mock::Host::unable_to_elevate(
            "/tmp/mixengine",
            "no polkit agent",
        ))
        .await;

        let status = elevation.status().await.expect("a status");

        assert!(!status.can_prompt);
        assert_eq!(status.reason.as_deref(), Some("no polkit agent"));
        assert!(
            status.helper.is_some(),
            "the helper is there; it is the prompt that is not"
        );
    }

    /// **The task line's own test**: three operations, one grant, one prompt.
    ///
    /// `.claude/decisions/0005-on-demand-elevation.md` calls elevating inside a loop a defect, and
    /// this is that rule asserted rather than asserted-about. The pair the mock records is the whole
    /// claim: one prompt, on the request the daemon had just written, with the helper it resolved.
    #[tokio::test]
    async fn three_operations_are_one_prompt_on_the_request_that_was_just_written() {
        let (_home, elevation, _events, machine) =
            registry(mock::Host::with_home("/tmp/mixengine")).await;

        three_waiting(&elevation).await;
        assert_eq!(elevation.summary().await.unwrap().pending, 3);

        let started = elevation.grant().await.expect("a grant becomes a job");
        finished(&elevation.jobs, started.id).await;

        let raised = machine.prompts_raised();
        assert_eq!(raised.len(), 1, "three operations, one prompt: {raised:?}");
        assert_eq!(raised[0].request.file_name().unwrap(), "request.json");
        assert!(
            raised[0]
                .helper
                .ends_with(format!("mixengine-elevate{}", std::env::consts::EXE_SUFFIX)),
            "{:?}",
            raised[0].helper
        );

        // The single-use directory is removed however the grant ended — `response.json`'s existence
        // is the anti-replay check, and a directory left behind would make the *next* grant's fresh
        // one the only thing keeping that true.
        assert!(!raised[0].request.exists());
    }

    /// The mock raises nothing and writes nothing, which makes "the helper ran and left no report"
    /// the default here rather than a case somebody had to think to write. T40a is explicit that
    /// `Completed` does not promise a file: a crash is not a per-OS event.
    #[tokio::test]
    async fn a_helper_that_left_no_report_fails_the_job_and_keeps_every_row() {
        let (_home, elevation, _events, _machine) =
            registry(mock::Host::with_home("/tmp/mixengine")).await;
        elevation.enqueue(&PrivilegedOp::Probe {}).await.unwrap();

        let started = elevation.grant().await.unwrap();
        let ended = finished(&elevation.jobs, started.id).await;

        assert_eq!(ended.state, mixengine_proto::JobState::Failed);
        assert_eq!(
            elevation.summary().await.unwrap().pending,
            1,
            "nothing was reported, so nothing may be assumed applied"
        );
    }

    /// ADR 0005: **a declined prompt is a normal outcome, never an error.** A failed job would put a
    /// red line in `mix job list` for a person exercising a choice the design offers them.
    #[tokio::test]
    async fn a_decline_leaves_the_queue_alone_and_the_job_succeeds() {
        let (_home, elevation, _events, _machine) =
            registry(mock::Host::declining_elevation("/tmp/mixengine")).await;
        elevation.enqueue(&PrivilegedOp::Probe {}).await.unwrap();

        let started = elevation.grant().await.unwrap();
        let ended = finished(&elevation.jobs, started.id).await;

        assert_eq!(ended.state, mixengine_proto::JobState::Succeeded);
        assert_eq!(elevation.summary().await.unwrap().pending, 1);

        let last = elevation
            .status()
            .await
            .unwrap()
            .last
            .expect("a last grant");
        assert_eq!(
            last.outcome,
            mixengine_proto::privileged::ElevationOutcome::Declined
        );
        assert_eq!(last.applied, 0);
        assert_eq!(last.still_pending, 1);

        // And the machine can still be asked: declined is not the same as impossible, which is the
        // distinction `probe()` exists to draw.
        assert!(elevation.summary().await.unwrap().can_prompt);
    }

    /// On Linux the reason is the whole `pkexec` command a person is meant to type. It is worthless
    /// if the daemon drops it, so it is asserted all the way through to `elevation.status`.
    #[tokio::test]
    async fn a_machine_that_cannot_prompt_keeps_the_reason_intact() {
        let (_home, elevation, _events, _machine) = registry(mock::Host::unable_to_elevate(
            "/tmp/mixengine",
            "no polkit agent; run: pkexec /opt/mixengine/mixengine-elevate /…",
        ))
        .await;
        elevation.enqueue(&PrivilegedOp::Probe {}).await.unwrap();

        let error = elevation
            .grant()
            .await
            .expect_err("there is no way to raise a prompt here");

        assert_eq!(error.code, mixengine_proto::ErrorCode::PrivilegedRequired);
        assert!(error.to_string().contains("pkexec"), "{error}");
        assert_eq!(elevation.summary().await.unwrap().pending, 1);

        let status = elevation.status().await.unwrap();
        assert_eq!(
            status.reason.as_deref(),
            Some("no polkit agent; run: pkexec /opt/mixengine/mixengine-elevate /…")
        );
    }

    /// D4's runtime half, asserted on the slot rather than on a race: two concurrent grants are two
    /// prompts for one queue, and refusing is the only answer that cannot become a loop.
    #[tokio::test]
    async fn a_second_grant_while_one_is_in_flight_is_refused_and_names_the_first() {
        let (_home, elevation, _events, _machine) =
            registry(mock::Host::with_home("/tmp/mixengine")).await;
        elevation.enqueue(&PrivilegedOp::Probe {}).await.unwrap();

        // Put the slot where a running grant puts it, without a grant that has to be slowed down to
        // stay there. The mock answers instantly, so racing one would be a test about scheduling.
        elevation.state.lock().unwrap().slot = Slot::Running(JobId(41));

        let error = elevation
            .grant()
            .await
            .expect_err("one prompt at a time, for one queue");

        assert_eq!(error.code, mixengine_proto::ErrorCode::Conflict);
        assert!(error.to_string().contains("41"), "{error}");

        // And a slot left free is a slot a grant can take, or one failed grant would wedge the
        // daemon for as long as it runs.
        elevation.state.lock().unwrap().slot = Slot::Free;
        let started = elevation.grant().await.expect("the slot was released");
        finished(&elevation.jobs, started.id).await;
        assert_eq!(elevation.state.lock().unwrap().slot, Slot::Free);
    }

    /// An empty queue asks for nothing. The helper refuses an empty batch outright, so spending a
    /// prompt to find that out would be a dialog raised for no reason at all.
    #[tokio::test]
    async fn granting_an_empty_queue_raises_nothing() {
        let (_home, elevation, _events, machine) =
            registry(mock::Host::with_home("/tmp/mixengine")).await;

        let error = elevation.grant().await.expect_err("nothing is waiting");

        assert_eq!(error.code, mixengine_proto::ErrorCode::PreconditionFailed);
        assert!(machine.prompts_raised().is_empty());
    }

    /// D9 and D11's second row: no helper beside the daemon is a different sentence from no way to
    /// prompt, and it is answered before anything is written.
    #[tokio::test]
    async fn granting_without_a_helper_beside_the_daemon_is_dependency_missing() {
        let (home, elevation, _events, machine) =
            registry(mock::Host::with_home("/tmp/mixengine")).await;
        elevation.enqueue(&PrivilegedOp::Probe {}).await.unwrap();

        std::fs::remove_file(
            home.path()
                .join(format!("mixengine-elevate{}", std::env::consts::EXE_SUFFIX)),
        )
        .expect("the helper goes");

        let error = elevation
            .grant()
            .await
            .expect_err("there is nothing to run");

        assert_eq!(error.code, mixengine_proto::ErrorCode::DependencyMissing);
        assert!(machine.prompts_raised().is_empty());
        assert_eq!(elevation.summary().await.unwrap().pending, 1);
    }

    /// D11: the machine already says what the database says it should, so nothing is queued. A row
    /// here would put an operation on `mix status` whose only possible outcome is `AlreadyDone`.
    #[tokio::test]
    async fn a_machine_that_already_agrees_is_not_asked_for_permission() {
        let (_home, elevation, _events, _machine) = registry(mock::Host::with_hosts(
            "/tmp/mixengine",
            ["127.0.0.1 blog.test"],
        ))
        .await;
        a_site_named(&elevation.store, "blog.test").await;

        elevation.require_hosts().await.unwrap();

        assert!(
            mixengine_core::elevation::pending(&elevation.store)
                .await
                .unwrap()
                .is_empty(),
            "nothing to do is not something to ask about"
        );
    }

    /// And when it disagrees, exactly one operation is waiting and one event was published.
    #[tokio::test]
    async fn a_machine_that_disagrees_is_asked_once() {
        let (_home, elevation, events, _machine) =
            registry(mock::Host::with_hosts("/tmp/mixengine", [])).await;
        let mut watching = events.subscribe();
        a_site_named(&elevation.store, "blog.test").await;

        elevation.require_hosts().await.unwrap();

        let waiting = mixengine_core::elevation::pending(&elevation.store)
            .await
            .unwrap();
        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].description.contains("blog.test"), "{waiting:?}");

        assert!(matches!(
            watching.next().await,
            Some(crate::api::events::Frame::Event(
                DaemonEvent::ElevationRequired { .. }
            ))
        ));
    }

    /// **The seam T44 built, in both directions** — the T44 design, D4.
    ///
    /// A home on the hosts file asks for the entry its sites declare. A home on DNS asks for an
    /// *empty* block, which is not the same as asking for nothing: the server answers the whole
    /// managed TLD by pattern, so an entry adds nothing, and the operation is what clears the names
    /// the other mode wrote. Skipping the queue would leave them resolving to loopback for ever.
    ///
    /// This is the test that stops D4 being tidied into "if DNS is on, do nothing".
    #[tokio::test]
    async fn a_home_on_dns_asks_for_an_empty_block_rather_than_for_nothing() {
        let (_home, elevation, _events, _machine) = registry_resolving(
            mock::Host::with_hosts("/tmp/mixengine", ["127.0.0.1 blog.test"]),
            crate::dns::Dns::wired_for_tests(),
        )
        .await;
        a_site_named(&elevation.store, "blog.test").await;

        elevation.require_hosts().await.unwrap();

        let waiting = mixengine_core::elevation::pending(&elevation.store)
            .await
            .unwrap();

        assert_eq!(
            waiting.len(),
            1,
            "the block the machine holds is not the block a DNS home wants"
        );
        assert_eq!(
            waiting[0].op,
            PrivilegedOp::hosts_apply(Vec::new()),
            "a home on DNS wants no managed names in that file"
        );
    }

    /// The same home, resolving the way every machine does until T45: the entry is asked for.
    #[tokio::test]
    async fn a_home_on_the_hosts_file_asks_for_the_names_its_sites_declare() {
        let (_home, elevation, _events, _machine) = registry_resolving(
            mock::Host::with_hosts("/tmp/mixengine", []),
            crate::dns::Dns::hosts_only_for_tests(),
        )
        .await;
        a_site_named(&elevation.store, "blog.test").await;

        elevation.require_hosts().await.unwrap();

        let waiting = mixengine_core::elevation::pending(&elevation.store)
            .await
            .unwrap();

        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].description.contains("blog.test"), "{waiting:?}");
    }

    /// D2 asserted rather than described: two sites before anybody clicks Allow are one row holding
    /// the *second* state, and one event per change rather than one row per change.
    #[tokio::test]
    async fn two_sites_before_a_grant_are_one_row_holding_the_second_state() {
        let (_home, elevation, events, _machine) =
            registry(mock::Host::with_hosts("/tmp/mixengine", [])).await;
        let mut watching = events.subscribe();

        a_site_named(&elevation.store, "blog.test").await;
        elevation.require_hosts().await.unwrap();

        a_site_named(&elevation.store, "shop.test").await;
        elevation.require_hosts().await.unwrap();

        let waiting = mixengine_core::elevation::pending(&elevation.store)
            .await
            .unwrap();
        assert_eq!(waiting.len(), 1, "{waiting:?}");
        assert!(waiting[0].description.contains("blog.test"), "{waiting:?}");
        assert!(waiting[0].description.contains("shop.test"), "{waiting:?}");

        for expected in ["blog.test", "shop.test"] {
            let published = watching.next().await.expect("an event");
            let crate::api::events::Frame::Event(DaemonEvent::ElevationRequired { pending }) =
                published
            else {
                panic!("the wrong event: {published:?}")
            };
            assert!(pending[0].description.contains(expected), "{pending:?}");
        }

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), watching.next())
                .await
                .is_err(),
            "and no third event"
        );
    }

    /// A read that fails is not a reason to refuse a site. The helper is the authority on what is in
    /// that file, and it will say so on the screen T64 built — a better place for "your hosts file
    /// has two BEGIN markers" than a site creation's error.
    #[tokio::test]
    async fn a_hosts_file_that_cannot_be_read_is_still_asked_about() {
        let (_home, elevation, _events, _machine) = registry(
            mock::Host::unable_to_read_the_hosts_file("/tmp/mixengine", "two BEGIN markers"),
        )
        .await;
        a_site_named(&elevation.store, "blog.test").await;

        elevation.require_hosts().await.unwrap();

        assert_eq!(
            mixengine_core::elevation::pending(&elevation.store)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// A project and a site holding one domain, written straight into the store.
    ///
    /// What this suite is about is the producer's decision; `Sites` has its own tests, and going
    /// through the API here would put two things under one assertion.
    async fn a_site_named(store: &Store, domain: &str) {
        // One project per call, because a root is unique and two sites under one root would be a
        // second thing this fixture had to decide.
        let root = std::env::temp_dir().join(format!("mixengine-t41-{domain}"));

        let project = mixengine_core::projects::create(
            store,
            &mixengine_core::projects::Registration {
                name: domain.replace('.', "-"),
                root,
                pins: std::collections::BTreeMap::new(),
            },
            Timestamp::from_system_time(std::time::SystemTime::UNIX_EPOCH),
        )
        .await
        .expect("a project");

        mixengine_core::sites::create(
            store,
            &mixengine_core::sites::NewSite {
                project_id: project.id,
                doc_root: String::new(),
                kind: mixengine_proto::SiteKind::Static,
                https_enabled: true,
                domains: vec![domain.to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("a site");
    }

    /// D7: a machine that needs a grant and has not got one leaves exactly one row in the queue and
    /// announces it once.
    #[tokio::test]
    async fn a_front_end_on_a_machine_with_no_grant_asks_for_one() {
        let (_home, elevation, events, _machine) = registry(mock::Host::without_port_access(
            "/tmp/mixengine",
            mixengine_platform::PortAccessMethod::Capability,
            "the binary holds no capability",
        ))
        .await;
        let mut watching = events.subscribe();

        elevation
            .require_port_access(Some(Path::new("/packages/caddy/caddy")))
            .await
            .unwrap();

        let waiting = mixengine_core::elevation::pending(&elevation.store)
            .await
            .unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].op.name(), "port-access-grant");
        assert!(matches!(
            watching.next().await,
            Some(crate::api::events::Frame::Event(
                DaemonEvent::ElevationRequired { .. }
            ))
        ));

        // A second start adds no second row: the dedupe key is the kind, and the state has not
        // changed — the T41 design, D2, which this operation reuses unchanged.
        elevation
            .require_port_access(Some(Path::new("/packages/caddy/caddy")))
            .await
            .unwrap();

        assert_eq!(
            mixengine_core::elevation::pending(&elevation.store)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// A machine that already allows it spends no prompt, which is the whole reason the disk is read
    /// before the queue is written.
    #[tokio::test]
    async fn a_machine_that_already_allows_it_asks_for_nothing() {
        let (_home, elevation, _events, _machine) = registry(mock::Host::with_port_access(
            "/tmp/mixengine",
            mixengine_platform::PortAccessMethod::Capability,
        ))
        .await;

        elevation
            .require_port_access(Some(Path::new("/packages/caddy/caddy")))
            .await
            .unwrap();

        assert!(
            mixengine_core::elevation::pending(&elevation.store)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A home with no front end has no binary to name, and on Linux the question cannot even be
    /// asked without one — D12's reason for the producer being one-directional.
    #[tokio::test]
    async fn a_home_with_no_front_end_asks_for_nothing() {
        let (_home, elevation, _events, _machine) = registry(mock::Host::without_port_access(
            "/tmp/mixengine",
            mixengine_platform::PortAccessMethod::Capability,
            "the binary holds no capability",
        ))
        .await;

        elevation.require_port_access(None).await.unwrap();

        assert!(
            mixengine_core::elevation::pending(&elevation.store)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A probe that fails is not a reason to ask for a prompt, and not a reason to fail a start
    /// either. It is logged, and the machine carries on — the opposite of `require_hosts`, and
    /// deliberately: there the helper is the authority on the file's contents and can refuse with a
    /// reason on the screen; here a probe that could not read one attribute tells us nothing about
    /// what to ask for.
    #[tokio::test]
    async fn a_probe_that_fails_asks_for_nothing_and_does_not_fail() {
        let (_home, elevation, _events, _machine) =
            registry(mock::Host::unable_to_probe_port_access(
                "/tmp/mixengine",
                "this filesystem carries no extended attributes",
            ))
            .await;

        elevation
            .require_port_access(Some(Path::new("/packages/caddy/caddy")))
            .await
            .expect("a probe that failed is not an error to the caller");

        assert!(
            mixengine_core::elevation::pending(&elevation.store)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Windows: the method says nothing is needed, so nothing is asked for even though the front end
    /// answers on 80. No `#[cfg]` anywhere above this crate is what makes that true.
    #[tokio::test]
    async fn a_machine_that_reserves_no_ports_asks_for_nothing() {
        let (_home, elevation, _events, _machine) = registry(mock::Host::with_port_access(
            "/tmp/mixengine",
            mixengine_platform::PortAccessMethod::Direct,
        ))
        .await;

        elevation
            .require_port_access(Some(Path::new("/packages/caddy/caddy.exe")))
            .await
            .unwrap();

        assert!(
            mixengine_core::elevation::pending(&elevation.store)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
