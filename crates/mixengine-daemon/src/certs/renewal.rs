//! Renewing this home's certificates on a clock — roadmap task **T52**.
//!
//! A leaf lives 90 days and is replaced once it has
//! [`RENEW_WITHIN_DAYS`](mixengine_core::certs::leaf::RENEW_WITHIN_DAYS) or fewer left. Until this
//! module every renewal came from a daemon **start**, which is enough for a machine switched off
//! more than it is on and nothing at all for one whose daemon runs for three months.
//!
//! **The question asked here is T50's and not a second copy of it.** One pass is
//! [`Certificates::issue`] with no site named — the same call the start makes — and everything in
//! this module is about what to do with the answer.

use std::collections::BTreeSet;

use crate::certs::Certificates;

/// One site whose certificate is running out and could not be replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    /// The site, by its primary domain.
    pub(crate) domain: String,

    /// Why the renewal did not happen.
    pub(crate) because: String,
}

/// What one pass over this home's certificates did.
///
/// **An enum rather than a struct with a count of zero in it.** A pass that stopped because there
/// is no authority to sign with and a pass that ran and found nothing due are two different things,
/// and under one shape the test for the first would pass whether or not it had been written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pass {
    /// Nothing was read and nothing was written, and this is why.
    Skipped {
        /// In words.
        because: String,
    },

    /// The pass ran.
    Ran {
        /// How many certificates were written. Reuse does not count: only a written certificate
        /// changes what the front end has to be told to re-read.
        renewed: usize,

        /// The sites whose certificates are running out and could not be replaced.
        failed: Vec<Failure>,
    },
}

/// Ask this home's certificates the question a start asks, and report what happened.
///
/// **Reads the authority first and stops there if it cannot sign.** That is `mix doctor`'s own
/// behaviour — its site-certificate check is `Skipped` on the same reasoning rather than reporting
/// every site as broken — and the reason is that one damaged authority is one problem. Announcing
/// it once per site would bury the single line that says what to fix.
pub(crate) async fn once(certificates: &Certificates) -> Pass {
    let skipped = |because: String| Pass::Skipped { because };

    match certificates.authority().await {
        Ok(mixengine_proto::CaState::Present { .. }) => {}
        Ok(mixengine_proto::CaState::Absent {}) => {
            return skipped("this home has no certificate authority to sign with".to_owned());
        }
        Ok(mixengine_proto::CaState::Unusable { because }) => {
            return skipped(format!(
                "this home's certificate authority cannot sign: {because:?}"
            ));
        }
        Err(error) => {
            return skipped(format!(
                "this home's certificate authority could not be read: {error}"
            ));
        }
    }

    match certificates.issue(None).await {
        Ok(report) => pass(&report),
        Err(error) => skipped(format!("this home's sites could not be read: {error}")),
    }
}

/// What a report means for the front end and for the event stream.
///
/// A free function so that the mapping can be tested against a report built by hand, with no
/// database, no authority and no disk.
fn pass(report: &mixengine_proto::CertIssueReport) -> Pass {
    let mut renewed = 0;
    let mut failed = Vec::new();

    for site in &report.sites {
        match &site.outcome {
            mixengine_proto::IssueOutcome::Issued {} => renewed += 1,

            // **`NotWanted` is not a failure**, which is the whole reason T52 added it. A site with
            // HTTPS off asked for no certificate, and an event per plaintext site every hour is
            // what this arm exists to prevent.
            mixengine_proto::IssueOutcome::Reused {}
            | mixengine_proto::IssueOutcome::NotWanted { .. } => {}

            mixengine_proto::IssueOutcome::Refused { because } => failed.push(Failure {
                domain: site.domain.clone(),
                because: because.clone(),
            }),
        }
    }

    Pass::Ran { renewed, failed }
}

/// The failures worth announcing, and `announced` updated to match.
///
/// **A producer reports a change and not a heartbeat** — the rule
/// [`mixengine_proto::DaemonEvent`] states about the 1024-message stream every client shares. A
/// renewal that fails will keep failing every pass, because a disk that is full at nine is full at
/// ten. So a domain is announced when it enters this set and is silent afterwards, and it leaves
/// the set when it recovers, so that a later outage is announced rather than swallowed.
fn newly(announced: &mut BTreeSet<String>, failed: &[Failure]) -> Vec<Failure> {
    let failing: BTreeSet<String> = failed.iter().map(|one| one.domain.clone()).collect();
    announced.retain(|domain| failing.contains(domain));

    failed
        .iter()
        .filter(|one| announced.insert(one.domain.clone()))
        .cloned()
        .collect()
}

