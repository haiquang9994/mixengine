//! Which version of a language a directory uses — the one answer every "which PHP is this?" has
//! been deferring to.
//!
//! One function, [`runtime`], used by the shims, the daemon, the CLI and the GUI alike, because two
//! implementations of this would be two answers to a question that has exactly one. The order is
//! [runtime-versions.md](../../../.claude/features/runtime-versions.md)'s, and each step is here
//! rather than in a client for the reason `CLAUDE.md` gives: a GUI that resolved differently from a
//! shim would make `php -v` in a terminal disagree with the version the window says it is using.
//!
//! ```text
//! 1  a flag, or MIXENGINE_PHP           read by the process the user invoked, passed in
//! 2  mixengine.toml                     walking up from the directory
//! 3  a project registered in this home  the nearest registered root at or above it
//! 4  the kind's default version         what `mix runtime default` set
//! ```
//!
//! # What a constraint is matched against
//!
//! **The installed versions, and never the downloadable ones.** The feature spec says so, and the
//! reason is worth keeping in view: the alternative is a `cd` into a directory quietly starting an
//! eighty-megabyte download. A constraint nothing installed satisfies is
//! [`Error::RuntimeUnresolved`], whose message names the install command that would fix it.
//!
//! Which of several matches wins is
//! [`RuntimeVersion::cmp_precedence`](mixengine_proto::RuntimeVersion::cmp_precedence) — the newest, as upstream
//! means "newest" rather than as ASCII does.
//!
//! # Two of the four sources read something that mostly is not there
//!
//! A `mixengine.toml` is optional and a project record cannot exist at all in this build — there are
//! no `project.*` methods until Phase 4, so the `projects` table is empty on every machine. Both are
//! implemented now anyway: the order is the contract, and a step left out of it would be a step
//! whose behaviour gets decided later by whichever task happens to need it, against a shim that has
//! already shipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mixengine_proto::{ResolvedRuntime, RuntimeKind, RuntimeSource, VersionConstraint};

use crate::{Error, Result, Store, runtimes};

/// The file a project pins its runtimes in, checked into the user's repository.
pub const MANIFEST_FILE_NAME: &str = "mixengine.toml";

/// What a caller wants resolved, and the two things only the caller can know.
#[derive(Debug, Clone, Copy)]
pub struct Question<'a> {
    /// Which language.
    pub kind: RuntimeKind,

    /// The directory being asked about, which has to be absolute.
    ///
    /// [`None`] means the caller has none to offer — a GUI panel asking what `php` means with no
    /// project open — and skips the two steps that walk it, rather than falling back to whatever
    /// directory this process happens to be in.
    pub cwd: Option<&'a Path>,

    /// A flag or an environment variable, already read by the process the user invoked.
    pub explicit: Option<&'a VersionConstraint>,
}

/// Resolve one runtime for one directory.
///
/// # Errors
///
/// [`Error::NotAbsolute`] for a relative directory; [`Error::Manifest`] for a `mixengine.toml` that
/// does not parse; [`Error::UnreadableProjectRow`] for a project row this build cannot read;
/// [`Error::RuntimeUnresolved`] when nothing installed satisfies what was asked for, and
/// [`Error::NoDefaultRuntime`] when nothing asked for anything and the kind has no default;
/// [`Error::Database`] and [`Error::Io`] when a table or a file cannot be read.
pub async fn runtime(store: &Store, question: &Question<'_>) -> Result<ResolvedRuntime> {
    if let Some(cwd) = question.cwd
        && !cwd.is_absolute()
    {
        return Err(Error::NotAbsolute {
            path: cwd.to_path_buf(),
        });
    }

    let installed = runtimes::records(store, Some(question.kind)).await?;

    let Some(asked) = asked_for(store, question).await? else {
        // Nothing named a version, so the kind's own default is the answer — and a kind with no
        // default has none, which is the state a home is left in by uninstalling the last one.
        let default = installed
            .into_iter()
            .find(|runtime| runtime.default)
            .ok_or(Error::NoDefaultRuntime {
                kind: question.kind,
            })?;

        return Ok(ResolvedRuntime {
            runtime: default,
            source: RuntimeSource::Default,
            constraint: None,
        });
    };

    // The newest of everything that matches, rather than the first: `8.10.0` and `8.9.0` both answer
    // `^8.3`, and only one of them is what somebody pinning a range meant.
    let chosen = installed
        .into_iter()
        .filter(|runtime| asked.constraint.matches(&runtime.version))
        .max_by(|left, right| left.version.cmp_precedence(&right.version));

    match chosen {
        Some(runtime) => {
            tracing::debug!(
                kind = question.kind.as_str(),
                version = runtime.version.as_str(),
                constraint = asked.constraint.as_str(),
                "a runtime was resolved"
            );

            Ok(ResolvedRuntime {
                runtime,
                source: asked.source,
                constraint: Some(asked.constraint),
            })
        }

        None => Err(Error::RuntimeUnresolved {
            kind: question.kind,
            constraint: asked.constraint,
            origin: describe(&asked.source),
        }),
    }
}

