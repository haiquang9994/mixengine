//! What MixEngine has written on this machine — roadmap task **T87**.
//!
//! **One enumeration, and both methods call it.** `daemon.uninstall_plan` renders it;
//! `daemon.uninstall` renders it, acts, and calls it again to measure. Two enumerations — one for
//! the dry run and one for the real run — is the second inventory the roadmap sentence refuses, and
//! it is the one that would actually have been built.
//!
//! **Nothing here writes and nothing here enqueues.** Every reading is one `mix doctor` already
//! takes, through the same `mixengine-platform` capability, which is what *"reads the same
//! inventory"* means: what is shared is the **readers** and not [`DoctorReport`]. That document
//! answers *is this as it should be?* and this one asks *is any of ours there?* — and
//! [`Outcome::Ok`] does not mean the same thing across its own rows. `Ok` on the trust check means
//! the authority **is** installed; `Ok` on the hosts check means the block **matches** what the
//! sites need, which on a wired machine is an *empty* block. An uninstall driven off that would
//! remove the trust store and skip the hosts block on one machine and do the reverse on the next
//! (the T87 design, D1).
//!
//! **A reader that fails is [`Removal::Failed`] and never [`Removal::Absent`].** An uninstall that
//! reported "nothing there" because it could not look is the one failure mode this whole feature
//! exists to prevent.
//!
//! [`DoctorReport`]: mixengine_proto::DoctorReport
//! [`Outcome::Ok`]: mixengine_proto::Outcome::Ok

use std::path::Path;

use mixengine_proto::privileged::{FIREWALL_LABEL, FirewallPlan, PrivilegedOp};
use mixengine_proto::{Error, Removal, Residue, ResidueId};

use super::Uninstall;

/// The label every firewall rule MixEngine writes carries, composed the one way `sites::sharing`
/// composes it. Written here as the same expression rather than a second spelling, so a rename in
/// one place is a compile error and not a rule nobody removes.
pub(crate) fn firewall_label() -> String {
    format!("{FIREWALL_LABEL}shared sites")
}

/// Everything this machine holds of MixEngine's, in a fixed order.
///
/// The order is the order the act works in — the machine first, this user's own things next, the
/// home last — so a person reading the plan reads it in the order it will happen.
pub(crate) async fn take(
    uninstall: &Uninstall,
    query: &mixengine_proto::UninstallQuery,
) -> Result<Vec<Residue>, Error> {
    let mut rows = vec![
        hosts_block(uninstall),
        resolver_wiring(uninstall),
        port_access(uninstall).await,
        firewall_rules(uninstall).await,
    ];

    let (trust, browsers) = certificate_rows(uninstall).await;
    rows.push(trust);
    rows.push(browsers);

    rows.push(privileged_helper());
    rows.push(audit_log());
    rows.push(autostart_entry(uninstall));
    rows.push(path_entry(uninstall));
    rows.push(home(uninstall.paths.root(), query.keep_home));

    // Every directory `[paths]` has moved out of the root, in `directories()`' own order. On an
    // ordinary home there are none: `Paths::directories` answers the root's own subdirectories, and
    // only a relocation makes one of them lie somewhere else.
    let root = uninstall.paths.root().to_path_buf();
    for directory in uninstall.paths.directories() {
        if directory != root && !directory.starts_with(&root) {
            rows.push(relocated(directory, query.keep_home));
        }
    }

    Ok(rows)
}

/// **1.** The block in this machine's hosts file, read the way T41 reads it.
fn hosts_block(uninstall: &Uninstall) -> Residue {
    let file = uninstall.host.hosts_file();
    let location = file.path().display().to_string();

    let outcome = match file.managed() {
        // An empty block is a machine that has never had one, which is not a failure — the trait's
        // own words.
        Ok(entries) if entries.is_empty() => Removal::Absent {},
        Ok(entries) => Removal::Planned {
            how: PrivilegedOp::hosts_apply(Vec::new()).describe()
                + &format!(", which today holds {} name(s)", entries.len()),
        },
        Err(error) => Removal::Failed {
            because: format!(
                "this machine's hosts file could not be read, so what is in it is unknown: {}",
                mixengine_proto::flatten(&error)
            ),
        },
    };

    Residue {
        id: ResidueId::HostsBlock,
        what: "the managed hosts block".to_owned(),
        location,
        outcome,
    }
}

