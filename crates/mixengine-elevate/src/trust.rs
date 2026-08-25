//! Deciding what may enter this machine's trust store, and what may leave it — roadmap task
//! **T49a**.
//!
//! **The two directions are not symmetrical, and this file is arranged around that.**
//!
//! An install is close to harmless. A daemon compromised badly enough to forge one already holds the
//! private key of the authority this machine trusts, and can sign any certificate for any name
//! without installing a second root — so the shape check below is not a boundary against an
//! attacker. It is there so that `mix cert ca-uninstall` (T54) and uninstall (T87) can **enumerate**
//! everything an install could ever have created, which an unconstrained one would make impossible.
//!
//! A removal is not harmless. One that could name an arbitrary certificate could take the root that
//! validates Windows Update out of this machine, through this audited binary and under the user's
//! own Allow click. The wire type therefore carries **no fingerprint at all** — see
//! `mixengine_proto::privileged::TrustTarget` — and what arrives is eight hex characters, checked
//! here before a store is opened and checked again against every certificate the store hands back.

use mixengine_platform::trust::{self, Change};
use mixengine_proto::privileged::{OpOutcome, TrustPlan, TrustTarget};

/// Make this machine trust the authority `plan` carries.
pub(crate) fn install(plan: &TrustPlan) -> OpOutcome {
    let der = match plan {
        TrustPlan::SystemRoot { der }
        | TrustPlan::SystemKeychain { der }
        | TrustPlan::CaCertificates { der }
        | TrustPlan::CaTrustAnchors { der } => der,
    };

    // **Before a store is opened, and before the machine-wide lock is taken.** A refusal costs no
    // privilege and no lock, and the reason it is a refusal rather than a failure is that the same
    // request will be refused again — nothing about the machine will change that.
    let authority = match trust::ours(der) {
        Ok(authority) => authority,
        Err(refused) => {
            return OpOutcome::Refused {
                reason: format!("this is not an authority MixEngine generated: {refused}"),
            };
        }
    };

    named(outcome(trust::apply(plan)), &authority.subject)
}

/// Take an authority back out of this machine's trust store.
pub(crate) fn remove(target: &TrustTarget) -> OpOutcome {
    let key_id = match target {
        TrustTarget::SystemRoot { key_id }
        | TrustTarget::SystemKeychain { key_id }
        | TrustTarget::CaCertificates { key_id }
        | TrustTarget::CaTrustAnchors { key_id } => key_id,
    };

    // **The first of two checks, and the cheap one.** Eight lowercase hex characters cannot describe
    // a corporate root or the one that validates this machine's own updates, so a request that has
    // got this far can only be aimed at something MixEngine named. The second check is in the
    // platform layer, against what the store actually hands back.
    if !trust::is_key_id(key_id) {
        return OpOutcome::Refused {
            reason: format!(
                "{key_id:?} is not a MixEngine authority's identifier, which is eight lowercase \
                 hexadecimal characters"
            ),
        };
    }

    outcome(trust::revoke(target))
}

/// Put the authority's name into an `Applied` detail, and leave every other outcome alone.
fn named(outcome: OpOutcome, subject: &str) -> OpOutcome {
    match outcome {
        OpOutcome::Applied { detail } => OpOutcome::Applied {
            detail: format!("{detail} ({subject})"),
        },
        other => other,
    }
}

/// One platform answer as the outcome the daemon reads.
///
/// The same mapping `crate::resolver` makes, and it is duplicated rather than shared for the reason
/// the two modules exist separately at all: what counts as `Refused` here is a plan for another
/// system's store, and there it is a plan for another system's resolver. Sharing the function would
/// mean sharing a judgement that happens to agree today.
fn outcome(change: mixengine_platform::Result<Change>) -> OpOutcome {
    match change {
        Ok(Change::Unchanged) => OpOutcome::AlreadyDone,
        Ok(Change::Written { detail }) => OpOutcome::Applied { detail },

        // What is wrong is the request, and the same one will be wrong again: a plan meant for
        // another operating system's store is not something retrying will fix.
        Err(error @ mixengine_platform::Error::UnsupportedPlatform { .. }) => OpOutcome::Refused {
            reason: error.to_string(),
        },

        // The operating system said no. Nothing about the request is wrong, and trying again may
        // work — a lock held by another helper is the case this exists for.
        // **`flatten` and not `to_string`.** `Error::Os` renders as "cannot <action>" and keeps the
        // operating system's own words as its `#[source]`, so `to_string` alone hands back a
        // sentence with the cause cut off — which is the half a person needs. `mix` already
        // flattens the same errors at its own boundary; this is that boundary for the helper.
        Err(error) => OpOutcome::Failed {
            message: mixengine_proto::flatten(&error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The most important test in this task.** The operation cannot be aimed at the certificate
    /// that validates this machine's own updates, or at a corporate root, because nothing that can
    /// describe one gets past the first check — and that check runs before a store is opened, so
    /// none of these ever touches the machine this test is running on.
    #[test]
    fn a_removal_that_names_something_other_than_an_authority_is_refused() {
        for named in [
            "../../../etc/ssl",
            "DigiCert Global Root CA",
            "0123abcdef",
            "0123ABCD",
            "0123abc",
            "",
            "d3adb33f!",
            "0123 bcd",
            "*",
        ] {
            let outcome = remove(&TrustTarget::SystemRoot {
                key_id: named.to_owned(),
            });

            assert!(
                matches!(outcome, OpOutcome::Refused { .. }),
                "accepted a removal naming {named:?}"
            );
        }
    }

    /// And every mechanism refuses the same way, so no system is the lenient one.
    #[test]
    fn every_store_refuses_a_name_that_is_not_an_authority() {
        let bad = "not-a-key-id".to_owned();

        for target in [
            TrustTarget::SystemRoot {
                key_id: bad.clone(),
            },
            TrustTarget::SystemKeychain {
                key_id: bad.clone(),
            },
            TrustTarget::CaCertificates {
                key_id: bad.clone(),
            },
            TrustTarget::CaTrustAnchors { key_id: bad },
        ] {
            assert!(matches!(remove(&target), OpOutcome::Refused { .. }));
        }
    }

    /// A certificate that is not one T48 makes never reaches a store — and, this refusal happening
    /// before anything is opened, never reaches an elevated call either.
    #[test]
    fn an_install_of_something_that_is_not_our_authority_is_refused() {
        for der in [
            b"not a certificate".to_vec(),
            Vec::new(),
            vec![0x30, 0x82, 0xff, 0xff],
            vec![0x30; mixengine_platform::trust::MAX_DER + 1],
        ] {
            let outcome = install(&TrustPlan::SystemRoot { der });

            assert!(matches!(outcome, OpOutcome::Refused { .. }));
        }
    }

    /// Every store refuses one too, for the reason above.
    #[test]
    fn every_store_refuses_something_that_is_not_a_certificate() {
        let rubbish = b"not a certificate".to_vec();

        for plan in [
            TrustPlan::SystemRoot {
                der: rubbish.clone(),
            },
            TrustPlan::SystemKeychain {
                der: rubbish.clone(),
            },
            TrustPlan::CaCertificates {
                der: rubbish.clone(),
            },
            TrustPlan::CaTrustAnchors { der: rubbish },
        ] {
            assert!(matches!(install(&plan), OpOutcome::Refused { .. }));
        }
    }
}
