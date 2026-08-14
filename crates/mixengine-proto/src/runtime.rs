//! The vocabulary a runtime is described in: which language, which version, which channel.
//!
//! The same split [`crate::job`] draws over [`crate::job_api`] — this module is what a runtime *is*,
//! and the shapes a client asks and renders with are next door in [`crate::runtime_api`].
//!
//! **Two of the three types here are closed and one is validated**, and neither decision is
//! cosmetic. [`RuntimeKind`] is closed because the set of languages MixEngine manages is a product
//! decision with a `CHECK` behind it in the schema; [`RuntimeChannel`] is closed because the index
//! publishes exactly three; and [`RuntimeVersion`] is validated on construction *and* on
//! deserialisation because it names a directory — `runtimes/<kind>/<version>/` — which is
//! [`ServiceId`](crate::ServiceId)'s reason exactly, arriving here from the other side of the wire.

use std::fmt;

/// Which language runtime something is a version of.
///
/// **Closed, unlike [`JobKind`](crate::JobKind) and like [`JobState`](crate::JobState).** The set
/// grows only when MixEngine learns to manage another language, which is a release of ours and a
/// migration of the `runtime_installs.kind` `CHECK` — never something a package index gets to
/// extend by publishing. An index naming a fifth one is describing something this build could not
/// install a shim for anyway.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// PHP. The one this product exists for, and the only one with artifacts today.
    Php,
    /// Node.js.
    Node,
    /// Python.
    Python,
    /// Ruby.
    Ruby,
}

impl RuntimeKind {
    /// Every kind, in the order a listing shows them.
    pub const ALL: [Self; 4] = [Self::Php, Self::Node, Self::Python, Self::Ruby];

    /// The word this is stored, published and typed as.
    ///
    /// One spelling for the `kind` column, the index's `kind` field, the wire and the command line —
    /// a second one would be a second vocabulary to keep in step with the first.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Php => "php",
            Self::Node => "node",
            Self::Python => "python",
            Self::Ruby => "ruby",
        }
    }

    /// Read one back, or [`None`] for a word this build does not know.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

impl fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which release channel a published version belongs to.
///
/// The index's own three, mirrored here rather than shared with it: the document's vocabulary
/// belongs to `mixengine_core::index`, which is a description of a *file we publish*, and this one
/// belongs to the wire. They agree today and the conversion between them is one `match` in `core`,
/// which is the price of the two being able to move apart — a channel added to the index for the
/// publishing pipeline's own purposes should not become an API change by accident.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeChannel {
    /// Offered by default.
    Stable,
    /// A release candidate. Behind a setting.
    Rc,
    /// A beta. Behind a setting.
    Beta,
}

impl RuntimeChannel {
    /// The word this is stored and published as.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Rc => "rc",
            Self::Beta => "beta",
        }
    }

    /// Read one back, or [`None`] for a word this build does not know.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stable" => Some(Self::Stable),
            "rc" => Some(Self::Rc),
            "beta" => Some(Self::Beta),
            _ => None,
        }
    }
}

impl fmt::Display for RuntimeChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Upstream's version string, exactly as upstream writes it.
///
/// **Validated because it is a path component**, on [`ServiceId`](crate::ServiceId)'s reasoning: an
/// install lands in `runtimes/<kind>/<version>/`, so a value carrying a separator, a `..` or a
/// trailing dot is not a lookup that fails — it is a write somewhere nobody meant. The charset is
/// narrow enough that none of those can be spelled at all, which is why the installer can `join`
/// this rather than escape it.
///
/// **Not normalised and not compared.** `8.3.33` is the string a user pinned in `mixengine.toml` and
/// the string the index published; rewriting it here would stop the two matching. Ordering versions
/// and reading `^8.3` need a grammar this type deliberately does not have — that is
/// [T24](../../../../.claude/roadmap/phase-2-runtimes.md), and until it exists a version is an
/// identifier and nothing more.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct RuntimeVersion(String);

impl RuntimeVersion {
    /// The longest a version may be, in bytes.
    ///
    /// Not a filesystem limit. The longest thing upstream has ever published is on the order of
    /// `8.5.0RC1-dev`, and a value approaching this is somebody sending a sentence.
    pub const MAX_LEN: usize = 32;

    /// The characters a version may contain besides ASCII letters and digits.
    ///
    /// `+` because semantic versioning's build metadata uses it and every target allows it in a
    /// filename; `~` is deliberately absent, because PHP's own ini parser refuses one in an unquoted
    /// value and a path containing it would fail silently at the worst possible moment — the bug
    /// T20a found on a runner and wrote up in the packaging notes.
    const PUNCTUATION: [char; 4] = ['.', '-', '_', '+'];

