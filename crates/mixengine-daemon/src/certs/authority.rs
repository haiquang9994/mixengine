//! Replacing this home's certificate authority, and taking it back out — roadmap task **T54**.
//!
//! **The only two operations in phase 5 that take something away.** Five tasks put an authority into
//! this machine and kept the leaves under it fresh; these undo that, and the difference in risk is
//! why the two are shaped differently from each other.
//!
//! **Here rather than on [`Certificates`].** That struct holds a directory, a host and a store;
//! these need the elevation queue and the job registry as well, and a struct that grew fields for
//! them would be holding most of the daemon. `certs::renewal` set the precedent in T52: what needs
//! more than certificates takes them as arguments.
//!
//! This module is a child of `crate::certs`, which is what lets it read [`Certificates`]' own fields
//! and reuse `super::blocking` rather than adding accessors nothing else would call.

use std::sync::Arc;

use mixengine_proto::{
    CaRotateReport, CaState, CaStatus, CaUninstallReport, Error, ErrorCode, JobSummary,
    RotateOutcome, UninstallOutcome,
};

use super::Certificates;
use crate::elevation::Elevation;
use crate::error::ToWire as _;
use crate::jobs::Jobs;

/// Take this home's authority out of every store that trusts it — `cert.ca_uninstall`.
///
/// **Trust and never a file.** `certs/ca/` and every leaf are left exactly as they are: removing
/// trust is undone by `mix doctor --repair`, and deleting a private key is undone by nothing.
/// Deleting is uninstall's, T87.
///
/// **Partial progress is the right answer here and would be the wrong one for a rotation.** Each
/// store is independent, so taking the authority out of Firefox is a complete action on Firefox
/// whatever the system store did — the browser databases need no privilege in either direction,
/// which is the line T49 was split on.
///
/// # Errors
///
/// Only when the job row cannot be written. Everything reached afterwards is an
/// [`UninstallOutcome`].
pub(crate) async fn uninstall(
    certificates: Certificates,
    elevation: Arc<Elevation>,
    jobs: &Arc<Jobs>,
) -> Result<JobSummary, Error> {
    let kind = mixengine_proto::JobKind::parse(mixengine_proto::rpc::method::CERT_CA_UNINSTALL)
        .expect("a valid kind");

    jobs.begin(&kind, move |handle| async move {
        let report = removing(&certificates, &elevation, &handle).await?;

        serde_json::to_value(&report).map_err(|error| {
            Error::new(
                ErrorCode::Internal,
                format!("the removal could not be described: {error}"),
            )
        })
    })
    .await
}

/// Replace this home's authority with a new one — `cert.ca_rotate`.
///
/// **Nothing is replaced until a fresh reading of the store agrees.** The candidate is generated
/// into a staging certificates root, one elevation grant covers taking the old certificate out and
/// putting the new one in, and a reading afterwards decides whether any of it is kept. A declined
/// prompt leaves this home exactly as it was — see [`commits`].
///
/// # Errors
///
/// Only when the job row cannot be written. Everything reached afterwards is a [`RotateOutcome`].
pub(crate) async fn rotate(
    certificates: Certificates,
    elevation: Arc<Elevation>,
    services: Arc<crate::services::Registry>,
    jobs: &Arc<Jobs>,
) -> Result<JobSummary, Error> {
    let kind = mixengine_proto::JobKind::parse(mixengine_proto::rpc::method::CERT_CA_ROTATE)
        .expect("a valid kind");

    jobs.begin(&kind, move |handle| async move {
        let report = rotating(&certificates, &elevation, &services, &handle).await?;

        serde_json::to_value(&report).map_err(|error| {
            Error::new(
                ErrorCode::Internal,
                format!("the rotation could not be described: {error}"),
            )
        })
    })
    .await
}