/// A constraint and where it was found.
struct Asked {
    /// What was asked for.
    constraint: VersionConstraint,

    /// Which of the four steps produced it.
    source: RuntimeSource,
}

/// Walk the order until something names a version, or nothing does.
async fn asked_for(store: &Store, question: &Question<'_>) -> Result<Option<Asked>> {
    if let Some(explicit) = question.explicit {
        return Ok(Some(Asked {
            constraint: explicit.clone(),
            source: RuntimeSource::Explicit,
        }));
    }

    let Some(cwd) = question.cwd else {
        return Ok(None);
    };

    // **Both walks run to the top before the next one starts**, which is the order the feature spec
    // lists and not the same thing as one walk asking both questions per directory. A file checked
    // into the repository outranks a registration on this machine even when the registration is
    // nearer, because the file is the half a colleague also has.
    if let Some(asked) = in_a_manifest(question.kind, cwd)? {
        return Ok(Some(asked));
    }

    in_a_project(store, question.kind, cwd).await
}

/// The nearest `mixengine.toml` above the directory **that pins this language**.
///
/// A manifest silent about PHP is not an answer about PHP: a repository whose root pins PHP and
/// whose sub-directory pins only Node has said what it wanted about both, and stopping at the
/// nearer file would silently drop the outer pin.
fn in_a_manifest(kind: RuntimeKind, cwd: &Path) -> Result<Option<Asked>> {
    for directory in cwd.ancestors() {
        let path = directory.join(MANIFEST_FILE_NAME);

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            // The ordinary case by far — most directories have no manifest — and a directory that
            // cannot be read at all is treated the same way rather than failing the resolution: the
            // walk passes through other people's directories on the way to the root.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                continue;
            }
            Err(source) => {
                return Err(Error::Io {
                    action: "read",
                    path,
                    source,
                });
            }
        };

        let manifest: Manifest = toml::from_str(&text).map_err(|source| Error::Manifest {
            path: path.clone(),
            source,
        })?;

        if let Some(constraint) = manifest.runtimes.get(&kind) {
            return Ok(Some(Asked {
                constraint: constraint.clone(),
                source: RuntimeSource::Manifest {
                    path: path.display().to_string(),
                },
            }));
        }
    }

    Ok(None)
}