/// **2.** Whatever sends a managed TLD to this daemon's own DNS server — T45's probe.
///
/// **Asked about the port the server is on, or about nothing.** A home whose DNS server never bound
/// has no wiring of its own to find, and a probe against a port nothing listens on would be asking
/// about somebody else's.
fn resolver_wiring(uninstall: &Uninstall) -> Residue {
    let what = "the DNS routing for this home's managed names".to_owned();

    let Some(port) = uninstall.dns.wirable_port() else {
        return Residue {
            id: ResidueId::ResolverWiring,
            what,
            location: "no DNS server of this home's is listening, so nothing can be routed to one"
                .to_owned(),
            outcome: Removal::Absent {},
        };
    };

    let want: Vec<&str> = mixengine_proto::domains::WIRED_TLDS.to_vec();

    let state = match uninstall.host.resolver().probe(&want, port) {
        Ok(state) => state,
        Err(error) => {
            return Residue {
                id: ResidueId::ResolverWiring,
                what,
                location: "this machine's resolver configuration".to_owned(),
                outcome: Removal::Failed {
                    because: format!(
                        "this machine's resolver could not be read, so what it routes is unknown: \
                         {}",
                        mixengine_proto::flatten(&error)
                    ),
                },
            };
        }
    };

    let location = resolver_place(state.method).to_owned();

    // **`wired` and not `method`.** A machine with a mechanism and nothing routed through it has
    // nothing of ours to remove, and asking to revoke there would spend a prompt on an operation
    // whose only outcome is `AlreadyDone` — T41's D11, one capability along.
    let outcome = match (state.wired.is_empty(), state.target()) {
        (false, Some(target)) => Removal::Planned {
            how: PrivilegedOp::ResolverRevoke { target }.describe(),
        },
        _ => Removal::Absent {},
    };

    Residue {
        id: ResidueId::ResolverWiring,
        what,
        location,
        outcome,
    }
}

/// **3.** The capability, or the packet-filter redirect with its anchor and its boot-time job.
async fn port_access(uninstall: &Uninstall) -> Residue {
    let what = "permission for this home's front end to answer on 80 and 443".to_owned();

    let Some(binary) = uninstall.services.front_end_program().await else {
        return Residue {
            id: ResidueId::PortAccess,
            what,
            location: "this home has no front end, so nothing was granted for one".to_owned(),
            outcome: Removal::Absent {},
        };
    };

    let state = match uninstall
        .host
        .port_access()
        .probe(&binary, &crate::elevation::Elevation::ANSWERING)
    {
        Ok(state) => state,
        Err(error) => {
            return Residue {
                id: ResidueId::PortAccess,
                what,
                location: binary.display().to_string(),
                outcome: Removal::Failed {
                    because: format!(
                        "this machine could not be asked what it has granted: {}",
                        mixengine_proto::flatten(&error)
                    ),
                },
            };
        }
    };

    let location = match state.method {
        mixengine_platform::PortAccessMethod::Capability => binary.display().to_string(),
        mixengine_platform::PortAccessMethod::Redirect => {
            "this machine's packet filter: its anchor, its block in /etc/pf.conf and the boot-time \
             job that enables it"
                .to_owned()
        }
        mixengine_platform::PortAccessMethod::Direct => {
            "this system reserves no port below 1024, so nothing was ever granted".to_owned()
        }
    };

    // **`granted` and not `method`**, on the resolver row's reasoning: a system that grants nothing
    // and a system that has not granted it both have nothing of ours to take away.
    let outcome = match (state.granted, state.target(&binary)) {
        (true, Some(target)) => Removal::Planned {
            how: PrivilegedOp::PortAccessRevoke { target }.describe(),
        },
        _ => Removal::Absent {},
    };

    Residue {
        id: ResidueId::PortAccess,
        what,
        location,
        outcome,
    }
}

