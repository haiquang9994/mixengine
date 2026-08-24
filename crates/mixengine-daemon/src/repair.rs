//! `daemon.doctor_repair`, which acts on what `mix doctor` found — roadmap task **T47b**.
//!
//! **One call, at most one prompt.** Repairs that live inside `MIXENGINE_HOME` are made here and
//! now; repairs that need root are *enqueued* through the same producers everything else uses, and
//! the queue is flushed once at the end. [ADR
//! 0005](../../../.claude/decisions/0005-on-demand-elevation.md) settled that asking twice for one
//! batch is the defect, and this module is the shape that obeys it.
//!
//! **Which repair a condition gets is decided by [`plan_for`], and that match is exhaustive.** It is
//! the whole of what T47a bought by closing [`ProblemId`]: an id added later stops this file
//! compiling until somebody decides what repairing it means. A `_ =>` arm would turn that compile
//! error into a silent no-op and let the two halves drift apart — which is the defect the closed
//! enum exists to prevent.
//!
//! Deciding is separated from doing so the table can be read, and tested, without a machine.

use std::sync::Arc;

use mixengine_proto::{Action, Outcome, ProblemId, Repair, RepairReport};

/// The `daemon.doctor_repair` half of the API.
#[derive(Debug)]
pub(crate) struct Repairs {
    /// What to repair. **Read at the top of every call**, never cached: a repair acting on a report
    /// from a minute ago is a repair for a machine that has moved.
    doctor: Arc<crate::doctor::Doctor>,

    /// The queue, and the one grant this call may raise.
    elevation: Arc<crate::elevation::Elevation>,

    /// This machine, for the one permission repair that needs no privilege at all.
    host: Arc<dyn mixengine_platform::Host>,

    /// The registry, for the front end's path and for the rows nothing is supervising.
    services: Arc<crate::services::Registry>,

    /// What these rows render to, for the repair that installs it.
    generator: mixengine_core::generate::Generator,

    /// The home's own directory.
    root: std::path::PathBuf,
}

impl Repairs {
    /// The one of these the API holds.
    ///
    /// `&Paths` rather than a root and a generator, for [`crate::doctor::Doctor::new`]'s reason: both
    /// are derived from it, and passing them apart lets a caller mix two homes.
    pub(crate) fn new(
        doctor: Arc<crate::doctor::Doctor>,
        elevation: Arc<crate::elevation::Elevation>,
        services: Arc<crate::services::Registry>,
        store: &mixengine_core::Store,
        paths: &mixengine_core::Paths,
    ) -> Arc<Self> {
        let host = elevation.host();
        let generator = crate::services::generator(paths, store, host.as_ref());

        Arc::new(Self {
            doctor,
            elevation,
            host,
            services,
            generator,
            root: paths.root().to_path_buf(),
        })
    }

    /// Repair everything this build can act on, and raise at most one prompt.
    ///
    /// **The grant comes last and only if something is waiting.** A home whose only fault was inside
    /// its own directory gets no prompt at all, and `granting` is [`None`] there — which is what a
    /// client renders "an administrator's permission is needed" off.
    pub(crate) async fn run(self: &Arc<Self>) -> RepairReport {
        let report = self.doctor.report().await;
        let mut actions = Vec::new();
        let mut wants_the_helper = false;

        for check in report.checks {
            let Outcome::Problem { id, .. } = check.outcome else {
                continue;
            };

            let outcome = match plan_for(id) {
                Planned::Untouched(because) => Action::Untouched {
                    because: because.to_owned(),
                },
                Planned::InHome(what) => self.in_home(what).await,
                Planned::Enqueue(what) => {
                    let action = self.enqueue(what).await;
                    wants_the_helper |= matches!(action, Action::Enqueued { .. });
                    action
                }
            };

            actions.push(Repair {
                id,
                name: check.name,
                outcome,
            });
        }

        RepairReport {
            granting: if wants_the_helper {
                self.flush().await
            } else {
                None
            },
            actions,
        }
    }

