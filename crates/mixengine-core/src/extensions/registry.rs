//! The published list of extensions, and the reading of it — roadmap task **T81**.
//!
//! # A second document, and the key that already exists
//!
//! `extensions.json` is published beside `index.json`, under the same moved tag, and signed with
//! **[`crate::index::PUBLIC_KEY`]** — no third key. The precedent that might argue for one is the
//! blueprint gallery, which took a key of its own because its blast radius differed: one compromise
//! of the index key costs the package index, one compromise of a key that vouches for a
//! `[scaffold]` costs the right to run arbitrary code on every machine that took a blueprint in.
//! **An extension has the package index's blast radius exactly** — a binary downloaded and
//! supervised — so a separate key would separate nothing, and would add a third constant to rotate,
//! a third Actions secret, and a third half-finished rotation to get wrong.
//!
//! # Two documents rather than one array added to the index
//!
//! Failure isolation rather than tidiness. An entry this build cannot read has to be skipped
//! ([`Registry::listing`]); inside a document that also holds every runtime, the code that skips
//! would have to be exactly right or `mix runtime list` dies of an extension. Two documents make
//! that structural: no reading of this one can fail the reading of that one, because they are not
//! the same read.
//!
//! # An entry *is* a manifest
//!
//! Not a pointer to one. [`manifest::ExtensionManifest`] already carries `[artifact.<target>]` with
//! its URL and hash, so a manifest is the entry a downloader needs — and because permissions arrive
//! with the listing, the question *"this wants to reach the LAN and read your project roots — go
//! on?"* can be asked **before a single artifact byte is fetched**. Asking afterwards is asking
//! after doing the thing somebody was about to refuse.
//!
//! Nothing here fetches an artifact or writes a row: this module answers *what exists*.

use std::path::{Path, PathBuf};

use crate::extensions::manifest::{self, ExtensionManifest};
use crate::index::{self, Timestamp};
use crate::{Error, Result};

/// The schema this build can read.
///
/// Bumped only for a change an existing client *cannot* read — [`crate::index::format`]'s rule,
/// which is why an unreadable *entry* is not one of those: a document that adds an extension of a
/// kind this build has never heard of is still a document it can read the rest of.
pub const SCHEMA: u32 = 1;

/// What the document is called, wherever it is served from.
///
/// Named rather than spelled twice: a mirror's URL is the package index's with this in place of its
/// last segment, which is what makes pointing at a mirror one setting instead of two.
pub const FILE_NAME: &str = "extensions.json";

/// Where the registry is published.
///
/// The same moved tag [`crate::index::DEFAULT_URL`] uses, so the URL never changes while the
/// document behind it does.
pub const DEFAULT_URL: &str =
    "https://github.com/mixnz/mixengine-packages/releases/download/index/extensions.json";

/// The published document.
///
/// **Entries are held as JSON until they are tried**, which is what makes [`Registry::listing`]
/// able to skip one — a `Vec<ExtensionManifest>` would fail the whole document on the first entry
/// written by a newer build.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Registry {
    /// The document version. Checked before anything else is believed.
    pub schema: u32,

    /// When the publishing pipeline generated this document — what makes a rollback detectable.
    pub generated_at: Timestamp,

    /// Every extension, each as its own manifest. Read through [`Registry::listing`].
    #[serde(default)]
    pub extensions: Vec<serde_json::Value>,
}

impl index::Document for Registry {
    const SCHEMA: u32 = SCHEMA;
    const LABEL: &'static str = "extension registry";
    const CACHE_FILE: &'static str = "extensions.json";

    fn schema(&self) -> u32 {
        self.schema
    }

    fn generated_at(&self) -> Timestamp {
        self.generated_at
    }
}

/// What a registry says, once every entry has been tried.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The extensions this build can read.
    pub extensions: Vec<ExtensionManifest>,

    /// How many entries it could not.
    ///
    /// **Counted rather than dropped in silence.** An extension that vanishes from a listing is one
    /// somebody goes looking for in the wrong place, and "your MixEngine is older than this entry"
    /// is an answer nothing else in the product can give them — so every surface that prints a
    /// listing says this when it is not zero.
    pub unreadable: usize,
}

