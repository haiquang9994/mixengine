//! `mixengine.toml` — the file a project pins its runtimes in, read and written in one place.
//!
//! **One reader** (spec D9). [`crate::resolve`] used to deserialise a deliberately narrow struct of
//! its own: `[runtimes]` and nothing else. T39 needs `[project] name` on import and a writer for
//! export, and two structs describing one file would be two answers to one question — so the narrow
//! one is gone and `resolve` is a caller.
//!
//! **Unknown sections are allowed through**, exactly as they were: the file also declares a site and
//! its services, which are T39a's, and a `deny_unknown_fields` here would make this build refuse the
//! manifests that task is going to write. What is still closed is the map inside `[runtimes]`: a key
//! naming a language MixEngine does not manage is a pin that would silently do nothing, which is
//! `config.toml`'s rule about typos in the one place it still applies.
//!
//! # The writer edits; it does not rewrite
//!
//! This file lives in the user's repository, under version control, with their comments and their
//! key order in it — and, after T39a, a `[site]` block they wrote by hand. Serialising a fresh
//! document over it would destroy all of that, and would do it to the one file whose entire purpose
//! is to be read by a person. So [`write`] edits a `toml_edit` document: it sets `[project] name`
//! and the `[runtimes]` keys it owns, and leaves every other byte alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mixengine_proto::{RuntimeKind, VersionConstraint};

use crate::{Error, Result};

/// The file a project pins its runtimes in, checked into the user's repository.
pub const FILE_NAME: &str = "mixengine.toml";

/// `mixengine.toml`, as this build understands it.
///
/// Two sections and no catch-all: what the writer preserves it preserves through the document it
/// edits rather than through a field nothing reads, so `[site]` and `[[services]]` survive an export
/// without this type having to hold them until T39a gives them meaning.
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Manifest {
    /// `[project]`, when the file has one.
    #[serde(default)]
    pub project: Option<Project>,

    /// The versions this project wants, by language.
    #[serde(default)]
    pub runtimes: BTreeMap<RuntimeKind, VersionConstraint>,
}

/// `[project]`.
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Project {
    /// What the project is called, when the file says.
    #[serde(default)]
    pub name: Option<String>,
}

/// Where a directory's manifest is.
#[must_use]
pub fn at(directory: &Path) -> PathBuf {
    directory.join(FILE_NAME)
}

/// Read one, or [`None`] where there is none to read.
///
/// **A file that cannot be opened is treated as one that is not there**, which is the rule
/// [`crate::resolve`] has always followed and the reason it can walk to the root: the ancestor walk
/// passes through other people's directories on the way up, and a permission error three levels
/// above somebody's project is not a fact about their project.
///
/// # Errors
///
/// [`Error::Manifest`] for a file that does not parse — including a `[runtimes]` key naming a
/// language this build does not manage — and [`Error::Io`] for a read that failed some other way.
pub fn read(path: &Path) -> Result<Option<Manifest>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(None);
        }
        Err(source) => {
            return Err(Error::Io {
                action: "read",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    toml::from_str(&text)
        .map(Some)
        .map_err(|source| Error::Manifest {
            path: path.to_path_buf(),
            source,
        })
}

/// Set `[project] name` and these `[runtimes]` keys in `<directory>/mixengine.toml`.
///
/// Answers whether the file had to be created. Keys this call does not name are left as they are —
/// a pin the user wrote and MixEngine does not know about is still theirs.
///
/// # Errors
///
/// [`Error::Manifest`] for an existing file that does not parse — refused before a byte is written,
/// so a broken manifest is never made worse — [`Error::ManifestEdit`] for one that parses as TOML
/// but not as a document this can edit, and [`Error::Io`] when the file cannot be read or written.
pub fn write(
    directory: &Path,
    name: &str,
    pins: &BTreeMap<RuntimeKind, VersionConstraint>,
) -> Result<bool> {
    let path = at(directory);

    // Validated through the reader first, so the failure a caller sees for a broken file is the
    // same `Error::Manifest` every other door gives it, naming the same path.
    let created = read(&path)?.is_none() && !path.exists();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::Io {
                action: "read",
                path,
                source,
            });
        }
    };

    let mut document: toml_edit::DocumentMut =
        text.parse()
            .map_err(|error: toml_edit::TomlError| Error::ManifestEdit {
                path: path.clone(),
                reason: error.to_string(),
            })?;

    set(&mut document, "project", |table| {
        table["name"] = toml_edit::value(name);
    });

    set(&mut document, "runtimes", |table| {
        for (kind, constraint) in pins {
            table[kind.as_str()] = toml_edit::value(constraint.as_str());
        }
    });

    std::fs::write(&path, document.to_string()).map_err(|source| Error::Io {
        action: "write",
        path: path.clone(),
        source,
    })?;

    tracing::info!(path = %path.display(), created, "a project manifest was written");

    Ok(created)
}

