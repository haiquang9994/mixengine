//! This home's certificate authority: made once, and reported on.
//!
//! **Made at start rather than when the first HTTPS site is created.**
//! `.claude/architecture/security-model.md` promises one elevation prompt at first run, covering the
//! CA, the resolver wiring and the port grant together. An authority that first appeared with the
//! first site would put its trust-store install (roadmap task T49) in a second batch and therefore
//! behind a second prompt — which is the finding T45 already made about the resolver and wrote down
//! in `main.rs` beside the block this one sits under. Generating here costs one ECDSA key on disk in
//! a home that never serves HTTPS; the alternative costs a second prompt to everybody who does.
//!
//! Everything about what an authority *is* lives in `mixengine_core::certs::ca`. This module is the
//! two things that are the daemon's: when it happens, and which thread it happens on.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use mixengine_proto::{BrowserDatabase, Browsers, CaState, CaStatus, Error, ErrorCode, Trust};

use crate::error::ToWire as _;

/// Everything this needs, which is one directory.
#[derive(Debug)]
pub(crate) struct Certificates {
    certs: PathBuf,

    /// This machine, for the trust-store half of the answer — roadmap task **T49a**.
    ///
    /// `Option` because `ensure` is called at start from a place that has `Paths` and not yet an
    /// `Api`, and a home whose authority is being made does not need the store read to make it.
    /// `status` always has one.
    host: Option<Arc<dyn mixengine_platform::Host>>,
}

impl Certificates {
    pub(crate) fn new(paths: &mixengine_core::Paths) -> Self {
        Self {
            certs: paths.certs().to_path_buf(),
            host: None,
        }
    }

    /// The one the API holds, which can also say whether this machine trusts what it finds.
    pub(crate) fn reading(
        paths: &mixengine_core::Paths,
        host: Arc<dyn mixengine_platform::Host>,
    ) -> Self {
        Self::in_directory(paths.certs(), host)
    }

    /// The same, for a caller that kept the directory rather than the whole `Paths`.
    ///
    /// `crate::repair` is the one: it stores `certs` alone, and rebuilding a `Paths` from a root to
    /// get back to it would be a second way of deciding where certificates live.
    pub(crate) fn in_directory(
        certs: &std::path::Path,
        host: Arc<dyn mixengine_platform::Host>,
    ) -> Self {
        Self {
            certs: certs.to_path_buf(),
            host: Some(host),
        }
    }

    /// Make the authority if this home has none, and answer with what is there either way.
    ///
    /// Idempotent, and never destructive: an authority that is present and broken is left alone and
    /// reported, because replacing it would invalidate every leaf and every trust store that holds
    /// it. See `mixengine_core::certs::ca`.
    ///
    /// # Errors
    ///
    /// Whatever `certs/ca/` could not be made or written, and the case where this machine will not
    /// produce a key pair at all. Callers at start log it and carry on.
    pub(crate) async fn ensure(&self) -> Result<CaStatus, Error> {
        let certs = self.certs.clone();

        // Key generation and two file writes. `.claude/standards/rust.md`'s rule for anything that
        // touches a disk from a runtime worker — and on Windows the private key's ACL is written by
        // running `icacls`, which is a process rather than a syscall.
        //
        // **The conversion to a wire error happens inside the closure**, not after the `await`.
        // `mixengine_core::Error` is over 128 bytes, so carrying it out through the task's own
        // `Result` puts a large error in two frames and `clippy::result_large_err` says so — the
        // same boundary every other module in this crate converts at.
        let state = blocking("making", move || {
            mixengine_core::certs::ca::ensure(&certs, SystemTime::now())
                .map_err(|error| error.to_wire())
        })
        .await??;

        Ok(CaStatus {
            trust: self.trust(&state),
            browsers: self.browsers(&state),
            state,
        })
    }

    /// What is on disk, without changing any of it.
    ///
    /// # Errors
    ///
    /// Only when the task reading it does not finish. A home with no authority, or one whose
    /// authority is damaged, is an answer rather than a failure — see
    /// [`CaState`].
    pub(crate) async fn status(&self) -> Result<CaStatus, Error> {
        let certs = self.certs.clone();

        let state = blocking("reading", move || {
            mixengine_core::certs::ca::read(&certs, SystemTime::now())
        })
        .await?;

        Ok(CaStatus {
            trust: self.trust(&state),
            browsers: self.browsers(&state),
            state,
        })
    }

