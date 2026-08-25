//! `mix doctor`, which reports and does not repair — roadmap task **T47a**.
//!
//! **Nothing here writes.** No row, no file, nothing enqueued, and no elevation prompt can result
//! from a call — which is what makes it safe to run on a timer, inside `mix status`, and inside
//! T93's bundle. Repairing what it finds is `daemon.doctor_repair`, and that is T47b's.
//!
//! Every check is assembled from the reader its subsystem already owns — the hosts block from T41's
//! comparison, the resolver from T45's probe, the domains from T46's report — rather than from a
//! second opinion. Two implementations of one question are two answers to it.
//!
//! # Three outcomes that are not failures
//!
//! [`Outcome::Ok`] is the ordinary one. [`Outcome::Note`] is a fact worth stating that nobody can
//! act on — `hosts_only` is a supported mode and macOS genuinely makes no promise about a killed
//! daemon's descendants — and reporting either as a fault would put a permanent problem on a
//! correctly working machine. [`Outcome::Skipped`] is a check that could not run and says why, which
//! is the difference between "there is nothing wrong here" and "nobody looked".

use std::sync::Arc;

use mixengine_proto::{Check, DoctorReport, Outcome, ProblemId};

/// The `daemon.doctor` half of the API.
#[derive(Debug)]
pub(crate) struct Doctor {
    /// The rows, for the hosts block this home's sites need.
    store: mixengine_core::Store,

    /// The server, and which TLDs this machine routes here — T44 and T45.
    dns: Arc<crate::dns::Dns>,

    /// This machine: its hosts file, its resolver, its permissions, its reserved ranges.
    host: Arc<dyn mixengine_platform::Host>,

    /// The queue, for what is waiting on a person.
    elevation: Arc<crate::elevation::Elevation>,

    /// The front end's program path, which is what the port-access probe is about.
    services: Arc<crate::services::Registry>,

    /// T46's report, rendered rather than recomputed.
    domains: Arc<crate::domains::Domains>,

    /// The home's own directory, for the permissions check.
    root: std::path::PathBuf,

    /// Where this home's authority lives, for the trust-store check — T49a.
    certs: std::path::PathBuf,

    /// What these rows render to, for the drift check — the registry's own generator.
    generator: mixengine_core::generate::Generator,
}

impl Doctor {
    /// The one of these the API holds.
    pub(crate) fn new(
        store: &mixengine_core::Store,
        dns: Arc<crate::dns::Dns>,
        host: Arc<dyn mixengine_platform::Host>,
        elevation: Arc<crate::elevation::Elevation>,
        services: Arc<crate::services::Registry>,
        domains: Arc<crate::domains::Domains>,
        paths: &mixengine_core::Paths,
    ) -> Arc<Self> {
        // **The home's layout rather than its root and its generator separately.** Both are derived
        // from it, and passing them apart let a caller hand this a generator built from one home and
        // a root from another — a check comparing a rendering the registry would never have written.
        let generator = crate::services::generator(paths, store, host.as_ref());

        Arc::new(Self {
            store: store.clone(),
            dns,
            host,
            elevation,
            services,
            domains,
            root: paths.root().to_path_buf(),
            certs: paths.certs().to_path_buf(),
            generator,
        })
    }

    /// Examine everything, in a fixed order.
    ///
    /// **Every check appears, whatever it answered.** A check that found nothing wrong is the
    /// evidence that it ran, and a shorter list on one system would read as a clean bill of health
    /// rather than as a question nobody asked.
    pub(crate) async fn report(&self) -> DoctorReport {
        DoctorReport {
            checks: vec![
                self.hosts_block().await,
                self.resolver(),
                self.trust_store(),
                self.browsers(),
                self.site_certificates().await,
                self.dns_server(),
                self.port_access().await,
                self.pending_permissions().await,
                self.domains().await,
                self.home_permissions(),
                self.descendants(),
                self.reserved_ports(),
                self.generated_config().await,
                self.unsupervised().await,
            ],
        }
    }

