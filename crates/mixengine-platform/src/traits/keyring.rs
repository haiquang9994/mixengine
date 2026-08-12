//! Where a password lives when nothing MixEngine writes is allowed to hold one.

use crate::Result;

/// The operating system's credential store.
///
/// `.claude/decisions/0006-servicespec-in-proto-and-secret-free.md` is what makes this a capability
/// rather than a convenience: a `ServiceSpec` can *name* a credential and cannot carry one, so the
/// value has to come from somewhere at the moment a child is built, and that somewhere is the
/// Credential Manager, the login Keychain or the session's secret service. MariaDB's generated root
/// password is the first user; extension tokens follow.
///
/// The pair `(service, key)` is the whole address of a credential and is exactly what
/// `mixengine_proto::EnvValue::Keyring` carries — one naming scheme, so a spec that resolves on one
/// OS resolves on all of them. `service` is the application-side namespace (`"mixengine"`,
/// `"mixengine.mariadb@main"`), `key` the account within it.
///
/// **Nothing here logs, formats or returns a credential by accident.** A failure names the entry and
/// never the value, which is why the error variants take `service` and `key` and no message from the
/// value's side.
///
/// # Blocking
///
/// Every method blocks, and on Linux it blocks on a D-Bus round trip to a daemon that may be
/// prompting the user to unlock their keyring. A caller inside the async runtime goes through
/// `spawn_blocking`, as `.claude/standards/rust.md` requires of anything that can hang.
pub trait Keyring: std::fmt::Debug + Send + Sync {
    /// The credential stored at `(service, key)`, or `None` when there is none.
    ///
    /// An absent entry is an ordinary answer rather than a failure — a service that has not been
    /// initialised yet has no password, and the caller is what knows whether that is a problem. It
    /// is deliberately the same shape as [`Detached::exited`](crate::process::Detached::exited): the
    /// capability reports what the machine says, and the decision belongs one level up.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) when this machine has no
    /// credential store to ask — a headless Linux with no secret service running is the case that
    /// happens — and [`Error::Secret`](crate::Error::Secret) when there is one and it refused.
    fn secret(&self, service: &str, key: &str) -> Result<Option<String>>;

    /// Store `secret` at `(service, key)`, replacing whatever was there.
    ///
    /// # Errors
    ///
    /// As [`secret`](Self::secret).
    fn set_secret(&self, service: &str, key: &str, secret: &str) -> Result<()>;

    /// Remove the credential at `(service, key)`.
    ///
    /// Idempotent: removing one that is not there succeeds, because the caller's intent — *there
    /// should be no credential here* — is satisfied either way, and uninstalling a service twice is
    /// not an error to report to anybody.
    ///
    /// # Errors
    ///
    /// As [`secret`](Self::secret).
    fn forget_secret(&self, service: &str, key: &str) -> Result<()>;
}
