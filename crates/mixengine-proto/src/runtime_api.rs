//! What `runtime.*` asks and answers, where [`crate::runtime`] is the vocabulary a runtime is
//! *described* in.
//!
//! **The `Extension*` types here are PHP extensions**, switched on for one installed runtime.
//! MixEngine's own extensions — Mailpit, phpMyAdmin, MixDB — are [`crate::extension`], and the two
//! vocabularies never meet.
//!
//! The same split [`crate::job_api`] draws over [`crate::job`]. Four of the seven methods take
//! [`RuntimeTarget`] — install, uninstall, set_default and list_extensions all name one version of
//! one kind, and
//! writing that question three times would be three places for it to drift, which is
//! [`JobQuery`](crate::JobQuery)'s reasoning one namespace across.
//!
//! **[`RuntimeQuestion`] is deliberately not a fourth user of it.** `runtime.resolve` is the one
//! method that is *asking* which version rather than naming one, so what it takes is a constraint
//! and a directory — and what it answers carries the reason as well as the answer.
//!
//! **`runtime.install` answers a [`JobSummary`](crate::JobSummary) and not a runtime.** An install
//! is tens of megabytes over somebody's connection, and
//! `.claude/architecture/daemon-and-ipc.md` says a long operation returns a job rather than holding
//! a call open. What the finished job carries as its result is a [`RuntimeSummary`] — the same
//! sentence `runtime.list_installed` answers with, so a client renders the ending of an install with
//! the function it already has.

use crate::{PackageChannel, PackageVersion, RuntimeKind, Timestamp, VersionConstraint};

/// Which extension of which runtime to turn round.
///
/// [`RuntimeTarget`] and two more fields, flattened so the wire spells the version exactly as every
/// other `runtime.*` call does. One name per call rather than a set: a refusal that named a whole
/// list would leave a client working out which half of its request happened.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ExtensionChoice {
    /// Which installed runtime.
    #[serde(flatten)]
    pub runtime: RuntimeTarget,

    /// Which extension, spelled as the index spells it.
    pub name: String,

    /// Whether it is to be loaded.
    pub enabled: bool,
}

/// Every extension one installed runtime has.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ExtensionList {
    /// Compiled-in first, then loadable, each in name order.
    pub extensions: Vec<RuntimeExtension>,
}

/// One extension, and everything a client needs to render a line about it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RuntimeExtension {
    /// What it is called.
    pub name: String,

    /// Whether it can be turned off at all.
    pub linkage: Linkage,

    /// Whether it is loaded.
    pub enabled: bool,

    /// Why it is in that state.
    pub source: ExtensionSource,
}

/// Whether an extension is part of the binary or a file beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Linkage {
    /// Compiled in. Always loaded, and `runtime.set_extension` refuses to turn it off.
    Static,

    /// A module inside the install that one generated ini line loads.
    Shared,
}

/// Who decided that an extension is on or off.
///
/// The question is asked precisely when the answer is surprising, and *on because the build says so*
/// and *on because you turned it on* are different answers to why xdebug is loaded — the second is
/// somebody's own doing and survives a reinstall; the first moves with the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ExtensionSource {
    /// What this build ships switched on.
    BuildDefault,

    /// A deviation somebody asked for.
    User,
}

/// What one `runtime.set_extension` did, and what it means for the pool.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ExtensionChange {
    /// The extension, as it now stands.
    pub extension: RuntimeExtension,

    /// Whether anything that was running heard about it.
    ///
    /// Carried rather than left to the client because a client guessing from the operating system it
    /// happens to be running on is a client that prints a confident wrong sentence: which pools can
    /// be handed a configuration is a property of the *recipe*, and only the daemon holds it.
    pub pool: PoolOutcome,
}

/// What happened to the pool that runs this version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum PoolOutcome {
    /// It was asked to re-read its configuration and will finish what it is serving first.
    Reloaded,

    /// It is running and nothing can hand it a new configuration; it loads the new set when somebody
    /// restarts it.
    RestartRequired,

    /// Nothing is running, which is neither a failure nor a reload: the set is read at start.
    PoolNotRunning,
}