/// Renew this home's certificates every `every`, until `shutdown`.
///
/// **The first tick is thrown away.** [`tokio::time::interval`] completes its first immediately,
/// and the daemon's start issues for every site a few lines above the call to this — so keeping it
/// would make every start do the same work twice.
///
/// **Nothing here catches up, and nothing needs to.** A machine suspended over a weekend counts
/// none of it on Linux or macOS, so a tick can arrive days late; that is why the period is not the
/// schedule this task promises. A certificate is replaced with thirty days left, which is the
/// tolerance that makes an imprecise clock a non-problem — and it is also why a late tick has
/// nothing to make up: a pass that finds nothing due does nothing.
pub(crate) fn start(
    certificates: Certificates,
    services: std::sync::Arc<crate::services::Registry>,
    events: crate::api::Events,
    every: std::time::Duration,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        let mut announced = BTreeSet::new();
        let mut ticker = tokio::time::interval(every);
        ticker.tick().await;

        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = ticker.tick() => {}
            }

            match once(&certificates).await {
                // Debug and not warn: this arrives every period, and a home with no usable
                // authority already has its own line at start, its own `mix doctor` check and its
                // own `cert.ca_status`. A warning every hour about something already reported
                // three ways is how a log stops being read.
                Pass::Skipped { because } => {
                    tracing::debug!(%because, "no certificate was renewed");
                }

                Pass::Ran { renewed, failed } => {
                    if renewed > 0 {
                        // **This is the whole reload.** Rendering a site file stamps the
                        // certificate's fingerprint into its header (T51), so a renewed
                        // certificate makes the file differ, `document::install` finds a change,
                        // and the front end is reloaded. A failure here installed nothing —
                        // `install` stages first — so the front end goes on serving what worked,
                        // which is the state `mix doctor`'s `GeneratedConfigStale` reports.
                        //
                        // **One line for both halves rather than one before and one after.** A
                        // certificate written and never handed on is the failure this task exists
                        // to prevent, and a log that announced the renewal before finding out
                        // would say the same thing either way.
                        match services.reconfigure().await {
                            Ok(()) => tracing::info!(
                                certificates = renewed,
                                "renewed certificates and told the front end"
                            ),
                            // `?` and not `%`, on `crate::extensions`' reasoning: `Undeclarable` is
                            // matched by its callers rather than printed, and this is a line in
                            // `daemon.log` rather than a sentence for a person.
                            Err(error) => tracing::warn!(
                                ?error,
                                certificates = renewed,
                                "renewed certificates and the front end has not been told"
                            ),
                        }
                    }

                    for failure in newly(&mut announced, &failed) {
                        tracing::warn!(
                            domain = %failure.domain,
                            because = %failure.because,
                            "a certificate is running out and could not be replaced"
                        );

                        events.publish(mixengine_proto::DaemonEvent::CertExpiring {
                            domain: failure.domain,
                            because: failure.because,
                        });
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_report(sites: Vec<mixengine_proto::SiteCertOutcome>) -> mixengine_proto::CertIssueReport {
        mixengine_proto::CertIssueReport { sites }
    }

    fn an_outcome(
        domain: &str,
        outcome: mixengine_proto::IssueOutcome,
    ) -> mixengine_proto::SiteCertOutcome {
        mixengine_proto::SiteCertOutcome {
            domain: domain.to_owned(),
            outcome,
            state: mixengine_proto::CertState::Absent {},
        }
    }

    fn a_failure(domain: &str) -> Failure {
        Failure {
            domain: domain.to_owned(),
            because: "the disk is full".to_owned(),
        }
    }

    /// **A home with no authority is `Skipped` and not an empty `Ran`**, which is the whole reason
    /// [`Pass`] is an enum: the two would otherwise be one value, and this test would pass whether
    /// or not the gate had been written.
    #[tokio::test]
    async fn a_home_with_no_authority_does_nothing_at_all() {
        let home = tempfile::tempdir().expect("a temp home");
        let paths = mixengine_core::Paths::new(
            home.path().to_path_buf(),
            &mixengine_core::config::PathOverrides::default(),
        );

        let pass = once(&Certificates::new(&paths)).await;

        let Pass::Skipped { because } = &pass else {
            panic!("a home with no authority renewed something: {pass:?}");
        };
        assert!(because.contains("authority"), "{because}");
    }

    /// Only a certificate that was **written** counts, because only that changes what the front end
    /// has to re-read. A pass that counted reuse would reload the front end every hour on a machine
    /// where nothing had happened.
    #[test]
    fn only_a_written_certificate_counts_as_renewed() {
        let report = a_report(vec![
            an_outcome("blog.test", mixengine_proto::IssueOutcome::Issued {}),
            an_outcome("shop.test", mixengine_proto::IssueOutcome::Reused {}),
        ]);

        assert_eq!(
            pass(&report),
            Pass::Ran {
                renewed: 1,
                failed: Vec::new()
            }
        );
    }

    /// **The assertion that keeps a plaintext site off the event stream.** T52's fourth outcome is
    /// only worth having if this holds.
    #[test]
    fn a_site_that_wanted_nothing_is_not_a_failure() {
        let report = a_report(vec![an_outcome(
            "plain.test",
            mixengine_proto::IssueOutcome::NotWanted {
                because: "this site does not declare HTTPS".to_owned(),
            },
        )]);

        assert_eq!(
            pass(&report),
            Pass::Ran {
                renewed: 0,
                failed: Vec::new()
            }
        );
    }

    #[test]
    fn a_refusal_is_a_failure_carrying_its_domain_and_its_reason() {
        let report = a_report(vec![an_outcome(
            "blog.test",
            mixengine_proto::IssueOutcome::Refused {
                because: "the disk is full".to_owned(),
            },
        )]);

        assert_eq!(
            pass(&report),
            Pass::Ran {
                renewed: 0,
                failed: vec![a_failure("blog.test")],
            }
        );
    }

    /// **A producer reports a change and not a heartbeat.** A disk that is full at nine is full at
    /// ten, and an event per pass would spend a client's whole allowance restating one fact.
    #[test]
    fn a_failure_is_announced_once_and_then_stays_quiet() {
        let failed = vec![a_failure("blog.test")];
        let mut announced = BTreeSet::new();

        assert_eq!(newly(&mut announced, &failed), failed);
        assert!(newly(&mut announced, &failed).is_empty());
    }

    /// And a domain that recovers re-arms, or the second outage is the one nobody hears about.
    #[test]
    fn a_domain_that_recovers_and_fails_again_is_announced_again() {
        let failed = vec![a_failure("blog.test")];
        let mut announced = BTreeSet::new();

        newly(&mut announced, &failed);
        newly(&mut announced, &[]);

        assert_eq!(newly(&mut announced, &failed), failed);
    }
}
