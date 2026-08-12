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

use crate::{ServiceId, ServiceState, StateReason, Timestamp};

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
