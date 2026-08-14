//! What `runtime.*` asks and answers, where [`crate::runtime`] is the vocabulary a runtime is
//! *described* in.
//!
//! The same split [`crate::job_api`] draws over [`crate::job`]. Three of the five methods take
//! [`RuntimeTarget`] — install, uninstall and set_default all name one version of one kind, and
//! writing that question three times would be three places for it to drift, which is
//! [`JobQuery`](crate::JobQuery)'s reasoning one namespace across.
//!
//! **`runtime.install` answers a [`JobSummary`](crate::JobSummary) and not a runtime.** An install
//! is tens of megabytes over somebody's connection, and
//! `.claude/architecture/daemon-and-ipc.md` says a long operation returns a job rather than holding
//! a call open. What the finished job carries as its result is a [`RuntimeSummary`] — the same
//! sentence `runtime.list_installed` answers with, so a client renders the ending of an install with
//! the function it already has.

use crate::{RuntimeChannel, RuntimeKind, RuntimeVersion, Timestamp};

/// Which runtime a call is about.
///
/// One params type for `runtime.install`, `runtime.uninstall` and `runtime.set_default`. Both fields
/// are **required** in all three: a kind with no version is not an installable thing, and a call
/// that guessed one — the newest, the default — would be a client deciding something, which is
/// exactly what `CLAUDE.md` puts in the daemon. Choosing a version from a constraint is
/// [T24](../../../../.claude/roadmap/phase-2-runtimes.md)'s, and it is a resolution step with its
/// own method rather than a default hidden in this one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeTarget {
    /// Which language.
    pub kind: RuntimeKind,

    /// Which version, exactly as the index publishes it.
    pub version: RuntimeVersion,
}

/// Which runtimes a listing should answer with.
///
/// Every field has a default, so both listings with no parameters are questions a person can type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct RuntimeFilter {
    /// Only this language, or all of them.
    ///
    /// A filter rather than a required argument because "what is installed" is a question a GUI's
    /// first paint asks about everything at once, and because the answer for a kind nobody has
    /// installed is an empty list rather than an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<RuntimeKind>,
}

/// What `runtime.list_installed` answers.
///
/// An object around the list rather than a bare array, on [`ServiceList`](crate::ServiceList)'s
/// precedent: a field can be added beside it without changing every existing client's parser.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeList {
    /// What is on this machine, by kind and then by the version string as it was published.
    pub runtimes: Vec<RuntimeSummary>,
}

/// What `runtime.list_available` answers.
///
/// Carries [`stale`](Self::stale) beside the list because the two are one answer: a version list
/// read from a cache the daemon could not refresh is still a usable list, and a client that showed
/// it without saying so would be claiming the network was reached. Why an old index is served at all
/// rather than refused is the index client's decision, not this type's — a tool that can list
/// nothing while the wifi is down is worse than a version list two days old.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeCatalogue {
    /// Every version the index offers **for this machine** — an artifact listed only for another
    /// operating system is not something this one can install, and offering it would turn an absence
    /// at list time into a failure at download time.
    pub runtimes: Vec<RuntimeRelease>,

    /// Whether this came from a cached index the daemon could not refresh.
    ///
    /// `false` for a fresh fetch and for a cache still inside its six hours — the distinction a
    /// person acts on is "could the publisher be reached", not "how old exactly".
    pub stale: bool,
}

/// One installed runtime. The whole of what `runtime.set_default` answers, and what a finished
/// `runtime.install` job carries.
///
/// One type for the listing, the install's result and the default being moved, on
/// [`ServiceSummary`](crate::ServiceSummary)'s precedent: all three are the same sentence about a
/// runtime, so a client renders them with one function.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeSummary {
    /// Which language.
    pub kind: RuntimeKind,

    /// Which version.
    pub version: RuntimeVersion,

    /// Which channel the index published it on.
    pub channel: RuntimeChannel,

    /// Where it landed, as a string for display.
    ///
    /// Not a `PathBuf`, for [`DaemonStatus`](crate::DaemonStatus)' reason: a path is a display value
    /// on the wire, and a client that is not on this machine — the GUI over the same API — has
    /// nothing to open it with.
    pub path: String,

    /// When it was installed.
    pub installed_at: Timestamp,

    /// How much disk it took, as the index declared the archive and the download proved it.
    ///
    /// The *download* size and not the unpacked one: it is the number the index carries, so
    /// reporting it costs nothing, where measuring a tree costs a walk of it on every listing.
    pub bytes: u64,

    /// Whether this is the version its kind resolves to when nothing else says otherwise.
    ///
    /// Exactly one installed version of a kind can carry this — a partial unique index on
    /// `runtime_installs` is what makes that true rather than a convention — and a kind can have
    /// none, which is what a home is left with when its only version is uninstalled.
    pub default: bool,
}

