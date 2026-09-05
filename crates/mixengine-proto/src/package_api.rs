//! What `package.*` asks and answers, for the servers, databases and caches a home runs.
//!
//! The same split [`crate::runtime_api`] draws over [`crate::runtime`], one namespace across: what a
//! package *is* on disk has no vocabulary of its own — a name and a [`PackageVersion`] is the whole
//! of it — so this module is the API surface and there is no `package.rs` beside it.
//!
//! # Two listings rather than one row type with a flag
//!
//! [`PackageList`] answers what is installed and [`PackageCatalogue`] answers what the index offers,
//! and they are made of different types on [`RuntimeRelease`](crate::RuntimeRelease)'s stated
//! reasoning: what is knowable about something installed and about something merely offered is
//! different, and one type carrying both would be a type where half the fields are meaningless in
//! half the answers. [`PackageRelease`] still carries [`installed`](PackageRelease::installed),
//! composed by the daemon out of the index and the `packages` rows rather than left to a client to
//! work out by cross-referencing two lists.
//!
//! # What a listing is allowed to name
//!
//! Only a package this build has a recipe for — `mixengine_core::generate::Catalogue`'s own set. An
//! index entry MixEngine cannot configure is a download that ends in a directory nothing can use,
//! and the refusal belongs at install time rather than at create time, where the disk is already
//! spent. Two MixEngine versions reading one index therefore answer differently, which is correct:
//! the answer is *what this build can run*.
//!
//! **Runtimes are not here.** They have `runtime_installs`, `runtimes/`, a default version and a
//! shim that reads it; a second door to the same room would either duplicate all of that or produce
//! a PHP the shim cannot see.
//!
//! [`PackageVersion`]: crate::PackageVersion

use crate::{PackageChannel, PackageVersion, ServiceId, Timestamp};

/// Which package a call is about.
///
/// One params type for `package.install` and `package.uninstall`, on
/// [`RuntimeTarget`](crate::RuntimeTarget)'s reasoning: both fields are required in both, because a
/// package with no version is not an installable thing and a call that guessed one — the newest, the
/// only one installed — would be a client deciding something.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PackageTarget {
    /// Which package, by the name a recipe is found under.
    ///
    /// A plain [`String`] rather than a closed enum: which packages exist is a property of the
    /// build, not of the wire, and the daemon holds the set — so a name this build cannot run is
    /// refused with the list of the ones it can, which is a better answer than a decode failure.
    pub package: String,

    /// Which version, exactly as the index publishes it.
    pub version: PackageVersion,
}

/// Which packages a listing should answer with.
///
/// Every field has a default, so both listings with no parameters are questions a person can type.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PackageFilter {
    /// Only this package, or all of them.
    ///
    /// A filter rather than a required argument for [`RuntimeFilter`](crate::RuntimeFilter)'s
    /// reason: a GUI's first paint asks about everything at once, and the answer for a package
    /// nobody has installed is an empty list rather than an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

/// What `package.list` answers.
///
/// An object around the list rather than a bare array, on [`ServiceList`](crate::ServiceList)'s
/// precedent: a field can be added beside it without changing every existing client's parser.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PackageList {
    /// What is on this machine, by name and then by the version string as it was published.
    pub packages: Vec<PackageSummary>,
}

/// One installed package, and what is holding it.
///
/// One type for the listing and for what an uninstall answers with, on
/// [`RuntimeSummary`](crate::RuntimeSummary)'s precedent: both are the same sentence about a
/// package, so a client renders them with one function.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PackageSummary {
    /// Which package.
    pub package: String,

    /// Which version.
    pub version: PackageVersion,

    /// Where it landed, as a string for display.
    ///
    /// Not a `PathBuf`, for [`RuntimeSummary`](crate::RuntimeSummary)'s reason: a path is a display
    /// value on the wire, and a client that is not on this machine has nothing to open it with.
    pub path: String,

    /// When it was installed.
    pub installed_at: Timestamp,

    /// How much disk it took, as the index declared the archive and the download proved it.
    pub bytes: u64,

    /// The services that are instances of this exact version, in [`ServiceId`] order.
    ///
    /// **What an uninstall refuses over**, and the reason it is on the summary rather than looked up
    /// by whoever needs it: `services.package_id` is `ON DELETE RESTRICT`, so a listing that did not
    /// say which packages are held would be a listing where "why can I not remove this" has no
    /// answer in it. Empty is a package nothing is using.
    pub services: Vec<ServiceId>,
}

/// What `package.list_available` answers.
///
/// Carries [`stale`](Self::stale) beside the list for
/// [`RuntimeCatalogue`](crate::RuntimeCatalogue)'s reason: a list read from a cache the daemon could
/// not refresh is still a usable list, and a client that showed it without saying so would be
/// claiming the network was reached.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PackageCatalogue {
    /// Every version the index offers **for this machine**, of the packages this build can run.
    pub packages: Vec<PackageRelease>,

    /// Whether this came from a cached index the daemon could not refresh.
    pub stale: bool,
}

/// One version the index offers, and whether this machine already has it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PackageRelease {
    /// Which package.
    pub package: String,

    /// Which version.
    pub version: PackageVersion,

    /// Which channel it is published on. Only [`PackageChannel::Stable`] is offered without a
    /// setting.
    pub channel: PackageChannel,

    /// Upstream's end of security support, when upstream states one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eol: Option<String>,

    /// How large the download is, so a client can say so before somebody commits to it.
    pub bytes: u64,

    /// Whether this exact version is already on this machine.
    pub installed: bool,
}

/// What `package.uninstall` answers.
///
/// The summary as it stood before the row went, which is the only moment anything could describe it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PackageRemoval {
    /// What is no longer installed.
    pub removed: PackageSummary,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both halves or it does not decode, on [`PackageTarget`]'s own stated reasoning.
    #[test]
    fn a_target_names_both_halves_or_does_not_decode() {
        let target: PackageTarget =
            serde_json::from_str(r#"{"package":"caddy","version":"2.11.4"}"#).expect("both halves");

        assert_eq!(target.package, "caddy");
        assert_eq!(target.version.as_str(), "2.11.4");

        serde_json::from_str::<PackageTarget>(r#"{"package":"caddy"}"#)
            .expect_err("a package with no version is not an installable thing");
    }

    /// The shape a client that types nothing gets.
    #[test]
    fn a_filter_with_no_parameters_means_every_package() {
        let filter: PackageFilter = serde_json::from_str("{}").expect("every field has a default");

        assert_eq!(filter, PackageFilter::default());
        assert_eq!(filter.package, None);
    }

    /// A held package says so in the listing, because that is where "why can I not remove this" is
    /// answered.
    #[test]
    fn a_summary_names_the_services_holding_it() {
        let summary = PackageSummary {
            package: "mariadb".to_owned(),
            version: PackageVersion::parse("11.4.2").expect("a version"),
            path: "/packages/mariadb/11.4.2".to_owned(),
            installed_at: Timestamp(1_760_000_000_000),
            bytes: 1024,
            services: vec![ServiceId::parse("mariadb@main").expect("an id")],
        };

        let encoded = serde_json::to_value(&summary).expect("a summary encodes");

        assert_eq!(encoded["services"][0], "mariadb@main");
        assert_eq!(
            serde_json::from_value::<PackageSummary>(encoded).expect("and decodes"),
            summary
        );
    }
}