    /// **1.** Compared the way `Elevation::require_hosts` compares it — as operations rather than as
    /// lists, so the ordering and the deduplication are `hosts_apply`'s in both places and there is
    /// one definition of "the same block".
    async fn hosts_block(&self) -> Check {
        let name = "the managed hosts block".to_owned();

        let Ok(desired) = mixengine_core::hosts::desired(&self.store, &self.dns.wired()).await
        else {
            return Check {
                name,
                outcome: Outcome::Skipped {
                    because: "this home's sites could not be read".to_owned(),
                },
            };
        };

        let wanted = mixengine_proto::privileged::PrivilegedOp::hosts_apply(desired);

        match self.host.hosts_file().managed() {
            // `present.clone()` and not `present`: a pattern guard may not move out of what it
            // matched, which is why `Elevation::require_hosts` writes it the same way one file over.
            Ok(present)
                if mixengine_proto::privileged::PrivilegedOp::hosts_apply(present.clone())
                    == wanted =>
            {
                Check {
                    name,
                    outcome: Outcome::Ok {},
                }
            }
            Ok(_) => Check {
                name,
                outcome: Outcome::Problem {
                    id: ProblemId::HostsBlockDiffers,
                    because: "the hosts file does not hold the names this home's sites need"
                        .to_owned(),
                },
            },
            Err(error) => Check {
                name,
                outcome: Outcome::Skipped {
                    because: format!("the hosts file could not be read: {error}"),
                },
            },
        }
    }

    /// **2.** A port the operating system chose is a port nothing may be wired to — T45 — so that is
    /// a `Note` and not a fault. Every test home in this workspace is on one.
    fn resolver(&self) -> Check {
        let name = "the resolver on this machine".to_owned();

        let Some(port) = self.dns.wirable_port() else {
            return Check {
                name,
                outcome: Outcome::Note {
                    because: "this home's DNS port is chosen by the operating system, so no \
                              resolver may be pointed at it"
                        .to_owned(),
                },
            };
        };

        let want: Vec<&str> = mixengine_proto::domains::WIRED_TLDS.to_vec();

        match self.host.resolver().probe(&want, port) {
            Ok(state) if state.wired.len() == want.len() => Check {
                name,
                outcome: Outcome::Ok {},
            },
            Ok(state) => Check {
                name,
                outcome: Outcome::Problem {
                    id: ProblemId::ResolverNotWired,
                    because: state.missing.unwrap_or_else(|| {
                        "nothing on this machine sends a managed TLD to this daemon".to_owned()
                    }),
                },
            },
            Err(error) => Check {
                name,
                outcome: Outcome::Skipped {
                    because: format!("this machine's resolver could not be read: {error}"),
                },
            },
        }
    }

    /// **3.** Whether this machine trusts the authority T48 made — roadmap task **T49a**.
    ///
    /// **A home with no authority is skipped, not a problem.** There is nothing to trust, and the
    /// daemon already warned about the generation that failed; reporting it twice would put a second
    /// condition on the screen for one cause. A machine with no store MixEngine knows how to write
    /// is a `Note` rather than a problem, for the reason the resolver check gives about `hosts_only`:
    /// it is a supported mode and calling it a fault would put a permanent problem on every machine
    /// that will never have one.
    ///
    /// **This answers "is it in the store", not "does a browser trust it".** Firefox and Chrome on
    /// Linux read NSS and not this store at all (T49b), and the honest end-to-end check is a live
    /// handshake, which is T53's.
    fn trust_store(&self) -> Check {
        let name = "this machine's trust in MixEngine's authority".to_owned();

        let der = match mixengine_core::certs::ca::read(&self.certs, std::time::SystemTime::now()) {
            mixengine_proto::CaState::Present { ca } => {
                mixengine_core::certs::ca::der(&ca.certificate_pem)
            }
            mixengine_proto::CaState::Absent {} | mixengine_proto::CaState::Unusable { .. } => None,
        };

        let Some(der) = der else {
            return Check {
                name,
                outcome: Outcome::Skipped {
                    because: "this home has no usable certificate authority, so there is nothing \
                              for this machine to trust — `mix cert ca-status` says which"
                        .to_owned(),
                },
            };
        };

        match self.host.trust_store().probe(&der) {
            Ok(state) if state.installed => Check {
                name,
                outcome: Outcome::Ok {},
            },
            Ok(state) if state.method == mixengine_platform::TrustStoreMethod::None => Check {
                name,
                outcome: Outcome::Note {
                    because: state.missing.unwrap_or_else(|| {
                        "this machine has no system trust store MixEngine knows how to write"
                            .to_owned()
                    }),
                },
            },
            Ok(state) => Check {
                name,
                outcome: Outcome::Problem {
                    id: ProblemId::CaNotTrusted,
                    because: state.missing.unwrap_or_else(|| {
                        "this machine does not hold MixEngine's certificate authority".to_owned()
                    }),
                },
            },
            Err(error) => Check {
                name,
                outcome: Outcome::Skipped {
                    because: format!("this machine's trust store could not be read: {error}"),
                },
            },
        }
    }