/// The body of [`rotate`], one step per line of the T54 design's D3.
async fn rotating(
    certificates: &Certificates,
    elevation: &Arc<Elevation>,
    services: &Arc<crate::services::Registry>,
    handle: &crate::jobs::JobHandle,
) -> Result<CaRotateReport, Error> {
    let state = certificates.authority().await?;

    if matches!(state, CaState::Absent {}) {
        return Ok(CaRotateReport {
            outcome: RotateOutcome::NothingToRotate {
                because: "this home has no certificate authority to replace".to_owned(),
            },
            previous: None,
            status: certificates.status().await?,
            sites: Vec::new(),
        });
    }

    // `None` for a damaged authority, which is a state this repairs rather than refuses. It is also
    // the one case where the old certificate is left in the store: T49a's D5 forbids naming a
    // removal target that could not be read, and guessing one is how a corporate root gets deleted.
    let previous = match &state {
        CaState::Present { ca } => Some(ca.clone()),
        _ => None,
    };

    handle
        .progress(10, "making a new certificate authority")
        .await;

    let candidate = stage(certificates).await?;

    // **Read before anything can remove it.** Asked afterwards this always answers "no", because
    // the removal is what made it so — and a rotation that read it late would commit every time,
    // leaving [`commits`]' third clause in the source and out of the behaviour.
    let before = reading(certificates, previous.as_ref()).await;

    handle
        .progress(30, "asking to change this machine's trust store")
        .await;

    // Both operations, one queue, one prompt — the argument phase 5 already made and recorded when
    // it refused to move the trust store per-user.
    if let Some(ca) = previous.as_ref() {
        certificates.require_untrust(elevation, ca).await?;
    }
    if let Some(der) = mixengine_core::certs::ca::der(&candidate.certificate_pem) {
        elevation.require_trust_store(Some(&der)).await?;
    }

    let asked = elevation.grant_within(handle).await;

    handle.progress(60, "reading the trust store back").await;

    let after = reading(certificates, Some(&candidate)).await;

    if let Err(because) = commits(&before, &after) {
        unstage(certificates).await?;

        let because = match &asked {
            Err(error) => format!("{because} ({})", mixengine_proto::flatten(error)),
            Ok(_) => because,
        };

        return Ok(CaRotateReport {
            outcome: RotateOutcome::NotCommitted { because },
            previous,
            status: certificates.status().await?,
            sites: Vec::new(),
        });
    }

    handle
        .progress(75, "reissuing every site's certificate")
        .await;

    commit_stage(certificates).await?;

    if previous.is_none() {
        tracing::warn!(
            "the old certificate was left in this machine's trust store: this home's previous \
             authority could not be read, and nothing is removed that cannot be named by key-id"
        );
    }

    // **No reissue code, and that is T50's fourth reuse question doing its job.** The moment
    // `certs/ca/root.crt` names a different authority, every leaf on disk is stale by the existing
    // rule, so the call `mix cert issue` already runs replaces all of them.
    let issued = certificates.issue(None).await?;

    let promoted = certificates.authority().await?;
    certificates.install_in_browsers(&promoted).await;
    if let Some(ca) = previous.as_ref() {
        certificates.remove_from_browsers(&ca.key_id).await;
    }

    handle.progress(90, "telling the front end").await;

    // The reload is T51's fingerprint and nothing new: a reissued certificate changes the rendered
    // header, so the file differs and the installer finds a change.
    if let Err(error) = services.reconfigure().await {
        tracing::warn!(
            ?error,
            "the authority was replaced and the front end has not been told"
        );
    }

    Ok(CaRotateReport {
        outcome: RotateOutcome::Rotated {},
        previous,
        status: certificates.status().await?,
        sites: issued.sites,
    })
}

/// Generate the candidate, throwing away anything a previous rotation left staged.
///
/// **`ensure` on a different root**, which is the whole of the T54 design's D3: the candidate is
/// made by the code that made the authority it will replace, so the two cannot be made differently.
async fn stage(certificates: &Certificates) -> Result<mixengine_proto::Ca, Error> {
    let certs = certificates.certs.clone();

    // **Each half converts its own error before the next runs.** `mixengine_core::Error` is over 128
    // bytes, so carrying one through a combinator puts a large error in every frame of the chain and
    // `clippy::result_large_err` says so — the boundary `Certificates::ensure` already converts at.
    let state = super::blocking("staging a new", move || {
        mixengine_core::certs::ca::discard(&certs).map_err(|error| error.to_wire())?;

        mixengine_core::certs::ca::ensure(
            &mixengine_core::certs::ca::pending_root(&certs),
            std::time::SystemTime::now(),
        )
        .map_err(|error| error.to_wire())
    })
    .await??;

    match state {
        CaState::Present { ca } => Ok(ca),
        other => Err(Error::new(
            ErrorCode::Internal,
            format!("a new certificate authority was made and is not usable: {other:?}"),
        )),
    }
}

