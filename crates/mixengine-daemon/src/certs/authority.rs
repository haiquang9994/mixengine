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
    CaState, CaStatus, CaUninstallReport, Error, ErrorCode, JobSummary, UninstallOutcome,
};

use super::Certificates;
use crate::elevation::Elevation;
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
