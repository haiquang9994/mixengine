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
use std::path::{Path, PathBuf};

use mixengine_proto::{PackageVersion, RuntimeKind};

use crate::{Error, Paths, Result, Store};

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

/// The variable both consumers set, and the only way the generated set reaches PHP.
///
/// **There is no `php.ini`.** This was measured to work on all three systems during T32, and a
/// second file is a second place for the truth to live.
pub const SCAN_DIR_ENV: &str = "PHP_INI_SCAN_DIR";

/// The file that carries `extension_dir` and MixEngine's opinion about a development machine.
const MIXENGINE_INI: &str = "00-mixengine.ini";

/// The two names PHP loads as engine extensions rather than as ordinary ones.
///
/// A fact about PHP and not about the index, which is why it is written here beside
/// [`super::smoke_test`] — the same place, and for the same reason, that "which flag prints a
/// version" lives. The value is the bare name on both systems; modern PHP resolves it to
/// `php_<name>.dll` on Windows itself.
const ZEND: [&str; 2] = ["opcache", "xdebug"];

/// What one extension's file is called, which is what decides load order.
///
/// `conf.d` is scanned in name order:
///
/// - `20` `igbinary`, because `redis` links against it when it can find it and silently stores a
///   serialisation nothing else reads when it cannot;
/// - `40` `opcache`, because an optimiser wants to be under whatever wraps it;
/// - `90` `xdebug`, which wants to be outermost and is the one whose presence changes how everything
///   else behaves;
/// - `50` for everything else.
fn prefix(name: &str) -> &'static str {
    match name {
        "igbinary" => "20",
        "opcache" => "40",
        "xdebug" => "90",
        _ => "50",
    }
}

/// Where this runtime's generated ini set lives: `etc/<kind>/<version>/conf.d/`.
///
/// **Under `etc/` and not inside the install**, which is what `.claude/features/runtime-versions.md`
/// said before T28 and what this changes: an install is a rename of a staging directory over the
/// destination, so a generated `conf.d` living inside it is destroyed by reinstalling the same
/// version — and generated configuration is disposable by the project's own rule.
#[must_use]
pub fn conf_d(etc: &Path, kind: RuntimeKind, version: &str) -> PathBuf {
    etc.join(kind.as_str()).join(version).join("conf.d")
}

impl State {
    /// Every file this runtime's ini set is made of, in the order they will be scanned.
    ///
    /// Empty for a runtime that declares no extension directory.
    #[must_use]
    pub fn documents(&self) -> Vec<crate::generate::Document> {
        let Some(directory) = &self.directory else {
            return Vec::new();
        };

        let mut documents = vec![crate::generate::Document::new(
            MIXENGINE_INI,
            self.mixengine_ini(directory),
        )];

        for name in self.loaded() {
            let directive = if ZEND.contains(&name.as_str()) {
                "zend_extension"
            } else {
                "extension"
            };

            documents.push(crate::generate::Document::new(
                format!("{}-{name}.ini", prefix(&name)),
                format!(
                    "; Generated by MixEngine for {} {}. Edits are overwritten.\n\
                     {directive} = {name}\n",
                    self.kind, self.version
                ),
            ));
        }

        documents
    }

    /// `extension_dir`, then the settings a development machine wants instead of PHP's shipping
    /// defaults.
    fn mixengine_ini(&self, directory: &str) -> String {
        // Absolute, and always written — see `conf_d` and the test beside it.
        let absolute = self.install_path.join(directory);

        format!(
            "; Generated by MixEngine for {} {}. Edits are overwritten, and nothing reads this file\n\
             ; back into state.\n\
             extension_dir = \"{}\"\n\
             \n\
             ; A development machine's defaults, which are not PHP's.\n\
             memory_limit = 512M\n\
             upload_max_filesize = 128M\n\
             post_max_size = 128M\n\
             max_execution_time = 120\n\
             display_errors = On\n\
             error_reporting = E_ALL\n\
             date.timezone = UTC\n\
             \n\
             ; Present and idle until an ini says otherwise, whether it is linked in or loaded.\n\
             ; `revalidate_freq = 0` is the difference between opcache in production and opcache on\n\
             ; a laptop: an edited file takes effect on the next request.\n\
             opcache.enable = 1\n\
             opcache.revalidate_freq = 0\n",
            self.kind,
            self.version,
            absolute.display()
        )
    }
}