/// Throw the candidate away. This home is then exactly what it was.
async fn unstage(certificates: &Certificates) -> Result<(), Error> {
    let certs = certificates.certs.clone();

    super::blocking("discarding a staged", move || {
        mixengine_core::certs::ca::discard(&certs).map_err(|error| error.to_wire())
    })
    .await?
}

/// Make the candidate this home's authority.
async fn commit_stage(certificates: &Certificates) -> Result<(), Error> {
    let certs = certificates.certs.clone();

    super::blocking("promoting a staged", move || {
        mixengine_core::certs::ca::promote(&certs).map_err(|error| error.to_wire())
    })
    .await?
}

/// What this machine's trust store said when it was asked about one certificate.
///
/// A reading and not a verdict: [`commits`] is what turns two of these into a decision, and keeping
/// them apart is what lets that decision be tested without a trust store — which matters, because no
/// machine running `cargo test` can raise the elevation prompt a rotation needs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Held {
    /// The store holds exactly these bytes.
    Yes,

    /// It does not, and it is a store MixEngine knows how to write.
    No,

    /// This machine has no system trust store MixEngine knows how to write.
    NoStore,

    /// The store could not be read.
    Unreadable {
        /// What went wrong.
        because: String,
    },
}

/// Ask this machine's trust store about one authority.
///
/// [`Held::Unreadable`] for a home with no host and for an authority with no readable DER: nothing
/// was asked, so nothing is known — and [`commits`] refuses on that, which is the safe direction.
async fn reading(certificates: &Certificates, ca: Option<&mixengine_proto::Ca>) -> Held {
    let (Some(host), Some(der)) = (
        certificates.host.clone(),
        ca.and_then(|ca| mixengine_core::certs::ca::der(&ca.certificate_pem)),
    ) else {
        return Held::Unreadable {
            because: "this machine's trust store was not read".to_owned(),
        };
    };

    super::blocking("reading this machine's trust store about", move || {
        held(host.as_ref(), &der)
    })
    .await
    .unwrap_or_else(|_| Held::Unreadable {
        because: "the task reading this machine's trust store did not finish".to_owned(),
    })
}

/// One probe, turned into a [`Held`].
fn held(host: &dyn mixengine_platform::Host, der: &[u8]) -> Held {
    match host.trust_store().probe(der) {
        Ok(state) if state.installed => Held::Yes,
        Ok(state) if state.method == mixengine_platform::TrustStoreMethod::None => Held::NoStore,
        Ok(_) => Held::No,
        Err(error) => Held::Unreadable {
            because: error.to_string(),
        },
    }
}

/// Whether a rotation may replace this home's authority — the T54 design, D7.
///
/// The question is **not** "is the new authority in the store". A machine with no store MixEngine
/// can write would never pass that, and such a machine is supported rather than broken. The question
/// is whether this machine is *less* able to trust the new authority than it was to trust the old
/// one.
///
/// `old` is read before the removal is applied and `new` after the grant.
///
/// # Errors
///
/// The sentence to put in [`RotateOutcome::NotCommitted`].
fn commits(old: &Held, new: &Held) -> Result<(), String> {
    match (old, new) {
        // The rotation did what it set out to do, or there is no store to be worse than — on Linux
        // the browsers are reached through NSS and not through a system store at all.
        (_, Held::Yes | Held::NoStore) => Ok(()),

        // This store was never trusting ours, so the rotation changes nothing about it.
        (Held::No, _) => Ok(()),

        // **Doubt refuses, where every other probe in this daemon carries on.** `require_trust_store`
        // and `require_port_access` can treat a failed read as "ask for nothing", because what they
        // do next is harmless. This is the one destructive operation in phase 5, and the staging
        // design makes refusing cost nothing but a deleted directory.
        (_, Held::Unreadable { because }) => Err(format!(
            "this machine's trust store could not be read afterwards, so whether it trusts the new \
             authority is unknown: {because}"
        )),

        (Held::Unreadable { because }, _) => Err(format!(
            "this machine's trust store could not be read beforehand, so whether this rotation \
             would take trust away is unknown: {because}"
        )),

        (Held::Yes | Held::NoStore, Held::No) => Err("this machine trusted the old authority and \
             does not trust the new one, so replacing it would have left every site serving a \
             certificate no browser accepts"
            .to_owned()),
    }
}

