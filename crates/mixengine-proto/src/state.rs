//! Where a service *is*, how it got there, and which moves the supervisor is allowed to make.
//!
//! Separate from [`crate::ServiceSpec`] because the two answer different questions and change for
//! different reasons: a spec is what the user declared and is only ever read, while a state is what
//! the machine currently believes and changes several times a minute. One is edited in the GUI, the
//! other is watched there.
//!
//! Three types, and the split is the sentence `.claude/architecture/process-supervision.md` writes:
//! *every transition is persisted and emitted with a reason.* [`ServiceState`] is the where,
//! [`StateReason`] is the why, and [`ServiceTransition`] is the pair travelling together — persisted
//! by `mixengine-core` and published by the daemon from the very same value, so the row and the
//! event cannot disagree about what happened.

use crate::{Millis, ServiceId, Timestamp};

/// What a supervised service is doing right now.
///
/// **Closed, unlike most of the wire vocabulary.** [`crate::DaemonEvent`] is `#[non_exhaustive]`
/// because a later phase invents new things to say; this enum is the state machine itself, and a
/// state machine with room for one more state is one nobody can reason about. The supervisor matches
/// on it exhaustively on purpose, so that adding a state is a compile error at every place that has
/// to decide what to do about it — and `services.state` carries the same closed list as a `CHECK`
/// constraint in the first migration.
///
/// The wire form is the snake_case name (`"restarting"`, `"failed"`), and it is also **exactly what
/// is stored** in `services.state`: one spelling, produced by [`ServiceState::as_str`] and read back by
/// [`ServiceState::parse`], rather than a serde form and a database form that drift apart on the day
/// somebody renames a variant.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Not running, and nobody asked it to be. The state a service is created in.
    Stopped,
    /// Somebody asked for it and it is not usable yet: its [`crate::ReadyCheck`] has not passed.
    ///
    /// **The process does not necessarily exist yet.** This is where a service sits while the
    /// supervisor is still trying to produce one, which is why [`StateReason::SpawnFailed`] and
    /// [`StateReason::DependencyFailed`] are both reached *from* here — a service that could never
    /// be spawned, and one whose dependency failed before it was ever reached, are both services
    /// somebody asked to start. `Stopped` would say nobody had.
    Starting,
    /// Ready. This is the only state in which traffic may be routed to it.
    Running,
    /// Alive, and failing its [`crate::HealthCheck`].
    ///
    /// Deliberately not [`ServiceState::Failed`]: the process is still there and may recover on its
    /// own, which is what the GUI shows in amber and what `mix doctor` explains. Collapsing the two
    /// would either restart a database that was merely slow or leave a dead one looking fine.
    Degraded,
    /// A stop is in progress — the [`crate::StopBehaviour`] has been applied and the group has not
    /// finished going away.
    Stopping,
    /// Waiting out a restart backoff before starting again.
    ///
    /// Its own state rather than a flavour of [`ServiceState::Starting`], because a service in it
    /// has no process at all: "starting" that has been starting for thirty seconds is a bug report,
    /// "restarting in 30 s" is an explanation.
    Restarting,
    /// It gave up, and it stays here until somebody explicitly starts it again.
    ///
    /// Reached by a ready check that timed out, a spawn that could not happen, or a restart budget
    /// that ran out. The distinction from [`ServiceState::Stopped`] is intent: nobody asked for
    /// this.
    Failed,
}

impl ServiceState {
    /// Every state, in the order the diagram in `process-supervision.md` reads.
    ///
    /// Exists so that a test can be exhaustive without restating the list — the migration's `CHECK`
    /// and the serde form are both checked against this, and a variant added without touching them
    /// fails rather than passes quietly.
    pub const ALL: [Self; 7] = [
        Self::Stopped,
        Self::Starting,
        Self::Running,
        Self::Degraded,
        Self::Stopping,
        Self::Restarting,
        Self::Failed,
    ];

