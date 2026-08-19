//! The vocabulary a runtime is described in: which language it is.
//!
//! The same split [`crate::job`] draws over [`crate::job_api`] — this module is what a runtime *is*,
//! and the shapes a client asks and renders with are next door in [`crate::runtime_api`].
//!
//! **[`RuntimeKind`] is closed**, and the decision is not cosmetic: the set of languages MixEngine
//! manages is a product decision with a `CHECK` behind it in the schema, never something a package
//! index gets to extend by publishing.
//!
//! A version and a channel used to live here too. They moved to [`crate::version`] in T31a, when a
//! service package turned out to need the same string with the same rules — they were never about
//! runtimes, only ever used by them first.

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

    /// The environment variable that overrides which version of this language is used.
    ///
    /// `MIXENGINE_PHP=8.1 php -v` is the first step of the resolution order
    /// [runtime-versions.md](../../../../.claude/features/runtime-versions.md) states, and the name
    /// is here — in the crate every client links — because **the process that reads it has to be the
    /// one the user invoked**. A daemon reading its own environment would answer with whatever it
    /// was started with, which is nobody's shell; so `mix` and the shim read this and send what they
    /// found, and the daemon is handed a constraint rather than left to guess where it came from.
    #[must_use]
    pub fn override_env(self) -> &'static str {
        match self {
            Self::Php => "MIXENGINE_PHP",
            Self::Node => "MIXENGINE_NODE",
            Self::Python => "MIXENGINE_PYTHON",
            Self::Ruby => "MIXENGINE_RUBY",
        }
    }
}

impl fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
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

    /// The name has to be the one a person exports, per the feature spec's own example.
    #[test]
    fn each_kind_names_the_variable_that_overrides_it() {
        assert_eq!(RuntimeKind::Php.override_env(), "MIXENGINE_PHP");

        for kind in RuntimeKind::ALL {
            assert_eq!(
                kind.override_env(),
                format!("MIXENGINE_{}", kind.as_str().to_uppercase()),
                "one spelling, derived from the kind's own word"
            );
        }
    }
}