/// The body of [`uninstall`], so the job closure stays one statement.
async fn removing(
    certificates: &Certificates,
    elevation: &Arc<Elevation>,
    handle: &crate::jobs::JobHandle,
) -> Result<CaUninstallReport, Error> {
    let state = certificates.authority().await?;

    let CaState::Present { ca } = &state else {
        return Ok(CaUninstallReport {
            outcome: UninstallOutcome::NothingToRemove {
                because: "this home has no usable certificate authority, so nothing that could be \
                          named is in any store"
                    .to_owned(),
            },
            status: certificates.status().await?,
        });
    };

    handle
        .progress(20, "asking this machine's browsers to let it go")
        .await;

    let browsers = certificates.remove_from_browsers(&ca.key_id).await;

    handle
        .progress(40, "asking to take it out of this machine's trust store")
        .await;

    let granted = match certificates.require_untrust(elevation, ca).await? {
        true => elevation.grant_within(handle).await.map(|_| ()),
        // Nothing was enqueued, so there is nothing to grant and no prompt to spend.
        false => Ok(()),
    };

    handle.progress(80, "reading the stores back").await;

    // **Measured, never reported** — the T54 design, D2. The helper is honest about what it did, but
    // it is a separate process describing finished work; this is a fresh reading of the thing
    // itself, and it costs no privilege on any of the three systems.
    let status = certificates.status().await?;

    Ok(CaUninstallReport {
        outcome: outcome(&status, granted.as_ref(), &browsers),
        status,
    })
}

