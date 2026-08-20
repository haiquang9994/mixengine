//! What an installed build can load, what it does load, and the file that says so — roadmap task
//! **T28**.
//!
//! Three facts meet here. Two are the artifact's, written down at install time by
//! [`super::remember`]: which of its extensions are linked in, and which are files it could load.
//! The third is the user's, and it is stored as a **deviation** — `{"xdebug": true}` — so that a
//! reinstall or a patch upgrade brings the new build's defaults with it and keeps only what somebody
//! deliberately turned round.
//!
//! # It is not a `match` on the kind
//!
//! Everything here keys off the artifact declaring an extension directory. A Node install declares
//! none, so its state is empty, it renders no documents, and it gets no directory under `etc/`. The
//! day a runtime that is not PHP publishes loadable modules, this needs no edit.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use mixengine_proto::{PackageVersion, RuntimeKind};

use crate::{Error, Result};

/// Whether an extension can be turned off at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Linkage {
    /// Compiled in. Always loaded, and no file switches it on or off.
    Static,
    /// A file inside the install that an ini line loads.
    Shared,
}

/// Why an extension is in the state it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// This build's own answer.
    BuildDefault,
    /// Somebody said otherwise.
    User,
}

/// One extension, as a listing describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    /// What it is called, as the index spells it.
    pub name: String,
    /// Whether it can be turned off.
    pub linkage: Linkage,
    /// Whether it is loaded.
    pub enabled: bool,
    /// Whether that is this build's answer or somebody's choice.
    pub source: Source,
}

/// Everything one installed runtime says about its extensions.
#[derive(Debug, Clone)]
pub struct State {
    /// Which language.
    pub kind: RuntimeKind,
    /// Which version.
    pub version: PackageVersion,
    /// Where the runtime is, so a rendered `extension_dir` can be absolute.
    pub install_path: PathBuf,
    /// Where its loadable extensions are inside that directory, when it has any.
    pub directory: Option<String>,
    /// What the artifact published.
    pub offered: crate::index::Extensions,
    /// What somebody turned round, by name.
    pub choices: BTreeMap<String, bool>,
}

impl State {
    /// The shared extensions this runtime loads, in name order.
    ///
    /// `enabled ∪ {chosen on} − {chosen off}`, intersected with `shared` — the intersection being
    /// what stops a choice about a name this build does not ship from producing a line PHP warns
    /// about on every start.
    #[must_use]
    pub fn loaded(&self) -> Vec<String> {
        let shared: BTreeSet<&String> = self.offered.shared.iter().collect();

        let mut loaded: BTreeSet<String> = self
            .offered
            .enabled
            .iter()
            .filter(|name| shared.contains(name))
            .cloned()
            .collect();

        for (name, wanted) in &self.choices {
            if !shared.contains(name) {
                continue;
            }

            if *wanted {
                loaded.insert(name.clone());
            } else {
                loaded.remove(name);
            }
        }

        loaded.into_iter().collect()
    }

    /// Every extension this build has, and what is true of each.
    #[must_use]
    pub fn listing(&self) -> Vec<Extension> {
        let loaded: BTreeSet<String> = self.loaded().into_iter().collect();

        let compiled_in = self.offered.compiled_in.iter().map(|name| Extension {
            name: name.clone(),
            linkage: Linkage::Static,
            enabled: true,
            source: Source::BuildDefault,
        });

        let shared = self.offered.shared.iter().map(|name| Extension {
            name: name.clone(),
            linkage: Linkage::Shared,
            enabled: loaded.contains(name),
            // A choice that agrees with the build is never stored — see `decide` — so a key being
            // present is the whole of the question.
            source: if self.choices.contains_key(name) {
                Source::User
            } else {
                Source::BuildDefault
            },
        });

        let mut listing: Vec<Extension> = compiled_in.chain(shared).collect();
        listing.sort_by(|left, right| {
            left.linkage
                .cmp(&right.linkage)
                .then_with(|| left.name.cmp(&right.name))
        });
        listing
    }

