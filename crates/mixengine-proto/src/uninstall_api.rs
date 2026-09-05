//! What `daemon.uninstall_plan` and `daemon.uninstall` answer — roadmap task **T87**.
//!
//! **A list of things that can be there, and not a list of things that were.** A report that
//! printed only what it removed would leave a person unable to tell *"there was no resolver
//! wiring"* from *"the resolver wiring was not looked at"* — which on the one command whose whole
//! promise is that nothing is left behind is the difference between an answer and a shrug. So every
//! row appears, whatever it answered, exactly as [`DoctorReport`](crate::DoctorReport) does and for
//! its reason.
//!
//! **It is a measurement and not a claim.** `daemon.uninstall` reads the machine again after the
//! grant and sets each [`Removal`] from what it finds; the helper is honest about what it did, but
//! it is a separate process describing finished work (the T87 design, D3).
//!
//! **[`Removal`] and not `Disposition`**, which is the word this document reached for first: this
//! crate already exports a `Disposition` — a blueprint's — and a flat re-export has room for one.

/// What both uninstall methods take.
///
/// One type for two methods, on [`PathReport`](crate::PathReport)'s precedent: they ask the same
/// question of the same machine and differ only in whether they act on the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UninstallQuery {
    /// Leave `MIXENGINE_HOME` where it is, and undo only what is outside it.
    ///
    /// **Defaults to `false`**, which is the complete removal the command exists for. The flag is
    /// here because a home holds `data/` — the MySQL and Postgres instances somebody may have spent
    /// a week filling — and a product whose only exit destroys them is one people do not try in the
    /// first place (the T87 design, D10).
    #[serde(default)]
    pub keep_home: bool,

    /// Flush the elevation queue in this same call, raising the one prompt.
    ///
    /// **Defaults to `false`**, and ignored by `daemon.uninstall_plan`, which raises nothing. The
    /// two-call path is T64's rule: what is about to be allowed is read before it is allowed.
    #[serde(default)]
    pub grant: bool,
}

/// What an uninstall found, and what became of each thing.
///
/// **No `granting` field, where [`RepairReport`](crate::RepairReport) has one.** That method raises
/// its prompt as a job of its own and hands the caller its id; this one *is* a job, and raises the
/// prompt inside itself — so the job a caller would be pointed at is the job they are already
/// following. A field that could only ever be null is a field every client has to handle for
/// nothing. What is waiting when no prompt was raised is on the rows, as
/// [`Removal::Enqueued`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UninstallReport {
    /// One entry per thing MixEngine can have written, in a fixed order, whatever each answered.
    ///
    /// Eleven of the twelve ids appear exactly once. [`ResidueId::RelocatedDirectory`] appears once
    /// per directory `[paths]` has moved out of the root, and on an ordinary home not at all.
    pub items: Vec<Residue>,
}

impl UninstallReport {
    /// Is anything still there that was supposed to go?
    ///
    /// **What `mix uninstall`'s exit code is.** [`Removal::OnExit`] and [`Removal::OnRestart`] are
    /// deliberately not failures: one is a removal this process is in the middle of performing and
    /// the other is one the operating system has accepted and will perform. Counting either would
    /// report every Windows run as a failure, because Windows cannot unlink the image the helper is
    /// running from (the T87 design, D8).
    #[must_use]
    pub fn left_behind(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item.outcome, Removal::Failed { .. }))
    }
}

/// One thing MixEngine can have written, and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Residue {
    /// A stable name for the thing, which a client renders and a test asserts on.
    pub id: ResidueId,

    /// What it is, phrased for a person: "the managed hosts block".
    pub what: String,

    /// Where it is, phrased for a person: `/etc/hosts`, `HKEY_CURRENT_USER\Environment\Path`, a
    /// name in the Task Scheduler library.
    ///
    /// A string and not a `PathBuf` for [`DaemonStatus`](crate::DaemonStatus)' reason — serde
    /// refuses a `PathBuf` that is not valid UTF-8 — and here it is often not a path at all.
    pub location: String,

    /// What became of it.
    pub outcome: Removal,
}