    /// The one spelling: the wire form, and the text in `services.state`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Degraded => "degraded",
            Self::Stopping => "stopping",
            Self::Restarting => "restarting",
            Self::Failed => "failed",
        }
    }

    /// Whether a service in this state is one something should be supervising right now.
    ///
    /// **The set crash recovery reconciles at boot** (roadmap task T18): a daemon that was killed
    /// leaves rows in exactly these five states with nothing supervising them, and the daemon that
    /// starts next has to either take the process over or say that it is gone. The other two are
    /// where a service is *left*, so a row in one of them is already telling the truth about a
    /// machine with no daemon on it.
    ///
    /// Here rather than in the daemon because it is a property of the machine, like
    /// [`can_become`](Self::can_become): `Restarting` counts although no process exists in it —
    /// somebody is meant to be waiting out that backoff, and after a daemon restart nobody is.
    #[must_use]
    pub const fn is_supervised(self) -> bool {
        use ServiceState::{Degraded, Restarting, Running, Starting, Stopping};

        matches!(self, Starting | Running | Degraded | Stopping | Restarting)
    }

    /// Read back what [`ServiceState::as_str`] wrote, or `None`.
    ///
    /// Returns an `Option` rather than a `Result` because the caller is what knows why this
    /// matters: the same unrecognised word is a corrupt row to `mixengine-core` — which names the
    /// service and the file — and a peer speaking a newer protocol to a client. Neither error
    /// belongs to a crate that has no I/O.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }

    /// Whether the machine may move from `self` to `next`.
    ///
    /// The whole table, which is the diagram in `process-supervision.md` with the edges that
    /// diagram compresses written out:
    ///
    /// | from | to | what just happened |
    /// | --- | --- | --- |
    /// | `Stopped` | `Starting` | somebody asked, or autostart did |
    /// | `Starting` | `Running` | the ready check passed |
    /// | `Starting` | `Stopping` | a stop arrived mid-start |
    /// | `Starting` | `Restarting` | it exited before it was ever ready, and the policy puts it back |
    /// | `Starting` | `Failed` | the ready check timed out, the spawn failed, or the policy does not |
    /// | `Running` | `Degraded` | the health check failed |
    /// | `Running` | `Stopping` | somebody asked |
    /// | `Running` | `Restarting` | it exited and the policy puts it back |
    /// | `Running` | `Failed` | it exited and the policy does not |
    /// | `Degraded` | `Running` | health came back |
    /// | `Degraded` | `Stopping` | somebody asked |
    /// | `Degraded` | `Restarting` | the policy gave up on it recovering |
    /// | `Degraded` | `Failed` | same, with no restarts left |
    /// | `Stopping` | `Stopped` | the group is gone |
    /// | `Stopping` | `Restarting` | this stop was the first half of a restart |
    /// | `Stopping` | `Failed` | it would not die, or the stop command failed |
    /// | `Restarting` | `Starting` | the backoff elapsed |
    /// | `Restarting` | `Stopping` | a stop arrived during the backoff |
    /// | `Restarting` | `Failed` | the crash-loop cutoff |
    /// | `Failed` | `Starting` | an explicit `service.start`, the only way out |
    /// | `Failed` | `Stopped` | an explicit `service.stop`, clearing the failure |
    ///
    /// **A state cannot become itself.** A transition is an event, and one that changes nothing
    /// would be persisted, published and rendered as though something had — five identical
    /// "running" lines in a log panel is how a user learns to stop reading it.
    ///
    /// Three edges are worth naming because the diagram does not draw them. `Running → Failed` and
    /// `Running → Restarting` are what a process exiting on its own looks like, which is the most
    /// common thing that ever happens to a service and is not a health-check failure: it never
    /// passes through `Degraded`, because there is nothing left to be degraded about.
    ///
    /// `Starting → Restarting` is the same event one step earlier, and it is not optional. A process
    /// that dies before its ready check ever passes — the port was taken, the config did not parse —
    /// is the ordinary way a service fails, and without this edge the only move left from `Starting`
    /// would be `Failed`, which is terminal. That would make
    /// [`crate::RestartPolicy::Always`] restart nothing at all and give
    /// [`crate::RestartPolicy::OnFailure`] none of its `max_retries`.
    #[must_use]
    pub const fn can_become(self, next: Self) -> bool {
        use ServiceState::{Degraded, Failed, Restarting, Running, Starting, Stopped, Stopping};

        matches!(
            (self, next),
            (Stopped, Starting)
                | (Starting, Running | Stopping | Restarting | Failed)
                | (Running, Degraded | Stopping | Restarting | Failed)
                | (Degraded, Running | Stopping | Restarting | Failed)
                | (Stopping, Stopped | Restarting | Failed)
                | (Restarting, Starting | Stopping | Failed)
                | (Failed, Starting | Stopped)
        )
    }
}

