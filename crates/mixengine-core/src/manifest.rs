//! `mixengine.toml` — the file a project pins its runtimes in, read and written in one place.
//!
//! **One reader** (spec D9). [`crate::resolve`] used to deserialise a deliberately narrow struct of
//! its own: `[runtimes]` and nothing else. T39 needs `[project] name` on import and a writer for
//! export, and two structs describing one file would be two answers to one question — so the narrow
//! one is gone and `resolve` is a caller.
//!
//! **Unknown sections are still allowed through.** `[site]` and `[[services]]` have types as of
//! T39a, but the file also has to hold what T43 and Phase 8 will add, and a `deny_unknown_fields`
//! here would make this build refuse the manifests those tasks write. What is still closed is the
//! map inside `[runtimes]` — a key naming a language MixEngine does not manage is a pin that would
//! silently do nothing — and the two typed sections' own required keys.
//!
//! # The writer edits; it does not rewrite
//!
//! This file lives in the user's repository, under version control, with their comments and their
//! key order in it — and, after T39a, a `[site]` block they wrote by hand. Serialising a fresh
//! document over it would destroy all of that, and would do it to the one file whose entire purpose
//! is to be read by a person. So [`write()`] edits a `toml_edit` document: it sets `[project] name`
//! and the `[runtimes]` keys it owns, and leaves every other byte alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mixengine_proto::{PackageVersion, RuntimeKind, SiteKind, VersionConstraint};

use crate::{Error, Result};

/// The file a project pins its runtimes in, checked into the user's repository.
pub const FILE_NAME: &str = "mixengine.toml";

/// `mixengine.toml`, as this build understands it.
///
/// Four sections and no catch-all: what the writer preserves beyond them it preserves through the
/// document it edits rather than through a field nothing reads, so a section T43 adds survives an
/// export without this type having to hold it.
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Manifest {
    /// `[project]`, when the file has one.
    #[serde(default)]
    pub project: Option<Project>,

    /// The versions this project wants, by language.
    #[serde(default)]
    pub runtimes: BTreeMap<RuntimeKind, VersionConstraint>,

    /// `[site]`, when the file declares one.
    #[serde(default)]
    pub site: Option<ManifestSite>,

    /// `[[services]]`, in the order the file lists them.
    #[serde(default)]
    pub services: Vec<ManifestService>,
}

/// `[site]` — what is served out of this directory, and at what name.
///
/// Every field is optional because every one of them falls through to a default the daemon knows
/// (spec D7): a manifest saying only `domain = "blog.test"` is a whole declaration.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ManifestSite {
    /// The primary domain.
    pub domain: Option<String>,

    /// Every other name it answers to.
    pub aliases: Vec<String>,

    /// Relative to the project's root.
    pub doc_root: Option<String>,

    /// Whether HTTPS is wanted.
    pub https: Option<bool>,

    /// What it serves, when the file says.
    ///
    /// Read from the **whole** `[site]` table rather than from a nested one, because
    /// [`SiteKind`] is internally tagged and its TOML representation is `kind = "reverse-proxy"`
    /// sitting flat beside `upstream = "…"`. One type reads the file and the wire, with nothing in
    /// between to drift.
    pub kind: Option<SiteKind>,
}

impl<'de> serde::Deserialize<'de> for ManifestSite {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;

        let table = toml::Table::deserialize(deserializer)?;

        // Read before the kind, because deserialising the kind consumes a clone of the whole table
        // and these four keys are not its business.
        let text = |key: &str| {
            table
                .get(key)
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        };

        let aliases = table
            .get("aliases")
            .and_then(|value| value.as_array())
            .map(|array| {
                array
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| D::Error::custom("an alias is a string"))
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();

        // Absent is `None`; present and wrong is the file being wrong, which the enum decides.
        let kind = match table.contains_key("kind") {
            true => Some(SiteKind::deserialize(table.clone()).map_err(D::Error::custom)?),
            false => None,
        };

        Ok(Self {
            domain: text("domain"),
            aliases,
            doc_root: text("doc_root"),
            https: table.get("https").and_then(toml::Value::as_bool),
            kind,
        })
    }
}

