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

use mixengine_proto::{
    BrowserDatabase, Browsers, CaState, CaStatus, CertIssueReport, Error, ErrorCode, IssueOutcome,
    SiteCertOutcome, Trust,
};

use crate::error::ToWire as _;

pub(crate) mod renewal;

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

    /// The rows, for the sites a certificate is issued *for* — roadmap task **T50**.
    ///
    /// `Option` for `host`'s reason: `ensure` is called at start from a place that has `Paths` and
    /// not yet a `Store`, and a home whose authority is being made does not need the site list to
    /// make it. Without one, `issue(None)` answers for no sites rather than failing.
    store: Option<mixengine_core::Store>,
}

impl Certificates {
    pub(crate) fn new(paths: &mixengine_core::Paths) -> Self {
        Self {
            certs: paths.certs().to_path_buf(),
            host: None,
            store: None,
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
            store: None,
        }
    }

    /// The one the API holds: it can read the machine's stores *and* walk this home's own sites.
    pub(crate) fn issuing(
        paths: &mixengine_core::Paths,
        host: Arc<dyn mixengine_platform::Host>,
        store: mixengine_core::Store,
    ) -> Self {
        Self {
            store: Some(store),
            ..Self::reading(paths, host)
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

    /// Give one site, or every HTTPS site, the certificate its names need — roadmap task **T50**.
    ///
    /// **Takes records and never a [`SiteRef`](mixengine_proto::SiteRef).** Resolving a reference to
    /// a row is `crate::sites::Sites::expect`, and T50 gives that struct a `Certificates` of its
    /// own so a site gains its certificate before `site.create` answers — a `Certificates` that
    /// resolved references would close that loop. The two callers that have a reference already have
    /// the row it names: the RPC dispatch resolves it, and `sites` is holding the record it just
    /// wrote.
    ///
    /// **One site's refusal never takes the others with it.** A home with no authority answers
    /// `Refused` for each site by name rather than failing the call, which is the shape T49b's
    /// `BrowserChange` already has and the only shape a report over N sites can honestly take.
    ///
    /// # Errors
    ///
    /// Only when the site rows cannot be read at all, which is the `None` form's first step. A home
    /// with no store answers for no sites — see the test.
    pub(crate) async fn issue(
        &self,
        site: Option<mixengine_core::sites::SiteRecord>,
    ) -> Result<CertIssueReport, Error> {
        let records = match site {
            Some(one) => vec![one],
            None => match self.store.as_ref() {
                Some(store) => mixengine_core::sites::records(store, None)
                    .await
                    .map_err(|error| error.to_wire())?,
                None => Vec::new(),
            },
        };

        let certs = self.certs.clone();
        let now = SystemTime::now();

        // Key generation and two file writes per site — the rule every disk-touching call in this
        // module follows.
        let sites = blocking("issuing certificates for", move || {
            records
                .into_iter()
                .map(|record| issued(&certs, &record, now))
                .collect()
        })
        .await?;

        Ok(CertIssueReport { sites })
    }

    /// What this home's authority is, and nothing about the machine holding it — task **T52**.
    ///
    /// [`Self::status`] also asks this machine's trust stores and its browser databases, and on
    /// Linux the second of those spawns `certutil` once per profile. That is a fair price on a
    /// start and an unfair one every hour, which is why the renewal loop reads this instead — and
    /// `status` is built on top of it so that the two cannot come to disagree about what reading an
    /// authority means.
    ///
    /// # Errors
    ///
    /// Only when the task reading it does not finish. A home with no authority, or one whose
    /// authority is damaged, is an answer rather than a failure — see [`CaState`].
    pub(crate) async fn authority(&self) -> Result<CaState, Error> {
        let certs = self.certs.clone();

        blocking("reading", move || {
            mixengine_core::certs::ca::read(&certs, SystemTime::now())
        })
        .await
    }

    /// What is on disk, without changing any of it.
    ///
    /// # Errors
    ///
    /// Only when the task reading it does not finish. A home with no authority, or one whose
    /// authority is damaged, is an answer rather than a failure — see
    /// [`CaState`].
    pub(crate) async fn status(&self) -> Result<CaStatus, Error> {
        let state = self.authority().await?;

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
/// One site's outcome — roadmap task **T50**.
///
/// A free function beside [`database`] and for its reason: it takes what it needs and holds nothing,
/// so the closure `issue` hands to a blocking thread can call it without carrying a `&self` across.
fn issued(
    certs: &std::path::Path,
    site: &mixengine_core::sites::SiteRecord,
    now: SystemTime,
) -> SiteCertOutcome {
    let domain = site.domains.first().cloned().unwrap_or_default();

    let refused = |because: String| SiteCertOutcome {
        domain: domain.clone(),
        outcome: IssueOutcome::Refused { because },
        state: mixengine_proto::CertState::Absent {},
    };

    if site.domains.is_empty() {
        return refused("this site has no domains".to_owned());
    }

    // **Not a refusal** — roadmap task **T52**. This site asked for no certificate, and the
    // renewal loop announces every failure it finds: under one name with `Refused` it would
    // announce one per plaintext site, once an hour, for as long as the daemon runs.
    if !site.https_enabled {
        return SiteCertOutcome {
            domain,
            outcome: IssueOutcome::NotWanted {
                because: "this site does not declare HTTPS".to_owned(),
            },
            state: mixengine_proto::CertState::Absent {},
        };
    }

    match mixengine_core::certs::leaf::ensure(certs, &site.domains, now) {
        Ok((mixengine_core::certs::leaf::Issued::Written, state)) => SiteCertOutcome {
            domain,
            outcome: IssueOutcome::Issued {},
            state,
        },
        Ok((mixengine_core::certs::leaf::Issued::Reused, state)) => SiteCertOutcome {
            domain,
            outcome: IssueOutcome::Reused {},
            state,
        },
        Err(error) => refused(mixengine_proto::flatten(&error)),
    }
}

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

    /// A site row, built here rather than through a `Store`.
    ///
    /// **`issue` takes records and never a `SiteRef`**, which is what lets this test exist without
    /// a database at all — and the reason for the signature is not the test: resolving a reference
    /// lives on `crate::sites::Sites`, and T50 gives *that* struct a `Certificates`. A
    /// `Certificates` that resolved references would close the loop.
    fn a_site(domains: &[&str]) -> mixengine_core::sites::SiteRecord {
        mixengine_core::sites::SiteRecord {
            id: 1,
            project_id: 1,
            doc_root: String::new(),
            kind: mixengine_proto::SiteKind::Static,
            https_enabled: true,
            state: mixengine_proto::SiteState::Enabled,
            domains: domains.iter().map(|one| (*one).to_owned()).collect(),
            services: Vec::new(),
        }
    }

    /// A site gets one, and the second ask writes nothing.
    #[tokio::test]
    async fn issuing_for_a_site_is_idempotent() {
        let home = tempfile::tempdir().expect("a temp home");
        let host = a_machine_with_a_browser(home.path());
        let paths = paths_under(home.path());
        std::fs::create_dir_all(paths.certs()).expect("the certs directory is made");

        let certificates = Certificates::reading(&paths, host);
        certificates.ensure().await.expect("an authority is made");

        let first = certificates
            .issue(Some(a_site(&["blog.test"])))
            .await
            .expect("it issues");

        assert_eq!(first.sites.len(), 1, "{first:?}");
        assert_eq!(first.sites[0].domain, "blog.test");
        assert!(
            matches!(first.sites[0].outcome, IssueOutcome::Issued {}),
            "{first:?}"
        );

        let second = certificates
            .issue(Some(a_site(&["blog.test"])))
            .await
            .expect("it answers");

        assert!(
            matches!(second.sites[0].outcome, IssueOutcome::Reused {}),
            "{second:?}"
        );
    }

    /// **A home with no authority refuses per site rather than failing the call**, so the answer
    /// still names the site and says what is wrong with it.
    #[tokio::test]
    async fn a_home_with_no_authority_refuses_each_site_by_name() {
        let home = tempfile::tempdir().expect("a temp home");
        let host = a_machine_with_a_browser(home.path());
        let paths = paths_under(home.path());

        let report = Certificates::reading(&paths, host)
            .issue(Some(a_site(&["blog.test"])))
            .await
            .expect("it answers");

        assert_eq!(report.sites[0].domain, "blog.test");
        let IssueOutcome::Refused { because } = &report.sites[0].outcome else {
            panic!("a home with no authority issued something: {report:?}");
        };
        assert!(!because.is_empty());
    }

    /// **A site that declares no HTTPS wanted nothing, and did not refuse** — roadmap task **T52**.
    ///
    /// The distinction is not cosmetic, which is why this test changed rather than being deleted.
    /// T52's renewal loop announces every failure it finds, and with this outcome spelled `Refused`
    /// it would announce one per plaintext site, once an hour, for as long as the daemon runs.
    #[tokio::test]
    async fn a_site_without_https_wanted_nothing_rather_than_being_refused() {
        let home = tempfile::tempdir().expect("a temp home");
        let host = a_machine_with_a_browser(home.path());
        let paths = paths_under(home.path());
        std::fs::create_dir_all(paths.certs()).expect("the certs directory is made");

        let certificates = Certificates::reading(&paths, host);
        certificates.ensure().await.expect("an authority is made");

        let plain = mixengine_core::sites::SiteRecord {
            https_enabled: false,
            ..a_site(&["blog.test"])
        };
        let report = certificates.issue(Some(plain)).await.expect("it answers");

        let IssueOutcome::NotWanted { because } = &report.sites[0].outcome else {
            panic!("a plaintext site is not a refusal: {report:?}");
        };
        assert!(because.contains("HTTPS"), "{because}");
        assert!(
            !mixengine_core::certs::leaf::certificate_path(paths.certs(), "blog.test").exists()
        );
    }

    /// **A `Certificates` with no store answers for no sites at all rather than failing.** That is
    /// the one built at start before the database is open, and a start that crashed there would be
    /// a home nobody can reach to fix.
    #[tokio::test]
    async fn a_certificates_with_no_store_issues_for_nothing() {
        let home = tempfile::tempdir().expect("a temp home");
        let paths = paths_under(home.path());

        let report = Certificates::new(&paths)
            .issue(None)
            .await
            .expect("it answers");

        assert!(report.sites.is_empty(), "{report:?}");
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