/// Read one runtime's extension state out of its row.
///
/// # Errors
///
/// [`Error::NotFound`] when that version is not installed, [`Error::UnreadableRuntimeRow`] when
/// either JSON column holds something this build cannot read, and [`Error::Database`] when the table
/// cannot be read.
pub async fn state(store: &Store, kind: RuntimeKind, version: &PackageVersion) -> Result<State> {
    let (kind_column, version_column) = (kind.as_str(), version.as_str());

    let row = sqlx::query!(
        "SELECT install_path, extension_dir, extensions_json, extension_choices_json
         FROM runtime_installs WHERE kind = ? AND version = ?",
        kind_column,
        version_column
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?
    .ok_or_else(|| Error::NotFound {
        kind: "runtime",
        id: format!("{kind} {version}"),
    })?;

    let unreadable = |column: &'static str, value: &str| Error::UnreadableRuntimeRow {
        column,
        value: value.to_owned(),
    };

    let offered: crate::index::Extensions = serde_json::from_str(&row.extensions_json)
        .map_err(|_| unreadable("extensions_json", &row.extensions_json))?;
    let choices: BTreeMap<String, bool> = serde_json::from_str(&row.extension_choices_json)
        .map_err(|_| unreadable("extension_choices_json", &row.extension_choices_json))?;

    Ok(State {
        kind,
        version: version.clone(),
        install_path: PathBuf::from(row.install_path),
        directory: Some(row.extension_dir).filter(|dir| !dir.is_empty()),
        offered,
        choices,
    })
}

