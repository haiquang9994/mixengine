//! `daemon.uninstall_plan` and `daemon.uninstall` — roadmap task **T87**.
//!
//! **Two methods on `daemon.doctor`/`daemon.doctor_repair`'s split, and not one flag.** The plan is
//! a read in the strict sense — no row written, nothing enqueued, no prompt possible — which is what
//! makes it safe to put in front of the one command that cannot be undone. The act is a job, because
//! it can raise the elevation prompt and what that waits on is a person reading a dialog.
//!
//! Both build their list from [`inventory::take`], which is the one enumeration: two of them, one
//! for the dry run and one for the real run, is the second inventory the roadmap sentence refuses.

mod inventory;

use std::sync::Arc;

use mixengine_proto::privileged::PrivilegedOp;
use mixengine_proto::{Error, Removal, Residue, ResidueId, UninstallQuery, UninstallReport};

use crate::api::Armed;

/// The doors an uninstall reaches this daemon's own subsystems through.
///
/// One argument rather than seven, on [`Supervision`](crate::api::Supervision)'s rule: a constructor
/// whose arguments have to be *counted* is one a caller gets wrong silently, and every one of these
/// is the only door into the thing it holds.
pub(crate) struct Doors {
    /// Where the DNS server is listening, which is what a resolver would have been pointed at.
    pub(crate) dns: Arc<crate::dns::Dns>,

    /// The front end's program path, which is what the port-access reading is about.
    pub(crate) services: Arc<crate::services::Registry>,

    /// `<root>/bin` and this user's PATH.
    pub(crate) shims: Arc<crate::shims::Shims>,

    /// The login entry.
    pub(crate) autostart: Arc<crate::autostart::Autostart>,

    /// The queue, and the one prompt an uninstall raises.
    pub(crate) elevation: Arc<crate::elevation::Elevation>,

    /// This home's authority, for the browsers it was put into.
    pub(crate) certificates: crate::certs::Certificates,

    /// Where the home's own directories are left for `main` to remove on the way out.
    pub(crate) armed: Arc<Armed>,
}

/// Both halves of the uninstall.
///
/// **Holding one set of readers**, so the plan and the act cannot disagree about what is on this
/// machine — and the same readers `Doctor` is given, reached through the door that already owns
/// each. A second `Host` here would be a second answer to *what does this machine's hosts file
/// hold*.
#[derive(Debug)]
pub(crate) struct Uninstall {
    /// The rows, for the one question this asks of them: is anything shared?
    store: mixengine_core::Store,

    /// This machine: its hosts file, its resolver, its trust store, its port access, its browsers.
    host: Arc<dyn mixengine_platform::Host>,

    /// Where the DNS server is listening, which is what a resolver would have been pointed at.
    dns: Arc<crate::dns::Dns>,

    /// The front end's program path, which is what the port-access reading is about.
    services: Arc<crate::services::Registry>,

    /// `<root>/bin` and this user's PATH — the first of the two things outside the home that need no
    /// token.
    shims: Arc<crate::shims::Shims>,

    /// The login entry — the second of them.
    autostart: Arc<crate::autostart::Autostart>,

    /// The queue, and the one prompt this raises.
    elevation: Arc<crate::elevation::Elevation>,

    /// This home's authority, for the browsers it was put into — the third unprivileged thing.
    certificates: crate::certs::Certificates,

    /// Where the home's own directories are left for `main` to remove on the way out.
    armed: Arc<Armed>,

    /// The home's own layout: where its authority is, and every directory it owns.
    paths: mixengine_core::Paths,
}