impl std::fmt::Display for ServiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a service changed state.
///
/// `#[non_exhaustive]`, unlike [`ServiceState`], and the asymmetry is the point: the set of states
/// is fixed by the machine, while the set of explanations grows every time a later phase learns to
/// distinguish two failures a user currently sees as one. A client renders what it knows and shows
/// the state alone for what it does not.
///
/// Every variant here is an edge in [`ServiceState::can_become`]. What is deliberately *not* here is
/// anything the code that would emit it does not exist for yet — which is why [`StateReason::Idle`]
/// arrived with T69 and not before, rather than being declared as a promise the build did not keep.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StateReason {
    /// A person asked, through `service.start`, `service.stop` or `service.restart` — or autostart
    /// did on their behalf at boot.
    Requested,

    /// The [`crate::ReadyCheck`] passed.
    Ready,

    /// The [`crate::ReadyCheck`] did not pass in time.
    ReadyTimeout {
        /// How long it was given, so the message can be "no TCP connect within 10 s" rather than
        /// "timed out" — the number is what tells the user whether to raise it or fix the service.
        after: Millis,
    },

    /// A port the service was going to listen on is held by something else — roadmap task **T38**.
    ///
    /// **Not a refusal to start: this is what a start that already failed turns out to have been.**
    /// A server whose bind is refused exits, or sits there never becoming ready, and both of those
    /// are reported honestly by [`StateReason::Exited`] and [`StateReason::ReadyTimeout`] — and
    /// neither sends the reader anywhere useful, because the program at fault is one MixEngine does
    /// not manage and has no row for. The daemon asks the OS who holds the port before it settles
    /// on a reason, and answers with this when somebody does.
    ///
    /// The two identifying fields are separate rather than one rendered string because how much of
    /// them can be filled in is per-OS, and a client may want the pid on its own — see
    /// `mixengine_platform::PortHolder`, whose answer this carries.
    PortInUse {
        /// The port, which is the only field that is always known.
        port: u16,

        /// The process holding it, where this account may learn it.
        pid: Option<u32>,

        /// The file name of that process's program, where this account may read it.
        program: Option<String>,
    },

    /// The process could not be started at all: the program is missing, or the OS refused.
    ///
    /// Distinct from a service that started and then died. This one never ran, so its log file is
    /// empty and there is nothing to attach — the explanation has to be in the reason itself.
    SpawnFailed,

    /// A first-run ritual this service has never had performed did not finish, so nothing was
    /// spawned — roadmap task **T33**.
    ///
    /// Its own variant rather than [`StateReason::SpawnFailed`], which would be a lie in the one way
    /// that matters: nothing was even attempted, and what a user has to fix is upstream of the
    /// process. The full account is the job — `mix job list` holds the failure with the step that
    /// produced it — and this carries one sentence so a listing does not have to be
    /// cross-referenced to be readable.
    FirstRunFailed {
        /// What went wrong, in the words of whatever refused: a machine with no credential store, a
        /// data directory that is not ours, a bootstrap that exited non-zero.
        detail: String,
    },

    /// The spec names a check this build or this machine cannot make.
    ///
    /// The typed answer `CLAUDE.md` requires instead of a `todo!()`, carried all the way to the
    /// user: an `https://` check, which the supervisor makes no TLS request for, or a `UnixSocket`
    /// check on Windows. It is deliberately **not** a ready *timeout* — a spec that cannot be checked was
    /// never going to become ready, and reporting it as a timeout would send whoever wrote it
    /// looking at the service instead of at the spec.
    ///
    /// Both strings come from the supervisor's own `Error::UnsupportedCheck`, which is where the
    /// distinction is drawn; they are re-worded nowhere, so there is one sentence about it rather
    /// than one per layer.
    Uncheckable {
        /// What was asked for, e.g. `"an HTTP ready check"`.
        check: String,
        /// Why it cannot be made, phrased for whoever wrote the spec.
        reason: String,
    },

    /// A service it depends on did not come up, so this one was never spawned.
    ///
    /// The other half of [`StateReason::SpawnFailed`]: neither service ever ran, and neither can
    /// explain itself from its own log — this one's is empty because nothing was started, which is
    /// exactly why the dependency has to be named here. Without it a user reads "failed" on four
    /// services and has to guess which of them is the one to fix.
    ///
    /// **Fail-fast rather than a spawn that is certain to fail.** A dependent started anyway would
    /// crash on a database that is not there, be restarted by its [`crate::RestartPolicy`], and
    /// arrive at [`StateReason::CrashLoop`] a minute later with a tail that says `connection
    /// refused` — an accurate report of the wrong problem. The dependency graph is assembled before
    /// anything is spawned (`mixengine_core::services::graph`), so this is known rather than
    /// discovered.
    DependencyFailed {
        /// The dependency that did not come up.
        ///
        /// The *direct* one this service names, not the root of the chain. Each service reports the
        /// edge it declared, so a chain of four reads as four honest sentences that lead to the one
        /// service that actually broke, rather than three copies of a name none of them mention.
        dependency: ServiceId,
    },

    /// The [`crate::HealthCheck`] failed.
    Unhealthy,

    /// The [`crate::HealthCheck`] passed again after failing.
    Healthy,

    /// The process ended by itself.
    Exited {
        /// The exit status, or `None` where the OS does not report one — a Unix process killed by a
        /// signal has no exit code, and reporting `0` for it would say "clean exit" about a crash.
        code: Option<i32>,
    },

    /// The backoff after a crash has elapsed and the service is being started again.
    BackoffElapsed {
        /// Which attempt this is since the last time the service was healthy, counted from 1. It is
        /// what makes "restarting (3 of 5)" possible, which is the sentence that tells a user
        /// whether to keep waiting.
        attempt: u32,
    },

    /// The restart budget ran out: too many failures inside the policy's window.
    ///
    /// Terminal by design — the service stays [`ServiceState::Failed`] until somebody starts it
    /// explicitly, rather than retrying forever against a port that is never going to be free.
    CrashLoop {
        /// How many attempts were made.
        attempts: u32,
        /// The window they were counted in. Both numbers, because "5 failures" means nothing
        /// without "in 5 minutes" — a service that crashes once a day is not in a crash loop.
        window: Millis,
        /// The last lines the service printed before the supervisor gave up, oldest first.
        ///
        /// **The one variant that carries evidence, and the one that has to.** Everything else here
        /// explains itself — a ready timeout says how long it waited, an exit says what it exited
        /// with — while "it kept crashing" explains nothing at all without the line that says
        /// `Address already in use`. Attaching it to the reason is what lets the GUI answer *why*
        /// where the user is already looking, instead of sending them to a log viewer to find out
        /// whether it is worth reading.
        ///
        /// Bounded by the supervisor (200 lines) rather than by the ring, so an event is never the
        /// size of a log file. Empty is a real answer: a service that said nothing before it died.
        #[serde(default)]
        tail: Vec<String>,
    },

    /// Its process did not survive the daemon that was supervising it.
    ///
    /// Written by crash recovery at the next start (roadmap task T18) for a row that claimed a
    /// running process and turned out not to have one: the pid names nothing, or it names something
    /// that began before the process we recorded and is therefore somebody else's. **Nothing is
    /// signalled on this path** — a pid that failed the identity check is precisely the pid this
    /// product must not act on.
    ///
    /// Not [`StateReason::Exited`], which says the supervisor watched a process end and read its
    /// status. Nobody watched this one: it went with a daemon that was killed, or with a machine
    /// that lost power, and how it ended is not recorded anywhere.
    Vanished,

    /// Its process *did* survive, and the daemon that found it stopped it rather than take it over.
    ///
    /// The other half of [`StateReason::Vanished`], and the rarer one: on Windows it takes the
    /// one-call-wide window
    /// `.claude/decisions/0007-supervised-child-owns-a-process-group.md` accepts, on Linux a
    /// grandchild, on macOS every killed daemon. Adoption needs both a declaration to supervise the
    /// process against and a state it can be resumed from, and a survivor that has neither is not
    /// left running: it would be holding the port and the data directory that the next start
    /// collides with.
    Unadopted {
        /// Which of those was missing, phrased for whoever reads the service list.
        ///
        /// A sentence rather than a code for the reason [`StateReason::Uncheckable`] carries one:
        /// the daemon knows why and the client is only laying it out, and the two reasons a
        /// survivor is refused — nothing declares it any more, and a state adoption cannot resume —
        /// are not a vocabulary anything branches on.
        reason: String,
    },

    /// Nothing was using it, for as long as its [`crate::IdlePolicy`] said to wait — roadmap task
    /// **T69**.
    ///
    /// **A reason on the transition rather than an event of its own.** "Why did my PHP stop?" is a
    /// question about a move, and a `ServiceIdled` beside `ServiceStateChanged` would announce one
    /// moment twice on a stream every client shares — with a client that handled only the new one
    /// missing idle stops from an older daemon, and one that handled only the transition showing a
    /// stop with no explanation.
    ///
    /// Idle is *measured*: a service with a connection open to it is never idle, one something
    /// running depends on is never idle, and one this machine could not measure at all is left
    /// running rather than stopped on no evidence.
    Idle {
        /// How long it was declared idle after.
        ///
        /// The policy's duration and not the observed one, which are the same number only when the
        /// sweep period divides it — and the useful one is the setting a person would change.
        after: Millis,
    },
}