/// **4.** The inbound rules a shared site needed — T74's, and the one row read from the rows.
///
/// **Read from this home's own sites rather than from the machine.** `FirewallRules` on the `Host`
/// answers what rules exist under *any* name, which is what `mix doctor` uses to report the
/// every-port rule Windows writes for `mixengined.exe` — a rule MixEngine did not make and must not
/// remove. What this home wrote is what this home shared, and the plan is whole-state either way:
/// the empty plan *is* the revoke.
async fn firewall_rules(uninstall: &Uninstall) -> Residue {
    let what = "the firewall rules a shared site needed".to_owned();
    let location = firewall_label();

    let outcome = match mixengine_core::sites::records(&uninstall.store, None).await {
        Ok(records) if records.iter().any(|record| record.sharing.is_some()) => Removal::Planned {
            how: PrivilegedOp::FirewallApply {
                plan: FirewallPlan {
                    ports: Vec::new(),
                    label: firewall_label(),
                },
            }
            .describe(),
        },
        Ok(_) => Removal::Absent {},
        Err(error) => Removal::Failed {
            because: format!(
                "this home's sites could not be read, so whether anything is shared is unknown: {}",
                mixengine_proto::flatten(&error)
            ),
        },
    };

    Residue {
        id: ResidueId::FirewallRules,
        what,
        location,
        outcome,
    }
}

/// **5 and 6.** The authority, in this machine's own store and in its browsers.
///
/// One function for two rows because they share one reading — the authority on disk — and taking it
/// twice would be two answers to *what is this home's certificate a moment apart*. They are separate
/// rows because they are separate stores, repaired and removed by different mechanisms: one needs a
/// token and the other does not, which is the line T49 was split on.
async fn certificate_rows(uninstall: &Uninstall) -> (Residue, Residue) {
    let trust_what = "MixEngine's certificate authority in this machine's trust store".to_owned();
    let browser_what = "the same authority in this machine's browsers".to_owned();

    let state =
        mixengine_core::certs::ca::read(uninstall.paths.certs(), std::time::SystemTime::now());

    let der = match &state {
        mixengine_proto::CaState::Present { ca } => {
            mixengine_core::certs::ca::der(&ca.certificate_pem).map(|der| (der, ca.key_id.clone()))
        }
        mixengine_proto::CaState::Absent {} | mixengine_proto::CaState::Unusable { .. } => None,
    };

    let Some((der, key_id)) = der else {
        let because =
            "this home has no usable certificate authority, so nothing that could be named is in \
             any store"
                .to_owned();

        return (
            Residue {
                id: ResidueId::TrustStore,
                what: trust_what,
                location: uninstall.paths.certs().display().to_string(),
                outcome: Removal::Absent {},
            },
            Residue {
                id: ResidueId::BrowserTrust,
                what: browser_what,
                location: because,
                outcome: Removal::Absent {},
            },
        );
    };

    let trust = match uninstall.host.trust_store().probe(&der) {
        Ok(probed) => Residue {
            id: ResidueId::TrustStore,
            what: trust_what,
            location: trust_place(probed.method).to_owned(),
            outcome: match (probed.installed, probed.target(&key_id)) {
                (true, Some(target)) => Removal::Planned {
                    how: PrivilegedOp::TrustCaRemove { target }.describe(),
                },
                _ => Removal::Absent {},
            },
        },
        Err(error) => Residue {
            id: ResidueId::TrustStore,
            what: trust_what,
            location: "this machine's trust store".to_owned(),
            outcome: Removal::Failed {
                because: format!(
                    "this machine's trust store could not be read, so whether it holds this \
                     authority is unknown: {}",
                    mixengine_proto::flatten(&error)
                ),
            },
        },
    };

    (trust, browser_row(uninstall, browser_what, der).await)
}