/// One version the index offers, and whether this machine already has it.
///
/// Deliberately *not* [`RuntimeSummary`] with empty fields. What is knowable about something
/// installed and about something merely offered is different — there is no path and no install
/// moment for the second, and no download size worth showing for the first — and one type carrying
/// both would be a type where half the fields are meaningless in half the answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeRelease {
    /// Which language.
    pub kind: RuntimeKind,

    /// Which version.
    pub version: RuntimeVersion,

    /// Which channel it is published on. Only [`RuntimeChannel::Stable`] is offered without a
    /// setting.
    pub channel: RuntimeChannel,

    /// Upstream's end of security support, when upstream states one.
    ///
    /// A version past it stays installable and is marked: the people who reach for a local
    /// development environment are very often the people maintaining something old.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eol: Option<String>,

    /// How large the download is, so a client can say so before somebody commits to it.
    pub bytes: u64,

    /// Whether this exact version is already on this machine.
    ///
    /// Composed by the daemon out of the index and the `runtime_installs` rows, rather than left to
    /// the client to work out by cross-referencing two lists — which is business logic, and a place
    /// for two clients to disagree about what "installed" means.
    pub installed: bool,
}

/// What `runtime.uninstall` answers.
///
/// The runtime **as it was**, plus the one consequence a caller cannot see from it: whether its kind
/// is now left with no default at all.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeRemoval {
    /// What was removed, as it stood a moment before.
    pub removed: RuntimeSummary,

    /// Whether the kind now has no default version.
    ///
    /// **True exactly when the removed version was the default**, because nothing is promoted in its
    /// place. Choosing a successor means deciding which remaining version is *newest*, and comparing
    /// two version strings needs the grammar
    /// [T24](../../../../.claude/roadmap/phase-2-runtimes.md) brings — so this build says out loud
    /// that there is now no default instead of picking one by row order and calling it the newest.
    pub default_cleared: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> RuntimeVersion {
        RuntimeVersion::parse(text).expect("a valid version")
    }

    #[test]
    fn a_listing_with_no_parameters_is_a_question_a_person_can_type() {
        let filter: RuntimeFilter = serde_json::from_str("{}").expect("every field has a default");

        assert_eq!(filter, RuntimeFilter::default());
        assert_eq!(filter.kind, None, "every kind");
    }

    #[test]
    fn a_target_names_both_halves_or_does_not_decode() {
        let target: RuntimeTarget =
            serde_json::from_str(r#"{"kind":"php","version":"8.3.33"}"#).expect("both halves");
        assert_eq!(target.kind, RuntimeKind::Php);
        assert_eq!(target.version.as_str(), "8.3.33");

        serde_json::from_str::<RuntimeTarget>(r#"{"kind":"php"}"#)
            .expect_err("a kind with no version is not an installable thing");
    }

    /// The one field of a release that is a `null` rather than an absence, and it is neither: a
    /// version upstream states no end of support for simply has no `eol` member.
    #[test]
    fn a_release_with_no_stated_end_of_support_omits_the_field() {
        let release = RuntimeRelease {
            kind: RuntimeKind::Node,
            version: version("20.11.0"),
            channel: RuntimeChannel::Stable,
            eol: None,
            bytes: 1024,
            installed: false,
        };

        let encoded = serde_json::to_value(&release).unwrap();
        assert!(encoded.get("eol").is_none(), "{encoded}");
        assert_eq!(
            serde_json::from_value::<RuntimeRelease>(encoded).unwrap(),
            release
        );
    }

    #[test]
    fn an_installed_runtime_round_trips_through_the_wire() {
        let summary = RuntimeSummary {
            kind: RuntimeKind::Php,
            version: version("8.3.33"),
            channel: RuntimeChannel::Stable,
            path: "/home/me/.local/share/mixengine/runtimes/php/8.3.33".to_owned(),
            installed_at: Timestamp(1_760_000_000_000),
            bytes: 41_000_000,
            default: true,
        };

        let encoded = serde_json::to_value(&summary).unwrap();
        assert_eq!(encoded["kind"], "php");
        assert_eq!(encoded["version"], "8.3.33");
        assert_eq!(
            serde_json::from_value::<RuntimeSummary>(encoded).unwrap(),
            summary
        );
    }
}
