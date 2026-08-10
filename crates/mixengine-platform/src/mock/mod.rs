//! An in-memory host. Always compiled — tests and `--dry-run` both run against it.
//!
//! Tests never touch the real machine (`.claude/standards/testing.md`), so every capability added
//! here answers from memory and, once mutations exist, records what it was asked to do so
//! assertions can be made on the recorded sequence rather than on side effects.

mod access;
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
    access: access::Access,
}

impl Host {
    /// A host whose default root is `home`.
    #[must_use]
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home::Home::answering(Some(home.into())),
            access: access::Access::recording(),
        }
    }

    /// A host that cannot say where the user's data belongs — the service-account case.
    #[must_use]
    pub fn without_home() -> Self {
        Self {
            home: home::Home::answering(None),
            access: access::Access::recording(),
        }
    }

    /// A host whose OS refuses to restrict a directory, with `reason`.
    ///
    /// For the caller's side of [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform):
    /// startup has to fail loudly rather than carry on with a world-readable home.
    #[must_use]
    pub fn refusing_to_restrict(home: impl Into<PathBuf>, reason: &'static str) -> Self {
        Self {
            home: home::Home::answering(Some(home.into())),
            access: access::Access::refusing(reason),
        }
    }

    /// Every path [`DirectoryAccess::restrict_to_owner`](crate::DirectoryAccess::restrict_to_owner)
    /// was called with, in order.
    #[must_use]
    pub fn restricted(&self) -> Vec<PathBuf> {
        self.access.restricted()
    }
}

impl crate::Host for Host {
    fn home_dirs(&self) -> &dyn crate::HomeDirs {
        &self.home
    }

    fn directory_access(&self) -> &dyn crate::DirectoryAccess {
        &self.access
    }
}
