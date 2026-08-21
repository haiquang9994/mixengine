//! What `service.*` answers, where [`crate::service`] is the vocabulary a spec is *written* in.
//!
//! The split is the same one [`crate::daemon`] draws for `daemon.*`: a [`ServiceSpec`] describes a
//! program to run and is the input to supervision, while these types describe what the daemon then
//! made of it and are the output. A client renders these and never sees a spec.
//!
//! **What is deliberately not here is a reason for the state a service is in.** `services` keeps
//! only the columns a restart has to recover, and no column holds the *why* of a transition — that
//! travels with [`crate::DaemonEvent::ServiceStateChanged`], which carries the
//! [`StateReason`] of every move as it happens. A summary read a minute later
//! can say `failed` and cannot say what failed; the one place a walk *can* say so is
//! [`ServiceFailure`], because there the daemon has just watched it happen.
//!
//! [`ServiceSpec`]: crate::ServiceSpec

use crate::{PackageVersion, ServiceId, ServiceState, StateReason, Timestamp};

/// Which services a call is about, and whether the caller waits for the answer to be true.
///
/// One params type for `service.start`, `service.stop` and `service.restart`, because the question
/// each asks is the same one: *which* service, and *how long may this take*.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceTarget {
    /// The service to act on, or [`None`] for every declared service.
    ///
    /// Naming one does not mean acting on one: a plan is the transitive set, so starting `php-fpm`
    /// pulls in what it depends on and stopping `mariadb` pulls in what depends on *it*. What the
    /// walk covered comes back in [`ServiceWalk::planned`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<ServiceId>,

    /// Whether to answer when the walk has finished rather than when it has been accepted.
    ///
    /// **Defaults to true, and the exit code is why.** `mix service start db && mix …` is a sentence
    /// about the database being up; an answer sent before the walk would make it exit `0` for a
    /// service that never came up, and the only way for a client to know better would be to re-derive
    /// the verdict from the event stream — the business-logic-in-a-client bug `CLAUDE.md` forbids.
    ///
    /// A GUI sends `false`: it is already subscribed to
    /// [`ServiceStateChanged`](crate::DaemonEvent::ServiceStateChanged) and would rather draw each
    /// service arriving than block a window on the slowest one. What comes back then is the plan and
    /// [`ServiceWalk::complete`] is `false` — the walk itself carries on inside the daemon.
    #[serde(default = "waits", skip_serializing_if = "is_waiting")]
    pub wait: bool,
}

impl Default for ServiceTarget {
    fn default() -> Self {
        Self {
            service: None,
            wait: waits(),
        }
    }
}

/// The `wait` a client that says nothing gets. See [`ServiceTarget::wait`].
fn waits() -> bool {
    true
}

/// Serialising the default is noise on the wire, and this is the one field with a non-`None` one.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "the signature serde's `skip_serializing_if` requires"
)]
fn is_waiting(wait: &bool) -> bool {
    *wait
}

/// Which service `service.status` is about.
///
/// Its own type rather than a [`ServiceTarget`] with the option filled in, because the id is
/// **required** here: a status with no subject is a `service.list` that was typed wrongly, and
/// answering it as one would hide the mistake instead of reporting `invalid_params`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceQuery {
    /// The service to describe.
    pub service: ServiceId,
}

/// What `service.list` answers.
///
/// An object around the list rather than a bare array, on [`crate::DaemonStatus`]'s precedent: a
/// field can be added beside it — a count, a note about where the declarations came from — without
/// changing the shape of every existing client's parser.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceList {
    /// Every declared service, in [`ServiceId`] order.
    pub services: Vec<ServiceSummary>,
}