impl std::fmt::Display for StateReason {
    /// The half-sentence that follows a state, as in `failed — not ready within 10s`.
    ///
    /// **Here rather than in each client, for the reason the type is here at all.** `mix` and the
    /// GUI show a user the same event, and two renderings written independently would disagree
    /// about what `crash_loop` means the week one of them is updated and the other is not — the
    /// same argument `.claude/decisions/0006-servicespec-in-proto-and-secret-free.md` makes for the
    /// vocabulary itself. What is left
    /// to a client is layout: [`StateReason::CrashLoop`]'s `tail` is evidence to be printed as
    /// lines, and it is deliberately not in this sentence.
    ///
    /// Lower case and no full stop, because every caller puts it after something else.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Exhaustive although the type is `non_exhaustive`, which is only a promise to *other*
        // crates: a variant added here without a sentence should not compile.
        match self {
            Self::Requested => f.write_str("somebody asked for it"),
            Self::Ready => f.write_str("the ready check passed"),
            Self::ReadyTimeout { after } => write!(f, "not ready within {after}"),
            // Three shapes rather than one, because the useless one has to stay useful: a port
            // that is taken by something nobody can name is still a port that is taken, and the
            // reader's next move — find out what is on 3306 — is the same in all three.
            Self::PortInUse {
                port,
                program: Some(program),
                ..
            } => write!(f, "port {port} is already held by {program}"),
            Self::PortInUse {
                port,
                pid: Some(pid),
                ..
            } => write!(f, "port {port} is already held by pid {pid}"),
            Self::PortInUse { port, .. } => {
                write!(
                    f,
                    "port {port} is already held by another program on this machine"
                )
            }
            Self::SpawnFailed => f.write_str("the process could not be started at all"),
            Self::FirstRunFailed { detail } => {
                write!(
                    f,
                    "what has to happen once before it ever starts did not: {detail}"
                )
            }
            Self::Uncheckable { check, reason } => write!(f, "{check} cannot be made: {reason}"),
            Self::DependencyFailed { dependency } => write!(f, "{dependency} did not come up"),
            Self::Unhealthy => f.write_str("the health check failed"),
            Self::Healthy => f.write_str("the health check passed again"),
            Self::Exited { code: Some(code) } => write!(f, "it exited with {code}"),
            // No code is a Unix process killed by a signal, and this crate does not know which one
            // — the supervisor that reaped it did, and chose not to carry it. Saying "exited with
            // 0" here would report a kill as a clean exit.
            Self::Exited { code: None } => f.write_str("it exited without a status"),
            Self::BackoffElapsed { attempt } => write!(f, "attempt {attempt} after a crash"),
            Self::CrashLoop {
                attempts, window, ..
            } => write!(f, "{attempts} failed starts within {window}"),
            Self::Vanished => {
                f.write_str("its process did not survive the daemon that was supervising it")
            }
            Self::Unadopted { reason } => {
                write!(f, "it outlived that daemon and was stopped: {reason}")
            }
            Self::Idle { after } => write!(f, "nothing used it for {after}"),
        }
    }
}