impl Registry {
    /// Every entry this build can read, and how many it could not.
    #[must_use]
    pub fn listing(&self) -> Listing {
        let mut extensions = Vec::new();
        let mut unreadable = 0;

        for entry in &self.extensions {
            match manifest::read_value(entry.clone()) {
                Ok(manifest) => extensions.push(manifest),
                Err(reason) => {
                    tracing::debug!(
                        error = %reason,
                        "skipping a registry entry this build cannot read"
                    );
                    unreadable += 1;
                }
            }
        }

        Listing {
            extensions,
            unreadable,
        }
    }

    /// One extension by id, or [`None`] where the registry does not list it.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<ExtensionManifest> {
        self.listing()
            .extensions
            .into_iter()
            .find(|manifest| manifest.extension.id.as_str() == id)
    }
}

/// A client for the registry at `url`, verified against `public_key` and cached under `cache_dir`.
///
/// **The key is a parameter for [`index::Client::with`]'s reason**: a test cannot hold the
/// production private key, and a verification path switched off for tests is one nothing checks.
///
/// # Errors
///
/// As [`index::Client::with`] — a key that is not one, or an HTTP client that cannot be built.
pub fn client(url: &str, public_key: &str, cache_dir: &Path) -> Result<index::Client<Registry>> {
    index::Client::with(url, public_key, cache_dir)
}

/// The registry as a document, for a caller that already has the bytes.
///
/// # Errors
///
/// [`Error::IndexUnreadable`] when the text is not this document.
pub fn parse(text: &str) -> Result<Registry> {
    serde_json::from_str(text).map_err(|source| Error::IndexUnreadable {
        document: <Registry as index::Document>::LABEL,
        url: DEFAULT_URL.to_owned(),
        source,
    })
}

/// Build the document the packaging repository publishes — roadmap task **T81a**.
///
/// `manifests` is `data/extensions/` in a `mixnz/mixengine-packages` checkout, `public_key` is the
/// `minisign.pub` committed beside it, and `generated_at` is the moment the run started. What comes
/// back is the [`Registry`] a client would read, so the caller's only remaining job is to serialise
/// it.
///
/// # The order the steps are in is the design
///
/// The key chain is proved **first**, before a manifest is opened, because a run that cannot sign
/// usefully should not spend time reading — and because the failure a person needs to see is the one
/// about the key, not the third TOML error above it.
///
/// Then every file goes through [`manifest::read`], the same function a `--path` install calls, and
/// every entry is written with [`manifest::to_value`], the same rendering the `manifest_json` column
/// stores. **The generator has no opinion of its own about what a manifest may say**: one reader,
/// one renderer, and no second set of rules to drift from the one an installed MixEngine enforces.
///
/// It adds one rule the reader cannot have, because the reader sees one file and not the directory
/// around it: a file's stem must be the id it declares. That is also what makes a repeated id
/// impossible, so `<id>.toml` is the roster's uniqueness rather than a convention.
///
/// Last, the document is read back through [`Registry::listing`] and an unreadable entry is an
/// error. On a user's machine an entry this build cannot read is survivable on purpose; here it can
/// only mean this build is older than its own inputs.
///
/// # Errors
///
/// [`Error::Io`] for a directory or file that cannot be read; [`Error::RegistryPublicKeyShape`] and
/// [`Error::RegistryKeyMismatch`] for the key chain; [`Error::ExtensionFileName`] for a stem that
/// disagrees with its id; everything [`manifest::read`] reports about a file; and
/// [`Error::RegistryUnreadable`] for the read-back.
pub fn assemble(manifests: &Path, public_key: &Path, generated_at: Timestamp) -> Result<Registry> {
    prove_key(public_key)?;

    let mut files = toml_files(manifests)?;
    files.sort();

    let mut entries = Vec::new();

    for file in files {
        let text = std::fs::read_to_string(&file).map_err(|source| Error::Io {
            action: "read",
            path: file.clone(),
            source,
        })?;

        let manifest = manifest::read(&file, &text)?;
        let id = manifest.extension.id.as_str();

        // `file_stem` is `Some` for every path `toml_files` hands back — each of them ends `.toml`.
        let stem = file.file_stem().unwrap_or_default().to_string_lossy();
        if stem != id {
            return Err(Error::ExtensionFileName {
                path: file.display().to_string(),
                id: id.to_owned(),
            });
        }

        entries.push((id.to_owned(), manifest::to_value(&manifest)));
    }

    // By id rather than by file name — the same string today, and the pair would stop agreeing the
    // moment anything above this line changed. Sorted at all so two runs over one directory differ
    // in `generated_at` and nowhere else.
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    let registry = Registry {
        schema: SCHEMA,
        generated_at,
        extensions: entries.into_iter().map(|(_, entry)| entry).collect(),
    };

    let unreadable = registry.listing().unreadable;
    if unreadable != 0 {
        return Err(Error::RegistryUnreadable { count: unreadable });
    }

    Ok(registry)
}

