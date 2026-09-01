//! The blueprint manifest: `schema`, `[blueprint]`, `[runtimes]`, `[site]`, `[[services]]`,
//! `[php]` and `[scaffold]`.
//!
//! **Its own type rather than `mixengine.toml`'s** — the T77 design, D1. The two files overlap but
//! are not one: a blueprint carries `domain_pattern` where a project manifest carries `domain` and
//! `aliases`, and it carries `database` and `user`, which the project manifest does not interpret.
//! They also have two lifetimes. `mixengine.toml` is written by a person, lives in their repository
//! under their comments and is edited byte-preservingly by [`crate::manifest::write`]; a blueprint
//! is generated, read once and thrown away. One struct serving both would make every key an
//! `Option` and hand the comment-preserving writer a second file shape to preserve.
//!
//! **There is no `[php] ini`** (D2). Every ini value MixEngine writes is a constant in
//! [`crate::runtimes::extensions`] — the same `memory_limit = 512M` on every machine this product
//! runs on — so there is no deviation to capture, and capturing it would be capturing a global
//! default, which is the one thing this task is defined against. The key arrives with the task that
//! gives a project an ini of its own.

use std::collections::BTreeMap;

use mixengine_proto::{RuntimeKind, SiteKind, VersionConstraint};

use crate::{Error, Result};

/// The only schema this build writes, and the highest one it reads.
pub const SCHEMA: u32 = 1;

/// The instance name that means "one of this project's own".
///
/// A word rather than the captured instance's literal name, and this is the trap D4 exists for: a
/// project `blog` using `mariadb@blog` has a *dedicated* server, and copying that name into the
/// blueprint would make applying it as `shop` plug the new project into the old one's database.
pub const PER_PROJECT: &str = "per-project";

/// A blueprint, as its file says it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct BlueprintManifest {
    /// The format version. Refused on the way in when it is higher than [`SCHEMA`].
    pub schema: u32,

    /// Who this blueprint is, and what wrote it.
    pub blueprint: Header,

    /// The languages it needs, by kind.
    #[serde(default)]
    pub runtimes: BTreeMap<RuntimeKind, VersionConstraint>,

    /// What is served, when the blueprint describes a site at all.
    #[serde(default)]
    pub site: Option<BlueprintSite>,

    /// The services it needs, in the order the file lists them.
    #[serde(default)]
    pub services: Vec<BlueprintService>,

    /// What PHP has to be able to load.
    #[serde(default)]
    pub php: Option<Php>,

    /// A command to run in the new project's directory.
    ///
    /// **Never written by [`crate::blueprints::capture`]**: capture does not invent a command to
    /// execute on somebody else's machine. A hand-written or gallery blueprint may carry one, and
    /// since roadmap task **T78a** an apply runs it — in the new project's directory, and only
    /// after somebody has agreed to the exact command, per apply, never on import.
    #[serde(default)]
    pub scaffold: Option<Scaffold>,
}

/// `[blueprint]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Header {
    /// The display name.
    pub name: String,

    /// What it is for.
    #[serde(default)]
    pub description: String,

    /// When it was captured, ISO-8601 UTC.
    pub created_at: String,

    /// What made it.
    pub created_on: Provenance,
}

/// `[blueprint.created_on]` — provenance a person reads.
///
/// **Deliberately not a machine identity**: no host name, no account, nothing that would make a
/// blueprint say where it has been.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Provenance {
    /// `windows`, `macos` or `linux`.
    pub os: String,

    /// The MixEngine that wrote it.
    pub version: String,
}

/// `[site]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintSite {
    /// What it serves.
    ///
    /// Read from the **whole** table rather than from a nested one, because [`SiteKind`] is
    /// internally tagged and its TOML spelling is `kind = "reverse-proxy"` sitting flat beside
    /// `upstream = "…"`. The same shape [`crate::manifest::ManifestSite`] reads, read the same way.
    pub kind: SiteKind,

    /// Relative to the project root; `""` is the root itself.
    pub doc_root: String,

    /// Whether HTTPS is declared.
    pub https: bool,

    /// The primary domain, with `{project}` where the captured project's name was.
    pub domain_pattern: String,

    /// Every other name, by the same rule.
    pub aliases: Vec<String>,
}