    /// A repair that needs no privilege, made now.
    ///
    /// **A repair that failed is not a repair that was made**, so a failure becomes an
    /// [`Action::Untouched`] carrying what went wrong rather than a `Repaired` that is not true.
    async fn in_home(&self, what: InHome) -> Action {
        match what {
            InHome::RestrictHome => {
                match self.host.directory_access().restrict_to_owner(&self.root) {
                    Ok(()) => Action::Repaired {
                        what: "this home is restricted to its owner again".to_owned(),
                    },
                    Err(error) => Action::Untouched {
                        because: format!("this home's permissions could not be written: {error}"),
                    },
                }
            }

            InHome::RenderConfiguration => match self.generator.declared().await {
                Ok(rendered) => {
                    let moved = rendered.iter().filter(|one| one.changed()).count();

                    Action::Repaired {
                        what: format!(
                            "{moved} service(s) had their configuration re-installed from their rows"
                        ),
                    }
                }
                Err(error) => Action::Untouched {
                    because: format!("the configuration could not be rendered: {error}"),
                },
            },

            InHome::ReconcileStrandedRows => {
                let reconciled = self.services.reconcile_stranded().await;

                // **`refused` is why this is not always a `Repaired`.** A survivor that would not
                // stop leaves its row exactly as it was found, which is the one outcome that leaves
                // the machine holding a port nothing supervises — and reporting it as repaired would
                // be the silence `Recovery::refused` exists to prevent.
                if reconciled.refused.is_empty() {
                    Action::Repaired {
                        what: format!(
                            "{} adopted, {} stopped, {} row(s) cleared",
                            reconciled.adopted.len(),
                            reconciled.stopped.len(),
                            reconciled.cleared.len()
                        ),
                    }
                } else {
                    Action::Untouched {
                        because: format!(
                            "{} service(s) would not stop and their rows are left as they were \
                             found",
                            reconciled.refused.len()
                        ),
                    }
                }
            }
        }
    }

    /// A repair the helper has to make, asked for rather than made.
    async fn enqueue(&self, what: Enqueue) -> Action {
        let asked = match what {
            Enqueue::Hosts => (
                self.elevation.require_hosts().await,
                "the managed hosts block",
            ),
            Enqueue::Resolver => (
                self.elevation.require_resolver().await,
                "sending this home's managed TLDs to its own DNS server",
            ),
            Enqueue::PortAccess => {
                let binary = self.services.front_end_program().await;

                (
                    self.elevation.require_port_access(binary.as_deref()).await,
                    "letting the front end answer on 80 and 443",
                )
            }

            // Nothing new goes into the queue: the queue *was* the condition, and the grant below is
            // the repair. What this entry adds is the count, which is the thing a person wants to see
            // before they answer a prompt.
            Enqueue::AlreadyWaiting => {
                return match self.elevation.status().await {
                    Ok(status) => Action::Enqueued {
                        what: format!(
                            "{} operation(s) were already waiting and are covered by the grant \
                             this call raised",
                            status.pending.len()
                        ),
                    },
                    Err(error) => Action::Untouched {
                        because: format!("the queue could not be read: {error}"),
                    },
                };
            }
        };

        match asked {
            (Ok(()), what) => Action::Enqueued {
                what: what.to_owned(),
            },
            (Err(error), _) => Action::Untouched {
                because: format!("this could not be added to the queue: {error}"),
            },
        }
    }

    /// The one prompt, raised once.
    ///
    /// **A grant that could not be raised leaves the enqueued entries standing**, which is the truth:
    /// they are still waiting, and a machine with no helper beside this daemon is a machine where
    /// they will keep waiting. The client sees `granting: null` next to them and can say so.
    async fn flush(self: &Arc<Self>) -> Option<mixengine_proto::JobId> {
        match self.elevation.grant().await {
            Ok(job) => Some(job.id),
            Err(error) => {
                tracing::warn!(
                    code = ?error.code,
                    "the repairs were queued and the prompt could not be raised"
                );

                None
            }
        }
    }
}

/// Which of the three things happens to a condition, decided without doing any of it.
///
/// Separated from the doing so the table is one function a test can read end to end — and so the
/// exhaustive match lives somewhere nothing else is going on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Planned {
    /// Do it now, inside the home. No prompt.
    InHome(InHome),

    /// Ask the helper for it.
    Enqueue(Enqueue),

    /// Leave it, and say why.
    Untouched(&'static str),
}

/// A repair that needs no privilege because everything it touches is under `MIXENGINE_HOME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InHome {
    /// Restrict the home to its owner again.
    RestrictHome,

    /// Install what these rows render to.
    RenderConfiguration,

    /// Adopt or stop the rows nothing is supervising.
    ReconcileStrandedRows,
}

