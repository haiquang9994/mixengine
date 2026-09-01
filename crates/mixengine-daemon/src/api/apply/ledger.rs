//! What an apply made, and what a failure does about it — roadmap task **T78**, the design's D4.
//!
//! # Intent, not success
//!
//! [`super::super::create`] keeps the `services` row it wrote when the rendering fails, and
//! `sites::create` does the same for a site, deliberately: *a declaration rolled back because the
//! rendering failed would leave a person with nothing to fix.* So an entry goes in **before** the
//! call that would make it, and every undo is "remove it if it is there". A ledger written only
//! after `Ok` would miss precisely the failures a rollback exists for.
//!
//! # What is undone belongs to the project; what is kept belongs to the machine
//!
//! A database is **never** dropped: by the time an apply has failed something may have migrated into
//! it, and there is no `database.drop` in this product. A runtime or package this apply installed
//! stays, because it belongs to the machine and is what a resumed apply would otherwise download
//! again. A PHP extension stays, for the reason the plan renderer already says on that line: the
//! choice reaches every project here. The project's directory stays and is named, on
//! `project.delete`'s standing rule — the files were never ours.
//!
//! Each of those is *named* in the failure rather than silently left, because a thing nobody was
//! told about is a thing nobody ever cleans up.

use mixengine_proto::{
    PackageVersion, ProjectQuery, ProjectRef, RuntimeKind, ServiceId, SiteQuery, SiteRef,
};

use crate::api::Api;

/// What this apply has made, in the order it made it.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    /// Things belonging to the project, which a rollback takes away.
    made: Vec<Made>,

    /// Things belonging to the machine, which a rollback leaves and names.
    kept: Vec<Kept>,
}

/// Something this apply set out to make, which a rollback removes.
#[derive(Debug)]
// `cfg_attr` on `jobs.rs`' precedent: these are constructed in the tests below, so an expectation
// that held in both builds would itself be a warning.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "constructed by the doers, which arrive with the next tasks in this series"
    )
)]
pub(crate) enum Made {
    /// A project row, by the name it was registered under.
    Project {
        /// Its name.
        name: String,
    },

    /// A site, by one of the names it answers to — which is the handle `site.delete` takes.
    Site {
        /// Any of its domains.
        domain: String,
    },

    /// A service instance this project alone was to use.
    ///
    /// **Only a dedicated one.** A shared instance that was already here is not this apply's to
    /// take away, and one this apply created *as* shared is one another project may already be
    /// pointing at by the time this fails.
    Service {
        /// Which instance.
        id: ServiceId,
    },
}

/// Something this apply made that a rollback keeps, and says it kept.
#[derive(Debug)]
// `cfg_attr` on `jobs.rs`' precedent: these are constructed in the tests below, so an expectation
// that held in both builds would itself be a warning.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "constructed by the doers, which arrive with the next tasks in this series"
    )
)]
pub(crate) enum Kept {
    /// A database and the account that reaches it.
    Database {
        /// Which instance holds it.
        service: ServiceId,

        /// Its name.
        name: String,
    },

    /// A language this apply installed.
    Runtime {
        /// Which language.
        kind: RuntimeKind,

        /// Which version.
        version: PackageVersion,
    },

    /// A service package this apply installed.
    Package {
        /// Which package.
        package: String,

        /// Which version.
        version: PackageVersion,
    },

    /// A PHP extension this apply turned on, which every project on this machine now loads.
    Extension {
        /// Its name.
        name: String,
    },

    /// The project's directory.
    Directory {
        /// Where it is.
        path: String,
    },
}

impl Ledger {
    /// Write down something this apply is **about** to make.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "called by the doers, which arrive with the next tasks in this series"
        )
    )]
    pub(crate) fn attempting(&mut self, made: Made) {
        self.made.push(made);
    }

    /// Write down something this apply made that a rollback will not take away.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "called by the doers, which arrive with the next tasks in this series"
        )
    )]
    pub(crate) fn keeping(&mut self, kept: Kept) {
        self.kept.push(kept);
    }

    /// What to undo, newest first.
    ///
    /// The reverse of the making, because a site belongs to a project and a project cannot be
    /// deleted out from under one.
    pub(crate) fn undoing(&self) -> impl Iterator<Item = &Made> {
        self.made.iter().rev()
    }
}