    /// Whether this machine holds the authority that was just read.
    ///
    /// **Every branch that cannot ask says why rather than guessing.** A client renders this
    /// sentence, and "not installed" printed because nothing was asked would be the daemon inventing
    /// an answer — which is the rule `.claude/CLAUDE.md` states as a client rendering only what the
    /// daemon returns, one layer up.
    fn trust(&self, state: &CaState) -> Trust {
        let CaState::Present { ca } = state else {
            return Trust::Unknown {
                because: "this home has no usable certificate authority, so nothing was asked about                           this machine's trust store"
                    .to_owned(),
            };
        };

        let (Some(host), Some(der)) = (
            self.host.as_ref(),
            mixengine_core::certs::ca::der(&ca.certificate_pem),
        ) else {
            return Trust::Unknown {
                because: "this machine's trust store was not read".to_owned(),
            };
        };

        match host.trust_store().probe(&der) {
            Ok(state) if state.installed => Trust::Installed {
                store: store(state.method),
            },
            Ok(state) if state.method == mixengine_platform::TrustStoreMethod::None => {
                Trust::NoStore {
                    because: state.missing.unwrap_or_else(|| {
                        "this machine has no system trust store MixEngine knows how to write"
                            .to_owned()
                    }),
                }
            }
            Ok(state) => Trust::NotInstalled {
                because: state
                    .missing
                    .unwrap_or_else(|| format!("{} does not hold it", store(state.method))),
            },
            Err(error) => Trust::Unknown {
                because: format!("this machine's trust store could not be read: {error}"),
            },
        }
    }

    /// Ask this machine's browsers to hold the authority this home has, if it has a usable one.
    ///
    /// **No prompt, and no elevation queue.** These databases belong to the user, which is the line
    /// T49 was split on: the system stores need root and ride in the first-run batch, and this is a
    /// subprocess in the user's own home.
    ///
    /// **Never fails a start, and never fails a repair.** A machine with no `certutil`, no browser
    /// profile, or a locked one is a machine that keeps working; what happened comes back in the
    /// return value, which is what the caller logs and what `mix doctor --repair` reports.
    ///
    /// **Takes the state rather than reading it.** Both callers already have one — the start made
    /// it a few lines above and the repair read it to decide there was anything to repair — and
    /// asking for it again here would run the *system* trust-store probe a second time on every
    /// daemon start, which on Linux means parsing the whole `ca-certificates.crt` bundle twice to
    /// answer a question about browsers.
    ///
    /// The state and not the DER, so that the one decision worth making — a damaged authority is
    /// not written anywhere — lives here and is tested, rather than being repeated at each call.
    pub(crate) async fn install_in_browsers(
        &self,
        state: &CaState,
    ) -> mixengine_platform::BrowserChange {
        let Some(host) = self.host.clone() else {
            return mixengine_platform::BrowserChange::default();
        };

        // Only a present authority has bytes to install, exactly as the trust-store producer
        // decides: asking a browser to hold a damaged certificate would be writing something T54
        // has to replace anyway.
        let CaState::Present { ca } = state else {
            return mixengine_platform::BrowserChange::default();
        };

        let Some(der) = mixengine_core::certs::ca::der(&ca.certificate_pem) else {
            return mixengine_platform::BrowserChange::default();
        };

        // Process spawns and file writes, so off the runtime — `.claude/standards/rust.md`'s rule
        // for anything that touches a disk from a worker.
        tokio::task::spawn_blocking(move || host.browsers().install(&der))
            .await
            .unwrap_or_else(|_| {
                Ok(mixengine_platform::BrowserChange {
                    written: Vec::new(),
                    refused: vec![
                        "the task asking this machine's browsers did not finish".to_owned(),
                    ],
                })
            })
            .unwrap_or_else(|error| mixengine_platform::BrowserChange {
                written: Vec::new(),
                refused: vec![mixengine_proto::flatten(&error)],
            })
    }

    /// What this machine's browsers hold — roadmap task **T49b**.
    ///
    /// **Every branch that cannot ask says why**, exactly as [`Self::trust`] does: a client renders
    /// this sentence, and "not installed" printed because nothing was asked would be the daemon
    /// inventing an answer.
    ///
    /// A different question from [`Self::trust`] and not a refinement of it — Firefox and Chrome on
    /// Linux read these databases and not the system store at all.
    fn browsers(&self, state: &CaState) -> Browsers {
        let CaState::Present { ca } = state else {
            return Browsers::Unknown {
                because: "this home has no usable certificate authority, so nothing was asked                           about this machine's browsers"
                    .to_owned(),
            };
        };

        let (Some(host), Some(der)) = (
            self.host.as_ref(),
            mixengine_core::certs::ca::der(&ca.certificate_pem),
        ) else {
            return Browsers::Unknown {
                because: "this machine's browsers were not asked".to_owned(),
            };
        };

        match host.browsers().survey(&der) {
            Ok(mixengine_platform::BrowserSurvey::Reached { databases }) => Browsers::Reached {
                databases: databases.into_iter().map(database).collect(),
            },
            Ok(mixengine_platform::BrowserSurvey::NoTool { because }) => {
                Browsers::NoTool { because }
            }
            Ok(mixengine_platform::BrowserSurvey::NotSearched { because }) => {
                Browsers::NotSearched { because }
            }
            Err(error) => Browsers::Unknown {
                because: format!("this machine's browsers could not be asked: {error}"),
            },
        }
    }
}

/// One database, as a client sees it.
///
/// The platform's own type and the wire's are deliberately separate — the split `TrustState` and
/// `Trust` already make, one capability along — so this is where they meet.
fn database(state: mixengine_platform::DatabaseState) -> BrowserDatabase {
    BrowserDatabase {
        path: state.path,
        owner: state.owner,
        installed: state.installed,
        because: state.because,
    }
}