/// One `[[services]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct BlueprintService {
    /// The package: `mariadb`, `redis`.
    pub name: String,

    /// The version wanted — exact when captured, a range when somebody wrote it by hand.
    #[serde(default)]
    pub version: Option<VersionConstraint>,

    /// [`PER_PROJECT`] for one of this project's own, or the name of a shared instance to reuse.
    #[serde(default)]
    pub instance: Option<String>,

    /// The database to create, `{project}` allowed.
    #[serde(default)]
    pub database: Option<String>,

    /// The account to create, `{project}` allowed. **Never a password.**
    #[serde(default)]
    pub user: Option<String>,
}

/// `[php]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Php {
    /// Extensions this project needs loaded, in name order.
    ///
    /// **Enabling only** (D2). A blueprint says what a project needs loaded; turning something
    /// *off* on the receiving machine would change the PHP every other project there runs, which is
    /// harm it was never asked to do.
    #[serde(default)]
    pub extensions: Vec<String>,
}

/// `[scaffold]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Scaffold {
    /// The command, run in the project directory and nowhere else.
    pub command: String,
}

impl<'de> serde::Deserialize<'de> for BlueprintSite {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;

        let table = toml::Table::deserialize(deserializer)?;

        // Read before the kind, because deserialising the kind consumes a clone of the whole table
        // and these keys are not its business.
        let text = |key: &str| {
            table
                .get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        };

        let aliases = table
            .get("aliases")
            .and_then(toml::Value::as_array)
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

        Ok(Self {
            kind: SiteKind::deserialize(table.clone()).map_err(D::Error::custom)?,
            doc_root: text("doc_root").unwrap_or_default(),
            // Absent is HTTPS, which is what a site created through `site.create` gets.
            https: table
                .get("https")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true),
            domain_pattern: text("domain_pattern")
                .ok_or_else(|| D::Error::custom("a [site] needs a domain_pattern"))?,
            aliases,
        })
    }
}

/// Read one.
///
/// **The version is read before the rest.** A file from a build that knew more than this one is
/// refused rather than half-understood: a manifest whose unknown sections were skipped would apply
/// as something other than what its author wrote down.
///
/// # Errors
///
/// [`Error::UnknownBlueprintSchema`] for a newer format, and [`Error::BlueprintManifest`] for a
/// file that does not parse.
pub fn read(text: &str) -> Result<BlueprintManifest> {
    /// Just enough to learn the version, and the name to say it with.
    #[derive(serde::Deserialize)]
    struct Versioned {
        schema: u32,
        #[serde(default)]
        blueprint: Option<NamedOnly>,
    }

    /// The one field of `[blueprint]` a refusal needs.
    #[derive(serde::Deserialize)]
    struct NamedOnly {
        #[serde(default)]
        name: String,
    }

    let versioned: Versioned =
        toml::from_str(text).map_err(|source| Error::BlueprintManifest { source })?;

    if versioned.schema > SCHEMA {
        return Err(Error::UnknownBlueprintSchema {
            name: versioned
                .blueprint
                .map(|header| header.name)
                .unwrap_or_default(),
            schema: versioned.schema,
        });
    }

    toml::from_str(text).map_err(|source| Error::BlueprintManifest { source })
}