impl Uninstall {
    /// The one of these the API holds.
    ///
    /// **The home's layout rather than its root and its certificate directory separately**, on
    /// `Doctor::new`'s rule: both are derived from it, and passing them apart lets a caller hand
    /// this a certificate directory from one home and a root from another.
    pub(crate) fn new(
        store: &mixengine_core::Store,
        host: Arc<dyn mixengine_platform::Host>,
        doors: Doors,
        paths: &mixengine_core::Paths,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: store.clone(),
            host,
            dns: doors.dns,
            services: doors.services,
            shims: doors.shims,
            autostart: doors.autostart,
            elevation: doors.elevation,
            certificates: doors.certificates,
            armed: doors.armed,
            paths: paths.clone(),
        })
    }

    /// `daemon.uninstall_plan` — everything an uninstall would take off this machine.
    ///
    /// **A read, and every branch of it.** Nothing here writes a row, enqueues an operation or can
    /// raise a prompt, which is what `daemon.doctor` established the shape for and what makes this
    /// safe to call from a client that is only showing somebody what would happen.
    ///
    /// # Errors
    ///
    /// The wire error of a home whose layout could not be read. A *machine* that could not be read
    /// is never an error: it is [`Removal::Failed`] on the row it
    /// is about, because an uninstall that reported "nothing there" for a question nobody could
    /// answer is the one failure this feature exists to prevent.
    pub(crate) async fn plan(&self, query: &UninstallQuery) -> Result<UninstallReport, Error> {
        Ok(UninstallReport {
            items: self.rows(query).await?,
        })
    }

    /// `daemon.uninstall` — the job that takes MixEngine off this machine.
    ///
    /// **Unprivileged first, privileged once, home last** (the T87 design, D7). This user's own
    /// things are undone before the prompt, so a declined grant still leaves the browsers, the
    /// `PATH` and the login entry clean; everything needing the helper goes into one batch, so there
    /// is one dialog; and the home goes last because the daemon is running out of it.
    ///
    /// **Nothing stops a service here, and nothing in the home is removed here.** Both happen on the
    /// way out: the home's directories are *armed*, and `mixengined` removes them after its own
    /// shutdown has stopped every service in dependency order within the configured grace and
    /// `Store::close` has checkpointed the write-ahead log. A stop walk of this method's own would be
    /// a second answer to a question `daemon.shutdown` already answers better.
    ///
    /// # Errors
    ///
    /// The wire error of a home whose layout could not be read, or of a queue that could not be
    /// written. **A declined prompt is not an error** — ADR 0005 — and neither is a machine that
    /// could not be read: both are rows.
    pub(crate) async fn run(
        &self,
        query: &UninstallQuery,
        handle: &crate::jobs::JobHandle,
    ) -> Result<UninstallReport, Error> {
        handle.progress(5, "reading what this machine holds").await;
        let planned = self.rows(query).await?;

        handle
            .progress(
                20,
                "taking this home out of your PATH, your login and your browsers",
            )
            .await;
        let mut done = self.undo_what_needs_no_token(&planned).await;

        handle.progress(45, "asking for permission").await;
        let asked = self.ask_for_the_rest(&planned).await?;

        // **Only when something is waiting.** `elevation.grant` refuses an empty queue outright, and
        // raising a dialog to discover that would be a prompt for nothing.
        let granted = match query.grant && asked > 0 {
            true => Some(self.elevation.grant_within(handle).await),
            false => None,
        };

        if let Some(Err(error)) = &granted {
            // Logged and carried, never returned: what the machine holds now is read below, and a
            // grant that failed against a store that turns out not to hold the authority anyway is
            // still a removal — `cert.ca_uninstall`'s rule, and the reason this method measures.
            tracing::warn!(%error, "the grant an uninstall raised did not finish");
        }

        handle.progress(80, "reading this machine back").await;
        let measured = self.rows(query).await?;
        let waiting = self.elevation.status().await?.pending;

        let mut items = Vec::with_capacity(planned.len());
        for (before, after) in planned.into_iter().zip(measured) {
            items.push(match done.remove(&before.id) {
                // One of the three that needed no token, acted on above.
                Some(outcome) => Residue { outcome, ..after },

                // One of the seven the helper answers for, settled against the second reading.
                None if needs_the_helper(before.id) => {
                    settle(before, after, granted.is_some(), &waiting)
                }

                // **The home, a relocated directory, or a row that had nothing to do.** `settle` may
                // not be let near these: it reads a row that is still `Planned` as one the helper was
                // asked about and did not manage, and the home is `Planned` right up until
                // `arm_the_home` rewrites it two statements below — so settling it turned every
                // complete uninstall into one that reported the home as waiting for a prompt, and
                // then kept the home because of it. Found by CI on 2026-09-04.
                None => after,
            });
        }

        // **The home is kept when the grant did not finish, whatever was asked for.** A home removed
        // while this machine still routes `.test` to a daemon that no longer exists is the worst of
        // the states available, and it is one nobody could repair without the home that knew how.
        let unfinished = matches!(granted, Some(Err(_)))
            || items.iter().any(|item| {
                matches!(
                    item.outcome,
                    Removal::Failed { .. } | Removal::Enqueued { .. }
                )
            });

        if !query.keep_home {
            handle
                .progress(95, "arming this home's own directories")
                .await;
            self.arm_the_home(&mut items, unfinished);
        }

        Ok(UninstallReport { items })
    }

    /// The three things outside the home that belong to this account rather than to the machine.
    ///
    /// **Before the prompt, and complete on their own.** None needs a token, so a person who then
    /// declines the dialog still ends up with their `PATH`, their login and their browsers clean —
    /// which is the half of an uninstall that would otherwise be lost to one click.
    ///
    /// Answers only the rows it acted on; everything else is settled from the second reading.
    async fn undo_what_needs_no_token(
        &self,
        planned: &[Residue],
    ) -> std::collections::HashMap<ResidueId, Removal> {
        let mut done = std::collections::HashMap::new();

        if planned_row(planned, ResidueId::PathEntry) {
            let shims = Arc::clone(&self.shims);
            done.insert(
                ResidueId::PathEntry,
                match crate::api::on_a_blocking_thread(move || shims.uninstall()).await {
                    Ok(report) => Removal::Removed {
                        what: format!("{} is no longer on your PATH", report.directory),
                    },
                    Err(error) => Removal::Failed {
                        because: format!("your PATH could not be written: {error}"),
                    },
                },
            );
        }

        if planned_row(planned, ResidueId::AutostartEntry) {
            let autostart = Arc::clone(&self.autostart);
            done.insert(
                ResidueId::AutostartEntry,
                match crate::api::on_a_blocking_thread(move || autostart.disable()).await {
                    Ok(report) => Removal::Removed {
                        what: format!("{} no longer starts this home at login", report.location),
                    },
                    Err(error) => Removal::Failed {
                        because: format!("the login entry could not be removed: {error}"),
                    },
                },
            );
        }

        if planned_row(planned, ResidueId::BrowserTrust)
            && let Ok(mixengine_proto::CaState::Present { ca }) =
                self.certificates.authority().await
        {
            let change = self.certificates.remove_from_browsers(&ca.key_id).await;

            done.insert(
                ResidueId::BrowserTrust,
                match change.refused.is_empty() {
                    true => Removal::Removed {
                        what: format!(
                            "{} browser database(s) no longer hold it",
                            change.written.len()
                        ),
                    },
                    false => Removal::Failed {
                        because: change.refused.join("; "),
                    },
                },
            );
        }

        done
    }

    /// Put everything that needs the helper in the queue, and say how many rows that was.
    ///
    /// **In the order the batch is applied**, which the queue preserves: the log's own removal goes
    /// in last, so the last thing it ever records is the removal of the binary that writes it.
    ///
    /// **Only the rows that said `Planned`.** A machine with no resolver mechanism, or one holding
    /// none of this home's wiring, asks for nothing — a row whose only possible outcome is
    /// `AlreadyDone` is a row that makes a dialog longer for no reason.
    async fn ask_for_the_rest(&self, planned: &[Residue]) -> Result<usize, Error> {
        let mut asked = 0;

        for id in [
            ResidueId::HostsBlock,
            ResidueId::ResolverWiring,
            ResidueId::PortAccess,
            ResidueId::FirewallRules,
            ResidueId::TrustStore,
            ResidueId::PrivilegedHelper,
            ResidueId::AuditLog,
        ] {
            if !planned_row(planned, id) {
                continue;
            }

            let Some(op) = self.operation_for(id).await else {
                continue;
            };

            self.elevation.enqueue(&op).await?;
            asked += 1;
        }

        Ok(asked)
    }

    /// The operation that undoes one row, rebuilt from the same probe the inventory read.
    ///
    /// **Rebuilt rather than carried on the row.** A `Residue` is a wire type a client renders, and
    /// putting a `PrivilegedOp` on it would put the elevation protocol into the uninstall report —
    /// where a client could read it, and where two clients could disagree about what a row means.
    async fn operation_for(&self, id: ResidueId) -> Option<PrivilegedOp> {
        match id {
            // Whole state: the empty block *is* the removal.
            ResidueId::HostsBlock => Some(PrivilegedOp::hosts_apply(Vec::new())),

            ResidueId::ResolverWiring => {
                let port = self.dns.wirable_port()?;
                let want: Vec<&str> = mixengine_proto::domains::WIRED_TLDS.to_vec();

                self.host
                    .resolver()
                    .probe(&want, port)
                    .ok()?
                    .target()
                    .map(|target| PrivilegedOp::ResolverRevoke { target })
            }

            ResidueId::PortAccess => {
                let binary = self.services.front_end_program().await?;

                self.host
                    .port_access()
                    .probe(&binary, &crate::elevation::Elevation::ANSWERING)
                    .ok()?
                    .target(&binary)
                    .map(|target| PrivilegedOp::PortAccessRevoke { target })
            }

            // Whole state again: the empty plan is the revoke, which is why T74 shipped no
            // `FirewallRevoke` beside it.
            ResidueId::FirewallRules => Some(PrivilegedOp::FirewallApply {
                plan: mixengine_proto::privileged::FirewallPlan {
                    ports: Vec::new(),
                    label: inventory::firewall_label(),
                },
            }),

            ResidueId::TrustStore => {
                let mixengine_proto::CaState::Present { ca } =
                    self.certificates.authority().await.ok()?
                else {
                    return None;
                };

                let der = mixengine_core::certs::ca::der(&ca.certificate_pem)?;

                self.host
                    .trust_store()
                    .probe(&der)
                    .ok()?
                    .target(&ca.key_id)
                    .map(|target| PrivilegedOp::TrustCaRemove { target })
            }

            ResidueId::PrivilegedHelper => Some(PrivilegedOp::HelperRemove {}),

            // Last, always: `mixengine-elevate` applies it after everything else in the batch and
            // records nothing for it, because the line would recreate the file.
            ResidueId::AuditLog => Some(PrivilegedOp::AuditLogRemove {}),

            // The three that need no token, and the two that are the home itself.
            ResidueId::BrowserTrust
            | ResidueId::AutostartEntry
            | ResidueId::PathEntry
            | ResidueId::Home
            | ResidueId::RelocatedDirectory => None,
        }
    }

    /// Remove what can go now, and leave the rest for `mixengined` to remove as it exits.
    ///
    /// **Nothing is removed here at all, in fact**, and that is deliberate rather than an economy:
    /// this daemon still has services running, a database open and a socket in `run/`. What this does
    /// is record the directories, and the process removes them on the way out — after its own
    /// shutdown has stopped every service and after `Store::close` has checkpointed the log.
    fn arm_the_home(&self, items: &mut [Residue], unfinished: bool) {
        let mut paths = Vec::new();

        for item in items.iter_mut() {
            if !matches!(item.id, ResidueId::Home | ResidueId::RelocatedDirectory) {
                continue;
            }

            item.outcome = match unfinished {
                true => Removal::Kept {
                    because:
                        "something outside this home is still there, and a home removed while \
                              this machine is still wired for it is one nothing could repair — run \
                              `mix uninstall` again once the rows above are clear"
                            .to_owned(),
                },
                false => {
                    paths.push(std::path::PathBuf::from(&item.location));

                    Removal::OnExit {
                        what: "this daemon removes it as it exits, which it is about to do"
                            .to_owned(),
                    }
                }
            };
        }

        if !paths.is_empty() {
            self.armed.arm(paths);
        }
    }

    /// The inventory, taken once.
    async fn rows(&self, query: &UninstallQuery) -> Result<Vec<Residue>, Error> {
        inventory::take(self, query).await
    }
}