/// Reach one top-level table, creating it if the file has none, and edit it.
///
/// `set_implicit(false)` is what makes a created table render its own `[header]`: a table
/// `toml_edit` believes is implicit is one it prints only through its children, and a `[project]`
/// that never appears is a file the reader is right about and a person is confused by.
fn set(
    document: &mut toml_edit::DocumentMut,
    section: &str,
    edit: impl FnOnce(&mut toml_edit::Table),
) {
    let item = document
        .entry(section)
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));

    if let Some(table) = item.as_table_mut() {
        table.set_implicit(false);
        edit(table);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn somewhere() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary directory")
    }

    fn pins(entries: &[(RuntimeKind, &str)]) -> BTreeMap<RuntimeKind, VersionConstraint> {
        entries
            .iter()
            .map(|(kind, text)| {
                (
                    *kind,
                    VersionConstraint::parse((*text).to_owned()).expect("a constraint"),
                )
            })
            .collect()
    }

    /// The whole file, where `resolve` used to read a third of it — and the sections this build has
    /// no types for still must not make it refuse the file.
    #[test]
    fn a_manifest_declaring_more_than_runtimes_is_read_whole() {
        let home = somewhere();
        std::fs::write(
            at(home.path()),
            "[project]\nname = \"blog\"\n\n[runtimes]\nphp = \"^8.3\"\n\n\
             [site]\ndomain = \"blog.test\"\n\n[[services]]\nname = \"redis\"\n",
        )
        .expect("a manifest");

        let manifest = read(&at(home.path()))
            .expect("it parses")
            .expect("it is there");

        assert_eq!(
            manifest.project.and_then(|project| project.name).as_deref(),
            Some("blog")
        );
        assert_eq!(
            manifest
                .runtimes
                .get(&RuntimeKind::Php)
                .map(VersionConstraint::as_str),
            Some("^8.3")
        );
    }

    /// A directory with no manifest is not a failure — it is the ordinary case.
    #[test]
    fn a_directory_with_no_manifest_answers_nothing_rather_than_failing() {
        let home = somewhere();

        assert_eq!(read(&at(home.path())).expect("no manifest is fine"), None);
    }

    /// A pin that does nothing looks exactly like a pin that does not work, so the file is refused
    /// by name — `Error::Manifest`'s own reasoning, kept when the reader moved here.
    #[test]
    fn a_manifest_that_does_not_parse_names_itself() {
        let home = somewhere();

        for body in [
            "[runtimes]\nphp = \"~8.3\"\n",
            "[runtimes]\nphhp = \"8.3\"\n",
            "[runtimes\n",
        ] {
            std::fs::write(at(home.path()), body).expect("a manifest");

            let error = read(&at(home.path())).expect_err("the manifest is wrong");

            assert!(
                matches!(&error, Error::Manifest { path, .. } if path.ends_with(FILE_NAME)),
                "{error:?} for {body:?}"
            );
        }
    }

    /// **What D10 is for.** An export is written into somebody's version-controlled file, so
    /// everything it does not own survives it byte for byte.
    #[test]
    fn writing_a_manifest_keeps_the_comments_the_order_and_the_sections_it_does_not_own() {
        let home = somewhere();
        let original = "# the blog\n\
                        [runtimes]\n\
                        node = \"22\"      # the front end build\n\
                        php = \"8.2\"\n\n\
                        [site]\n\
                        domain = \"blog.test\"\n\n\
                        [[services]]\n\
                        name = \"redis\"\n";
        std::fs::write(at(home.path()), original).expect("a manifest");

        let created = write(home.path(), "blog", &pins(&[(RuntimeKind::Php, "^8.3")]))
            .expect("it is written");

        let after = std::fs::read_to_string(at(home.path())).expect("the file");

        assert!(!created, "the file was already there");
        assert!(after.contains("# the blog"), "{after}");
        assert!(after.contains("# the front end build"), "{after}");
        assert!(
            after.contains("[site]") && after.contains("blog.test"),
            "{after}"
        );
        assert!(after.contains("[[services]]"), "{after}");
        assert!(
            after.find("node =").expect("node") < after.find("php =").expect("php"),
            "the key order the user chose is theirs: {after}"
        );
        assert!(
            after.contains("php = \"^8.3\""),
            "the owned key changed: {after}"
        );
        assert!(
            after.contains("name = \"blog\""),
            "the name was written: {after}"
        );

        // And what it wrote is what the reader reads back.
        let manifest = read(&at(home.path()))
            .expect("it parses")
            .expect("it is there");
        assert_eq!(
            manifest
                .runtimes
                .get(&RuntimeKind::Php)
                .map(VersionConstraint::as_str),
            Some("^8.3")
        );
    }

    /// A directory with no manifest gets one, and says that it did.
    #[test]
    fn a_directory_with_no_manifest_gets_one_written() {
        let home = somewhere();

        let created =
            write(home.path(), "blog", &pins(&[(RuntimeKind::Php, "8.3")])).expect("it is written");

        assert!(created);
        assert_eq!(
            read(&at(home.path()))
                .expect("it parses")
                .expect("it is there")
                .project
                .and_then(|project| project.name)
                .as_deref(),
            Some("blog")
        );
    }
}