    /// The choices this runtime would have after `name` is turned `enabled`.
    ///
    /// **A choice that agrees with the build is removed rather than written.** A stored deviation
    /// that deviates from nothing would survive the upgrade that changes the default and would then
    /// silently keep the old answer — which is the exact failure storing deviations avoids.
    ///
    /// # Errors
    ///
    /// [`Error::ExtensionCompiledIn`] for a name this build links in, and [`Error::NotFound`] for a
    /// name it has never heard of.
    pub fn decide(&self, name: &str, enabled: bool) -> Result<BTreeMap<String, bool>> {
        if self.offered.compiled_in.iter().any(|linked| linked == name) {
            return Err(Error::ExtensionCompiledIn {
                kind: self.kind,
                version: self.version.clone(),
                name: name.to_owned(),
            });
        }

        if !self.offered.shared.iter().any(|shared| shared == name) {
            return Err(Error::NotFound {
                kind: "extension",
                id: name.to_owned(),
            });
        }

        let mut choices = self.choices.clone();
        let by_default = self.offered.enabled.iter().any(|on| on == name);

        if enabled == by_default {
            choices.remove(name);
        } else {
            choices.insert(name.to_owned(), enabled);
        }

        Ok(choices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(choices: &str) -> State {
        State {
            kind: RuntimeKind::Php,
            version: PackageVersion::parse("8.3.33").expect("a version"),
            install_path: PathBuf::from("/runtimes/php/8.3.33"),
            directory: Some("lib/php/extensions".to_owned()),
            offered: crate::index::Extensions {
                compiled_in: vec!["opcache".to_owned(), "core".to_owned()],
                shared: vec![
                    "igbinary".to_owned(),
                    "redis".to_owned(),
                    "mongodb".to_owned(),
                    "xdebug".to_owned(),
                ],
                enabled: vec![
                    "igbinary".to_owned(),
                    "redis".to_owned(),
                    "mongodb".to_owned(),
                ],
            },
            choices: serde_json::from_str(choices).expect("choices"),
        }
    }

    /// What the build enables is what is loaded when nobody has said otherwise.
    #[test]
    fn a_build_with_no_choices_loads_what_it_enables() {
        assert_eq!(state("{}").loaded(), ["igbinary", "mongodb", "redis"]);
    }

    /// Deviations in both directions, which is the whole point of storing them rather than a set.
    #[test]
    fn a_choice_turns_one_name_round_and_leaves_the_rest() {
        let loaded = state(r#"{"xdebug": true, "mongodb": false}"#).loaded();

        assert_eq!(loaded, ["igbinary", "redis", "xdebug"]);
    }

    /// A choice about a name this build does not ship loadable cannot smuggle it in.
    #[test]
    fn a_choice_is_intersected_with_what_the_build_ships() {
        assert_eq!(
            state(r#"{"imagick": true}"#).loaded(),
            ["igbinary", "mongodb", "redis"]
        );
    }

    /// A listing says *why* something is on, because the question is asked when the answer is
    /// surprising.
    #[test]
    fn a_listing_says_whether_the_build_or_the_user_decided() {
        let listed = state(r#"{"xdebug": true}"#).listing();
        let of = |name: &str| {
            listed
                .iter()
                .find(|extension| extension.name == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name} is not in the listing"))
        };

        assert_eq!(of("opcache").linkage, Linkage::Static);
        assert!(
            of("opcache").enabled,
            "compiled in and therefore always loaded"
        );
        assert_eq!(of("opcache").source, Source::BuildDefault);

        assert_eq!(of("redis").source, Source::BuildDefault);
        assert!(of("redis").enabled);

        assert_eq!(of("xdebug").source, Source::User);
        assert!(of("xdebug").enabled);
    }

    /// Compiled into this build, and a different build is what it would take.
    #[test]
    fn a_compiled_in_extension_cannot_be_turned_off() {
        let error = state("{}")
            .decide("opcache", false)
            .expect_err("a static extension is not disableable");

        assert!(
            matches!(error, Error::ExtensionCompiledIn { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("opcache"));
    }

    /// A name this build has never heard of is refused rather than written down.
    #[test]
    fn an_unknown_name_is_refused() {
        let error = state("{}")
            .decide("swoole", true)
            .expect_err("a name no cell carries");

        assert!(
            matches!(
                error,
                Error::NotFound {
                    kind: "extension",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// Choosing what the build already does is not stored: a deviation that deviates from nothing
    /// would survive the upgrade that changes the default, which is what this model exists to avoid.
    #[test]
    fn a_choice_that_agrees_with_the_build_is_forgotten() {
        let choices = state(r#"{"xdebug": true}"#)
            .decide("xdebug", false)
            .expect("turning it back off is allowed");

        assert!(choices.is_empty(), "{choices:?}");
    }
}