/// One service, as the daemon currently sees it. Also the whole of what `service.status` answers.
///
/// One type for the list and for the single lookup on purpose: they are the same sentence about a
/// service, so a client renders them with one function and a field added here reaches both.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceSummary {
    /// Which service this is.
    pub id: ServiceId,

    /// What the `services` row says it is doing.
    ///
    /// **[`None`] means declared but never created**: something describes how to run this service
    /// and the daemon has no row for it, so it has no state to be in and cannot be started until one
    /// exists. Not a case a finished MixEngine reaches — from T30 onwards a declaration is *rendered
    /// from* a row — which is exactly why it is reported rather than smoothed over into `stopped`: a
    /// service that quietly claims to be stopped and then refuses to start explains nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ServiceState>,

    /// Whether a task in this daemon is supervising it right now.
    ///
    /// Not a second opinion about [`ServiceSummary::state`] — the row is the only thing that says
    /// what a service is doing — but the answer to a different question: a row saying `running` with
    /// nothing supervising it is what a daemon that was killed leaves behind, and adopting or
    /// clearing those is T18. Until then this is where that gap is visible instead of implied.
    pub supervised: bool,

    /// The process it is running as, where there is one.
    pub pid: Option<u32>,

    /// The port its row was given, and [`None`] for a service that listens on none.
    ///
    /// **The number in the row, not one derived from the running process** — roadmap task
    /// **T34c**. It is allocated once, when the service is created, and never recomputed: it is in
    /// somebody's `.env` and in a colleague's shell history by the end of the afternoon, so a
    /// service that quietly moved between restarts would break both. A stopped service has it just
    /// as a running one does, which is the point of reporting it here rather than only at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// When it was last started, whether or not it is still running.
    pub last_started_at: Option<Timestamp>,

    /// What its process exited with the last time one ended. [`None`] where the OS reported none —
    /// a Unix process killed by a signal — for the same reason [`StateReason::Exited`] carries an
    /// option there.
    pub last_exit_code: Option<i32>,

    /// What it declares it needs, as the graph holds it: each dependency once, in [`ServiceId`]
    /// order. Empty for a service that depends on nothing.
    pub depends_on: Vec<ServiceId>,
}

/// What a `service.start`, `service.stop` or `service.restart` did.
///
/// **Not a success or a failure**, which is why it is a struct rather than an error: a plan of six
/// services where the fourth fails leaves three running, one failed and two never tried, and a
/// client that has to render that needs all three lists. The failure is still a failure — `mix`
/// exits non-zero on one — but it is a described failure and not a lost walk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceWalk {
    /// Everything the plan covered, in the order it was walked.
    ///
    /// This is the transitive set and not what the caller named: it is what a client watches the
    /// event stream for, and the only way to know that starting one service is about to touch four.
    pub planned: Vec<ServiceId>,

    /// Whether the rest of this answer describes a finished walk.
    ///
    /// `false` for a call that asked not to wait ([`ServiceTarget::wait`]), where the plan has been
    /// accepted and is being walked behind this answer — the three lists below are then empty
    /// because nothing has happened yet, not because nothing happened.
    pub complete: bool,

    /// Services that reached what the walk was aiming for, in the order they got there.
    pub reached: Vec<ServiceId>,

    /// The service that stopped the walk, where one did.
    ///
    /// Always [`None`] for a stop: a service that is not running is already where the caller wants
    /// it, so there is no state a stop fails to reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<ServiceFailure>,

    /// Services never tried, because something they depend on failed.
    ///
    /// Each was recorded as `failed` with
    /// [`DependencyFailed`](StateReason::DependencyFailed) naming the direct edge it declared, so a
    /// chain of four reads as four sentences leading to the one service to fix.
    pub blocked: Vec<ServiceId>,
}

/// What `service.create` takes: which service, from which version of its package.
///
/// **The package is not a field**, because [`ServiceId::name`] already is one: it documents itself
/// as "the part before `@` — the package this is an instance of", so a second parameter would either
/// repeat the id or be a pair somebody has to police for agreement. The invariant is better held by
/// construction than by a check.
///
/// **The version is required**, on [`RuntimeTarget`](crate::RuntimeTarget)'s reasoning: choosing a
/// version for somebody is a decision, and there is no `service.resolve` to make it.
///
/// Everything else is optional because the column behind it is nullable and null already means
/// something: a service with no port is one whose recipe renders no port line, and a service with no
/// data directory is one the generator places under `data/<package>` itself.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceCreate {
    /// Which service to create, and — through its name — which package it is an instance of.
    pub id: ServiceId,

    /// Which installed version of that package to run.
    pub version: PackageVersion,

    /// The port it listens on, or [`None`] for the recipe's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// The address it binds, or [`None`] for `127.0.0.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_addr: Option<String>,

    /// Where its data lives, or [`None`] for the home's own layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,

    /// Whether it starts with the daemon. [`None`] is the column's default, which is no.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,

    /// The settings this instance overrides, checked against the recipe before the row is kept.
    ///
    /// A map rather than a [`String`] of JSON: the column holds a document, and a client that had to
    /// serialise one itself would be a client that could send something that is not an object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Why a service is not on the port its recipe would have preferred — roadmap task **T34c**.