    /// **3b.** What Firefox and Chrome hold, which is a different question from the one above:
    /// they read NSS databases and not the system store at all.
    ///
    /// **No tool is a `Note` and not a problem**, on the reasoning the resolver's `hosts_only` arm
    /// states — a machine that will never run a browser would otherwise carry a permanent fault. So
    /// is a system MixEngine does not search. A machine with databases that simply lack it *is* a
    /// problem, and the reason names them.
    fn browsers(&self) -> Check {
        let name = "MixEngine's authority in this machine's browsers".to_owned();

        let der = match mixengine_core::certs::ca::read(&self.certs, std::time::SystemTime::now()) {
            mixengine_proto::CaState::Present { ca } => {
                mixengine_core::certs::ca::der(&ca.certificate_pem)
            }
            mixengine_proto::CaState::Absent {} | mixengine_proto::CaState::Unusable { .. } => None,
        };

        let Some(der) = der else {
            return Check {
                name,
                outcome: Outcome::Skipped {
                    because: "this home has no usable certificate authority, so there is nothing                               for a browser to trust — `mix cert ca-status` says which"
                        .to_owned(),
                },
            };
        };

        let survey = match self.host.browsers().survey(&der) {
            Ok(survey) => survey,
            Err(error) => {
                return Check {
                    name,
                    outcome: Outcome::Skipped {
                        because: format!("this machine's browsers could not be asked: {error}"),
                    },
                };
            }
        };

        let lacking = survey.lacking();

        if !lacking.is_empty() {
            let because = lacking
                .iter()
                .map(|one| one.path.as_str())
                .collect::<Vec<_>>()
                .join(", ");

            return Check {
                name,
                outcome: Outcome::Problem {
                    id: ProblemId::BrowsersNotTrusted,
                    because: format!(
                        "these browser databases do not hold MixEngine's authority: {because}"
                    ),
                },
            };
        }

        match survey {
            // Nothing lacks it because nothing could be asked, which is a true thing to say and not
            // an `Ok` — the distinction this whole check turns on.
            mixengine_platform::BrowserSurvey::NoTool { because }
            | mixengine_platform::BrowserSurvey::NotSearched { because } => Check {
                name,
                outcome: Outcome::Note { because },
            },
            mixengine_platform::BrowserSurvey::Reached { .. } => Check {
                name,
                outcome: Outcome::Ok {},
            },
        }
    }