/// One `[[services]]` entry.
///
/// **`database` and `user` are not here.** This build creates no databases, and a key read and then
/// quietly ignored is a promise not kept — so they pass through untouched and survive the writer
/// untouched (spec D8). Provisioning is Phase 8's `blueprint.apply`.
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct ManifestService {
    /// The package, which is the first half of a [`mixengine_proto::ServiceId`].
    pub name: String,

    /// The instance, when the file names one.
    ///
    /// Absent is **not** the same as `"main"`: what an absent instance means is decided by the
    /// lookup, which tries the bare name — what a single-instance package such as `caddy` is
    /// actually called — before `name@main`.
    #[serde(default)]
    pub instance: Option<String>,

    /// The version wanted. Its *syntax* is refused here; whether anything satisfies it is the
    /// daemon's question and is reported rather than refused.
    #[serde(default)]
    pub version: Option<VersionConstraint>,
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

/// What `project.export` puts into somebody's repository.
///
/// A struct rather than a widening argument list: this is one thing — *the project, as a colleague
/// should receive it* — and a signature whose arguments have to be counted is one a caller gets
/// wrong silently.
#[derive(Debug, Clone)]
pub struct Export {
    /// `[project] name`.
    pub name: String,

    /// `[runtimes]`, the keys this export owns.
    pub pins: BTreeMap<RuntimeKind, VersionConstraint>,

    /// The project's site, when it has exactly one.
    ///
    /// **A manifest holds one `[site]`** — a limit of the file format rather than of the model — so
    /// a project with two writes none, and the omitted names come back in `ProjectExport`.
    pub site: Option<ExportSite>,
}

/// `[site]`, as an export writes it.
#[derive(Debug, Clone)]
pub struct ExportSite {
    /// The primary.
    pub domain: String,
    /// Every other name.
    pub aliases: Vec<String>,
    /// Relative, as stored.
    pub doc_root: String,
    /// Whether HTTPS is declared.
    pub https: bool,
    /// What it serves, written from an exhaustive match so a fifth kind cannot be forgotten.
    pub kind: SiteKind,
    /// The services it declares.
    pub services: Vec<ExportService>,
}

/// One `[[services]]` entry, as an export writes it.
#[derive(Debug, Clone)]
pub struct ExportService {
    /// The package.
    pub name: String,
    /// The instance, spelled out even when it is `main`, because the file is read by a person.
    pub instance: String,
    /// What is installed here, so a colleague knows what to install. Omitted when it cannot be read.
    pub version: Option<PackageVersion>,
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
pub fn write(directory: &Path, export: &Export) -> Result<bool> {
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
        table["name"] = toml_edit::value(export.name.as_str());
    });

    set(&mut document, "runtimes", |table| {
        for (kind, constraint) in &export.pins {
            table[kind.as_str()] = toml_edit::value(constraint.as_str());
        }
    });

    if let Some(site) = &export.site {
        set(&mut document, "site", |table| {
            table["domain"] = toml_edit::value(site.domain.as_str());

            // Written even when empty, because an alias removed in the database and left in the
            // file would be a file that disagrees with the home it came from — and `aliases` is a
            // key this export owns outright, unlike an entry of `[[services]]`.
            let mut aliases = toml_edit::Array::new();
            for alias in &site.aliases {
                aliases.push(alias.as_str());
            }
            table["aliases"] = toml_edit::value(aliases);

            table["doc_root"] = toml_edit::value(site.doc_root.as_str());
            table["https"] = toml_edit::value(site.https);

            // Exhaustive, so a fifth kind is a compile error here rather than a key silently
            // missing from somebody's manifest.
            match &site.kind {
                SiteKind::PhpFpm { .. } => {
                    table["kind"] = toml_edit::value("php-fpm");
                }
                SiteKind::Static => {
                    table["kind"] = toml_edit::value("static");
                }
                SiteKind::ReverseProxy { upstream } => {
                    table["kind"] = toml_edit::value("reverse-proxy");
                    table["upstream"] = toml_edit::value(upstream.as_str());
                }
                SiteKind::NodeApp { port } => {
                    table["kind"] = toml_edit::value("node-app");
                    table["port"] = toml_edit::value(i64::from(*port));
                }
            }
        });

        merge_services(&mut document, &site.services);
    }

    std::fs::write(&path, document.to_string()).map_err(|source| Error::Io {
        action: "write",
        path: path.clone(),
        source,
    })?;

    tracing::info!(path = %path.display(), created, "a project manifest was written");

    Ok(created)
}

