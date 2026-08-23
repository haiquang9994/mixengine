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
    ///
    /// An `expect` and not an `allow`: the check that reads this lands in the next commit, and once
    /// it does the attribute is unfulfilled, which is a compile error in this workspace — so the
    /// scaffolding cannot outlive its reason.
    #[expect(dead_code, reason = "read by the domains check, one commit along")]
    domains: Arc<crate::domains::Domains>,

    /// The home's own directory, for the permissions check.
    root: std::path::PathBuf,
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
        root: std::path::PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: store.clone(),
            dns,
            host,
            elevation,
            services,
            domains,
            root,
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
                self.dns_server(),
                self.port_access().await,
                self.pending_permissions().await,
                self.domains().await,
                self.home_permissions(),
                self.descendants(),
                self.reserved_ports(),
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

    /// **6.** T46's report. Written in the next commit; until then it says so rather than claiming
    /// something it has not looked at.
    #[expect(clippy::unused_async, reason = "reads T46's report, one commit along")]
    async fn domains(&self) -> Check {
        Check {
            name: "every domain this home declares".to_owned(),
            outcome: Outcome::Skipped {
                because: "not examined by this build".to_owned(),
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

    /// **9.** Written in the next commit, with the domains above.
    fn reserved_ports(&self) -> Check {
        Check {
            name: "ports this system has reserved".to_owned(),
            outcome: Outcome::Skipped {
                because: "not examined by this build".to_owned(),
            },
        }
    }
}