/// One move of the state machine: what changed, why, and when.
///
/// The same value is written to `services.state` and published as
/// [`crate::DaemonEvent::ServiceStateChanged`], which is the only reason this is a type rather than
/// four arguments. `mixengine-core` returns one from the transaction that persisted it and the
/// daemon publishes exactly that — so the row and the event are not two descriptions of an event
/// that can disagree, they are one description used twice.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceTransition {
    /// Which service moved.
    pub service: ServiceId,
    /// Where it was. Carried rather than left for the client to remember, because an event stream
    /// is best-effort: a client that missed the previous one still gets a complete sentence.
    pub from: ServiceState,
    /// Where it is now.
    pub to: ServiceState,
    /// What caused it.
    pub reason: StateReason,
    /// When it happened, as read by the caller that made the move.
    ///
    /// Carried by the event, not stored: `services` keeps only the state a restart has to recover,
    /// and no column holds the moment of a transition. This is what orders the lines in a GUI's
    /// activity list within one run of the daemon, and it is supplied rather than read from the
    /// clock here so that a test can say when.
    pub at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reason a client can be handed reads as a sentence after a state, with the numbers that
    /// make it actionable in it. The clause a rendering puts in front is `failed — …`, so nothing
    /// here may start with a capital or end with a full stop.
    #[test]
    fn a_reason_explains_itself_in_the_clause_that_follows_a_state() {
        let sentences = [
            (
                StateReason::ReadyTimeout {
                    after: Millis::from_secs(10),
                },
                "not ready within 10s",
            ),
            (
                StateReason::DependencyFailed {
                    dependency: ServiceId::parse("mariadb@main").unwrap(),
                },
                "mariadb@main did not come up",
            ),
            (
                StateReason::CrashLoop {
                    attempts: 5,
                    window: Millis::from_secs(300),
                    // The evidence is a client's to lay out as lines; the sentence is the count.
                    tail: vec!["Address already in use".to_owned()],
                },
                "5 failed starts within 5m",
            ),
            (StateReason::Exited { code: Some(1) }, "it exited with 1"),
            (
                StateReason::Exited { code: None },
                "it exited without a status",
            ),
            (
                StateReason::PortInUse {
                    port: 3306,
                    pid: Some(4242),
                    program: Some("mysqld.exe".to_owned()),
                },
                "port 3306 is already held by mysqld.exe",
            ),
            // The Windows service and the other user's daemon: seen, not named.
            (
                StateReason::PortInUse {
                    port: 3306,
                    pid: Some(4242),
                    program: None,
                },
                "port 3306 is already held by pid 4242",
            ),
            // Linux, where a socket belonging to another account maps to no readable pid at all.
            (
                StateReason::PortInUse {
                    port: 3306,
                    pid: None,
                    program: None,
                },
                "port 3306 is already held by another program on this machine",
            ),
            (
                StateReason::Vanished,
                "its process did not survive the daemon that was supervising it",
            ),
            (
                StateReason::Unadopted {
                    reason: "nothing declares it any more".to_owned(),
                },
                "it outlived that daemon and was stopped: nothing declares it any more",
            ),
            (
                StateReason::Idle {
                    after: Millis::from_secs(30 * 60),
                },
                "nothing used it for 30m",
            ),
        ];

        for (reason, expected) in sentences {
            assert_eq!(reason.to_string(), expected);
        }
    }

    /// The rule the whole module rests on: one spelling for the wire and for `services.state`.
    /// A `#[serde(rename)]` on a single variant would otherwise put a value in the database that
    /// nothing can read back, and only for that variant.
    #[test]
    fn the_stored_spelling_is_the_wire_spelling() {
        for state in ServiceState::ALL {
            let encoded = serde_json::to_string(&state).unwrap();

            assert_eq!(
                encoded,
                format!(r#""{}""#, state.as_str()),
                "{state:?} is written differently by serde than by as_str"
            );
            assert_eq!(ServiceState::parse(state.as_str()), Some(state));
        }
    }

    #[test]
    fn a_word_that_is_not_a_state_is_not_guessed_at() {
        assert_eq!(ServiceState::parse("Running"), None, "lowercase only");
        assert_eq!(ServiceState::parse("crashed"), None);
        assert_eq!(ServiceState::parse(""), None);
    }

    #[test]
    fn a_state_never_becomes_itself() {
        for state in ServiceState::ALL {
            assert!(
                !state.can_become(state),
                "{state} → {state} is an event in which nothing happened"
            );
        }
    }

    /// Every state can be reached and every state can be left, or the machine has a hole in it: a
    /// state nothing enters is dead code, and one nothing leaves is a service that can never be
    /// touched again.
    #[test]
    fn no_state_is_a_dead_end_or_unreachable() {
        for state in ServiceState::ALL {
            assert!(
                ServiceState::ALL
                    .into_iter()
                    .any(|other| other.can_become(state)),
                "nothing can reach {state}"
            );
            assert!(
                ServiceState::ALL
                    .into_iter()
                    .any(|other| state.can_become(other)),
                "nothing can leave {state}"
            );
        }
    }

    /// The ones the prose promises and the diagram does not draw.
    #[test]
    fn a_process_that_exits_on_its_own_does_not_pass_through_degraded() {
        assert!(ServiceState::Running.can_become(ServiceState::Failed));
        assert!(ServiceState::Running.can_become(ServiceState::Restarting));
    }

    /// A process can die before it is ever ready — the port was taken, the config did not parse —
    /// and that is the ordinary failure, not an exotic one. Without this edge the only move out of
    /// `Starting` after a crash would be the terminal `Failed`, so a `RestartPolicy` would restart
    /// nothing: pinned here because the gap is invisible until a real service refuses to come up.
    #[test]
    fn a_crash_before_ready_can_still_be_restarted() {
        assert!(ServiceState::Starting.can_become(ServiceState::Restarting));
    }

    /// Every state a restart policy can put a service back from must reach `Restarting`, or the
    /// policy is unenforceable from there. `Stopped` is the exception by definition: nothing is
    /// running to crash.
    #[test]
    fn every_state_that_can_hold_a_process_can_be_restarted() {
        for state in [
            ServiceState::Starting,
            ServiceState::Running,
            ServiceState::Degraded,
            ServiceState::Stopping,
        ] {
            assert!(
                state.can_become(ServiceState::Restarting),
                "a service in {state} could never be put back by a RestartPolicy"
            );
        }
    }

    /// The two states a daemon that was killed can leave a service in without lying about it, and
    /// therefore the two crash recovery has nothing to do to. Everything else claims a supervisor.
    #[test]
    fn only_the_states_nobody_has_to_be_watching_are_left_alone_after_a_crash() {
        let unsupervised: Vec<_> = ServiceState::ALL
            .into_iter()
            .filter(|state| !state.is_supervised())
            .collect();

        assert_eq!(unsupervised, [ServiceState::Stopped, ServiceState::Failed]);
    }

    /// Every state crash recovery meets has to be able to reach the one it puts a service in, or
    /// the reconciliation would be a row it could describe and not move. `Stopping` is that state
    /// for all five, which is why recovery needs no new edge in the machine.
    #[test]
    fn every_supervised_state_can_be_recovered_from() {
        for state in ServiceState::ALL
            .into_iter()
            .filter(|state| state.is_supervised())
        {
            assert!(
                state == ServiceState::Stopping || state.can_become(ServiceState::Stopping),
                "a daemon restart could not reconcile a service left in {state}"
            );
        }

        assert!(ServiceState::Stopping.can_become(ServiceState::Stopped));
    }

    /// `Failed` is where a service is *kept*, not where it is stuck: the two ways out are both
    /// somebody explicitly asking for one.
    #[test]
    fn the_only_ways_out_of_failed_are_asked_for() {
        let ways_out: Vec<_> = ServiceState::ALL
            .into_iter()
            .filter(|state| ServiceState::Failed.can_become(*state))
            .collect();

        assert_eq!(ways_out, [ServiceState::Stopped, ServiceState::Starting]);
    }

    /// A stopped service has exactly one thing that can happen to it. Worth pinning: it is the
    /// state every service is created in, so a spurious edge out of it would be reachable on a
    /// machine that has never started anything.
    #[test]
    fn a_stopped_service_can_only_start() {
        let moves: Vec<_> = ServiceState::ALL
            .into_iter()
            .filter(|state| ServiceState::Stopped.can_become(*state))
            .collect();

        assert_eq!(moves, [ServiceState::Starting]);
    }

    #[test]
    fn a_reason_carries_its_own_discriminator_and_its_numbers() {
        let reason = StateReason::CrashLoop {
            attempts: 5,
            window: Millis::from_secs(300),
            tail: vec!["mariadbd: [ERROR] Address already in use".to_owned()],
        };

        let encoded = serde_json::to_string(&reason).unwrap();
        assert_eq!(
            encoded,
            r#"{"kind":"crash_loop","attempts":5,"window":300000,"tail":["mariadbd: [ERROR] Address already in use"]}"#
        );
        assert_eq!(
            serde_json::from_str::<StateReason>(&encoded).unwrap(),
            reason
        );
    }

    /// The evidence is optional on the way in, so a client or an older event without it still
    /// reads — an empty tail is a service that said nothing, which is a real thing to have happened.
    #[test]
    fn a_crash_loop_without_evidence_still_reads() {
        let decoded: StateReason =
            serde_json::from_str(r#"{"kind":"crash_loop","attempts":5,"window":300000}"#).unwrap();

        assert_eq!(
            decoded,
            StateReason::CrashLoop {
                attempts: 5,
                window: Millis::from_secs(300),
                tail: Vec::new(),
            }
        );
    }

    /// The dependency has to travel with the reason, because the service this is reported for is
    /// the one with nothing in its log to read: it was never spawned.
    #[test]
    fn a_dependency_failure_names_the_dependency() {
        let reason = StateReason::DependencyFailed {
            dependency: ServiceId::parse("mariadb@main").unwrap(),
        };

        let encoded = serde_json::to_string(&reason).unwrap();
        assert_eq!(
            encoded,
            r#"{"kind":"dependency_failed","dependency":"mariadb@main"}"#
        );
        assert_eq!(
            serde_json::from_str::<StateReason>(&encoded).unwrap(),
            reason
        );
    }

    /// A signal-killed process on Unix has no exit code, and `None` has to survive the round trip
    /// as itself — `0` would read as a clean exit.
    #[test]
    fn an_exit_without_a_code_stays_without_one() {
        let reason = StateReason::Exited { code: None };

        let encoded = serde_json::to_string(&reason).unwrap();
        assert_eq!(encoded, r#"{"kind":"exited","code":null}"#);
        assert_eq!(
            serde_json::from_str::<StateReason>(&encoded).unwrap(),
            reason
        );
    }

    #[test]
    fn a_transition_is_a_complete_sentence_on_its_own() {
        let transition = ServiceTransition {
            service: ServiceId::parse("mariadb@main").unwrap(),
            from: ServiceState::Starting,
            to: ServiceState::Running,
            reason: StateReason::Ready,
            at: Timestamp(1_760_000_000_000),
        };

        assert_eq!(
            serde_json::to_string(&transition).unwrap(),
            r#"{"service":"mariadb@main","from":"starting","to":"running","reason":{"kind":"ready"},"at":1760000000000}"#
        );
    }
}
