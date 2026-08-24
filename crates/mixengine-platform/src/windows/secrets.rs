//! Whether a `keyring` failure means this machine has no credential store at all.
//!
//! **The shortest of the three, and the most exact.** `keyring`'s Credential Manager backend spends
//! `NoStorageAccess` on one Windows error and no other: `ERROR_NO_SUCH_LOGON_SESSION`. That is the
//! whole of the absent case here, because the Credential Manager is part of Windows rather than
//! something installed beside it — what a caller can lack is not the store but a logon session to
//! read it under.
//!
//! Which is a shape MixEngine can genuinely meet: a service running as `LocalSystem` or under a
//! virtual account has no user profile loaded, so there is no per-user credential vault to open.

use keyring::error::Error as KeyringError;

/// The workaround for a machine with no credential store, or `None` when it has one.
pub(crate) fn absent_store(source: &KeyringError) -> Option<&'static str> {
    matches!(source, KeyringError::NoStorageAccess(_)).then_some(
        "this process has no logon session, so Windows has no per-user Credential Manager vault to \
         open — run MixEngine as a signed-in user rather than as a service account without a \
         loaded profile",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two answers, and nothing between them.
    #[test]
    fn only_no_storage_access_is_read_as_an_absent_vault() {
        let absent = KeyringError::NoStorageAccess(Box::new(std::io::Error::other(
            "ERROR_NO_SUCH_LOGON_SESSION",
        )));

        assert!(absent_store(&absent).is_some_and(|advice| !advice.is_empty()));

        // A vault that is there and refused — the case the whole distinction exists for.
        let refused =
            KeyringError::PlatformFailure(Box::new(std::io::Error::other("ERROR_ACCESS_DENIED")));

        assert_eq!(absent_store(&refused), None);
        assert_eq!(absent_store(&KeyringError::NoEntry), None);
    }
}