/// Every `*.toml` directly inside `directory`, and nothing else that lives there.
///
/// A `README.md` beside the manifests is not an error: until T82 it is the only thing in that
/// directory, because git does not carry an empty one.
fn toml_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let failed = |source: std::io::Error| Error::Io {
        action: "read",
        path: directory.to_path_buf(),
        source,
    };

    let mut files = Vec::new();

    for entry in std::fs::read_dir(directory).map_err(failed)? {
        let path = entry.map_err(failed)?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            files.push(path);
        }
    }

    Ok(files)
}

/// Prove the key this repository would sign with is the key an installed MixEngine checks against —
/// the T81a design's D3.
///
/// `tools/blueprints.py` does the equivalent over in the packaging repository, and pays for it with
/// a regex over a source file and a failure mode for the regex missing. This runs *inside* a build
/// of the checkout being published, so it holds [`index::PUBLIC_KEY`] rather than going to look for
/// it: there is nothing to scrape, and no branch for the scrape failing.
fn prove_key(path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        action: "read",
        path: path.to_path_buf(),
        source,
    })?;

    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    // `minisign -G` writes an untrusted comment and one key line, in that order.
    let [_comment, committed] = lines.as_slice() else {
        return Err(Error::RegistryPublicKeyShape {
            path: path.display().to_string(),
            lines: lines.len(),
        });
    };

    if *committed != index::PUBLIC_KEY {
        return Err(Error::RegistryKeyMismatch {
            path: path.display().to_string(),
            committed: (*committed).to_owned(),
            compiled: index::PUBLIC_KEY,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registry holding the four fixtures, plus whatever entries a test adds.
    fn registry(entries: Vec<serde_json::Value>) -> Registry {
        Registry {
            schema: SCHEMA,
            generated_at: serde_json::from_value(serde_json::Value::String(
                "2026-09-02T09:00:00Z".to_owned(),
            ))
            .expect("a timestamp"),
            extensions: entries,
        }
    }

    /// One entry as the document would carry it.
    fn entry(text: &str) -> serde_json::Value {
        let manifest =
            manifest::read(std::path::Path::new("extension.toml"), text).expect("a fixture parses");

        manifest::to_value(&manifest)
    }

    /// **One entry this build cannot read costs that entry and nothing else** — the T81 design's
    /// D4.
    #[test]
    fn an_unreadable_entry_is_skipped_and_counted() {
        let registry = registry(vec![
            entry(mixengine_testkit::extension::MAILPIT),
            serde_json::json!({
                "schema": 2,
                "extension": { "id": "from-the-future", "kind": "quantum-tunnel" }
            }),
            entry(mixengine_testkit::extension::PHPMYADMIN),
        ]);

        let listing = registry.listing();

        assert_eq!(listing.extensions.len(), 2);
        assert_eq!(listing.unreadable, 1);
        assert_eq!(listing.extensions[0].extension.id.as_str(), "mailpit");
    }

    /// And the entries that do read are the manifests they were published as.
    #[test]
    fn an_entry_reads_back_as_the_manifest_it_was_published_as() {
        let published = manifest::read(
            std::path::Path::new("extension.toml"),
            mixengine_testkit::extension::MAILPIT,
        )
        .expect("a fixture parses");
        let registry = registry(vec![manifest::to_value(&published)]);

        let found = registry.find("mailpit").expect("the registry lists it");

        assert_eq!(found, published);
        assert!(registry.find("nothing-like-it").is_none());
    }

    /// An empty registry is a registry, not a failure: the day the document is published before
    /// anything is in it is the day this has to answer "nothing yet".
    #[test]
    fn an_empty_registry_lists_nothing_and_is_not_an_error() {
        let listing = registry(Vec::new()).listing();

        assert!(listing.extensions.is_empty());
        assert_eq!(listing.unreadable, 0);
    }
}