    /// **3c.** Whether every site that declares HTTPS has a certificate that still covers its
    /// names — roadmap task **T50**.
    ///
    /// **A check on disk and not a handshake.** Whether a browser accepts what the front end
    /// actually serves is a stronger claim and it is `mix cert status`' (T53); this one answers
    /// whether the file exists and matches the row, which is the question that catches the most
    /// common report — a domain added and a certificate not reissued.
    async fn site_certificates(&self) -> Check {
        let name = "a certificate for every site that declares HTTPS".to_owned();

        let Ok(sites) = mixengine_core::sites::records(&self.store, None).await else {
            return Check {
                name,
                outcome: Outcome::Skipped {
                    because: "this home's sites could not be read".to_owned(),
                },
            };
        };

        let wanted: Vec<_> = sites
            .into_iter()
            .filter(|site| site.https_enabled && !site.domains.is_empty())
            .collect();

        if wanted.is_empty() {
            return Check {
                name,
                outcome: Outcome::Ok {},
            };
        }

        if !matches!(
            mixengine_core::certs::ca::read(&self.certs, std::time::SystemTime::now()),
            mixengine_proto::CaState::Present { .. }
        ) {
            return Check {
                name,
                outcome: Outcome::Skipped {
                    because: "this home has no usable certificate authority to sign with — \
                              `mix cert ca-status` says which"
                        .to_owned(),
                },
            };
        }

        let now = std::time::SystemTime::now();

        // **Three of `leaf::ensure`'s four questions and deliberately not the fourth.** A leaf
        // signed by an authority this home has since replaced is caught by `ensure` when the repair
        // below runs; asserting it here as well would be two copies of one rule to keep in step,
        // and the copy that drifted would report a machine as faulty for a certificate the repair
        // then declined to replace.
        let lacking: Vec<String> = wanted
            .iter()
            .filter(|site| {
                let primary = &site.domains[0];

                !matches!(
                    mixengine_core::certs::leaf::read(&self.certs, primary, now),
                    mixengine_proto::CertState::Present { ref cert }
                        if cert.sans == site.domains
                            && cert.days_left > mixengine_core::certs::leaf::RENEW_WITHIN_DAYS
                )
            })
            .map(|site| site.domains[0].clone())
            .collect();

        if lacking.is_empty() {
            Check {
                name,
                outcome: Outcome::Ok {},
            }
        } else {
            Check {
                name,
                outcome: Outcome::Problem {
                    id: ProblemId::SiteCertificateMissing,
                    because: format!(
                        "these sites have no certificate covering their names: {}",
                        lacking.join(", ")
                    ),
                },
            }
        }
    }

    /// **3.** `hosts_only` is a **mode T46a closed as supported**, not a degradation — so it is a
    /// `Note`. Only a bind that failed is a problem, and that distinction is the whole of this
    /// check: calling the supported mode a fault would put a permanent problem on every machine
    /// that never wired a resolver.
    fn dns_server(&self) -> Check {
        let name = "the built-in DNS server".to_owned();
        let status = self.dns.status();

        match (status.listening, status.because) {
            (Some(_), _) => Check {
                name,
                outcome: Outcome::Ok {},
            },
            (None, Some(because)) => Check {
                name,
                outcome: Outcome::Problem {
                    id: ProblemId::DnsServerUnavailable,
                    because,
                },
            },
            (None, None) => Check {
                name,
                outcome: Outcome::Note {
                    because: "the DNS server is switched off in config.toml".to_owned(),
                },
            },
        }
    }

    /// **4.** A home with no front end has nothing that needs to answer on 80 or 443.
    async fn port_access(&self) -> Check {
        let name = "answering on 80 and 443".to_owned();

        let Some(binary) = self.services.front_end_program().await else {
            return Check {
                name,
                outcome: Outcome::Note {
                    because: "this home has no front end, so nothing needs those ports".to_owned(),
                },
            };
        };

        match self
            .host
            .port_access()
            .probe(&binary, &crate::elevation::Elevation::ANSWERING)
        {
            Ok(state) if state.granted => Check {
                name,
                outcome: Outcome::Ok {},
            },
            Ok(state) => Check {
                name,
                outcome: Outcome::Problem {
                    id: ProblemId::PortAccessMissing,
                    because: state
                        .missing
                        .unwrap_or_else(|| "this machine has not granted it".to_owned()),
                },
            },
            Err(error) => Check {
                name,
                outcome: Outcome::Skipped {
                    because: format!("this machine could not be asked: {error}"),
                },
            },
        }
    }