/// What to call the result, from what the stores say afterwards.
///
/// **The measurement decides and the error only supplies words.** A grant that failed against a
/// store that turns out not to hold it anyway is a removal, because the question this report answers
/// is what the machine holds and not what the daemon attempted.
fn outcome(
    status: &CaStatus,
    granted: Result<&(), &Error>,
    browsers: &mixengine_platform::BrowserChange,
) -> UninstallOutcome {
    if matches!(status.trust, mixengine_proto::Trust::Installed { .. }) {
        return UninstallOutcome::PartlyRemoved {
            because: match granted {
                Err(error) => format!(
                    "this machine's trust store still holds it: {}",
                    mixengine_proto::flatten(error)
                ),
                Ok(()) => "this machine's trust store still holds it".to_owned(),
            },
        };
    }

    match browsers.refused.first() {
        Some(refused) => UninstallOutcome::PartlyRemoved {
            because: format!("a browser database still holds it: {refused}"),
        },
        None => UninstallOutcome::Removed {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(trust: mixengine_proto::Trust) -> CaStatus {
        CaStatus {
            state: CaState::Absent {},
            trust,
            browsers: mixengine_proto::Browsers::NoTool {
                because: "certutil is not installed".to_owned(),
            },
        }
    }

    fn refusing(because: &str) -> mixengine_platform::BrowserChange {
        mixengine_platform::BrowserChange {
            written: Vec::new(),
            refused: vec![because.to_owned()],
        }
    }

    /// The store holds the new authority: the rotation did what it set out to do.
    ///
    /// **These are the tests that matter most in T54.** No machine running `cargo test` can raise
    /// an elevation prompt, so no integration test can drive a rotation to the commit branch — which
    /// is why the decision is a pure function and why this is where it is proved.
    #[test]
    fn a_store_that_holds_the_new_authority_commits() {
        assert_eq!(commits(&Held::Yes, &Held::Yes), Ok(()));
        assert_eq!(commits(&Held::No, &Held::Yes), Ok(()));
    }

    /// **A machine with no store MixEngine can write is supported, not broken** — T49a's D7.
    ///
    /// Requiring an install there would make rotation impossible on it forever, and its browsers
    /// are reached through NSS rather than through a system store at all.
    #[test]
    fn a_machine_with_no_store_commits() {
        assert_eq!(commits(&Held::NoStore, &Held::NoStore), Ok(()));
    }

    /// This store was never trusting ours, so the rotation changes nothing about it.
    ///
    /// **Read before the removal ran.** Asked afterwards the answer is always "no", because the
    /// removal is what made it so — and a rotation that read it late would commit every time,
    /// leaving this clause in the source and out of the behaviour.
    #[test]
    fn a_store_that_never_held_ours_commits() {
        assert_eq!(commits(&Held::No, &Held::No), Ok(()));
        assert_eq!(
            commits(
                &Held::No,
                &Held::Unreadable {
                    because: "the store could not be opened".to_owned(),
                }
            ),
            Ok(()),
            "a store that never held ours is not made worse by a reading that failed"
        );
    }

    /// **The clause the whole staging design exists for.** The old authority was trusted, the new
    /// one is not, and committing would leave every site serving a certificate no browser accepts.
    #[test]
    fn a_store_that_holds_the_old_authority_and_not_the_new_one_refuses() {
        let refused = commits(&Held::Yes, &Held::No).expect_err("this must not commit");

        assert!(
            refused.contains("no browser accepts"),
            "the refusal says what committing would have cost: {refused}"
        );
    }

    /// A reading that failed has said nothing, and this is the one destructive operation in phase 5
    /// — where every other probe in this daemon treats a failure as "ask for nothing and carry on",
    /// because what those do next is harmless.
    #[test]
    fn a_reading_that_failed_refuses_in_both_directions() {
        let because = "the store could not be opened".to_owned();

        let after = commits(
            &Held::Yes,
            &Held::Unreadable {
                because: because.clone(),
            },
        )
        .expect_err("an unreadable store afterwards must not commit");
        assert!(after.contains("afterwards"), "{after}");

        let before = commits(&Held::Unreadable { because }, &Held::No)
            .expect_err("an unreadable store beforehand must not commit");
        assert!(before.contains("beforehand"), "{before}");
    }

    /// **The exception to the clause above**: a new authority demonstrably in the store is enough on
    /// its own, whatever could not be read before it. Nothing was taken away that is not now back.
    #[test]
    fn a_new_authority_that_is_demonstrably_there_commits_whatever_came_before() {
        assert_eq!(
            commits(
                &Held::Unreadable {
                    because: "the store could not be opened".to_owned(),
                },
                &Held::Yes
            ),
            Ok(())
        );
    }

    /// A store that no longer holds it, and browsers that refused nothing, is a clean removal.
    #[test]
    fn a_machine_holding_none_of_it_is_a_removal() {
        let clean = status(mixengine_proto::Trust::NotInstalled {
            because: "the store does not hold it".to_owned(),
        });

        assert_eq!(
            outcome(
                &clean,
                Ok(&()),
                &mixengine_platform::BrowserChange::default()
            ),
            UninstallOutcome::Removed {}
        );
    }

    /// **The measurement decides.** The grant failed, and the store turns out not to hold it — which
    /// is what a machine with no store, or one that never trusted ours, looks like. Reporting a
    /// failure there would be describing the attempt rather than the machine.
    #[test]
    fn a_grant_that_failed_against_a_store_that_is_already_clean_is_a_removal() {
        let clean = status(mixengine_proto::Trust::NoStore {
            because: "this machine has no system trust store MixEngine knows how to write"
                .to_owned(),
        });
        let refused = Error::new(ErrorCode::PrivilegedRequired, "nobody could be asked");

        assert_eq!(
            outcome(
                &clean,
                Err(&refused),
                &mixengine_platform::BrowserChange::default()
            ),
            UninstallOutcome::Removed {}
        );
    }

    /// A store that still holds it says so, and carries the reason the grant gave.
    #[test]
    fn a_store_that_still_holds_it_is_partly_removed_and_says_why() {
        let held = status(mixengine_proto::Trust::Installed {
            store: "this machine's Trusted Root Certification Authorities".to_owned(),
        });
        let refused = Error::new(ErrorCode::PrivilegedRequired, "nobody could be asked");

        let UninstallOutcome::PartlyRemoved { because } =
            outcome(&held, Err(&refused), &refusing("a locked profile"))
        else {
            panic!("a store that still holds it is not a clean removal");
        };

        assert!(because.contains("trust store"), "{because}");
        assert!(
            because.contains("nobody could be asked"),
            "the reason the grant gave is carried: {because}"
        );
    }

    /// **The system store is clean and a browser is not** — a state only Linux reaches, and one the
    /// trust-store reading alone cannot see, because Firefox and Chrome do not read that store.
    #[test]
    fn a_browser_that_refused_is_reported_even_when_the_trust_store_is_clean() {
        let clean = status(mixengine_proto::Trust::NotInstalled {
            because: "the store does not hold it".to_owned(),
        });

        let UninstallOutcome::PartlyRemoved { because } =
            outcome(&clean, Ok(&()), &refusing("this database is locked"))
        else {
            panic!("a refused database is not a clean removal");
        };

        assert!(because.contains("this database is locked"), "{because}");
    }
}
