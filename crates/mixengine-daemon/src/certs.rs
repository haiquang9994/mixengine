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

pub(crate) mod authority;
pub(crate) mod handshake;
pub(crate) mod renewal;

/// Everything this needs, which is one directory.
///
/// **`Clone` because a job takes an owned one** — roadmap task **T54**. Every field clones cheaply:
/// a path, an `Arc`, and a `Store` whose own documentation says one pool sits behind it.
#[derive(Debug, Clone)]
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

    /// Whether this site is actually served over TLS right now — roadmap task **T74**.
    ///
    /// **The same two questions `generate::served` asks**, and deliberately the same two: a site
    /// declaring HTTPS with no usable pair on disk renders no TLS listener at all, so a firewall
    /// plan that opened the TLS port for it would be opening a port nothing answers on. Asking the
    /// row alone would do exactly that.
    pub(crate) fn serves_tls(&self, site: &mixengine_core::sites::SiteRecord) -> bool {
        if !site.https_enabled {
            return false;
        }

        let Some(primary) = site.domains.first() else {
            return false;
        };

        matches!(
            mixengine_core::certs::leaf::read(&self.certs, primary, std::time::SystemTime::now()),
            mixengine_proto::CertState::Present { .. }
        )
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

    /// What every site's certificate is doing, on disk and on the wire — roadmap task **T53**.
    ///
    /// **The one answer in this module that comes from a socket.** Everything else here reads a
    /// file and believes it; this opens a connection to the front end running on this machine and
    /// reports the certificate that server actually presents, which is the only thing a browser
    /// ever sees.
    ///
    /// `tls_port` is [`None`] for a home whose front end is not installed or cannot be read. Every
    /// site then answers `NotServed`, which is what is true.
    ///
    /// **Reads only.** A site with a problem is reported and never repaired: a diagnostic that
    /// fixed what it found would be unable to report the state it had just fixed, and
    /// `ServedCertificateDiffers` in particular would disappear the moment it was looked at.
    ///
    /// # Errors
    ///
    /// Whatever reading this home's rows or its authority failed with. A site nothing answers for
    /// is an answer rather than a failure — see [`mixengine_proto::Handshake`].
    pub(crate) async fn site_status(
        &self,
        site: Option<mixengine_core::sites::SiteRecord>,
        tls_port: Option<u16>,
    ) -> Result<mixengine_proto::CertStatusReport, Error> {
        let records = match site {
            Some(one) => vec![one],
            None => match self.store.as_ref() {
                Some(store) => mixengine_core::sites::records(store, None)
                    .await
                    .map_err(|error| error.to_wire())?,
                None => Vec::new(),
            },
        };

        // Read once for the whole walk rather than per site: it is the same authority for all of
        // them, and it is the trust root every handshake below is judged against.
        let authority = match self.authority().await? {
            CaState::Present { ca } => mixengine_core::certs::ca::der(&ca.certificate_pem),
            CaState::Absent {} | CaState::Unusable { .. } => None,
        };

        let now = SystemTime::now();
        let mut sites = Vec::with_capacity(records.len());

        for record in records {
            let Some(domain) = record.domains.first().cloned() else {
                continue;
            };

            let certs = self.certs.clone();
            let primary = domain.clone();
            let disk = blocking("reading a certificate for", move || {
                mixengine_core::certs::leaf::read(&certs, &primary, now)
            })
            .await?;

            let handshake = match (record.https_enabled, tls_port, authority.as_deref()) {
                (false, _, _) => mixengine_proto::Handshake::NotAsked {},

                (true, Some(port), Some(authority)) => {
                    crate::certs::handshake::against(&domain, port, authority, now).await
                }

                (true, None, _) => mixengine_proto::Handshake::NotServed {
                    because: "this home has no front end that could serve it".to_owned(),
                },

                (true, Some(_), None) => mixengine_proto::Handshake::NotServed {
                    because: "this home has no usable authority to judge a certificate against"
                        .to_owned(),
                },
            };

            // **What the issuer covered, not what the row lists** — roadmap task **T75**, fixing a
            // defect shipped with T74: that task put the LAN address into the certificate and left
            // this comparison reading the bare domain list, so every shared site answered
            // `NamesDiffer` for as long as it was shared.
            let covered =
                mixengine_core::certs::leaf::covered(&record.domains, record.sharing.as_ref());

            sites.push(mixengine_proto::SiteCertStatus {
                problem: problem(record.https_enabled, &covered, &disk, &handshake),
                domain,
                domains: record.domains,
                disk,
                handshake,
            });
        }

        Ok(mixengine_proto::CertStatusReport { sites })
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
                because: "this home has no usable certificate authority, so its trust store was not asked"
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

    /// Ask this machine's browsers to let the authority `key_id` names go — task **T54**.
    ///
    /// The mirror of [`install_in_browsers`](Self::install_in_browsers), and unprivileged for its
    /// reason: these databases belong to the user, which is the line T49 was split on.
    ///
    /// **Names an authority and never a certificate** — T49a's D5. What sits under the nickname is
    /// read back and checked before anything is deleted, which is
    /// [`BrowserTrust::remove`](mixengine_platform::BrowserTrust::remove)'s own guarantee and not
    /// this method's to repeat.
    pub(crate) async fn remove_from_browsers(
        &self,
        key_id: &str,
    ) -> mixengine_platform::BrowserChange {
        let Some(host) = self.host.clone() else {
            return mixengine_platform::BrowserChange::default();
        };

        let key_id = key_id.to_owned();

        // A process spawn per profile, so off the runtime — `.claude/standards/rust.md`.
        tokio::task::spawn_blocking(move || host.browsers().remove(&key_id))
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

    /// Enqueue taking `ca` out of this machine's trust store, and say whether anything was.
    ///
    /// `Ok(false)` is a machine with no store MixEngine can write, one that is not holding this
    /// authority, or one whose store could not be read — **T41's D11 one capability along**: a
    /// prompt spent on a row whose only possible outcome is `AlreadyDone` is a prompt spent for
    /// nothing, and a probe that failed has said nothing about what to ask for.
    ///
    /// # Errors
    ///
    /// The wire error of a row that could not be written.
    pub(crate) async fn require_untrust(
        &self,
        elevation: &crate::elevation::Elevation,
        ca: &mixengine_proto::Ca,
    ) -> Result<bool, Error> {
        let (Some(host), Some(der)) = (
            self.host.as_ref(),
            mixengine_core::certs::ca::der(&ca.certificate_pem),
        ) else {
            return Ok(false);
        };

        let state = match host.trust_store().probe(&der) {
            Ok(state) if state.installed => state,
            Ok(_) => return Ok(false),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "this machine's trust store cannot be read; asking to remove nothing"
                );
                return Ok(false);
            }
        };

        // **By key-id and never by fingerprint** — T49a's D5. A removal that could name a
        // fingerprint could name the root that validates this machine's own updates.
        match state.target(&ca.key_id) {
            Some(target) => {
                elevation
                    .enqueue(&mixengine_proto::privileged::PrivilegedOp::TrustCaRemove { target })
                    .await?;
                Ok(true)
            }
            None => Ok(false),
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
                because: "this home has no usable certificate authority, so no browser was asked"
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

    // The sharing row itself, when this site is shared — roadmap tasks **T74** and **T75**. It
    // travels with the row rather than as an argument, so every path that reissues a certificate
    // covers what the site currently answers on: a share, an unshare, and the renewal timer that
    // knows about neither. What that comes to is `leaf::covered`'s to say, not this function's.
    match mixengine_core::certs::leaf::ensure(certs, &site.domains, site.sharing.as_ref(), now) {
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

/// The one condition worth acting on, in the order a person would act — roadmap task **T53**.
///
/// **First match wins, and the order is the whole of it.** A site can be wrong in several ways at
/// once, and naming all of them leaves whoever reads the answer to decide which to fix first —
/// which is the work this function exists to do. `NoCertificate` comes before `NamesDiffer` because
/// there is nothing to compare names against; the wire comes before the clock because a padlock
/// that is red now outranks one that will be.
fn problem(
    https: bool,
    domains: &[String],
    disk: &mixengine_proto::CertState,
    handshake: &mixengine_proto::Handshake,
) -> Option<mixengine_proto::CertProblem> {
    use mixengine_proto::{CertProblem, CertState, Handshake, Verdict};

    // **A site that asked for no certificate has no problem**, which is the distinction T52 added
    // to `IssueOutcome` for this reason. Without this line every plaintext site in the home would
    // be reported as missing a certificate it never wanted.
    if !https {
        return None;
    }

    let CertState::Present { cert } = disk else {
        return Some(CertProblem::NoCertificate);
    };

    if cert.sans != domains {
        return Some(CertProblem::NamesDiffer);
    }

    match handshake {
        // Unreachable while `https` is true, and answered rather than asserted: a panic here would
        // turn the command that diagnoses this home into the thing that needs diagnosing.
        Handshake::NotAsked {} => return None,

        Handshake::NotServed { .. } | Handshake::Failed { .. } => {
            return Some(CertProblem::NotServed);
        }

        Handshake::Presented {
            cert: served,
            trust,
        } => {
            // **By fingerprint and not by names.** A hash differs whenever anything differs — a
            // renewal, a rotation, a reissue covering the same names — where comparing names would
            // call a server holding last month's certificate correct.
            if served.fingerprint != cert.fingerprint {
                return Some(CertProblem::ServedCertificateDiffers);
            }

            if matches!(trust, Verdict::Rejected { .. }) {
                return Some(CertProblem::NotTrusted);
            }
        }
    }

    (cert.days_left <= mixengine_core::certs::leaf::RENEW_WITHIN_DAYS)
        .then_some(CertProblem::Expiring)
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
            owner: mixengine_core::sites::SiteOwner::Project(1),
            doc_root: String::new(),
            kind: mixengine_proto::SiteKind::Static,
            https_enabled: true,
            state: mixengine_proto::SiteState::Enabled,
            domains: domains.iter().map(|one| (*one).to_owned()).collect(),
            services: Vec::new(),
            sharing: None,
        }
    }

    fn a_leaf(sans: &[&str], days_left: i64, fingerprint: &str) -> mixengine_proto::SiteCert {
        mixengine_proto::SiteCert {
            subject: "CN=blog.test".to_owned(),
            sans: sans.iter().map(|one| (*one).to_owned()).collect(),
            issuer: "CN=MixEngine Local CA 9dc40c23".to_owned(),
            fingerprint: fingerprint.to_owned(),
            not_before: mixengine_proto::Timestamp(0),
            not_after: mixengine_proto::Timestamp(0),
            days_left,
        }
    }

    fn on_disk(cert: mixengine_proto::SiteCert) -> mixengine_proto::CertState {
        mixengine_proto::CertState::Present { cert }
    }

    fn presented(cert: mixengine_proto::SiteCert) -> mixengine_proto::Handshake {
        mixengine_proto::Handshake::Presented {
            cert,
            trust: mixengine_proto::Verdict::Trusted {},
        }
    }

    fn blog() -> Vec<String> {
        vec!["blog.test".to_owned()]
    }

    /// **A site that declares no HTTPS has nothing wrong with it** — the same distinction T52 had
    /// to add to `IssueOutcome`, and the third time this phase has had to make it. Without this
    /// rule every plaintext site in the home would be reported as missing a certificate it never
    /// asked for.
    #[test]
    fn a_site_that_declares_no_https_has_nothing_wrong_with_it() {
        assert_eq!(
            problem(
                false,
                &["plain.test".to_owned()],
                &mixengine_proto::CertState::Absent {},
                &mixengine_proto::Handshake::NotAsked {},
            ),
            None
        );
    }

    #[test]
    fn a_site_with_no_certificate_on_disk_is_named_that_way() {
        assert_eq!(
            problem(
                true,
                &blog(),
                &mixengine_proto::CertState::Absent {},
                &mixengine_proto::Handshake::NotServed {
                    because: "nothing answered".to_owned()
                },
            ),
            Some(mixengine_proto::CertProblem::NoCertificate)
        );
    }

    /// **A shared site is not a site whose names differ** — roadmap task **T75**, fixing a defect
    /// shipped with T74.
    ///
    /// T74 put the LAN address into the certificate and left this comparison reading the bare
    /// domain list, so every shared site reported `NamesDiffer` for as long as it was shared. The
    /// comparison has to ask the same question the issuer answered, which is what `leaf::covered`
    /// is for.
    #[test]
    fn a_shared_site_whose_certificate_covers_its_address_has_no_problem() {
        let sharing = mixengine_core::sites::Sharing {
            interface: "Wi-Fi".to_owned(),
            address: [192, 168, 1, 10].into(),
            since: mixengine_proto::Timestamp(1),
            until: None,
        };

        let cert = a_leaf(
            &["blog.test", "blog-mixengine.local", "192.168.1.10"],
            80,
            "aa",
        );

        // **The bare domain list is the defect, kept here as the other half of the assertion.**
        // This is what the call site passed until T75, and it is why every shared site reported a
        // problem it did not have. Asserted rather than described, so that a change that quietly
        // reinstates it fails here.
        assert_eq!(
            problem(
                true,
                &blog(),
                &on_disk(cert.clone()),
                &presented(cert.clone())
            ),
            Some(mixengine_proto::CertProblem::NamesDiffer)
        );

        let covered = mixengine_core::certs::leaf::covered(&blog(), Some(&sharing));

        assert_eq!(
            problem(true, &covered, &on_disk(cert.clone()), &presented(cert)),
            None
        );
    }

    /// Judged against the **file** rather than the wire, because the fix is a reissue: no server
    /// can serve a name nothing has signed.
    #[test]
    fn a_certificate_that_does_not_cover_the_sites_names_is_named_that_way() {
        let cert = a_leaf(&["blog.test"], 80, "aa");

        assert_eq!(
            problem(
                true,
                &["blog.test".to_owned(), "www.blog.test".to_owned()],
                &on_disk(cert.clone()),
                &presented(cert),
            ),
            Some(mixengine_proto::CertProblem::NamesDiffer)
        );
    }

    #[test]
    fn a_site_nothing_answers_for_is_named_that_way() {
        let cert = a_leaf(&["blog.test"], 80, "aa");

        assert_eq!(
            problem(
                true,
                &blog(),
                &on_disk(cert),
                &mixengine_proto::Handshake::NotServed {
                    because: "nothing answered on 127.0.0.1:443".to_owned()
                },
            ),
            Some(mixengine_proto::CertProblem::NotServed)
        );
    }

    /// **The report this task was built for**: the file is right and the server is still holding
    /// the one before it. Every check that reads files calls this machine healthy.
    #[test]
    fn a_server_holding_the_previous_certificate_is_named_that_way() {
        assert_eq!(
            problem(
                true,
                &blog(),
                &on_disk(a_leaf(&["blog.test"], 80, "new")),
                &presented(a_leaf(&["blog.test"], 80, "old")),
            ),
            Some(mixengine_proto::CertProblem::ServedCertificateDiffers)
        );
    }

    #[test]
    fn a_chain_the_verifier_refused_is_named_that_way() {
        let cert = a_leaf(&["blog.test"], 80, "aa");

        assert_eq!(
            problem(
                true,
                &blog(),
                &on_disk(cert.clone()),
                &mixengine_proto::Handshake::Presented {
                    cert,
                    trust: mixengine_proto::Verdict::Rejected {
                        because: "unknown issuer".to_owned()
                    },
                },
            ),
            Some(mixengine_proto::CertProblem::NotTrusted)
        );
    }

    /// Last, because it is the only one of these that is not broken yet — and T52's loop is already
    /// replacing it.
    #[test]
    fn a_certificate_inside_the_renewal_window_is_named_last() {
        let cert = a_leaf(&["blog.test"], 10, "aa");

        assert_eq!(
            problem(true, &blog(), &on_disk(cert.clone()), &presented(cert)),
            Some(mixengine_proto::CertProblem::Expiring)
        );
    }

    #[test]
    fn a_site_with_nothing_wrong_has_no_problem() {
        let cert = a_leaf(&["blog.test"], 80, "aa");

        assert_eq!(
            problem(true, &blog(), &on_disk(cert.clone()), &presented(cert)),
            None
        );
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

    /// **The acceptance criterion, enumerated** — roadmap task **T54**, and the control that makes
    /// the enumeration mean something.
    ///
    /// `.claude/features/tls.md` asks that `mix cert ca-uninstall` leave no MixEngine certificate in
    /// any store, *"verified by an integration test that enumerates the stores"*. A machine running
    /// `cargo test` has a real trust store it must not touch (testing rule 1) and may have no
    /// `certutil` at all, so the enumeration happens here — against the mock T49b built
    /// `browsers_removed` for, with T54 named in its documentation as the producer.
    ///
    /// **The control is the first assertion.** "The list does not contain it" passes just as well
    /// when the list was never built, which is the failure mode that fooled three measurements in
    /// T49b before a handshake corrected them.
    #[tokio::test]
    async fn a_removal_names_the_authority_every_browser_was_asked_to_hold() {
        let home = tempfile::tempdir().expect("a temp home");
        let host = a_machine_with_a_browser(home.path());
        let paths = paths_under(home.path());
        std::fs::create_dir_all(paths.certs()).expect("the certs directory is made");

        let certificates = Certificates::reading(&paths, host.clone());
        let made = certificates.ensure().await.expect("an authority is made");
        let CaState::Present { ca } = &made.state else {
            panic!("this home has an authority: {made:?}");
        };

        certificates.install_in_browsers(&made.state).await;

        assert_eq!(
            host.browsers_installed().len(),
            1,
            "the control: the browsers were asked to hold it in the first place"
        );

        let change = certificates.remove_from_browsers(&ca.key_id).await;

        assert_eq!(
            host.browsers_removed(),
            vec![ca.key_id.clone()],
            "the removal names this home's authority, by key-id and never by fingerprint; \
             refused: {:?}",
            change.refused
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
