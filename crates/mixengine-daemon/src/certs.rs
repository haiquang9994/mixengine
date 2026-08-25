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
        Self {
            certs: paths.certs().to_path_buf(),
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