    /// **5.** Something waiting on a person is not a fault of the machine, but it *is* something
    /// this home asked for and has not got — and T47b's flush is exactly the repair for it.
    ///
    /// Read through `elevation.status`, which is what the client screen T64 built reads, so "what is
    /// waiting" has one definition rather than two that have to agree.
    async fn pending_permissions(&self) -> Check {
        let name = "operations waiting for permission".to_owned();

        let waiting = match self.elevation.status().await {
            Ok(status) => status.pending,
            Err(error) => {
                return Check {
                    name,
                    outcome: Outcome::Skipped {
                        because: format!("the queue could not be read: {error}"),
                    },
                };
            }
        };

        if waiting.is_empty() {
            return Check {
                name,
                outcome: Outcome::Ok {},
            };
        }

        Check {
            name,
            outcome: Outcome::Problem {
                id: ProblemId::PermissionPending,
                because: format!(
                    "{} operation(s) are waiting to be granted or dropped",
                    waiting.len()
                ),
            },
        }
    }

    /// **6.** T46's report, rendered — **never recomputed**. T46's own roadmap entry asks for that
    /// in as many words, and the reason is this whole module's: two implementations of one question
    /// are two answers to it.
    ///
    /// **One `Problem` naming every unreachable domain**, rather than one per domain. A home with
    /// twelve names that do not resolve has one fault with one cause, and twelve rows would bury the
    /// eight checks around them.
    async fn domains(&self) -> Check {
        let name = "every domain this home declares".to_owned();

        let Ok(report) = self
            .domains
            .status(&mixengine_proto::DomainStatusQuery { domain: None })
            .await
        else {
            return Check {
                name,
                outcome: Outcome::Skipped {
                    because: "this home's domains could not be read".to_owned(),
                },
            };
        };

        let unreachable: Vec<&str> = report
            .domains
            .iter()
            .filter(|row| row.resolves_to.is_empty())
            .map(|row| row.domain.as_str())
            .collect();

        if unreachable.is_empty() {
            return Check {
                name,
                outcome: Outcome::Ok {},
            };
        }

        Check {
            name,
            outcome: Outcome::Problem {
                id: ProblemId::DomainUnreachable,
                because: format!("this machine does not resolve {}", unreachable.join(", ")),
            },
        }
    }

    /// **7.** `is_restricted_to_owner`'s first caller — and what settles the `icacls` question T3a
    /// left open, since the whole of what this needs is "inheritance is intact, yes or no".
    fn home_permissions(&self) -> Check {
        let name = "the home is readable only by its owner".to_owned();

        match self
            .host
            .directory_access()
            .is_restricted_to_owner(&self.root)
        {
            Ok(true) => Check {
                name,
                outcome: Outcome::Ok {},
            },
            Ok(false) => Check {
                name,
                outcome: Outcome::Problem {
                    id: ProblemId::HomePermissionsLost,
                    because: "this home is no longer restricted to its owner, which a move onto \
                              another volume or a restore from a backup does silently"
                        .to_owned(),
                },
            },
            Err(error) => Check {
                name,
                outcome: Outcome::Skipped {
                    because: format!("this home's permissions could not be read: {error}"),
                },
            },
        }
    }

    /// **8.** Always a `Note`, on every system — the whole of the design's D4, and
    /// [ADR 0007](../../../.claude/decisions/0007-supervised-child-owns-a-process-group.md)'s own
    /// table read out loud.
    fn descendants(&self) -> Check {
        Check {
            name: "what this system promises about a service's descendants".to_owned(),
            outcome: Outcome::Note {
                because: mixengine_platform::orphan_guarantee().because().to_owned(),
            },
        }
    }