/// Add and update `[[services]]`; never delete.
///
/// **The honest consequence, stated rather than discovered: an export is a merge, not a mirror.**
/// Removing a link in the database does not remove its line from the file, and there is no
/// `--prune`. The alternative is an export that deletes a hand-written `database = "blog"` from a
/// file under version control, which is not a trade this makes.
///
/// Identity is `name` plus `instance`, with an absent `instance` in the file matching `main` —
/// the same rule the reader's lookup follows, so an export does not create a second entry for a
/// service the file already names.
fn merge_services(document: &mut toml_edit::DocumentMut, services: &[ExportService]) {
    let array = document
        .entry("services")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));

    let Some(array) = array.as_array_of_tables_mut() else {
        // The file calls `services` something other than an array of tables. Left alone: this
        // export does not own the key badly enough to overwrite whatever a person meant by it.
        return;
    };

    for service in services {
        let existing = array.iter_mut().find(|table| {
            table.get("name").and_then(|value| value.as_str()) == Some(service.name.as_str())
                && table
                    .get("instance")
                    .and_then(|value| value.as_str())
                    .unwrap_or("main")
                    == service.instance
        });

        let table = match existing {
            Some(table) => table,
            None => {
                let mut fresh = toml_edit::Table::new();
                fresh["name"] = toml_edit::value(service.name.as_str());
                fresh["instance"] = toml_edit::value(service.instance.as_str());
                array.push(fresh);
                array.iter_mut().last().expect("the table just pushed")
            }
        };

        if let Some(version) = &service.version {
            table["version"] = toml_edit::value(version.as_str());
        }
    }
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

    fn export(site: Option<ExportSite>) -> Export {
        Export {
            name: "blog".to_owned(),
            pins: pins(&[(RuntimeKind::Php, "^8.3")]),
            site,
        }
    }

    /// **D9.** The export writes the site, and everything the daemon does not own survives it —
    /// including a hand-written key inside an entry it *does* update.
    #[test]
    fn an_export_writes_the_site_and_leaves_every_hand_written_key_alone() {
        let home = somewhere();
        let original = "# the blog\n\
                        [runtimes]\n\
                        php = \"8.2\"\n\n\
                        [[services]]\n\
                        name = \"mariadb\"\n\
                        instance = \"main\"\n\
                        version = \"11.3\"\n\
                        database = \"blog\"      # provisioned by hand, for now\n\n\
                        [[services]]\n\
                        name = \"meilisearch\"   # nothing in this build knows what this is\n";
        std::fs::write(at(home.path()), original).expect("a manifest");

        write(
            home.path(),
            &export(Some(ExportSite {
                domain: "blog.test".to_owned(),
                aliases: vec!["api.blog.test".to_owned()],
                doc_root: "public".to_owned(),
                https: true,
                kind: mixengine_proto::SiteKind::PhpFpm { pool: None },
                services: vec![ExportService {
                    name: "mariadb".to_owned(),
                    instance: "main".to_owned(),
                    version: Some(PackageVersion::parse("11.4.2").expect("a version")),
                }],
            })),
        )
        .expect("it is written");

        let after = std::fs::read_to_string(at(home.path())).expect("the file");

        assert!(after.contains("# the blog"), "{after}");
        assert!(after.contains("domain = \"blog.test\""), "{after}");
        assert!(after.contains("aliases = [\"api.blog.test\"]"), "{after}");
        assert!(after.contains("doc_root = \"public\""), "{after}");
        assert!(after.contains("kind = \"php-fpm\""), "{after}");
        assert!(after.contains("https = true"), "{after}");

        // Add and update; never delete.
        assert!(
            after.contains("version = \"11.4.2\""),
            "the link was updated: {after}"
        );
        assert!(
            after.contains("database = \"blog\""),
            "a hand-written key inside an updated entry survives: {after}"
        );
        assert!(
            after.contains("meilisearch"),
            "an entry the daemon knows nothing about is left exactly as it is: {after}"
        );
        assert!(after.contains("# provisioned by hand"), "{after}");
    }

    /// A kind's payload is written from an exhaustive match, so a fifth kind cannot be forgotten.
    #[test]
    fn a_proxy_is_written_with_the_address_it_forwards_to() {
        let home = somewhere();

        write(
            home.path(),
            &export(Some(ExportSite {
                domain: "app.test".to_owned(),
                aliases: Vec::new(),
                doc_root: String::new(),
                https: false,
                kind: mixengine_proto::SiteKind::ReverseProxy {
                    upstream: "http://127.0.0.1:5173".to_owned(),
                },
                services: Vec::new(),
            })),
        )
        .expect("it is written");

        let after = std::fs::read_to_string(at(home.path())).expect("the file");

        assert!(after.contains("kind = \"reverse-proxy\""), "{after}");
        assert!(
            after.contains("upstream = \"http://127.0.0.1:5173\""),
            "{after}"
        );

        // And it reads back as the kind it was written from.
        let read_back = read(&at(home.path()))
            .expect("it parses")
            .expect("it is there")
            .site
            .expect("a [site]");
        assert_eq!(
            read_back.kind,
            Some(mixengine_proto::SiteKind::ReverseProxy {
                upstream: "http://127.0.0.1:5173".to_owned()
            })
        );
    }

    /// A project with no site writes no `[site]`, and does not delete one somebody wrote by hand.
    #[test]
    fn an_export_with_no_site_leaves_a_hand_written_one_alone() {
        let home = somewhere();
        std::fs::write(at(home.path()), "[site]\ndomain = \"typed.test\"\n").expect("a manifest");

        write(home.path(), &export(None)).expect("it is written");

        let after = std::fs::read_to_string(at(home.path())).expect("the file");
        assert!(after.contains("typed.test"), "{after}");
    }

    /// **D7.** `[site]` with no `kind` is a manifest this build has always accepted, and the type
    /// has to keep accepting it.
    #[test]
    fn a_site_with_no_kind_reads_as_one_that_named_none() {
        let home = somewhere();
        std::fs::write(
            at(home.path()),
            "[site]\ndomain = \"blog.test\"\naliases = [\"api.blog.test\"]\ndoc_root = \"public\"\n",
        )
        .expect("a manifest");

        let site = read(&at(home.path()))
            .expect("it parses")
            .expect("it is there")
            .site
            .expect("a [site]");

        assert_eq!(site.domain.as_deref(), Some("blog.test"));
        assert_eq!(site.aliases, ["api.blog.test"]);
        assert_eq!(site.doc_root.as_deref(), Some("public"));
        assert_eq!(site.kind, None, "no kind is not the same as php-fpm");
        assert_eq!(site.https, None);
    }

    /// A kind reads out of the flat table beside the keys it has no use for.
    #[test]
    fn a_kind_reads_from_the_table_it_shares_with_the_rest_of_the_site() {
        let home = somewhere();
        std::fs::write(
            at(home.path()),
            "[site]\ndomain = \"blog.test\"\nkind = \"reverse-proxy\"\n\
             upstream = \"http://127.0.0.1:5173\"\nhttps = true\n",
        )
        .expect("a manifest");

        let site = read(&at(home.path()))
            .expect("it parses")
            .expect("it is there")
            .site
            .expect("a [site]");

        assert_eq!(
            site.kind,
            Some(mixengine_proto::SiteKind::ReverseProxy {
                upstream: "http://127.0.0.1:5173".to_owned()
            })
        );
        assert_eq!(site.https, Some(true));
    }

    /// And a kind that cannot be one is refused by the enum, naming the file.
    #[test]
    fn a_proxy_with_no_upstream_is_refused_by_the_definition() {
        let home = somewhere();
        std::fs::write(
            at(home.path()),
            "[site]\ndomain = \"blog.test\"\nkind = \"reverse-proxy\"\n",
        )
        .expect("a manifest");

        let error = read(&at(home.path())).expect_err("a proxy with nowhere to go");

        assert!(
            matches!(&error, Error::Manifest { path, .. } if path.ends_with(FILE_NAME)),
            "{error:?}"
        );
    }

    /// **D8.** `[[services]]` is read as a name, an instance and a constraint — and the keys this
    /// build does not interpret survive being read past.
    #[test]
    fn services_are_read_as_names_instances_and_constraints() {
        let home = somewhere();
        std::fs::write(
            at(home.path()),
            "[[services]]\nname = \"mariadb\"\ninstance = \"main\"\nversion = \"11.4\"\n\
             database = \"blog\"\n\n[[services]]\nname = \"redis\"\n",
        )
        .expect("a manifest");

        let services = read(&at(home.path()))
            .expect("it parses")
            .expect("it is there")
            .services;

        assert_eq!(services.len(), 2);
        assert_eq!(services[0].name, "mariadb");
        assert_eq!(services[0].instance.as_deref(), Some("main"));
        assert_eq!(
            services[0].version.as_ref().map(VersionConstraint::as_str),
            Some("11.4")
        );
        assert_eq!(
            services[1].instance, None,
            "absent is not the same as \"main\""
        );
        assert_eq!(services[1].version, None);
    }

    /// A `version` whose *syntax* is wrong is the file being wrong, and is refused. Whether
    /// anything installed satisfies it is a different question, asked by the daemon and reported.
    #[test]
    fn a_version_that_is_not_a_constraint_is_refused() {
        let home = somewhere();
        std::fs::write(
            at(home.path()),
            "[[services]]\nname = \"mariadb\"\nversion = \"~11.4\"\n",
        )
        .expect("a manifest");

        assert!(matches!(
            read(&at(home.path())),
            Err(Error::Manifest { .. })
        ));
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

        let created = write(home.path(), &export(None)).expect("it is written");

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

        let created = write(home.path(), &export(None)).expect("it is written");

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