/// Write one, in one fixed order.
///
/// **Deterministic by construction** (D7), which is what makes "capturing the same project twice
/// produces two identical files" true rather than lucky: the section order is this function's, the
/// map inside `[runtimes]` is a [`BTreeMap`], and the two lists are sorted by whoever built them.
///
/// A hand-built document rather than a derived `Serialize`, for the reason the module note gives:
/// [`SiteKind`] is internally tagged, and leaving both the key order and TOML's
/// scalars-before-tables rule to a derive would leave the one property this function exists for to
/// chance.
#[must_use]
pub fn render(manifest: &BlueprintManifest) -> String {
    use toml_edit::{Array, DocumentMut, Item, Table, value};

    let mut document = DocumentMut::new();
    document["schema"] = value(i64::from(manifest.schema));

    let mut header = Table::new();
    header["name"] = value(&manifest.blueprint.name);
    if !manifest.blueprint.description.is_empty() {
        header["description"] = value(&manifest.blueprint.description);
    }
    header["created_at"] = value(&manifest.blueprint.created_at);

    let mut created_on = Table::new();
    created_on["os"] = value(&manifest.blueprint.created_on.os);
    created_on["version"] = value(&manifest.blueprint.created_on.version);
    header["created_on"] = Item::Table(created_on);
    document["blueprint"] = Item::Table(header);

    if !manifest.runtimes.is_empty() {
        let mut runtimes = Table::new();
        for (kind, constraint) in &manifest.runtimes {
            runtimes[kind.as_str()] = value(constraint.as_str());
        }
        document["runtimes"] = Item::Table(runtimes);
    }

    if let Some(site) = &manifest.site {
        let mut table = Table::new();

        // The kind renders flat — `kind = "php-fpm"` beside whatever that kind carries — and it is
        // serialised through `toml::Value` so that one spelling of that shape exists, the same one
        // the reader above accepts.
        if let Ok(toml::Value::Table(flat)) = toml::Value::try_from(&site.kind) {
            for (key, item) in flat {
                match item {
                    toml::Value::String(text) => table[key.as_str()] = value(text),
                    toml::Value::Integer(number) => table[key.as_str()] = value(number),
                    toml::Value::Boolean(flag) => table[key.as_str()] = value(flag),
                    // Nothing else is a `SiteKind` payload today, and a variant that grew one would
                    // rather be missing here — and caught by the round-trip test — than rendered as
                    // something the reader cannot take back.
                    _ => {}
                }
            }
        }

        table["doc_root"] = value(&site.doc_root);
        table["https"] = value(site.https);
        table["domain_pattern"] = value(&site.domain_pattern);

        if !site.aliases.is_empty() {
            let mut aliases = Array::new();
            for alias in &site.aliases {
                aliases.push(alias.as_str());
            }
            table["aliases"] = value(aliases);
        }

        document["site"] = Item::Table(table);
    }

    if !manifest.services.is_empty() {
        let mut services = toml_edit::ArrayOfTables::new();

        for service in &manifest.services {
            let mut table = Table::new();
            table["name"] = value(&service.name);

            if let Some(version) = &service.version {
                table["version"] = value(version.as_str());
            }
            if let Some(instance) = &service.instance {
                table["instance"] = value(instance);
            }
            if let Some(database) = &service.database {
                table["database"] = value(database);
            }
            if let Some(user) = &service.user {
                table["user"] = value(user);
            }

            services.push(table);
        }

        document["services"] = Item::ArrayOfTables(services);
    }

    if let Some(php) = &manifest.php
        && !php.extensions.is_empty()
    {
        let mut table = Table::new();
        let mut extensions = Array::new();
        for name in &php.extensions {
            extensions.push(name.as_str());
        }
        table["extensions"] = value(extensions);
        document["php"] = Item::Table(table);
    }

    if let Some(scaffold) = &manifest.scaffold {
        let mut table = Table::new();
        table["command"] = value(&scaffold.command);
        document["scaffold"] = Item::Table(table);
    }

    document.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_manifest() -> BlueprintManifest {
        BlueprintManifest {
            schema: SCHEMA,
            blueprint: Header {
                name: "laravel-php82".to_owned(),
                description: "Laravel + MariaDB".to_owned(),
                created_at: "2026-09-01T09:00:00Z".to_owned(),
                created_on: Provenance {
                    os: "windows".to_owned(),
                    version: "0.1.0".to_owned(),
                },
            },
            runtimes: [(
                RuntimeKind::Php,
                VersionConstraint::parse("8.2.23").expect("a constraint"),
            )]
            .into_iter()
            .collect(),
            site: Some(BlueprintSite {
                kind: SiteKind::PhpFpm { pool: None },
                doc_root: "public".to_owned(),
                https: true,
                domain_pattern: "{project}.test".to_owned(),
                aliases: vec!["api.{project}.test".to_owned()],
            }),
            services: vec![BlueprintService {
                name: "mariadb".to_owned(),
                version: Some(VersionConstraint::parse("11.4.3").expect("a constraint")),
                instance: Some("main".to_owned()),
                database: Some("{project}".to_owned()),
                user: Some("{project}".to_owned()),
            }],
            php: Some(Php {
                extensions: vec!["redis".to_owned(), "xdebug".to_owned()],
            }),
            scaffold: None,
        }
    }

    /// A manifest with every section this feature has, in the order the renderer writes them.
    ///
    /// Taken from the renderer's own output rather than written by hand, which is what makes the
    /// assertion below about the *format* rather than about somebody's typing.
    const GALLERY_SHAPED: &str = r#"schema = 1

[blueprint]
name = "laravel-php82"
description = "Laravel + MariaDB"
created_at = "2026-09-01T09:00:00Z"

[blueprint.created_on]
os = "windows"
version = "0.1.0"

[runtimes]
php = "8.2.23"

[site]
kind = "php-fpm"
doc_root = "public"
https = true
domain_pattern = "{project}.test"
aliases = ["api.{project}.test"]

[[services]]
name = "mariadb"
version = "11.4.3"
instance = "main"
database = "{project}"
user = "{project}"

[php]
extensions = ["redis", "xdebug"]

[scaffold]
command = "composer create-project laravel/laravel {project}"
"#;

    /// **The rendering is not the signed artifact, and in practice it is the same bytes** — roadmap
    /// task **T78a**, its design's D16. Nothing depends on this: trust is decided over the bytes
    /// that were handed in, once, at import. But a gallery file that came back differently would
    /// mean anybody checking a `.minisig` against `blueprints/<slug>.toml` finds a failure with no
    /// tampering behind it, and this is what T77's byte-identical renderer was for.
    #[test]
    fn a_gallery_shaped_manifest_renders_back_byte_for_byte() {
        let read_back = read(GALLERY_SHAPED).expect("it parses");

        assert_eq!(render(&read_back), GALLERY_SHAPED);
    }

    /// What is written can be read, and reading it back gives the same value — the property every
    /// later task leans on.
    #[test]
    fn a_rendered_manifest_reads_back_as_itself() {
        let manifest = a_manifest();
        let rendered = render(&manifest);

        assert_eq!(read(&rendered).expect("it parses"), manifest, "{rendered}");
    }

    /// Every kind survives the round trip, including the two that carry a payload beside the tag.
    #[test]
    fn every_site_kind_survives_being_written_and_read() {
        for kind in [
            SiteKind::PhpFpm { pool: None },
            SiteKind::Static,
            SiteKind::ReverseProxy {
                upstream: "http://127.0.0.1:3000".to_owned(),
            },
            SiteKind::NodeApp { port: 3000 },
        ] {
            let mut manifest = a_manifest();
            manifest.site.as_mut().expect("a site").kind = kind.clone();

            let rendered = render(&manifest);
            let read_back = read(&rendered).expect("it parses");

            assert_eq!(
                read_back.site.expect("a site").kind,
                kind,
                "{kind:?} did not survive:\n{rendered}"
            );
        }
    }

    /// **D7.** Two captures of one project must produce two identical files, or a golden test says
    /// nothing and a re-capture's diff is noise.
    #[test]
    fn rendering_is_deterministic_and_puts_the_schema_first() {
        let rendered = render(&a_manifest());

        assert_eq!(rendered, render(&a_manifest()));
        assert!(rendered.starts_with("schema = 1\n"), "{rendered}");
        assert!(
            rendered.find("[blueprint]") < rendered.find("[runtimes]"),
            "{rendered}"
        );
        assert!(
            rendered.find("[[services]]") < rendered.find("[php]"),
            "{rendered}"
        );
    }

    /// The pool is a fact about the machine that was captured, and `SiteKind::PhpFpm` can carry
    /// one. It never goes in.
    #[test]
    fn a_site_kind_is_written_flat_and_carries_no_pool() {
        let rendered = render(&a_manifest());

        assert!(rendered.contains("kind = \"php-fpm\""), "{rendered}");
        assert!(!rendered.contains("pool"), "{rendered}");
    }

    /// A file from a build that knew more than this one is refused by name rather than half-read.
    #[test]
    fn a_newer_schema_is_refused_by_name() {
        let text = render(&a_manifest()).replace("schema = 1", "schema = 2");

        assert!(
            matches!(
                read(&text),
                Err(Error::UnknownBlueprintSchema { schema: 2, ref name }) if name == "laravel-php82"
            ),
            "{:?}",
            read(&text)
        );
    }

    /// A `[site]` without a name to answer to is not a site, and saying so beats defaulting to
    /// something the author did not write.
    #[test]
    fn a_site_without_a_domain_pattern_does_not_parse() {
        let text = r#"
schema = 1

[blueprint]
name = "x"
created_at = "2026-09-01T09:00:00Z"

[blueprint.created_on]
os = "linux"
version = "0.1.0"

[site]
kind = "static"
doc_root = "public"
"#;

        assert!(matches!(read(text), Err(Error::BlueprintManifest { .. })));
    }
}