/// **6.** What Firefox and Chrome hold, which is a different question from the store above.
///
/// A process spawn per profile, so off the runtime — `.claude/standards/rust.md`, and the same
/// arrangement `Certificates::remove_from_browsers` uses for the write.
async fn browser_row(uninstall: &Uninstall, what: String, der: Vec<u8>) -> Residue {
    let host = uninstall.host.clone();

    let surveyed = tokio::task::spawn_blocking(move || host.browsers().survey(&der)).await;

    let (location, outcome) = match surveyed {
        Ok(Ok(mixengine_platform::BrowserSurvey::Reached { databases })) => {
            let holding: Vec<&mixengine_platform::DatabaseState> =
                databases.iter().filter(|one| one.installed).collect();

            match holding.is_empty() {
                true => (
                    match databases.is_empty() {
                        true => "no browser database was found on this machine".to_owned(),
                        false => "no browser database on this machine holds it".to_owned(),
                    },
                    Removal::Absent {},
                ),
                false => (
                    holding
                        .iter()
                        .map(|one| format!("{} ({})", one.path, one.owner))
                        .collect::<Vec<String>>()
                        .join(", "),
                    Removal::Planned {
                        how: format!(
                            "take MixEngine's authority out of {} browser database(s), which needs \
                             no administrator",
                            holding.len()
                        ),
                    },
                ),
            }
        }

        // Neither is a failure: one is a machine without `libnss3-tools` and the other is a system
        // MixEngine does not search at all. Both carry their own sentence, which is why this reads
        // it out rather than writing a second one.
        Ok(Ok(
            mixengine_platform::BrowserSurvey::NoTool { because }
            | mixengine_platform::BrowserSurvey::NotSearched { because },
        )) => (because, Removal::Absent {}),

        Ok(Err(error)) => (
            "this machine's browsers".to_owned(),
            Removal::Failed {
                because: format!(
                    "this machine's browsers could not be asked, so what they hold is unknown: {}",
                    mixengine_proto::flatten(&error)
                ),
            },
        ),

        Err(join) => (
            "this machine's browsers".to_owned(),
            Removal::Failed {
                because: format!("the task asking this machine's browsers did not finish: {join}"),
            },
        ),
    };

    Residue {
        id: ResidueId::BrowserTrust,
        what,
        location,
        outcome,
    }
}

/// **7.** `mixengine-elevate`, in the one directory an ordinary account cannot write.
///
/// **`symlink_metadata` and not `exists`**, which answers `false` for a dangling link somebody
/// planted — the rule `mixengine-elevate`'s own validation runs on, applied to the reading side.
fn privileged_helper() -> Residue {
    let what = "MixEngine's privileged helper".to_owned();

    let Ok(path) = mixengine_platform::install::helper_path() else {
        return Residue {
            id: ResidueId::PrivilegedHelper,
            what,
            location: "this machine will not name a directory for a privileged helper".to_owned(),
            outcome: Removal::Absent {},
        };
    };

    let outcome = match there(&path) {
        true => Removal::Planned {
            how: PrivilegedOp::HelperRemove {}.describe(),
        },
        false => Removal::Absent {},
    };

    Residue {
        id: ResidueId::PrivilegedHelper,
        what,
        location: path.display().to_string(),
        outcome,
    }
}

/// **8.** The root-owned record of what ran as root, outside `MIXENGINE_HOME`.
fn audit_log() -> Residue {
    let what = "the log of everything MixEngine has done as an administrator".to_owned();

    let Ok(directory) = mixengine_platform::elevated::audit_directory() else {
        return Residue {
            id: ResidueId::AuditLog,
            what,
            location: "this machine will not name a directory for the log".to_owned(),
            outcome: Removal::Absent {},
        };
    };

    let path = directory.join("elevate.log");
    let outcome = match there(&path) {
        true => Removal::Planned {
            how: PrivilegedOp::AuditLogRemove {}.describe(),
        },
        false => Removal::Absent {},
    };

    Residue {
        id: ResidueId::AuditLog,
        what,
        location: path.display().to_string(),
        outcome,
    }
}