/// A repair only the elevated helper can make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Enqueue {
    /// The managed hosts block.
    Hosts,

    /// The resolver for this home's managed TLDs.
    Resolver,

    /// Permission for the front end to answer on 80 and 443.
    PortAccess,

    /// Nothing new: the queue itself was the condition.
    AlreadyWaiting,
}

/// What repairing `id` means.
///
/// **No wildcard arm, and that is the point.** See this module's header.
fn plan_for(id: ProblemId) -> Planned {
    match id {
        ProblemId::HostsBlockDiffers => Planned::Enqueue(Enqueue::Hosts),
        ProblemId::ResolverNotWired => Planned::Enqueue(Enqueue::Resolver),
        ProblemId::PortAccessMissing => Planned::Enqueue(Enqueue::PortAccess),
        ProblemId::PermissionPending => Planned::Enqueue(Enqueue::AlreadyWaiting),

        ProblemId::HomePermissionsLost => Planned::InHome(InHome::RestrictHome),
        ProblemId::GeneratedConfigStale => Planned::InHome(InHome::RenderConfiguration),
        ProblemId::ServiceUnsupervised => Planned::InHome(InHome::ReconcileStrandedRows),

        ProblemId::DomainUnreachable => Planned::Untouched(
            "a name resolves once the hosts block and the resolver are what they should be, and \
             this same call repairs both when they were wrong",
        ),
        ProblemId::PortRangeReserved => Planned::Untouched(
            "this system reserved the range out from under everything, and MixEngine cannot \
             un-reserve it",
        ),
        ProblemId::DnsServerUnavailable => Planned::Untouched(
            "this build has no way to bind the DNS server again once it has failed to",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole table, asserted as a table: every condition this build can report has an arm, and
    /// three of them — and only those three — say why nothing was done.
    ///
    /// A `ProblemId` added later fails to compile in `plan_for` before it ever reaches this test,
    /// which is the order that matters: the compiler is the check, and this is the record of what
    /// the compiler was told.
    #[test]
    fn every_condition_has_an_arm_and_only_the_three_untouched_ones_say_why() {
        for id in [
            ProblemId::HostsBlockDiffers,
            ProblemId::ResolverNotWired,
            ProblemId::DnsServerUnavailable,
            ProblemId::PortAccessMissing,
            ProblemId::PermissionPending,
            ProblemId::DomainUnreachable,
            ProblemId::HomePermissionsLost,
            ProblemId::PortRangeReserved,
            ProblemId::GeneratedConfigStale,
            ProblemId::ServiceUnsupervised,
        ] {
            let planned = plan_for(id);

            let untouched = matches!(
                id,
                ProblemId::DomainUnreachable
                    | ProblemId::PortRangeReserved
                    | ProblemId::DnsServerUnavailable
            );

            match (untouched, planned) {
                (true, Planned::Untouched(because)) => {
                    assert!(!because.is_empty(), "{id:?} was left with no reason")
                }
                (true, other) => panic!("{id:?} has no repair in this build but planned {other:?}"),
                (false, Planned::Untouched(_)) => panic!("{id:?} has a repair and was left alone"),
                (false, _) => {}
            }
        }
    }

    /// Nothing repaired inside the home may want the helper. A prompt for something under
    /// `MIXENGINE_HOME` would be a prompt for a directory this account already owns.
    #[test]
    fn nothing_repaired_inside_the_home_wants_the_helper() {
        for id in [
            ProblemId::HomePermissionsLost,
            ProblemId::GeneratedConfigStale,
            ProblemId::ServiceUnsupervised,
        ] {
            assert!(
                matches!(plan_for(id), Planned::InHome(_)),
                "{id:?} would raise a prompt for something inside the home"
            );
        }
    }

    /// And the other direction: every condition that is repaired at all is repaired by exactly one
    /// of the two mechanisms, so a reader of the table cannot be left wondering which.
    #[test]
    fn a_repairable_condition_is_either_in_the_home_or_in_the_queue() {
        for id in [
            ProblemId::HostsBlockDiffers,
            ProblemId::ResolverNotWired,
            ProblemId::PortAccessMissing,
            ProblemId::PermissionPending,
        ] {
            assert!(
                matches!(plan_for(id), Planned::Enqueue(_)),
                "{id:?} is repaired by the helper and the table says otherwise"
            );
        }
    }
}
