//! An in-memory host. Always compiled — tests and `--dry-run` both run against it.
//!
//! Tests never touch the real machine (`.claude/standards/testing.md`), so every capability added
//! here answers from memory and, once mutations exist, records what it was asked to do so
//! assertions can be made on the recorded sequence rather than on side effects.

mod access;
mod home;
mod keyring;
mod path;

use std::path::PathBuf;
use std::time::Duration;

pub use keyring::SecretOp;
pub use path::PathOp;

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
    secrets: keyring::Secrets,
    env: path::Env,
}

impl Host {
    /// A host whose default root is `home`.
    #[must_use]
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self::answering(Some(home.into()))
    }

    /// A host that cannot say where the user's data belongs — the service-account case.
    #[must_use]
    pub fn without_home() -> Self {
        Self::answering(None)
    }

    /// A host whose OS refuses to restrict a directory, with `reason`.
    ///
    /// For the caller's side of [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform):
    /// startup has to fail loudly rather than carry on with a world-readable home.
    #[must_use]
    pub fn refusing_to_restrict(home: impl Into<PathBuf>, reason: &'static str) -> Self {
        Self {
            access: access::Access::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// A host with no credential store, with `reason`.
    ///
    /// The headless-Linux case: a session with no secret service running. What the caller does about
    /// it is the interesting part — a spec naming a credential cannot be started, and saying so is
    /// better than starting a service with an empty password.
    #[must_use]
    pub fn without_keyring(home: impl Into<PathBuf>, reason: &'static str) -> Self {
        Self {
            secrets: keyring::Secrets::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// A host whose credential store takes `how_long` to answer a read.
    ///
    /// The locked-keyring case, which is not the missing-keyring one above: a store that is prompting
    /// a user who is not at the machine answers late or never, where a store that is absent answers
    /// at once. Every deadline a caller puts around a keyring read is written against this, and
    /// nothing could reach it before.
    #[must_use]
    pub fn stalling_on_the_keyring(home: impl Into<PathBuf>, how_long: Duration) -> Self {
        Self {
            secrets: keyring::Secrets::stalling(how_long),
            ..Self::with_home(home)
        }
    }

    /// A host whose OS will not put anything on the PATH, with `reason`.
    ///
    /// The headless case for this capability: an account with no home directory to write a shell
    /// profile into. What matters is that the caller says so rather than reporting a PATH it did
    /// not change.
    #[must_use]
    pub fn refusing_to_change_the_path(home: impl Into<PathBuf>, reason: &'static str) -> Self {
        Self {
            env: path::Env::refusing(reason),
            ..Self::with_home(home)
        }
    }

    /// The one place every constructor above starts from, so a capability added here is added to
    /// all of them rather than to whichever four somebody remembered.
    fn answering(home: Option<PathBuf>) -> Self {
        Self {
            home: home::Home::answering(home),
            access: access::Access::recording(),
            secrets: keyring::Secrets::remembering(),
            env: path::Env::recording(),
        }
    }

    /// Every path [`DirectoryAccess::restrict_to_owner`](crate::DirectoryAccess::restrict_to_owner)
    /// was called with, in order.
    #[must_use]
    pub fn restricted(&self) -> Vec<PathBuf> {
        self.access.restricted()
    }

    /// Every credential this host was asked to store or forget, in order.
    ///
    /// Reads are absent on purpose, and so are the values: see [`SecretOp`].
    #[must_use]
    pub fn secret_operations(&self) -> Vec<SecretOp> {
        self.secrets.operations()
    }

    /// Every directory this host was asked to put on the PATH or take off it, in order.
    ///
    /// Reads are absent for [`SecretOp`]'s reason: what a test has to be able to see is the
    /// mutations, and a `state` that changed nothing is not one.
    #[must_use]
    pub fn path_operations(&self) -> Vec<PathOp> {
        self.env.operations()
    }
}

impl crate::Host for Host {
    fn home_dirs(&self) -> &dyn crate::HomeDirs {
        &self.home
    }

    fn directory_access(&self) -> &dyn crate::DirectoryAccess {
        &self.access
    }

    fn keyring(&self) -> &dyn crate::Keyring {
        &self.secrets
    }

    fn path_integration(&self) -> &dyn crate::PathIntegration {
        &self.env
    }
}
