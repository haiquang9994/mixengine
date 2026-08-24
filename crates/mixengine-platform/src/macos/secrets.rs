//! Whether a `keyring` failure means this machine has no credential store at all.
//!
//! **The short answer on this system, and the reason `linux/secrets.rs` is long.** `keyring`'s
//! Keychain backend spends `NoStorageAccess` on exactly the four `Security.framework` codes that
//! mean there is no keychain to open — `errSecNotAvailable`, `errSecReadOnly`, `errSecNoSuchKeychain`
//! and `errSecInvalidKeychain` — so its own vocabulary already says what this module has to answer,
//! and nothing has to be read out of a boxed source.
//!
//! A login Keychain is part of a macOS account, so the absent case is a machine used in a way macOS
//! does not really offer: a `launchd` daemon in the system context, or an SSH login into an account
//! that has never been logged into at the console.

use keyring::error::Error as KeyringError;

/// The workaround for a machine with no credential store, or `None` when it has one.
pub(crate) fn absent_store(source: &KeyringError) -> Option<&'static str> {
    matches!(source, KeyringError::NoStorageAccess(_)).then_some(
        "this session has no login Keychain to open — log in at the console once so macOS creates \
         it, or run MixEngine as a user that has",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two answers, and nothing between them.
    #[test]
    fn only_no_storage_access_is_read_as_an_absent_keychain() {
        let absent =
            KeyringError::NoStorageAccess(Box::new(std::io::Error::other("errSecNoSuchKeychain")));

        assert!(absent_store(&absent).is_some_and(|advice| !advice.is_empty()));

        // A keychain that is there and refused — the case the whole distinction exists for.
        let refused =
            KeyringError::PlatformFailure(Box::new(std::io::Error::other("errSecAuthFailed")));

        assert_eq!(absent_store(&refused), None);
        assert_eq!(absent_store(&KeyringError::NoEntry), None);
    }
}