/// Put this runtime's ini set on disk, and say whether anything about it moved.
///
/// **The sweep is the half [`crate::generate::document::install`] does not do.** It writes what it is
/// given and prunes nothing, so an extension turned off would leave its file behind and go on being
/// loaded. Anything in the directory that is not one of the documents is removed, which also repairs
/// a directory left by a build that named its files differently.
///
/// # Errors
///
/// [`Error::Io`] naming the file or directory that could not be read, written or removed.
pub async fn render(paths: &Paths, state: &State) -> Result<bool> {
    let directory = conf_d(paths.etc(), state.kind, state.version.as_str());
    let documents = state.documents();

    if documents.is_empty() {
        return Ok(false);
    }

    let written = crate::generate::document::install(&directory, &documents, None).await?;
    let mut changed = written.iter().any(|one| one.changed());

    let ours: BTreeSet<PathBuf> = documents
        .iter()
        .map(|document| document.relative().to_path_buf())
        .collect();

    let unreadable = |source| Error::Io {
        action: "read the generated directory at",
        path: directory.clone(),
        source,
    };

    let mut entries = tokio::fs::read_dir(&directory).await.map_err(unreadable)?;

    while let Some(entry) = entries.next_entry().await.map_err(unreadable)? {
        let name = PathBuf::from(entry.file_name());

        if ours.contains(&name) || name.extension().is_none_or(|kind| kind != "ini") {
            continue;
        }

        tokio::fs::remove_file(entry.path())
            .await
            .map_err(|source| Error::Io {
                action: "remove the generated file at",
                path: entry.path(),
                source,
            })?;

        tracing::debug!(
            file = %entry.path().display(),
            "an extension that is no longer loaded left a file behind"
        );
        changed = true;
    }

    Ok(changed)
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

    fn rendered(state: &State) -> BTreeMap<String, String> {
        state
            .documents()
            .into_iter()
            .map(|document| {
                (
                    document.relative().display().to_string(),
                    document.contents().to_owned(),
                )
            })
            .collect()
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

    /// `extension_dir` is absolute, and written even where PHP would find its own — upstream PHP for
    /// Windows bakes an absolute `C:\php\ext` into the binary that would otherwise be consulted by
    /// accident on a machine where that path happens to exist.
    #[test]
    fn the_extension_directory_is_written_as_an_absolute_path() {
        let files = rendered(&state("{}"));
        let mixengine = &files["00-mixengine.ini"];
        let expected = PathBuf::from("/runtimes/php/8.3.33").join("lib/php/extensions");

        assert!(
            mixengine.contains(&format!("extension_dir = \"{}\"", expected.display())),
            "{mixengine}"
        );
    }

    /// The dev-tuned block is MixEngine's opinion, and it is written whole.
    #[test]
    fn a_development_machine_gets_the_settings_it_wants() {
        let files = rendered(&state("{}"));
        let mixengine = &files["00-mixengine.ini"];

        for line in [
            "memory_limit = 512M",
            "upload_max_filesize = 128M",
            "post_max_size = 128M",
            "max_execution_time = 120",
            "display_errors = On",
            "error_reporting = E_ALL",
            "date.timezone = UTC",
            "opcache.enable = 1",
            "opcache.revalidate_freq = 0",
        ] {
            assert!(
                mixengine.contains(line),
                "{line} is missing
{mixengine}"
            );
        }
    }

    /// `conf.d` is scanned in name order, and order is load order.
    #[test]
    fn the_file_names_carry_the_load_order() {
        let files = rendered(&state(r#"{"xdebug": true}"#));
        let names: Vec<&str> = files.keys().map(String::as_str).collect();

        assert_eq!(
            names,
            [
                "00-mixengine.ini",
                "20-igbinary.ini",
                "50-mongodb.ini",
                "50-redis.ini",
                "90-xdebug.ini"
            ],
            "igbinary loads before the redis that links against it, and xdebug wraps everything"
        );
    }

    /// Two names are engine extensions and the rest are not. A `zend_extension` PHP cannot load is a
    /// startup warning rather than a refusal to start, which is why this is asserted here and again
    /// against a real PHP in `crates/mixengine-cli/tests/php_extensions.rs`.
    #[test]
    fn the_two_zend_extensions_are_spelled_as_such() {
        let files = rendered(&state(r#"{"xdebug": true}"#));

        assert!(files["90-xdebug.ini"].contains("zend_extension = xdebug"));
        assert!(files["50-redis.ini"].contains("extension = redis"));
        assert!(
            !files["50-redis.ini"].contains("zend_extension"),
            "an ordinary extension loaded as an engine one is a PHP that will not start"
        );
    }

    /// The same state on two systems, decided by the index rather than by a `cfg`: opcache is
    /// compiled in on the Unix cells and is a DLL on Windows, so one gets a file and the other does
    /// not — while both are told `opcache.enable = 1`, because a static opcache is present and idle
    /// until an ini says otherwise.
    #[test]
    fn opcache_renders_from_what_the_index_says_about_this_artifact() {
        let unix = rendered(&state("{}"));
        assert!(!unix.contains_key("40-opcache.ini"), "compiled in here");
        assert!(unix["00-mixengine.ini"].contains("opcache.enable = 1"));

        let mut windows = state("{}");
        windows.offered.compiled_in = vec!["core".to_owned()];
        windows.offered.shared.push("opcache".to_owned());
        windows.offered.enabled.push("opcache".to_owned());

        let windows = rendered(&windows);
        assert!(windows["40-opcache.ini"].contains("zend_extension = opcache"));
        assert!(windows["00-mixengine.ini"].contains("opcache.enable = 1"));
    }

    /// A runtime that can load nothing renders nothing — which is what keeps Node, Python and Ruby
    /// out of this without anything asking what kind it is.
    #[test]
    fn a_runtime_that_declares_no_extension_directory_renders_nothing() {
        let mut node = state("{}");
        node.directory = None;
        node.offered = crate::index::Extensions::default();

        assert!(node.documents().is_empty());
    }

    /// **The sweep.** `document::install` prunes nothing, so an extension turned off would leave its
    /// file behind and go on being loaded.
    #[tokio::test]
    async fn a_file_left_by_an_earlier_state_is_removed() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let paths = crate::Paths::new(
            home.path().to_path_buf(),
            &crate::config::PathOverrides::default(),
        );

        let on = state(r#"{"xdebug": true}"#);
        assert!(render(&paths, &on).await.expect("a rendering"));

        let directory = conf_d(paths.etc(), RuntimeKind::Php, on.version.as_str());
        assert!(directory.join("90-xdebug.ini").is_file());

        let off = state("{}");
        assert!(
            render(&paths, &off).await.expect("a rendering"),
            "removing a file is a change, and the pool has to hear about it"
        );
        assert!(
            !directory.join("90-xdebug.ini").exists(),
            "xdebug was turned off and its file went on loading it"
        );
        assert!(
            directory.join("50-redis.ini").is_file(),
            "the rest of the set is untouched"
        );

        assert!(
            !render(&paths, &off).await.expect("a rendering"),
            "a second render of the same state changes nothing, or every daemon start reloads pools"
        );
    }
}
