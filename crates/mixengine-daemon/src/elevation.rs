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

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use mixengine_core::{Paths, Store};
use mixengine_platform::{ElevationSupport, Host};
use mixengine_proto::privileged::PrivilegedOp;
use mixengine_proto::{
    DaemonEvent, ElevationDrop, ElevationStatus, ElevationSummary, Error, GrantOutcome, JobId,
    Timestamp,
};

use crate::api::Events;
use crate::error::ToWire as _;

/// The single grant slot — the runtime half of "no code path elevates in a loop".
///
/// Two concurrent grants are two prompts for one queue, which is the defect ADR 0005 names, and
/// refusing the second is the only answer that cannot itself become a loop.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[expect(
    dead_code,
    reason = "the two busy states are constructed by `elevation.grant`, T40b's next task; the slot \
              is declared with the state it guards so that task adds a method and not a field"
)]
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
    #[expect(
        dead_code,
        reason = "read and written by `elevation.grant`, which is T40b's next task"
    )]
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
    #[expect(
        dead_code,
        reason = "T40b's next task turns a grant into a job; the registry is held from the \
                  moment the type exists so that task is a method and not a constructor change"
    )]
    jobs: Arc<crate::jobs::Jobs>,

    /// The OS: `probe()` for the degraded mode, `run()` for the grant.
    host: Arc<dyn Host>,

    /// `MIXENGINE_HOME`, which every request names and the helper checks the ownership of.
    #[expect(
        dead_code,
        reason = "the request document names it, and that is written by the grant in the next task"
    )]
    home: PathBuf,

    /// `<root>/run/elevate` — the parent of every single-use request directory.
    #[expect(
        dead_code,
        reason = "the single-use directory is made by the grant in the next task"
    )]
    elevate: PathBuf,

    /// The program that is running, which is what the helper is found beside (D9).
    program: PathBuf,

    /// Whether **this daemon** holds an administrative token, read once at construction.
    ///
    /// Reported and not refused — the T40b design, D10. Read once because it cannot change under a
    /// running process, and reading it per request would be a syscall per `mix status`.
    elevated: bool,

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
            state: Mutex::new(State::default()),
        })
    }

    /// Put an operation in the queue, and announce the batch when that changed something.
    ///
    /// **No caller in this build**, and that is not an oversight — see the module documentation.
    /// T41's `HostsApply` is the first producer; the queue and its event land first so that T41 is
    /// one operation rather than an operation plus a mechanism.
    ///
    /// # Errors
    ///
    /// The wire error of a row that could not be written.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "T41's HostsApply is the first producer; the queue lands first so T41 is one \
                      operation and not an operation plus a mechanism"
        )
    )]
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

#[cfg(test)]
mod tests {
    use super::*;

    use mixengine_platform::mock;

    /// A registry over a temporary home, with a machine that accepts every prompt.
    ///
    /// The helper is a **file that exists** and is never run: everything in this module stops at
    /// `mock::Host`, which records the prompt and raises nothing.
    async fn registry(machine: mock::Host) -> (tempfile::TempDir, Arc<Elevation>, Events) {
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

        let elevation = Elevation::new(
            &paths,
            &store,
            events.clone(),
            jobs,
            Arc::new(machine),
            program,
        );

        (home, elevation, events)
    }

    #[tokio::test]
    async fn a_machine_with_nothing_waiting_is_not_degraded() {
        let (_home, elevation, _events) = registry(mock::Host::with_home("/tmp/mixengine")).await;

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
        let (_home, elevation, events) = registry(mock::Host::with_home("/tmp/mixengine")).await;
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
        let (_home, elevation, _events) = registry(mock::Host::with_home("/tmp/mixengine")).await;

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
        let (_home, elevation, _events) = registry(mock::Host::unable_to_elevate(
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
}