/// Which runtime a call is about.
///
/// One params type for `runtime.install`, `runtime.uninstall` and `runtime.set_default`. Both fields
/// are **required** in all three: a kind with no version is not an installable thing, and a call
/// that guessed one — the newest, the default — would be a client deciding something, which is
/// exactly what `CLAUDE.md` puts in the daemon. Choosing a version from a constraint has a method of
/// its own ([`RuntimeQuestion`]) rather than a default hidden in this one, and it answers from what
/// is *installed* — which is why none of these three can take a range: an install picking the newest
/// `8.3` would be picking between versions none of which are here yet.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RuntimeTarget {
    /// Which language.
    pub kind: RuntimeKind,

    /// Which version, exactly as the index publishes it.
    pub version: PackageVersion,
}

/// What `runtime.uninstall` takes: a version, and whether to cross a refusal.
///
/// Flattened rather than made a field on [`RuntimeTarget`]: that type is also `runtime.install`'s
/// and `runtime.set_default`'s parameter, where a `force` would mean nothing. The flatten keeps
/// today's wire shape and adds one optional key, so an older client's request still parses.
///
/// **It crosses the project-pin refusal and nothing else** (spec D8). A broken pin is a statement
/// about the future — the next `cd` into that directory fails with a message naming the install
/// that fixes it — and a person who has been shown the affected projects is entitled to decide. A
/// running php-fpm pool is a fact about the present, and no flag buys a live process with no files
/// under it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RuntimeUninstall {
    /// Which version.
    #[serde(flatten)]
    pub target: RuntimeTarget,

    /// Remove it even though a registered project pins it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
}

/// Which runtimes a listing should answer with.
///
/// Every field has a default, so both listings with no parameters are questions a person can type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RuntimeSummary {
    /// Which language.
    pub kind: RuntimeKind,

    /// Which version.
    pub version: PackageVersion,

    /// Which channel the index published it on.
    pub channel: PackageChannel,

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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RuntimeRelease {
    /// Which language.
    pub kind: RuntimeKind,

    /// Which version.
    pub version: PackageVersion,

    /// Which channel it is published on. Only [`PackageChannel::Stable`] is offered without a
    /// setting.
    pub channel: PackageChannel,

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

/// What `runtime.resolve` asks: which language, from where, and what the caller was already told.
///
/// **Both optional fields are things only the caller can know**, which is why they are asked for
/// rather than found. The daemon's own working directory is wherever it was started, and its
/// environment is whatever started it — neither is the shell somebody is typing in — so a client
/// sends the directory it is in and the value it read from `--php` or
/// [`RuntimeKind::override_env`], and the daemon does every step that reads a file or a table.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RuntimeQuestion {
    /// Which language is being resolved.
    pub kind: RuntimeKind,

    /// The directory the question is being asked from.
    ///
    /// Absolute, and refused when it is not: everything found by walking up from it —
    /// `mixengine.toml`, a registered project root — would otherwise be walked from the daemon's
    /// own directory, which is a different machine's worth of surprise.
    ///
    /// Absent means *do not look*: a caller with no directory to name is asking what the flag and
    /// the default say, and gets exactly that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// What the caller was told to use, ahead of anything the directory says.
    ///
    /// A flag or an environment variable, already read by the process the user invoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<VersionConstraint>,
}

/// What `runtime.resolve` answers: the installed runtime a command would use here, and why that one.
///
/// **The reason travels with the answer** rather than being left for a client to reconstruct.
/// "Which PHP is this?" is a question people ask precisely when the answer is surprising, and a
/// version with no account of where it came from sends them looking through four possible sources by
/// hand — which is also four chances for a client to explain it differently from the next one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ResolvedRuntime {
    /// The installed runtime itself, as [`RuntimeSummary`] describes every other one.
    pub runtime: RuntimeSummary,

    /// Which of the four sources decided it.
    pub source: RuntimeSource,

    /// The constraint that source carried, when it carried one.
    ///
    /// Absent exactly for [`RuntimeSource::Default`], which names no version: it *is* the version
    /// its kind falls back to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<VersionConstraint>,
}

/// Where the version a directory resolves to was decided.
///
/// The four steps of
/// [runtime-versions.md](../../../../.claude/features/runtime-versions.md)'s order, in that order,
/// each carrying the one thing a person would ask next: *which* file, *which* project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum RuntimeSource {
    /// A flag or an environment variable the caller passed in.
    Explicit,

    /// A `mixengine.toml`, found by walking up from the directory the question came from.
    Manifest {
        /// The file that pinned it.
        path: String,
    },

    /// A project registered in this home, whose root is the directory or contains it.
    Project {
        /// That project's root directory.
        root: String,
    },

    /// The kind's default version, because nothing else said anything.
    Default,
}