/// Is this one of the rows the elevated helper answers for?
///
/// **The whole of what [`settle`] may be applied to.** The three that need no token are answered
/// where they are acted on, and the home is answered by `arm_the_home` — which runs *after* the
/// fold, so the home is still `Planned` when the fold sees it.
fn needs_the_helper(id: ResidueId) -> bool {
    matches!(
        id,
        ResidueId::HostsBlock
            | ResidueId::ResolverWiring
            | ResidueId::PortAccess
            | ResidueId::FirewallRules
            | ResidueId::TrustStore
            | ResidueId::PrivilegedHelper
            | ResidueId::AuditLog
    )
}

/// Did the plan say there was something of ours here?
fn planned_row(planned: &[Residue], id: ResidueId) -> bool {
    planned
        .iter()
        .any(|row| row.id == id && matches!(row.outcome, Removal::Planned { .. }))
}

/// What became of one privileged row, from the second reading rather than from what was attempted.
///
/// **`Removed` is a measurement.** The helper is honest about what it did, but it is a separate
/// process describing finished work; this is a fresh reading of the thing itself, and it costs no
/// privilege for any row on the list (the T87 design, D3).
///
/// **And `OnRestart` is settled by the queue and the disk**, not by reading the helper's sentence: an
/// operation that is no longer waiting has been applied, so a file that is nonetheless still there is
/// one the operating system accepted and has not got to yet. That is Windows and the privileged
/// helper — a running image cannot be unlinked there — and it is the only row that can answer it.
fn settle(
    before: Residue,
    after: Residue,
    granted: bool,
    waiting: &[mixengine_proto::PendingOp],
) -> Residue {
    let Removal::Planned { how } = &before.outcome else {
        // Absent, Kept or Failed before anything was attempted: the second reading is the answer.
        return after;
    };

    if matches!(after.outcome, Removal::Absent {}) {
        return Residue {
            outcome: Removal::Removed { what: how.clone() },
            ..after
        };
    }

    if !granted {
        return Residue {
            outcome: Removal::Enqueued { what: how.clone() },
            ..after
        };
    }

    let still_queued = waiting.iter().any(|pending| {
        matches!(
            (&pending.op, before.id),
            (PrivilegedOp::HostsApply { .. }, ResidueId::HostsBlock)
                | (
                    PrivilegedOp::ResolverRevoke { .. },
                    ResidueId::ResolverWiring
                )
                | (PrivilegedOp::PortAccessRevoke { .. }, ResidueId::PortAccess)
                | (PrivilegedOp::FirewallApply { .. }, ResidueId::FirewallRules)
                | (PrivilegedOp::TrustCaRemove { .. }, ResidueId::TrustStore)
                | (PrivilegedOp::HelperRemove {}, ResidueId::PrivilegedHelper)
                | (PrivilegedOp::AuditLogRemove {}, ResidueId::AuditLog)
        )
    });

    let outcome = match (still_queued, before.id) {
        (false, ResidueId::PrivilegedHelper) => Removal::OnRestart {
            what: format!(
                "this system accepted the removal and will carry it out {}, because a program \
                 cannot be deleted while it is running",
                mixengine_proto::privileged::AT_NEXT_RESTART
            ),
        },
        (false, _) => Removal::Failed {
            because: "the operation was applied and this is still here".to_owned(),
        },
        (true, _) => Removal::Failed {
            because: "this is still here, and the operation that removes it is still waiting for \
                      permission"
                .to_owned(),
        },
    };

    Residue { outcome, ..after }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The home is not the helper's business, and this is the assertion that says so.**
    ///
    /// `settle` reads a row that is still `Planned` as one the helper was asked about and did not
    /// manage — which is right for the seven it answers for and wrong for everything else. The home
    /// is `Planned` for two statements longer than the fold, because `arm_the_home` rewrites it
    /// afterwards; settled in between, every complete uninstall reported the home as waiting for a
    /// prompt and then kept it for that reason. Found by CI on 2026-09-04.
    #[test]
    fn the_helper_answers_for_seven_rows_and_the_home_is_not_one_of_them() {
        for id in [
            ResidueId::HostsBlock,
            ResidueId::ResolverWiring,
            ResidueId::PortAccess,
            ResidueId::FirewallRules,
            ResidueId::TrustStore,
            ResidueId::PrivilegedHelper,
            ResidueId::AuditLog,
        ] {
            assert!(needs_the_helper(id), "{id:?}");
        }

        for id in [
            // The three that need no token, answered where they are acted on.
            ResidueId::BrowserTrust,
            ResidueId::AutostartEntry,
            ResidueId::PathEntry,
            // And the two the daemon removes itself, answered by `arm_the_home`.
            ResidueId::Home,
            ResidueId::RelocatedDirectory,
        ] {
            assert!(!needs_the_helper(id), "{id:?}");
        }

        assert_eq!(
            ResidueId::ALL
                .iter()
                .filter(|id| needs_the_helper(**id))
                .count(),
            7,
            "every id is on exactly one side of this, and a new one has to choose"
        );
    }

    /// What `settle` does to a row that is still there and was never granted — the behaviour that is
    /// correct for a privileged row and catastrophic for the home.
    #[test]
    fn a_row_that_was_never_granted_is_still_waiting() {
        let settled = settle(planned(), planned(), false, &[]);

        assert!(
            matches!(settled.outcome, Removal::Enqueued { .. }),
            "{settled:?}"
        );
    }

    /// And one the machine no longer holds is a removal, whatever was attempted: the second reading
    /// decides, not the helper's own account of itself (the T87 design, D3).
    #[test]
    fn a_row_the_machine_no_longer_holds_is_a_removal() {
        let mut gone = planned();
        gone.outcome = Removal::Absent {};

        let settled = settle(planned(), gone, true, &[]);

        assert!(
            matches!(settled.outcome, Removal::Removed { .. }),
            "{settled:?}"
        );
    }

    /// One row, planned, for the two above to work on.
    fn planned() -> Residue {
        Residue {
            id: ResidueId::PrivilegedHelper,
            what: "MixEngine's privileged helper".to_owned(),
            location: "somewhere only an administrator can write".to_owned(),
            outcome: Removal::Planned {
                how: "remove it".to_owned(),
            },
        }
    }
}
