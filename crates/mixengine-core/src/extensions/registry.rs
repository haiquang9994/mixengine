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

use std::path::Path;

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