/// The pin held by the nearest registered project at or above the directory.
///
/// One query rather than one per ancestor: `projects` holds a few dozen rows, and the nearest match
/// has to be chosen by walking anyway — a `WHERE root_path IN (…)` would answer the same rows in an
/// order that says nothing about which is nearer.
///
/// **Paths are compared as they were written.** Canonicalising here would be the wrong place for it:
/// the row is written by `project.create`, which is Phase 4's, and normalising on the way *in* is
/// what makes one directory one project — doing it on the way out would leave two spellings able to
/// register twice and only one of them findable.
async fn in_a_project(store: &Store, kind: RuntimeKind, cwd: &Path) -> Result<Option<Asked>> {
    let rows = sqlx::query!("SELECT root_path, runtime_pins_json FROM projects")
        .fetch_all(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    for directory in cwd.ancestors() {
        let Some(row) = rows
            .iter()
            .find(|row| Path::new(&row.root_path) == directory)
        else {
            continue;
        };

        // Read as strings and then parsed one value at a time, rather than straight into a map of
        // our own vocabulary: a row written by a build that manages a fifth language must not stop
        // this one from resolving PHP.
        let pins: BTreeMap<String, String> =
            serde_json::from_str(&row.runtime_pins_json).map_err(|_| {
                Error::UnreadableProjectRow {
                    root: row.root_path.clone(),
                    column: "runtime_pins_json",
                    value: row.runtime_pins_json.clone(),
                }
            })?;

        let Some(pinned) = pins.get(kind.as_str()) else {
            continue;
        };

        let constraint =
            VersionConstraint::parse(pinned.clone()).map_err(|_| Error::UnreadableProjectRow {
                root: row.root_path.clone(),
                column: "runtime_pins_json",
                value: pinned.clone(),
            })?;

        return Ok(Some(Asked {
            constraint,
            source: RuntimeSource::Project {
                root: row.root_path.clone(),
            },
        }));
    }

    Ok(None)
}

/// Where a constraint came from, as a phrase that finishes "asked for by …".
///
/// Part of the message rather than of the source enum, because the wire already carries the source
/// as something a client can branch on — this is the half a person reads when the resolution failed.
fn describe(source: &RuntimeSource) -> String {
    match source {
        RuntimeSource::Explicit => "the version you asked for".to_owned(),
        RuntimeSource::Manifest { path } => path.clone(),
        RuntimeSource::Project { root } => format!("the project at {root}"),
        // Unreachable: the default names an installed version rather than a constraint, so it never
        // reaches a failed match. Written out rather than unreachable!(), because nothing here
        // panics.
        RuntimeSource::Default => "the default for its kind".to_owned(),
    }
}

/// `mixengine.toml`, as far as version resolution is concerned.
///
/// **Only `[runtimes]`, and unknown sections are allowed through.** The file also declares a site,
/// services and a project name — all of them Phase 4's — and a `deny_unknown_fields` here would make
/// this build refuse the very manifests that phase is going to write. What *is* closed is the map
/// itself: a key inside `[runtimes]` naming a language MixEngine does not manage is a pin that would
/// silently do nothing, which is `config.toml`'s rule about typos in the one place it still applies.
#[derive(Debug, Default, serde::Deserialize)]
struct Manifest {
    /// The versions this project wants, by language.
    #[serde(default)]
    runtimes: BTreeMap<RuntimeKind, VersionConstraint>,
}

/// The command that would satisfy a constraint nothing installed does.
///
/// Public because the hint on the wire is the daemon's to write and this is the sentence it needs:
/// a constraint naming one release becomes the install that would produce it, and a range becomes
/// the listing that shows what could be installed instead. Guessing a version for a range would be
/// this crate inventing a release nobody published.
#[must_use]
pub fn install_command(kind: RuntimeKind, constraint: &VersionConstraint) -> String {
    match constraint.exact() {
        Some(version) => {
            format!("`mix runtime install {kind} {version}` installs exactly that one")
        }
        None => format!(
            "`mix runtime available --kind {kind}` lists what could satisfy {constraint}, and \
             `mix runtime install {kind} <version>` installs one"
        ),
    }
}

/// Where this home would look for a manifest, from a directory outwards.
///
/// Public for `mix doctor` and for the GUI's "why this version" panel, both of which show the walk
/// rather than only its answer — a person asking why a pin is not taking effect is asking about the
/// files that were *looked* at.
#[must_use]
pub fn manifest_candidates(cwd: &Path) -> Vec<PathBuf> {
    cwd.ancestors()
        .map(|directory| directory.join(MANIFEST_FILE_NAME))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mixengine_proto::{RuntimeChannel, RuntimeVersion, Timestamp};

    use super::*;
    use crate::runtimes::Installation;

    const NOW: Timestamp = Timestamp(1_760_000_000_000);

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    /// Write the rows an install would have written, without the eighty megabytes.
    async fn install(store: &Store, kind: RuntimeKind, versions: &[&str]) {
        for version in versions {
            runtimes::remember(
                store,
                &Installation {
                    kind,
                    version: RuntimeVersion::parse(*version).expect("a version"),
                    channel: RuntimeChannel::Stable,
                    path: PathBuf::from("/home/runtimes")
                        .join(kind.as_str())
                        .join(version),
                    bytes: 41_000_000,
                    url: format!("https://example.invalid/{kind}-{version}.tar.zst"),
                    sha256: "00".to_owned(),
                    provides: [(kind.as_str().to_owned(), format!("bin/{kind}"))]
                        .into_iter()
                        .collect(),
                },
                NOW,
            )
            .await
            .expect("a row");
        }
    }

    fn constraint(text: &str) -> VersionConstraint {
        VersionConstraint::parse(text).expect("a constraint")
    }

    /// A directory tree under a temporary root, so the walk has somewhere to walk.
    fn tree(depth: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().expect("a temporary directory");
        let mut path = root.path().to_path_buf();
        for name in depth {
            path = path.join(name);
        }
        std::fs::create_dir_all(&path).expect("a directory");

        (root, path)
    }

    fn manifest(directory: &Path, body: &str) {
        std::fs::write(directory.join(MANIFEST_FILE_NAME), body).expect("a manifest");
    }

    /// The whole order in one test, because the order *is* the contract: each step is added on top
    /// of the last and takes it over.
    #[tokio::test]
    async fn each_source_takes_precedence_over_the_one_below_it() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.1.30", "8.2.20", "8.3.33"]).await;
        let (_root, cwd) = tree(&["blog", "public"]);

        // 4 — the default, which the first install became.
        let resolved = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: Some(&cwd),
                explicit: None,
            },
        )
        .await
        .expect("a default");
        assert_eq!(resolved.runtime.version.as_str(), "8.1.30");
        assert_eq!(resolved.source, RuntimeSource::Default);
        assert_eq!(resolved.constraint, None, "a default names no constraint");

        // 3 — a registered project, whose root is above the directory.
        let root = cwd.parent().expect("a parent").display().to_string();
        sqlx::query(
            "INSERT INTO projects (name, root_path, runtime_pins_json, created_at)
             VALUES ('blog', ?, '{\"php\": \"8.2\", \"go\": \"1.22\"}', '2026-08-14T06:55:12Z')",
        )
        .bind(&root)
        .execute(store.pool())
        .await
        .expect("a project");

        let resolved = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: Some(&cwd),
                explicit: None,
            },
        )
        .await
        .expect("a project pin");
        assert_eq!(resolved.runtime.version.as_str(), "8.2.20");
        assert_eq!(
            resolved.source,
            RuntimeSource::Project { root },
            "a language this build does not manage sitting beside the pin changes nothing"
        );

        // 2 — a manifest, which outranks the project record even from further up.
        manifest(
            cwd.parent().expect("a parent"),
            "[runtimes]\nphp = \"^8.3\"\n",
        );
        let resolved = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: Some(&cwd),
                explicit: None,
            },
        )
        .await
        .expect("a manifest pin");
        assert_eq!(resolved.runtime.version.as_str(), "8.3.33");
        assert!(matches!(resolved.source, RuntimeSource::Manifest { .. }));
        assert_eq!(
            resolved.constraint.as_ref().map(VersionConstraint::as_str),
            Some("^8.3")
        );

        // 1 — what the caller was told, which beats everything the directory says.
        let resolved = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: Some(&cwd),
                explicit: Some(&constraint("8.1.30")),
            },
        )
        .await
        .expect("a flag");
        assert_eq!(resolved.runtime.version.as_str(), "8.1.30");
        assert_eq!(resolved.source, RuntimeSource::Explicit);
    }

    /// The reason this needed a grammar at all: choosing between two installed versions.
    #[tokio::test]
    async fn a_range_resolves_to_the_newest_installed_version_that_answers_it() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.9", "8.3.33", "8.4.1"]).await;
        let (_root, cwd) = tree(&["blog"]);

        for (asked, expected) in [("8.3", "8.3.33"), ("^8.3", "8.4.1"), ("8.3.9", "8.3.9")] {
            let resolved = runtime(
                &store,
                &Question {
                    kind: RuntimeKind::Php,
                    cwd: Some(&cwd),
                    explicit: Some(&constraint(asked)),
                },
            )
            .await
            .unwrap_or_else(|error| panic!("{asked}: {error}"));

            assert_eq!(
                resolved.runtime.version.as_str(),
                expected,
                "{asked} should have chosen {expected}"
            );
        }
    }

    /// A manifest that says nothing about this language is not an answer about it — the outer pin
    /// still is.
    #[tokio::test]
    async fn the_nearest_manifest_that_names_the_language_is_the_one_that_wins() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.1.30", "8.3.33"]).await;
        let (root, cwd) = tree(&["blog", "public"]);

        manifest(root.path(), "[runtimes]\nphp = \"8.3\"\n");
        manifest(
            cwd.parent().expect("a parent"),
            "[project]\nname = \"blog\"\n\n[runtimes]\nnode = \"22\"\n",
        );

        let resolved = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: Some(&cwd),
                explicit: None,
            },
        )
        .await
        .expect("the outer manifest");

        assert_eq!(resolved.runtime.version.as_str(), "8.3.33");
        assert!(
            matches!(&resolved.source, RuntimeSource::Manifest { path } if path.starts_with(&root.path().display().to_string())),
            "{:?} should be the manifest at the root",
            resolved.source
        );
    }

    /// Sections this build does not read yet must not make it refuse the file — Phase 4 writes them.
    #[tokio::test]
    async fn a_manifest_declaring_more_than_runtimes_is_still_read() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.33"]).await;
        let (_root, cwd) = tree(&["blog"]);

        manifest(
            &cwd,
            "[project]\nname = \"blog\"\n\n[runtimes]\nphp = \"8.3\"\n\n\
             [site]\ndomain = \"blog.test\"\n\n[[services]]\nname = \"redis\"\n",
        );

        let resolved = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: Some(&cwd),
                explicit: None,
            },
        )
        .await
        .expect("only [runtimes] is read");

        assert_eq!(resolved.runtime.version.as_str(), "8.3.33");
    }

    /// A pin that does nothing looks exactly like a pin that does not work, which is `config.toml`'s
    /// rule and the one place it still applies inside this file.
    #[tokio::test]
    async fn a_manifest_that_does_not_parse_names_itself() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.33"]).await;
        let (_root, cwd) = tree(&["blog"]);

        for body in [
            "[runtimes]\nphp = \"~8.3\"\n",
            "[runtimes]\nphhp = \"8.3\"\n",
            "[runtimes\n",
        ] {
            manifest(&cwd, body);

            let error = runtime(
                &store,
                &Question {
                    kind: RuntimeKind::Php,
                    cwd: Some(&cwd),
                    explicit: None,
                },
            )
            .await
            .expect_err("the manifest is wrong");

            assert!(
                matches!(&error, Error::Manifest { path, .. } if path.ends_with(MANIFEST_FILE_NAME)),
                "{error:?} for {body:?}"
            );
        }
    }

    /// What somebody sees when the version their project asks for is not on this machine — and the
    /// half of it that tells them what to type.
    #[tokio::test]
    async fn a_constraint_nothing_installed_satisfies_names_what_would_satisfy_it() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.33"]).await;
        let (_root, cwd) = tree(&["blog"]);
        manifest(&cwd, "[runtimes]\nphp = \"8.1.30\"\n");

        let error = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: Some(&cwd),
                explicit: None,
            },
        )
        .await
        .expect_err("8.1.30 is not installed");

        let said = error.to_string();
        assert!(
            said.contains("8.1.30") && said.contains(MANIFEST_FILE_NAME),
            "{said}"
        );
        assert!(
            matches!(&error, Error::RuntimeUnresolved { constraint, .. }
                if install_command(RuntimeKind::Php, constraint).contains("install php 8.1.30")),
            "an exact constraint becomes the exact command: {error:?}"
        );

        // A range cannot become one, because inventing the version would be inventing a release.
        assert!(
            install_command(RuntimeKind::Php, &constraint("^8.9")).contains("runtime available"),
            "a range sends somebody to the listing instead"
        );
    }

    /// The state a home is left in by uninstalling the last version of a kind, asked about.
    #[tokio::test]
    async fn a_kind_with_no_default_says_so_rather_than_choosing_one() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.33"]).await;
        runtimes::forget(
            &store,
            RuntimeKind::Php,
            &RuntimeVersion::parse("8.3.33").expect("a version"),
        )
        .await
        .expect("the row");

        let error = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: None,
                explicit: None,
            },
        )
        .await
        .expect_err("nothing is installed");

        assert!(matches!(error, Error::NoDefaultRuntime { .. }), "{error:?}");
    }

    /// A relative directory would be walked from wherever the *daemon* was started, which is nobody's
    /// project — so it is refused rather than answered.
    #[tokio::test]
    async fn a_directory_that_is_not_absolute_is_refused() {
        let (_home, store) = store().await;
        install(&store, RuntimeKind::Php, &["8.3.33"]).await;

        let error = runtime(
            &store,
            &Question {
                kind: RuntimeKind::Php,
                cwd: Some(Path::new("blog/public")),
                explicit: None,
            },
        )
        .await
        .expect_err("a relative directory means nothing here");

        assert!(matches!(error, Error::NotAbsolute { .. }), "{error:?}");
    }

    /// The walk is the thing a person asking "why is it not taking my pin" needs to see.
    #[test]
    fn the_files_a_walk_would_look_at_are_listed_from_the_directory_outwards() {
        let candidates = manifest_candidates(Path::new(if cfg!(windows) {
            r"C:\srv\blog\public"
        } else {
            "/srv/blog/public"
        }));

        assert!(candidates.len() >= 3, "{candidates:?}");
        assert!(candidates[0].ends_with(PathBuf::from("public").join(MANIFEST_FILE_NAME)));
        assert!(candidates[1].ends_with(PathBuf::from("blog").join(MANIFEST_FILE_NAME)));
    }
}