    /// Read a version, refusing anything that could not be a directory name.
    ///
    /// # Errors
    ///
    /// [`VersionError`] naming what is wrong with the value, phrased for whoever typed it.
    pub fn parse(value: impl Into<String>) -> Result<Self, VersionError> {
        let value = value.into();

        let reject = |reason: &str| {
            Err(VersionError {
                value: value.clone(),
                reason: reason.to_owned(),
            })
        };

        if value.is_empty() {
            return reject("it is empty");
        }
        if value.len() > Self::MAX_LEN {
            return reject(&format!("it is longer than {} characters", Self::MAX_LEN));
        }

        // **A version begins with a digit**, which is where most of the safety comes from rather
        // than from a list of refusals: `.`, `..`, `-rf` and every name Windows reserves (`CON`,
        // `AUX`, `NUL`) are all excluded by this one rule, and every version any of these four
        // upstreams has ever published satisfies it.
        if !value.starts_with(|first: char| first.is_ascii_digit()) {
            return reject("it does not begin with a digit");
        }
        if value.ends_with('.') {
            // Windows strips a trailing dot from a directory name, so `8.3.` and `8.3` would be the
            // same directory while being different rows.
            return reject("it ends with a dot");
        }
        if let Some(bad) = value
            .chars()
            .find(|c| !c.is_ascii_alphanumeric() && !Self::PUNCTUATION.contains(c))
        {
            return reject(&format!("it contains {bad:?}"));
        }

        Ok(Self(value))
    }

    /// The string, for a path, a query or a message.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuntimeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for RuntimeVersion {
    /// Validating, for [`ServiceId`](crate::ServiceId)'s reason: a version that cannot be a
    /// directory name fails later, further from the cause, in the middle of an install.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize<'_>>::deserialize(deserializer)?;

        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// What is wrong with a version somebody typed.
///
/// Its own type rather than a variant of [`SpecError`](crate::SpecError): that one belongs to the
/// service-spec vocabulary, and a runtime version is refused in places — a command line, a wire
/// request — that have nothing to do with a spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{value:?} is not a runtime version: {reason}")]
pub struct VersionError {
    /// What was offered.
    pub value: String,
    /// Why it was refused, as a sentence that completes "it …".
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_has_one_spelling_everywhere() {
        for kind in RuntimeKind::ALL {
            assert_eq!(RuntimeKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{}\"", kind.as_str()),
                "the wire spelling and the stored one have to be the same word"
            );
        }
    }

    #[test]
    fn a_kind_this_build_does_not_know_is_not_read_as_one() {
        assert_eq!(RuntimeKind::parse("perl"), None);
        serde_json::from_str::<RuntimeKind>("\"perl\"").expect_err("a closed set");
    }

    #[test]
    fn a_channel_round_trips_through_the_word_it_is_stored_as() {
        for channel in [
            RuntimeChannel::Stable,
            RuntimeChannel::Rc,
            RuntimeChannel::Beta,
        ] {
            assert_eq!(RuntimeChannel::parse(channel.as_str()), Some(channel));
        }
    }

    #[test]
    fn the_versions_these_upstreams_actually_publish_are_accepted() {
        for version in [
            "8.3.33",
            "7.0.33",
            "8.5.0RC1",
            "20.11.0",
            "3.12.1",
            "3.3.6",
            "1.2.3+build4",
        ] {
            assert_eq!(
                RuntimeVersion::parse(version).expect(version).as_str(),
                version
            );
        }
    }

    /// The whole reason this type validates: every one of these is a write somewhere nobody meant.
    #[test]
    fn a_version_that_could_leave_its_own_directory_is_refused() {
        for version in [
            "",
            "..",
            ".",
            "../../etc/passwd",
            "8.3/../..",
            "8.3\\33",
            "8.3.",
            "-rf",
            "CON",
            "8.3 33",
            "8.3.33-longer-than-anything-upstream-has-ever-published",
        ] {
            assert!(
                RuntimeVersion::parse(version).is_err(),
                "{version:?} should be refused"
            );
        }
    }

    /// Refused on the way in as well as on construction — the wire is where an untrusted one
    /// arrives.
    #[test]
    fn a_version_is_validated_when_it_is_read_off_the_wire() {
        let error = serde_json::from_str::<RuntimeVersion>(r#""../escape""#)
            .expect_err("not a version")
            .to_string();

        assert!(error.contains("does not begin with a digit"), "{error}");
    }
}
