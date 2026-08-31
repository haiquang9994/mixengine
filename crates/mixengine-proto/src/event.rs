//! What arrives on `GET /events`.
//!
//! The vocabulary in `.claude/architecture/daemon-and-ipc.md` — `MetricsSample`, `CertExpiring` and
//! the rest — is declared here **one variant at a time, as the code that emits it lands**. Writing
//! them all up front would mean inventing identifier types before the code that issues them has an
//! opinion, and publishing a wire contract nothing can produce.
//! [`DaemonEvent::ServiceStateChanged`] is the first to arrive that way, with T14; the two job
//! variants are the second, with T22, and they are the case that shows why the rule is worth
//! keeping — `JobId` turned out to be the rowid of a table that did not exist when the architecture
//! document named the type.
//!
//! `LogLine` is the one variant listed there that will never be built: output travels on its own
//! endpoint, per [ADR 0009](https://github.com/mixnz/mixengine/blob/master/.claude/decisions/0009-logs-travel-on-their-own-stream.md).
//!
//! What is here is the part of the stream that belongs to the stream itself, and the rule the
//! architecture states plainly: **events are best-effort and must never be the only way state is
//! learned.** A client that reconnects, or that is told to [`DaemonEvent::Resync`], calls the
//! matching `*.list` and rebuilds what it knows.

use crate::{JobFinish, JobProgress, PendingOp, ServiceTransition, SharingChange, SiteSharing};