/// The things this build knows how to take off a machine.
///
/// **Closed rather than a string**, on [`ProblemId`](crate::ProblemId)'s rule: a client keying off a
/// spelling is a client that silently stops matching, and a row nothing produces does not compile.
///
/// `Hash` and `Ord` because the daemon keys a map on it while it works through the list, and a test
/// sorts it to prove no two variants share a spelling. Both are free on a fieldless enum.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ResidueId {
    /// The block MixEngine keeps in this machine's hosts file.
    HostsBlock,

    /// Whatever sends a managed TLD to this daemon's own DNS server.
    ResolverWiring,

    /// The capability, or the packet-filter redirect and the boot-time job that enables it.
    PortAccess,

    /// The inbound rules a shared site needed.
    FirewallRules,

    /// MixEngine's certificate authority in this machine's own store.
    TrustStore,

    /// The same authority in Firefox's and Chrome's databases, which are a different store.
    BrowserTrust,

    /// `mixengine-elevate`, in the one directory an ordinary account cannot write.
    PrivilegedHelper,

    /// The root-owned record of what ran as root, outside `MIXENGINE_HOME`.
    AuditLog,

    /// The entry that starts this home's daemon at login.
    AutostartEntry,

    /// `<root>/bin` on this user's `PATH`.
    PathEntry,

    /// `MIXENGINE_HOME` itself.
    Home,

    /// A directory `[paths]` in `config.toml` has moved out of the root.
    ///
    /// **Its own id rather than a second path hidden inside [`Home`](Self::Home)**: the client reads
    /// these back one by one once the daemon is gone, and a row is what it reads back.
    RelocatedDirectory,
}

impl ResidueId {
    /// Every one of them, so a test can assert the wire spellings are distinct.
    ///
    /// Written out rather than derived: a list a macro produced would be as wrong as the enum on the
    /// day somebody gave two variants one `rename`, which is the mistake it exists to catch.
    pub const ALL: &'static [Self] = &[
        Self::HostsBlock,
        Self::ResolverWiring,
        Self::PortAccess,
        Self::FirewallRules,
        Self::TrustStore,
        Self::BrowserTrust,
        Self::PrivilegedHelper,
        Self::AuditLog,
        Self::AutostartEntry,
        Self::PathEntry,
        Self::Home,
        Self::RelocatedDirectory,
    ];
}