///
/// **A port a person did not pick is one they have to be told about.** A recipe names the number
/// its product is documented under — 3306 for both databases, 6379 for Redis — and the first row to
/// ask for it is given it; every later asker is given the first free port above. So the sentence
/// this carries is either "the number you know is in use by `mysqld.exe`" or "another MixEngine
/// service already has it", and both of them are things a developer whose `.env` says 3306 needs to
/// read at the moment of creation rather than to discover from a connection that is refused.
///
/// The two identifying fields are separate rather than one rendered string for
/// [`StateReason::PortInUse`](crate::StateReason::PortInUse)'s reason, and are empty in the case
/// that variant cannot have: a port lost to another *MixEngine* service is held by a process the
/// daemon knows about and may not be running at all.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PortMoved {
    /// The port the recipe asked for, and did not get.
    pub preferred: u16,

    /// The process holding it, where this account may learn it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,

    /// The file name of that process's program, where this account may read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
}

/// What `service.create` answers: the service, and the port story where there is one.
///
/// **Its own type rather than a [`ServiceSummary`]** — roadmap task **T34c** — because a create
/// answers two different questions and only one of them is true forever. What the service *is*
/// outlives the call and is a [`ServiceSummary`]; why it is not on the port its recipe prefers is
/// true only of this moment, and a field for it on every listing would be a sentence about a
/// decision nobody is making any more.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceCreation {
    /// The service that now exists, as `service.status` would answer it.
    pub service: ServiceSummary,

    /// Why its port is not the one its recipe asked for, when it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<PortMoved>,
}

/// What `service.delete` answers: what went, and what deliberately did not.
///
/// **A delete removes a row and a configuration directory, and never a data directory.** Generated
/// config is disposable and can be rendered again from the row; a data directory is somebody's
/// databases, and there is no undo behind a local development tool. So the path is *named* rather
/// than removed, because a directory nobody was told about is a directory nobody ever cleans up.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceRemoval {
    /// The service as it stood before its row went, which is the only moment anything could describe
    /// it.
    pub removed: ServiceSummary,

    /// The data directory left in place, when there was one on disk to leave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_kept: Option<String>,
}