/// Take back what this apply made, and answer with whatever could not be taken back.
///
/// **Every failure here is a line in the log and nothing more**: what the job reports is the error
/// that caused the rollback, because that is the one a person can act on.
pub(crate) async fn unwind(api: &Api, ledger: &Ledger) -> Vec<String> {
    let mut stubborn = Vec::new();

    for made in ledger.undoing() {
        let outcome = match made {
            Made::Site { domain } => api
                .sites
                .delete(&SiteQuery {
                    site: SiteRef::Domain(domain.clone()),
                })
                .await
                .map(|_| ()),

            // `force`, because the site that declared it is being removed in the same breath and
            // the two orders would otherwise deadlock against each other.
            Made::Service { id } => api.service_delete(id, true).await.map(|_| ()),

            Made::Project { name } => api
                .projects
                .delete(&ProjectQuery {
                    project: ProjectRef::Name(name.clone()),
                })
                .await
                .map(|_| ()),
        };

        if let Err(error) = outcome {
            tracing::warn!(?made, %error, "a failed apply could not take back what it made");
            stubborn.push(format!("{made:?}: {}", error.message));
        }
    }

    stubborn
}

/// The sentence a failure carries about what was left behind.
///
/// Empty when nothing was, so that a failure before anything was made does not print a list with a
/// colon in front of it.
pub(crate) fn left_behind(ledger: &Ledger) -> String {
    let said: Vec<String> = ledger
        .kept
        .iter()
        .map(|kept| match kept {
            Kept::Database { service, name } => format!("the database {name} on {service}"),
            Kept::Runtime { kind, version } => {
                format!("{} {}, now installed", kind.as_str(), version.as_str())
            }
            Kept::Package { package, version } => {
                format!("{package} {}, now installed", version.as_str())
            }
            Kept::Extension { name } => format!("the PHP extension {name}, now on"),
            Kept::Directory { path } => format!("the directory {path}"),
        })
        .collect();

    match said.is_empty() {
        true => String::new(),
        false => format!(" — left in place: {}", said.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The order is the reverse of the making**, because a site belongs to a project and a
    /// project cannot be deleted out from under one.
    #[test]
    fn a_rollback_undoes_the_newest_thing_first() {
        let mut ledger = Ledger::default();
        ledger.attempting(Made::Project {
            name: "shop".to_owned(),
        });
        ledger.attempting(Made::Service {
            id: ServiceId::parse("mariadb@shop").expect("an id"),
        });
        ledger.attempting(Made::Site {
            domain: "shop.test".to_owned(),
        });

        let order: Vec<&Made> = ledger.undoing().collect();

        assert!(matches!(order[0], Made::Site { .. }));
        assert!(matches!(order[1], Made::Service { .. }));
        assert!(matches!(order[2], Made::Project { .. }));
    }

    /// **What is kept is named.** A rollback that silently left a database behind would be a
    /// rollback nobody could act on.
    #[test]
    fn what_a_rollback_leaves_behind_is_said_in_one_sentence() {
        let mut ledger = Ledger::default();
        ledger.keeping(Kept::Database {
            service: ServiceId::parse("mariadb@main").expect("an id"),
            name: "shop".to_owned(),
        });
        ledger.keeping(Kept::Runtime {
            kind: RuntimeKind::Php,
            version: PackageVersion::parse("8.2.23").expect("a version"),
        });
        ledger.keeping(Kept::Package {
            package: "mariadb".to_owned(),
            version: PackageVersion::parse("11.4.3").expect("a version"),
        });
        ledger.keeping(Kept::Extension {
            name: "redis".to_owned(),
        });
        ledger.keeping(Kept::Directory {
            path: "/tmp/shop".to_owned(),
        });

        let said = left_behind(&ledger);

        // All five, because each is a different thing to go and look at afterwards, and a sentence
        // that named four of them would be a sentence that hid one.
        assert!(said.contains("mariadb@main"), "{said}");
        assert!(said.contains("shop"), "{said}");
        assert!(said.contains("php 8.2.23"), "{said}");
        assert!(said.contains("mariadb 11.4.3"), "{said}");
        assert!(said.contains("redis"), "{said}");
        assert!(said.contains("/tmp/shop"), "{said}");
    }

    /// A failure before anything was made says nothing about what was kept, rather than an empty
    /// list with a colon in front of it.
    #[test]
    fn a_failure_before_anything_was_made_says_nothing_about_what_was_kept() {
        assert!(left_behind(&Ledger::default()).is_empty());
    }
}