/// **9.** The entry that starts this home's daemon at login — T85b's, and unprivileged.
///
/// **Only when it is this home's.** One entry per user means enabling from a second home replaces
/// it, so an entry naming another home is that home's to remove and not this one's — which is
/// exactly what `AutostartReport::for_this_home` exists to say.
fn autostart_entry(uninstall: &Uninstall) -> Residue {
    let what = "the entry that starts this home's daemon at login".to_owned();

    match uninstall.autostart.status() {
        Ok(report) => Residue {
            id: ResidueId::AutostartEntry,
            what,
            location: report.location.clone(),
            outcome: match report.enabled && report.for_this_home {
                true => Removal::Planned {
                    how:
                        "remove the entry that starts this home's daemon at login, which needs no \
                          administrator"
                            .to_owned(),
                },
                false => Removal::Absent {},
            },
        },
        Err(error) => Residue {
            id: ResidueId::AutostartEntry,
            what,
            location: "this machine's login configuration".to_owned(),
            outcome: Removal::Failed {
                because: format!(
                    "this machine could not be asked what it starts at login: {}",
                    mixengine_proto::flatten(&error)
                ),
            },
        },
    }
}

/// **10.** `<root>/bin` on this user's `PATH` — T26's, and unprivileged.
fn path_entry(uninstall: &Uninstall) -> Residue {
    let what = "this home's commands on your PATH".to_owned();

    match uninstall.shims.status() {
        Ok(report) => {
            let carrying: Vec<&mixengine_proto::PathPlace> =
                report.places.iter().filter(|place| place.present).collect();

            Residue {
                id: ResidueId::PathEntry,
                what,
                location: match carrying.is_empty() {
                    true => report.directory.clone(),
                    false => carrying
                        .iter()
                        .map(|place| place.name.clone())
                        .collect::<Vec<String>>()
                        .join(", "),
                },
                outcome: match carrying.is_empty() {
                    true => Removal::Absent {},
                    false => Removal::Planned {
                        how: format!(
                            "take {} off your PATH in {} place(s), which needs no administrator",
                            report.directory,
                            carrying.len()
                        ),
                    },
                },
            }
        }
        Err(error) => Residue {
            id: ResidueId::PathEntry,
            what,
            location: "this user's PATH".to_owned(),
            outcome: Removal::Failed {
                because: format!(
                    "this user's PATH could not be read, so what is on it is unknown: {}",
                    mixengine_proto::flatten(&error)
                ),
            },
        },
    }
}

/// **11.** `MIXENGINE_HOME` itself, and the one row that is never `Absent`.
///
/// **`Kept` says so rather than the row disappearing.** A person reading the plan has to see that
/// the one irreversible thing on the list was considered and deliberately left.
pub(crate) fn home(root: &Path, keep: bool) -> Residue {
    Residue {
        id: ResidueId::Home,
        what: "this home's own directory, and everything in it".to_owned(),
        location: root.display().to_string(),
        outcome: match keep {
            true => Removal::Kept {
                because: "you asked for this home to be left where it is".to_owned(),
            },
            false => Removal::Planned {
                how: "remove this directory and everything under it, including the databases in \
                      data/ and every certificate this home has issued"
                    .to_owned(),
            },
        },
    }
}

/// **11b.** A directory `[paths]` has moved out of the root.
fn relocated(directory: &Path, keep: bool) -> Residue {
    Residue {
        id: ResidueId::RelocatedDirectory,
        what: "a directory this home was configured to keep somewhere else".to_owned(),
        location: directory.display().to_string(),
        outcome: match keep {
            true => Removal::Kept {
                because: "you asked for this home to be left where it is".to_owned(),
            },
            false => Removal::Planned {
                how: "remove this directory and everything under it".to_owned(),
            },
        },
    }
}