/// The service a walk stopped at, and what was persisted about why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceFailure {
    /// Which service.
    pub service: ServiceId,

    /// What was written about it, which is the same value the matching
    /// [`ServiceStateChanged`](crate::DaemonEvent::ServiceStateChanged) carried.
    ///
    /// [`None`] when the failure was the daemon's own — a database that would not take the write, a
    /// supervising task that panicked. That is in `daemon.log` and is not a state a client could
    /// render; a client meeting one says so rather than inventing a reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<StateReason>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(id: &str) -> ServiceId {
        ServiceId::parse(id).expect("a valid service id")
    }

    /// One service, with nothing interesting about it.
    fn summary(id: &str) -> ServiceSummary {
        ServiceSummary {
            id: service(id),
            state: Some(ServiceState::Stopped),
            supervised: false,
            pid: None,
            port: None,
            last_started_at: None,
            last_exit_code: None,
            depends_on: Vec::new(),
        }
    }

    /// A create names the service and the version, and derives the package from the id — which is
    /// what [`ServiceId::name`] has always said it is.
    #[test]
    fn a_create_takes_an_id_and_a_version_and_nothing_redundant() {
        let create: ServiceCreate =
            serde_json::from_str(r#"{"id":"mariadb@main","version":"11.4.2"}"#)
                .expect("the two required fields");

        assert_eq!(create.id.name(), "mariadb");
        assert_eq!(create.id.instance(), Some("main"));
        assert_eq!(create.version.as_str(), "11.4.2");
        assert_eq!(
            create.port, None,
            "a port nobody named is the recipe's own default"
        );
    }

    /// A delete says what it kept, because what it kept is somebody's databases.
    #[test]
    fn a_removal_names_the_data_it_did_not_touch() {
        let removal = ServiceRemoval {
            removed: summary("mariadb@main"),
            data_kept: Some("/home/me/.local/share/mixengine/data/mariadb/main".to_owned()),
        };

        let encoded = serde_json::to_value(&removal).expect("a removal encodes");

        assert_eq!(
            encoded["data_kept"],
            "/home/me/.local/share/mixengine/data/mariadb/main"
        );
        assert_eq!(
            serde_json::from_value::<ServiceRemoval>(encoded).expect("and decodes"),
            removal
        );
    }

    /// The shape a client that types nothing gets: everything, and wait for it.
    #[test]
    fn a_target_with_no_parameters_means_every_service_and_waiting() {
        let target: ServiceTarget = serde_json::from_str("{}").expect("every field has a default");

        assert_eq!(target, ServiceTarget::default());
        assert_eq!(target.service, None);
        assert!(target.wait, "a client that said nothing is waited for");
    }

    #[test]
    fn not_waiting_is_the_one_thing_a_target_has_to_spell_out() {
        let target: ServiceTarget =
            serde_json::from_str(r#"{"service":"mariadb@main","wait":false}"#)
                .expect("both fields");

        assert_eq!(target.service, Some(service("mariadb@main")));
        assert!(!target.wait);

        // The default is not written back out: `{"service":"…"}` and `{"service":"…","wait":true}`
        // are the same request, and only one of them is worth putting on the wire.
        assert_eq!(
            serde_json::to_string(&ServiceTarget {
                service: Some(service("mariadb@main")),
                wait: true,
            })
            .unwrap(),
            r#"{"service":"mariadb@main"}"#
        );
    }

    #[test]
    fn a_status_without_a_service_does_not_decode() {
        serde_json::from_str::<ServiceQuery>("{}")
            .expect_err("a status with no subject is a mistyped list, not a list");
    }

    #[test]
    fn a_summary_of_a_service_with_no_row_omits_the_state_rather_than_inventing_one() {
        let summary = ServiceSummary {
            id: service("mailpit"),
            state: None,
            supervised: false,
            pid: None,
            port: None,
            last_started_at: None,
            last_exit_code: None,
            depends_on: Vec::new(),
        };

        let encoded = serde_json::to_value(&summary).unwrap();
        assert!(encoded.get("state").is_none(), "{encoded}");
        assert_eq!(encoded["supervised"], false);
        assert_eq!(
            serde_json::from_value::<ServiceSummary>(encoded).unwrap(),
            summary
        );
    }

    #[test]
    fn a_walk_that_stopped_carries_the_reason_the_transition_did() {
        let walk = ServiceWalk {
            planned: vec![service("mariadb@main"), service("php-fpm@8.3")],
            complete: true,
            reached: Vec::new(),
            failed: Some(ServiceFailure {
                service: service("mariadb@main"),
                reason: Some(StateReason::ReadyTimeout {
                    after: crate::Millis::from_secs(10),
                }),
            }),
            blocked: vec![service("php-fpm@8.3")],
        };

        let encoded = serde_json::to_value(&walk).unwrap();
        assert_eq!(encoded["failed"]["service"], "mariadb@main");
        assert_eq!(encoded["failed"]["reason"]["kind"], "ready_timeout");
        assert_eq!(
            serde_json::from_value::<ServiceWalk>(encoded).unwrap(),
            walk
        );
    }

    #[test]
    fn a_walk_that_was_not_waited_for_says_so_rather_than_looking_like_one_that_did_nothing() {
        let accepted = ServiceWalk {
            planned: vec![service("mariadb@main")],
            complete: false,
            reached: Vec::new(),
            failed: None,
            blocked: Vec::new(),
        };

        let encoded = serde_json::to_value(&accepted).unwrap();
        assert_eq!(encoded["complete"], false);
        assert!(
            encoded.get("failed").is_none(),
            "nothing failed — nothing has happened yet: {encoded}"
        );
    }
}