/// One message on the event stream.
///
/// Internally tagged, so the discriminator travels inside the JSON object rather than in the SSE
/// `event:` line. That gives the GUI one `onmessage` handler that switches on `type` instead of one
/// `addEventListener` per variant — and it means a variant added in a later phase reaches an older
/// client as an object it can recognise and ignore, rather than as an event type it never
/// subscribed to and silently never sees.
/// **Not [`Eq`], since T22.** [`JobOutcome::Succeeded`](crate::JobOutcome) carries a
/// [`serde_json::Value`], whose float variant has no total equality — and the payload of a job is
/// the one thing on this stream whose shape belongs to the method that produced it rather than to
/// this crate. [`PartialEq`] is what tests compare with anyway.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DaemonEvent {
    /// This receiver fell behind and messages were dropped for it.
    ///
    /// The stream is bounded and a slow consumer is dropped rather than buffered without limit, so
    /// this is the honest thing to send instead of a gap nobody mentions: whatever the client
    /// believes about the daemon's state may now be wrong, and the way to fix that is to ask again
    /// rather than to wait for the next event.
    Resync {
        /// How many messages this receiver missed. For a log line, not for logic — the answer to a
        /// resync is the same whether one was dropped or a thousand.
        missed: u64,
    },

    /// A service moved: started, became ready, went unhealthy, crashed, gave up.
    ///
    /// Carries the [`ServiceTransition`] that was persisted, unchanged and unrepacked — the daemon
    /// publishes the value `mixengine-core` handed back from the transaction that wrote the row.
    /// A second description of the same event, built beside the first, is a second description that
    /// can be wrong.
    ///
    /// A newtype variant rather than inline fields so that the persisted value and the published
    /// one are the same type and not merely the same shape. Internally tagged, so it still arrives
    /// as one flat object: `{"type":"service_state_changed","service":"caddy",…}`.
    ServiceStateChanged(ServiceTransition),

    /// One or more privileged operations are waiting for permission — roadmap task **T40b**.
    ///
    /// **The whole queue and not the row that was just written.** T64's client prints every
    /// operation an `ElevationRequired` batches and what each will change, and a variant carrying
    /// only the newest would make it fetch the rest before it could say anything. The list is read
    /// back inside the transaction that inserted, on
    /// [`ServiceStateChanged`](DaemonEvent::ServiceStateChanged)'s rule: what is announced is what
    /// survived the write.
    ///
    /// An enqueue that changed nothing publishes nothing. The same operation asked for twice is one
    /// row (the T40b design, D2), and an event per attempt would put a producer's retry loop on a
    /// client's screen.
    ElevationRequired {
        /// Everything waiting, oldest first.
        pending: Vec<PendingOp>,
    },

    /// A long operation moved along: a download reached 40%, a verification began.
    ///
    /// **The one event on this stream that is allowed to repeat itself**, and the reason it is
    /// bounded by its producer rather than by this type: the stream holds 1024 messages for the
    /// whole daemon, and a download reporting every socket read would spend a client's entire
    /// allowance on a progress bar — losing exactly the
    /// [`ServiceStateChanged`](DaemonEvent::ServiceStateChanged) the client opened the stream for.
    /// That is the same argument [ADR
    /// 0009](https://github.com/mixnz/mixengine/blob/master/.claude/decisions/0009-logs-travel-on-their-own-stream.md)
    /// makes about log lines, and it lands differently here: a job's progress *is* state, there are
    /// a handful of jobs rather than thousands of lines a second, and it is the producer's job to
    /// report a change and not a heartbeat.
    JobProgress(JobProgress),

    /// A long operation ended, one way or another.
    ///
    /// Carries the [`JobFinish`] that was persisted, unchanged — the same rule
    /// [`ServiceStateChanged`](DaemonEvent::ServiceStateChanged) follows, so an ending that did not
    /// survive its transaction cannot be announced. A client waiting on a job may stop here without
    /// asking again; `job.status` exists for the one that missed it.
    JobFinished(JobFinish),

    /// A site's certificate is running out and MixEngine could not replace it — task **T52**.
    ///
    /// **Only a renewal that failed.** A renewal that worked is not news: the certificate is there,
    /// the front end has been told, and there is nothing for anybody to do about it. This is the
    /// one case a client can act on — a full disk, a damaged authority, a directory that is no
    /// longer writable — and it arrives once per outage rather than once per attempt, on the rule
    /// this module states above. A renewal that fails will keep failing every pass, so an event
    /// each time would spend a client's whole allowance restating one fact.
    ///
    /// No `days_left`. Events are best-effort and never the only way state is learned; a client
    /// that wants the number calls `cert.status` or runs `mix doctor`, both of which report it, and
    /// a number carried here would be a second copy that is stale by definition.
    CertExpiring {
        /// The site, by its primary domain.
        domain: String,

        /// Why the renewal did not happen, in words.
        because: String,
    },

    /// A site went onto the local network, or came off it — roadmap task **T76**.
    ///
    /// **One variant for both directions**, because a client has one question: *is anything shared,
    /// and why did that change?* That is the tray icon `.claude/features/lan-sharing.md` asks for,
    /// and two variants would leave a client that missed one of them drawing the wrong one.
    ///
    /// [`sharing`](Self::SiteSharingChanged::sharing) is the value `site.share` answers with,
    /// unchanged — the rule [`ServiceStateChanged`](DaemonEvent::ServiceStateChanged) states — and
    /// [`None`] for a site that is no longer shared. `because` is what T76 adds over T74: a share
    /// somebody switched off and a share that ended because the laptop moved leave the same state
    /// behind and are different news.
    SiteSharingChanged {
        /// The site, by its primary domain.
        domain: String,

        /// What it is now, or [`None`] for a site no longer shared.
        sharing: Option<SiteSharing>,

        /// Why it changed.
        because: SharingChange,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The newtype variant has to flatten, or the GUI's one `onmessage` handler would have to
    /// unwrap a nested object for this variant and not for the others.
    #[test]
    fn a_state_change_arrives_as_one_flat_object() {
        let event = DaemonEvent::ServiceStateChanged(ServiceTransition {
            service: crate::ServiceId::parse("caddy").unwrap(),
            from: crate::ServiceState::Running,
            to: crate::ServiceState::Degraded,
            reason: crate::StateReason::Unhealthy,
            at: crate::Timestamp(1_760_000_000_000),
        });

        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"service_state_changed","service":"caddy","from":"running","to":"degraded","reason":{"kind":"unhealthy"},"at":1760000000000}"#
        );
        assert_eq!(
            serde_json::from_str::<DaemonEvent>(&encoded).unwrap(),
            event
        );
    }

    /// The same flattening, for the variants T22 added: a job's progress is one object with a
    /// `type`, not a `job_progress` wrapper the GUI would have to unwrap for two variants out of
    /// four.
    #[test]
    fn a_job_moving_arrives_as_one_flat_object() {
        let event = DaemonEvent::JobProgress(JobProgress {
            job: crate::JobId(7),
            percent: 40,
            message: "verifying the download".to_owned(),
            at: crate::Timestamp(1_760_000_000_000),
        });

        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"job_progress","job":7,"percent":40,"message":"verifying the download","at":1760000000000}"#
        );
        assert_eq!(
            serde_json::from_str::<DaemonEvent>(&encoded).unwrap(),
            event
        );
    }

    /// An ending carries its own discriminator *and* the outcome's, which is why the second is
    /// spelled `ending`: two words that both said `type` or both said `outcome` would collide the
    /// moment this variant flattens.
    #[test]
    fn a_job_ending_carries_both_discriminators_without_colliding() {
        let event = DaemonEvent::JobFinished(JobFinish {
            job: crate::JobId(7),
            outcome: crate::JobOutcome::Cancelled,
            at: crate::Timestamp(1_760_000_000_000),
        });

        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"job_finished","job":7,"ending":"cancelled","at":1760000000000}"#
        );
        assert_eq!(
            serde_json::from_str::<DaemonEvent>(&encoded).unwrap(),
            event
        );
    }

    /// The variant flattens like every other, and the reason carries its own discriminator without
    /// colliding with the event's — the shape `JobFinished` established with `ending`.
    #[test]
    fn a_share_ending_arrives_as_one_flat_object_and_says_why() {
        let event = DaemonEvent::SiteSharingChanged {
            domain: "blog.test".to_owned(),
            sharing: None,
            because: SharingChange::NetworkChanged {
                was: "192.168.1.10".to_owned(),
                now: None,
            },
        };

        let encoded = serde_json::to_value(&event).expect("an event serialises");
        assert_eq!(encoded["type"], "site_sharing_changed");
        assert_eq!(encoded["because"]["kind"], "network_changed");
        assert_eq!(encoded["because"]["was"], "192.168.1.10");
        assert!(encoded["sharing"].is_null());
        assert!(
            encoded["because"].get("now").is_none(),
            "an interface that is gone carries no second address: {encoded}"
        );

        assert_eq!(
            serde_json::from_value::<DaemonEvent>(encoded).expect("it reads back"),
            event
        );
    }

    #[test]
    fn an_event_carries_its_own_discriminator() {
        let event = DaemonEvent::Resync { missed: 12 };

        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"type":"resync","missed":12}"#
        );
        assert_eq!(
            serde_json::from_str::<DaemonEvent>(r#"{"type":"resync","missed":12}"#).unwrap(),
            event
        );
    }

    /// D8: the event carries the whole batch, so a client that renders "3 operations are waiting"
    /// does not have to fetch the other two — and it arrives flat, like every other variant.
    #[test]
    fn an_elevation_request_carries_the_whole_queue_as_one_flat_object() {
        let op = crate::privileged::PrivilegedOp::Probe {};
        let event = DaemonEvent::ElevationRequired {
            pending: vec![crate::PendingOp {
                id: crate::PendingOpId(1),
                description: op.describe(),
                op,
                requested_at: crate::Timestamp(1_760_000_000_000),
            }],
        };

        let encoded = serde_json::to_value(&event).unwrap();
        assert_eq!(encoded["type"], "elevation_required");
        assert_eq!(encoded["pending"][0]["op"]["op"], "probe");

        assert_eq!(
            serde_json::from_value::<DaemonEvent>(encoded).unwrap(),
            event
        );
    }
}
