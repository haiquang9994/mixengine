//! An in-memory host. Always compiled — tests and `--dry-run` both run against it.
//!
//! Tests never touch the real machine (`.claude/standards/testing.md`), so every capability added
//! here answers from memory and, once mutations exist, records what it was asked to do so
//! assertions can be made on the recorded sequence rather than on side effects.

mod home;

use std::path::PathBuf;

/// A host that exists only in memory.
///
/// ```
/// use mixengine_platform::{Host as _, mock};
///
/// let host = mock::Host::with_home("/tmp/mixengine-test");
/// assert_eq!(
///     host.home_dirs().default_home().unwrap(),
///     std::path::Path::new("/tmp/mixengine-test")
/// );
/// ```
#[derive(Debug)]
pub struct Host {
    home: home::Home,
}

impl Host {
    /// A host whose default root is `home`.
    #[must_use]
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home::Home::answering(Some(home.into())),
        }
    }

    /// A host that cannot say where the user's data belongs — the service-account case.
    #[must_use]
    pub fn without_home() -> Self {
        Self {
            home: home::Home::answering(None),
        }
    }
}

impl crate::Host for Host {
    fn home_dirs(&self) -> &dyn crate::HomeDirs {
        &self.home
    }
}