/// What became of one thing on the list.
///
/// **Internally tagged**, so a client matches on a word rather than working out which fields
/// arrived — [`Outcome`](crate::Outcome)'s rule.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "removal", rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Removal {
    /// Nothing of ours is there. The ordinary answer on most of this list, on most machines.
    ///
    /// An empty struct variant rather than a unit one, for [`Outcome::Ok`](crate::Outcome::Ok)'s
    /// reason: a unit variant of an internally tagged enum is read through `deserialize_any`, where
    /// `deny_unknown_fields` never gets a chance to fire.
    Absent {},

    /// What would be done. The only acting answer `daemon.uninstall_plan` gives.
    Planned {
        /// What the real run would do, in a sentence.
        ///
        /// For everything that needs the helper this is
        /// [`PrivilegedOp::describe`](crate::privileged::PrivilegedOp::describe)'s own sentence, so
        /// that the plan and the elevation prompt say the same thing about the same operation.
        how: String,
    },

    /// Gone — read back off the machine afterwards rather than claimed.
    Removed {
        /// What was done, in a sentence.
        what: String,
    },

    /// Waiting on a prompt nobody has answered yet, because this call was not asked to raise one.
    Enqueued {
        /// What is waiting, in a sentence.
        what: String,
    },

    /// Goes when this daemon exits, which it is about to do.
    ///
    /// **`MIXENGINE_HOME`'s answer, and only ever its.** A process cannot measure the removal of the
    /// directory holding the database it has open; `mix` reads these paths back once the daemon is
    /// gone, which is what makes this command's exit code mean anything (the T87 design, D9).
    OnExit {
        /// What goes, in a sentence.
        what: String,
    },

    /// Goes at the next restart of this machine, and why it could not go now.
    ///
    /// **Windows' answer for the privileged helper, and nothing else's.** A file with a mapped image
    /// section cannot be unlinked, and the helper is running when it is asked to remove itself — so
    /// the operating system's own removal queue is what takes it (the T87 design, D8).
    OnRestart {
        /// What is scheduled and why it had to be, in a sentence.
        what: String,
    },

    /// Deliberately left, because the caller asked for it to be.
    Kept {
        /// Why it was left, in a sentence.
        because: String,
    },

    /// It was acted on and it is still there.
    Failed {
        /// What the machine said, and the fact that the thing is still there.
        because: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The outcome is tagged, so a client matches on a word rather than on which fields arrived —
    /// `Outcome`'s rule in `doctor_api`, and for its reason.
    #[test]
    fn a_residue_travels_tagged_and_names_what_and_where() {
        let residue = Residue {
            id: ResidueId::HostsBlock,
            what: "the managed hosts block".to_owned(),
            location: "/etc/hosts".to_owned(),
            outcome: Removal::Planned {
                how: "clear MixEngine's block".to_owned(),
            },
        };

        let wire = serde_json::to_string(&residue).expect("a residue serialises");

        assert_eq!(
            wire,
            r#"{"id":"hosts_block","what":"the managed hosts block","location":"/etc/hosts","outcome":{"removal":"planned","how":"clear MixEngine's block"}}"#
        );
    }

    /// `Absent` carries nothing and must still be spellable in both directions.
    #[test]
    fn an_absent_residue_round_trips() {
        let wire = r#"{"id":"audit_log","what":"the audit log","location":"/var/log/mixengine/elevate.log","outcome":{"removal":"absent"}}"#;

        let residue: Residue = serde_json::from_str(wire).expect("a residue");

        assert_eq!(residue.outcome, Removal::Absent {});
        assert_eq!(serde_json::to_string(&residue).expect("back"), wire);
    }

    /// The exit code of `mix uninstall` is this function and nothing else. `OnExit` is a removal
    /// this process is performing and `OnRestart` is one the operating system has accepted;
    /// counting either as a failure would report every Windows run as a failed uninstall.
    #[test]
    fn only_a_failure_is_something_left_behind() {
        let mut report = UninstallReport {
            items: vec![
                residue(Removal::Removed {
                    what: "gone".to_owned(),
                }),
                residue(Removal::OnExit {
                    what: "with this daemon".to_owned(),
                }),
                residue(Removal::OnRestart {
                    what: "at the next restart".to_owned(),
                }),
                residue(Removal::Absent {}),
                residue(Removal::Kept {
                    because: "you asked".to_owned(),
                }),
                residue(Removal::Enqueued {
                    what: "waiting".to_owned(),
                }),
            ],
        };

        assert!(!report.left_behind());

        report.items.push(residue(Removal::Failed {
            because: "the store still holds it".to_owned(),
        }));

        assert!(report.left_behind());
    }

    /// Every id is spelled in `snake_case` on the wire, and each is its own word: `mix` matches on
    /// these, and a repeated spelling is two rows a renderer cannot tell apart.
    #[test]
    fn every_id_has_its_own_spelling() {
        let spellings: Vec<String> = ResidueId::ALL
            .iter()
            .map(|id| serde_json::to_string(id).expect("an id"))
            .collect();

        let mut unique = spellings.clone();
        unique.sort();
        unique.dedup();

        assert_eq!(unique.len(), spellings.len(), "{spellings:?}");
        assert_eq!(spellings.len(), 12);
    }

    /// A query that leaves both fields out is the ordinary one, and both default to the safe
    /// direction: keep nothing back, and raise no prompt the caller has not shown anybody.
    #[test]
    fn a_query_with_no_fields_is_the_complete_removal_that_asks_first() {
        let query: UninstallQuery = serde_json::from_str("{}").expect("no options is a shape");

        assert!(!query.keep_home);
        assert!(!query.grant);

        serde_json::from_str::<UninstallQuery>(r#"{"nonsense":true}"#)
            .expect_err("an unknown field is an error, never a warning");
    }

    fn residue(outcome: Removal) -> Residue {
        Residue {
            id: ResidueId::HostsBlock,
            what: "a thing".to_owned(),
            location: "somewhere".to_owned(),
            outcome,
        }
    }
}