/// What `runtime.uninstall` answers.
///
/// The runtime **as it was**, plus the one consequence a caller cannot see from it: whether its kind
/// is now left with no default at all.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RuntimeRemoval {
    /// What was removed, as it stood a moment before.
    pub removed: RuntimeSummary,

    /// Whether the kind now has no default version.
    ///
    /// **True exactly when the removed version was the default**, because nothing is promoted in its
    /// place — not because nothing *could* be. Naming the newest remaining version is one call now,
    /// and an uninstall still does not get to move what `php` means: the project that would break is
    /// the one nobody was thinking about while typing this. So the daemon says out loud that there
    /// is no default, and `runtime.set_default` is one call as well.
    pub default_cleared: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> PackageVersion {
        PackageVersion::parse(text).expect("a valid version")
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

    /// An older client sends what it always sent, and it still parses.
    #[test]
    fn an_uninstall_without_a_force_is_still_an_uninstall() {
        let asked: RuntimeUninstall =
            serde_json::from_value(serde_json::json!({"kind": "php", "version": "8.3.33"}))
                .expect("the shape every client has sent since T23");

        assert!(!asked.force);
        assert_eq!(asked.target.kind, crate::RuntimeKind::Php);
    }

    /// The one field of a release that is a `null` rather than an absence, and it is neither: a
    /// version upstream states no end of support for simply has no `eol` member.
    #[test]
    fn a_release_with_no_stated_end_of_support_omits_the_field() {
        let release = RuntimeRelease {
            kind: RuntimeKind::Node,
            version: version("20.11.0"),
            channel: PackageChannel::Stable,
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

    /// A question a shim asks a thousand times a day: one kind, one directory, nothing else.
    #[test]
    fn a_resolution_asks_with_what_only_the_caller_knows() {
        let question: RuntimeQuestion =
            serde_json::from_str(r#"{"kind":"php"}"#).expect("both other fields are optional");
        assert_eq!(question.cwd, None, "no directory means do not look");
        assert_eq!(question.version, None);

        let asked: RuntimeQuestion =
            serde_json::from_str(r#"{"kind":"php","cwd":"/srv/blog","version":"^8.3"}"#)
                .expect("a directory and a constraint");
        assert_eq!(
            asked.version.as_ref().map(VersionConstraint::as_str),
            Some("^8.3")
        );

        serde_json::from_str::<RuntimeQuestion>(r#"{"kind":"php","version":"~8.3"}"#)
            .expect_err("a constraint is validated where every other identifier is");
    }

    /// The source is the half of the answer people are actually asking for, so it has to survive the
    /// wire as something a client can branch on rather than as a sentence.
    #[test]
    fn a_resolution_says_which_of_the_four_sources_decided_it() {
        let resolved = ResolvedRuntime {
            runtime: RuntimeSummary {
                kind: RuntimeKind::Php,
                version: version("8.3.33"),
                channel: PackageChannel::Stable,
                path: "/home/me/.local/share/mixengine/runtimes/php/8.3.33".to_owned(),
                installed_at: Timestamp(1_760_000_000_000),
                bytes: 41_000_000,
                default: false,
            },
            source: RuntimeSource::Manifest {
                path: "/srv/blog/mixengine.toml".to_owned(),
            },
            constraint: Some(VersionConstraint::parse("^8.3").expect("a constraint")),
        };

        let encoded = serde_json::to_value(&resolved).unwrap();
        assert_eq!(encoded["source"]["from"], "manifest");
        assert_eq!(encoded["source"]["path"], "/srv/blog/mixengine.toml");
        assert_eq!(encoded["constraint"], "^8.3");
        assert_eq!(
            serde_json::from_value::<ResolvedRuntime>(encoded).unwrap(),
            resolved
        );

        // The one source that names no version, which is what makes `constraint` optional.
        let encoded = serde_json::to_value(RuntimeSource::Default).unwrap();
        assert_eq!(encoded, serde_json::json!({"from": "default"}));
    }

    #[test]
    fn an_installed_runtime_round_trips_through_the_wire() {
        let summary = RuntimeSummary {
            kind: RuntimeKind::Php,
            version: version("8.3.33"),
            channel: PackageChannel::Stable,
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