    /// **9.** A `Problem` **only where a reserved range holds a port this home actually needs** —
    /// the ranges are the operating system's business until they collide with ours.
    ///
    /// This is the one check that saves a person from a wrong search rather than telling them
    /// something they could have found: a bind into a reserved range fails with an access error, so
    /// it reads as a permission problem, and elevation, UAC and the firewall are all the wrong place
    /// to look.
    fn reserved_ports(&self) -> Check {
        let name = "ports this system has reserved".to_owned();

        let ranges = match self.host.reserved_ports().reserved() {
            Ok(ranges) => ranges,
            Err(error) => {
                return Check {
                    name,
                    outcome: Outcome::Skipped {
                        because: error.to_string(),
                    },
                };
            }
        };

        let taken: Vec<u16> = [80, 443]
            .into_iter()
            .chain(self.dns.port())
            .filter(|port| ranges.iter().any(|range| range.holds(*port)))
            .collect();

        if taken.is_empty() {
            return Check {
                name,
                outcome: if ranges.is_empty() {
                    Outcome::Ok {}
                } else {
                    Outcome::Note {
                        because: format!(
                            "{} port range(s) are reserved on this system, and none holds a port \
                             this home needs",
                            ranges.len()
                        ),
                    }
                },
            };
        }

        Check {
            name,
            outcome: Outcome::Problem {
                id: ProblemId::PortRangeReserved,
                because: format!(
                    "this system has reserved {}, so binding it fails with an error that reads \
                     like a permission problem and is not one",
                    taken
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
            },
        }
    }

    /// **10.** Is what is installed what these rows render to?
    ///
    /// **Rendered again and compared, never parsed back** — the workspace rule, and the reason this
    /// check could not be built in T47a: answering the question *is* the first half of the repair.
    /// [`Generator::drift`](mixengine_core::generate::Generator::drift) builds the rendering in
    /// memory and installs nothing, so this stays a read.
    ///
    /// **One `Problem` naming every service that drifted**, on the domains check's reasoning: a home
    /// whose rows all moved has one fault with one cause, and a row per service would bury the ten
    /// checks around them.
    ///
    /// What this is a fault *about* is worth being exact on, because generated configuration is
    /// disposable and the next write corrects it anyway: the fault is that a service is running on a
    /// rendering nothing in this home asked for, right now.
    async fn generated_config(&self) -> Check {
        let name = "the generated configuration".to_owned();

        let drifts = match self.generator.drift().await {
            Ok(drifts) => drifts,
            Err(error) => {
                return Check {
                    name,
                    outcome: Outcome::Skipped {
                        because: format!(
                            "what these rows render to could not be worked out: {error}"
                        ),
                    },
                };
            }
        };

        let stale: Vec<&str> = drifts
            .iter()
            .filter(|one| !one.drift.is_empty())
            .map(|one| one.service.as_str())
            .collect();

        if stale.is_empty() {
            return Check {
                name,
                outcome: Outcome::Ok {},
            };
        }

        Check {
            name,
            outcome: Outcome::Problem {
                id: ProblemId::GeneratedConfigStale,
                because: format!(
                    "what is installed for {} is not what its row renders to, so what is being \
                     served is not what this home says",
                    stale.join(", ")
                ),
            },
        }
    }

    /// **11.** A row that claims a supervisor this daemon does not have.
    ///
    /// **Narrow on purpose, and [`Registry::recover`](crate::services::Registry::recover) is why.**
    /// That function answers the same question at every boot by walking *every* row, which is right
    /// when nothing is supervised yet and wrong on a running daemon: it would stop services that are
    /// working. The rows a live daemon may reconcile are the ones it holds no runner for, and that is
    /// exactly this set.
    async fn unsupervised(&self) -> Check {
        let name = "every service this daemon is supervising".to_owned();

        let records = match mixengine_core::services::records(&self.store).await {
            Ok(records) => records,
            Err(error) => {
                return Check {
                    name,
                    outcome: Outcome::Skipped {
                        because: format!("the services could not be read: {error}"),
                    },
                };
            }
        };

        // `records` is keyed by the id as a `String` and `supervised` answers `ServiceId`s. Compared
        // as strings rather than by parsing, because a row whose id no longer parses is still a row
        // this daemon is not supervising.
        let supervised = self.services.supervised();
        let held: std::collections::BTreeSet<&str> = supervised
            .iter()
            .map(mixengine_proto::ServiceId::as_str)
            .collect();

        let stranded = stranded(
            records.iter().map(|(stored, record)| {
                (
                    stored.as_str(),
                    record.state.is_supervised() || record.pid.is_some(),
                )
            }),
            &held,
        );

        if stranded.is_empty() {
            return Check {
                name,
                outcome: Outcome::Ok {},
            };
        }

        Check {
            name,
            outcome: Outcome::Problem {
                id: ProblemId::ServiceUnsupervised,
                because: format!(
                    "{} claim(s) a supervisor this daemon does not have, so a port and a data \
                     directory are held by something nothing is watching",
                    stranded.len()
                ),
            },
        }
    }
}

/// Which of these rows claim a supervisor nobody is.
///
/// **Free and pure**, so the one decision in check 11 is tested without a database, a registry or a
/// daemon — `mixengine_platform`'s reserved-range parser one check along, and for the same reason.
/// Each item is an id and whether its row claims a supervisor at all; `held` is what this registry is
/// actually running.
fn stranded<'a>(
    rows: impl Iterator<Item = (&'a str, bool)>,
    held: &std::collections::BTreeSet<&str>,
) -> Vec<String> {
    rows.filter(|(id, claims)| *claims && !held.contains(id))
        .map(|(id, _)| id.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use mixengine_platform::Host as _;

    /// **The failing arm of check 9, on every system.** What the real read answers is whatever the
    /// machine running the suite happens to have reserved, and no test may change that — so without
    /// the mock this branch would ship having never run.
    #[test]
    fn a_reserved_range_holding_port_80_is_found() {
        let host = mixengine_platform::mock::Host::with_reserved_ports("/mixengine", &[(60, 100)]);

        let ranges = host
            .reserved_ports()
            .reserved()
            .expect("the mock always answers");

        assert!(ranges.iter().any(|range| range.holds(80)), "{ranges:?}");
        assert!(!ranges.iter().any(|range| range.holds(443)), "{ranges:?}");
    }

    /// And the ordinary arm, so the assertion above is a comparison rather than a coincidence.
    #[test]
    fn a_machine_that_reserves_nothing_holds_nothing() {
        let host = mixengine_platform::mock::Host::with_reserved_ports("/mixengine", &[]);

        assert!(
            host.reserved_ports()
                .reserved()
                .expect("the mock always answers")
                .is_empty()
        );
    }
    /// The one decision in check 11, with the database and the registry taken out of it.
    ///
    /// A row is stranded when it *claims* a supervisor — its state says so, or it names a pid — and
    /// this daemon holds no runner for it. Both halves are in the table: a claimed row that is held
    /// is not stranded, and an unheld row that claims nothing is not either.
    #[test]
    fn a_row_is_stranded_only_when_it_claims_a_supervisor_nobody_is() {
        let held: std::collections::BTreeSet<&str> = ["fakeservice@held"].into_iter().collect();

        let stranded = super::stranded(
            [
                ("fakeservice@held", true),
                ("fakeservice@lost", true),
                ("fakeservice@quiet", false),
            ]
            .into_iter(),
            &held,
        );

        assert_eq!(stranded, vec!["fakeservice@lost".to_owned()]);
    }

    /// And the quiet case, so the assertion above is a comparison rather than a coincidence: a
    /// daemon supervising everything it has rows for strands nothing.
    #[test]
    fn a_daemon_supervising_what_it_has_rows_for_strands_nothing() {
        let held: std::collections::BTreeSet<&str> = ["fakeservice@main"].into_iter().collect();

        assert!(super::stranded([("fakeservice@main", true)].into_iter(), &held).is_empty());
    }
}
