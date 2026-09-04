//! What a release says about itself — roadmap task **T88**.
//!
//! **A third [`Document`](crate::index::Document) and not a second client.** The three properties an
//! update feed needs are the three [`crate::index::Client`] already does and already has tests for:
//! the signature is checked before the bytes are parsed, so a JSON parser never runs on unverified
//! input; the cache is re-verified on every read, because a file in the user's home is a file any
//! local process can rewrite; and a correctly signed document from before the one we hold is
//! refused. That last one is the attack that matters here — somebody who can answer the URL replays
//! yesterday's feed to keep a machine on a version with a known hole — and getting it for free is
//! the whole argument for the shape of this module.
//!
//! Nothing here is `#[serde(deny_unknown_fields)]`, on [`crate::index::format`]'s rule and for its
//! reason: this document is written by us and read by builds older than it, so a release that adds
//! a field must stay readable by every copy already installed.

use crate::index::{Arch, Artifact, Os, Timestamp};

/// The document version this build can read.
///
/// Bumped only for a change an existing client *cannot* read. Adding an optional field is not one.
pub const SCHEMA: u32 = 1;

/// Where the feed is published.
///
/// The stable release-asset redirect and **not** the GitHub API, which is rate-limited to 60
/// requests an hour per address for unauthenticated callers — `.claude/features/updates.md`. A
/// *draft* release is not `latest`, which is what makes tagging and publishing two different acts
/// and is why the release workflow assembles a draft somebody publishes by hand.
pub const DEFAULT_URL: &str =
    "https://github.com/mixnz/mixengine/releases/latest/download/latest.json";

/// One published release, and where its payload is for each machine.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Feed {
    /// The document version. Checked against [`SCHEMA`] before anything else here is believed.
    pub schema: u32,

    /// When the release pipeline wrote this document.
    ///
    /// **Two fields and not one, with [`Feed::version`].** `version` is what is offered; this is
    /// what makes a replay detectable, and a feed re-published for the same version — a corrected
    /// note, an added architecture — must be able to move forward without pretending to be a new
    /// release.
    pub generated_at: Timestamp,

    /// The version being offered.
    pub version: String,

    /// When that release was published, for a person to read.
    pub published_at: Timestamp,

    /// What changed, as the tag's own commit subjects.
    ///
    /// Written by `packaging/feed.sh` from `git log` rather than by GitHub: this document is signed
    /// before the draft release exists, so `--generate-notes` cannot reach it. See the T88 design,
    /// D13.
    pub notes: String,

    /// Where the notes somebody may have edited afterwards live.
    ///
    /// Optional because nothing breaks when it is absent — it is a link printed after the notes —
    /// and because a feed generated before this field existed must still read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_url: Option<String>,

    /// One payload per machine this release was built for.
    ///
    /// [`Artifact`] itself rather than a type of this module's own, which is what lets
    /// [`crate::install::Installer`] apply one unchanged: `provides` says where the binaries are
    /// inside the archive, `sha256` is what binds the payload to this signed document, and
    /// `requires` carries the glibc floor the packaging pipeline already measures. See the T88
    /// design, D3.
    pub artifacts: Vec<Artifact>,
}

impl Feed {
    /// The payload for one machine, or [`None`] when this release has no build for it.
    ///
    /// macOS publishes one *universal* archive under two rows, one per architecture, so a caller
    /// asks with the pair it already has rather than learning what "universal" means — the T88
    /// design, D6.
    #[must_use]
    pub fn artifact(&self, os: Os, arch: Arch) -> Option<&Artifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.os == os && artifact.arch == arch)
    }
}

impl crate::index::Document for Feed {
    const SCHEMA: u32 = SCHEMA;
    const LABEL: &'static str = "update feed";
    const CACHE_FILE: &'static str = "latest.json";

    fn schema(&self) -> u32 {
        self.schema
    }

    fn generated_at(&self) -> Timestamp {
        self.generated_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document the packaging pipeline writes, as `packaging/feed.sh` writes it.
    fn document() -> serde_json::Value {
        serde_json::json!({
            "schema": 1,
            "generated_at": "2026-09-05T09:12:00Z",
            "version": "0.2.0",
            "published_at": "2026-09-05T09:12:00Z",
            "notes": "feat(cli): mix self-update",
            "notes_url": "https://github.com/mixnz/mixengine/releases/tag/v0.2.0",
            "artifacts": [{
                "os": "windows",
                "arch": "x86_64",
                "url": "https://example.invalid/mixengine-0.2.0-windows-x86_64.zip",
                "sha256": "00",
                "size": 1,
                "provides": { "mix": "mixengine/mix.exe" }
            }]
        })
    }

    #[test]
    fn the_feed_reads_what_the_packaging_pipeline_writes() {
        let feed: Feed = serde_json::from_value(document()).expect("a feed");

        assert_eq!(feed.version, "0.2.0");
        assert_eq!(feed.schema, SCHEMA);
        assert!(feed.artifact(Os::Windows, Arch::X86_64).is_some());
        assert!(
            feed.artifact(Os::Linux, Arch::X86_64).is_none(),
            "a release with no build for this pair must answer None rather than the first row it \
             happens to hold"
        );
    }

    /// A field a later release adds must not stop this build reading the document — the rule
    /// `index/format.rs` states, and the reason there is no `deny_unknown_fields` here.
    #[test]
    fn a_field_this_build_does_not_know_is_ignored() {
        let mut document = document();
        document["channel"] = serde_json::json!("beta");

        serde_json::from_value::<Feed>(document).expect("an unknown field is ignored");
    }

    /// `notes_url` is what a person follows when somebody edited the draft's notes after the tag
    /// was signed (the T88 design, D13). A feed written before that field existed still reads.
    #[test]
    fn a_feed_with_no_notes_url_still_reads() {
        let mut document = document();
        document
            .as_object_mut()
            .expect("an object")
            .remove("notes_url");

        let feed: Feed = serde_json::from_value(document).expect("a feed");
        assert_eq!(feed.notes_url, None);
    }
}