/// A store, in words a person can go and look in.
fn store(method: mixengine_platform::TrustStoreMethod) -> String {
    use mixengine_platform::TrustStoreMethod as Method;

    match method {
        Method::SystemRoot => "this machine's Trusted Root Certification Authorities",
        Method::SystemKeychain => "/Library/Keychains/System.keychain",
        Method::CaCertificates => "/usr/local/share/ca-certificates",
        Method::CaTrustAnchors => "/etc/pki/ca-trust/source/anchors",
        Method::None => "this machine has no system trust store MixEngine knows how to write",
    }
    .to_owned()
}

/// Run `work` off the runtime, and turn a task that did not finish into a sentence.
async fn blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(work).await.map_err(|_| {
        Error::new(
            ErrorCode::Internal,
            format!("the task {what} this home's certificate authority did not finish"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine with browser databases, none of which hold the authority yet.
    fn a_machine_with_a_browser(home: &std::path::Path) -> Arc<mixengine_platform::mock::Host> {
        Arc::new(mixengine_platform::mock::Host::with_browsers(
            home,
            mixengine_platform::BrowserSurvey::Reached {
                databases: vec![mixengine_platform::DatabaseState {
                    path: "/home/someone/.pki/nssdb".to_owned(),
                    owner: "Chrome and Chromium".to_owned(),
                    installed: false,
                    because: Some("it does not hold this authority".to_owned()),
                }],
            },
        ))
    }

    fn paths_under(home: &std::path::Path) -> mixengine_core::Paths {
        mixengine_core::Paths::new(
            home.to_path_buf(),
            &mixengine_core::config::PathOverrides::default(),
        )
    }

    /// **Every start asks the browsers to hold it, and asking costs no prompt.** The system store
    /// needs an elevation batch; these databases are the user's own, which is the line T49 was
    /// split on.
    #[tokio::test]
    async fn a_start_asks_this_machines_browsers_to_hold_the_authority() {
        let home = tempfile::tempdir().expect("a temp home");
        let host = a_machine_with_a_browser(home.path());
        let paths = paths_under(home.path());
        std::fs::create_dir_all(paths.certs()).expect("the certs directory is made");

        let certificates = Certificates::reading(&paths, host.clone());
        let made = certificates.ensure().await.expect("an authority is made");

        let change = certificates.install_in_browsers(&made.state).await;

        assert_eq!(change.written.len(), 1, "refused: {:?}", change.refused);
        assert_eq!(
            host.browsers_installed().len(),
            1,
            "the browsers were not asked"
        );
    }

    /// **A home with no authority asks for nothing**, which is what a start whose generation failed
    /// gets. Writing something a browser would have to be told to forget is worse than writing
    /// nothing.
    #[tokio::test]
    async fn a_home_with_no_authority_asks_the_browsers_for_nothing() {
        let home = tempfile::tempdir().expect("a temp home");
        let host = a_machine_with_a_browser(home.path());
        let paths = paths_under(home.path());

        let change = Certificates::reading(&paths, host.clone())
            .install_in_browsers(&CaState::Absent {})
            .await;

        // The mock records everything it is asked, so an empty log is the assertion: a home with no
        // authority never reaches the browsers at all.
        assert!(change.written.is_empty(), "wrote: {change:?}");
        assert!(host.browsers_installed().is_empty());
    }

    /// **And a damaged one is not written either**, which is the half an `Absent` home does not
    /// cover: `Unusable` is a certificate that exists, and installing one into somebody's browser
    /// would be spending their trust on something T54 has to replace anyway.
    #[tokio::test]
    async fn a_damaged_authority_is_not_written_into_a_browser() {
        let home = tempfile::tempdir().expect("a temp home");
        let host = a_machine_with_a_browser(home.path());
        let paths = paths_under(home.path());

        let change = Certificates::reading(&paths, host.clone())
            .install_in_browsers(&CaState::Unusable {
                because: mixengine_proto::Unusable::KeyMissing,
            })
            .await;

        assert!(change.written.is_empty(), "wrote: {change:?}");
        assert!(host.browsers_installed().is_empty());
    }

    /// And the status carries what the browsers said, beside what the machine said.
    #[tokio::test]
    async fn a_status_reports_the_browsers_beside_the_trust_store() {
        let home = tempfile::tempdir().expect("a temp home");
        let host = a_machine_with_a_browser(home.path());
        let paths = paths_under(home.path());
        std::fs::create_dir_all(paths.certs()).expect("the certs directory is made");

        let certificates = Certificates::reading(&paths, host);
        certificates.ensure().await.expect("an authority is made");

        let status = certificates.status().await.expect("a status");

        let Browsers::Reached { databases } = status.browsers else {
            panic!(
                "the fixture's databases were not reported: {:?}",
                status.browsers
            );
        };

        assert_eq!(databases.len(), 1);
        assert!(!databases[0].installed);
        assert_eq!(databases[0].owner, "Chrome and Chromium");
    }
}