/// Is something there, without following a symlink at the end of the path?
fn there(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Where this machine keeps the resolver configuration, in words.
///
/// **In words and not as a path**, because two of the three mechanisms are not files: NRPT is a
/// registry key and `systemd-networkd` is a link that has to be reloaded. A row that named a path
/// for one and a mechanism for the others would be inviting a person to go and look at something
/// that is not there.
fn resolver_place(method: mixengine_platform::ResolverMethod) -> &'static str {
    match method {
        mixengine_platform::ResolverMethod::ResolverDirectory => {
            "this machine's per-TLD resolver files"
        }
        mixengine_platform::ResolverMethod::SystemdLink => {
            "this machine's systemd-networkd link configuration"
        }
        mixengine_platform::ResolverMethod::Nrpt => "this machine's Name Resolution Policy Table",
        mixengine_platform::ResolverMethod::None => {
            "this machine has no mechanism for routing one TLD to one server"
        }
    }
}

/// Which store this machine keeps its trusted authorities in, in words — [`resolver_place`]'s rule.
fn trust_place(method: mixengine_platform::TrustStoreMethod) -> &'static str {
    match method {
        mixengine_platform::TrustStoreMethod::SystemRoot => {
            "this machine's Root store, under Local Machine"
        }
        mixengine_platform::TrustStoreMethod::SystemKeychain => "this machine's System keychain",
        mixengine_platform::TrustStoreMethod::CaCertificates
        | mixengine_platform::TrustStoreMethod::CaTrustAnchors => {
            "this machine's certificate anchors directory"
        }
        mixengine_platform::TrustStoreMethod::None => {
            "this machine has no system trust store MixEngine knows how to write"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `keep_home` is a row's answer and not a missing row: a person reading the plan has to see
    /// that the home was considered and deliberately left.
    #[test]
    fn keeping_the_home_says_so_on_the_home_row() {
        let kept = home(Path::new("/tmp/home"), true);

        assert_eq!(kept.id, ResidueId::Home);
        assert!(matches!(kept.outcome, Removal::Kept { .. }), "{kept:?}");
        assert_eq!(kept.location, "/tmp/home");
    }

    /// And removing it names what goes with it. The databases in `data/` are the thing a person
    /// most needs to have been told about before they answer the question.
    #[test]
    fn removing_the_home_says_what_goes_with_it() {
        let planned = home(Path::new("/tmp/home"), false);

        let Removal::Planned { how } = &planned.outcome else {
            panic!("{planned:?}");
        };

        assert!(how.contains("data/"), "{how}");
    }

    /// Every mechanism has somewhere to point a person, including the two that mean "this machine
    /// has none of that": a row whose location is empty is a row nobody can act on.
    #[test]
    fn every_mechanism_names_a_place_to_look() {
        for method in [
            mixengine_platform::ResolverMethod::ResolverDirectory,
            mixengine_platform::ResolverMethod::SystemdLink,
            mixengine_platform::ResolverMethod::Nrpt,
            mixengine_platform::ResolverMethod::None,
        ] {
            assert!(!resolver_place(method).is_empty());
        }

        for method in [
            mixengine_platform::TrustStoreMethod::SystemRoot,
            mixengine_platform::TrustStoreMethod::SystemKeychain,
            mixengine_platform::TrustStoreMethod::CaCertificates,
            mixengine_platform::TrustStoreMethod::CaTrustAnchors,
            mixengine_platform::TrustStoreMethod::None,
        ] {
            assert!(!trust_place(method).is_empty());
        }
    }

    /// The label is composed once. A rule written under one name and looked for under another is a
    /// rule nobody ever removes.
    #[test]
    fn the_firewall_label_is_the_one_sharing_writes() {
        assert_eq!(firewall_label(), format!("{FIREWALL_LABEL}shared sites"));
    }

    /// A dangling symlink is something, not nothing: `exists` answers `false` for one, and a plan
    /// that skipped it would leave a link named after MixEngine's helper on the machine.
    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_still_something_that_is_there() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let link = directory.path().join("helper");
        std::os::unix::fs::symlink(directory.path().join("nowhere"), &link).expect("the link");

        assert!(!link.exists());
        assert!(there(&link));
    }
}
