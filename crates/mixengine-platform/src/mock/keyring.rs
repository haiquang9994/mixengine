//! A credential store that is a `HashMap`, and remembers what it was asked to change.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::{Error, Keyring, Result};

/// What a test asked this store to do, in order.
///
/// Reads are not recorded and stores do not carry the value: an assertion worth making is "the
/// supervisor stored a root password for `mariadb@main` and later forgot it", and a recorder holding
/// the value would be the one place in the tree where a credential sits in memory after the process
/// that needed it is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretOp {
    /// A credential was written at `(service, key)`.
    Stored {
        /// The namespace it was written in.
        service: String,
        /// The account within it.
        key: String,
    },

    /// A credential was removed — or asked to be, having never been there.
    Forgotten {
        /// The namespace it was removed from.
        service: String,
        /// The account within it.
        key: String,
    },
}

#[derive(Debug, Default)]
pub(super) struct Secrets {
    stored: Mutex<HashMap<(String, String), String>>,
    operations: Mutex<Vec<SecretOp>>,
    /// Set by a test that wants to see what the caller does when the machine has no store.
    refuse: Option<&'static str>,

    /// Set by a test that wants a store which does not answer — see [`Secrets::stalling`].
    stall: Option<Duration>,
}

impl Secrets {
    pub(super) fn remembering() -> Self {
        Self::default()
    }

    pub(super) fn refusing(reason: &'static str) -> Self {
        Self {
            refuse: Some(reason),
            ..Self::default()
        }
    }

    /// A store that takes `how_long` to answer a read.
    ///
    /// **A different case from [`refusing`](Self::refusing), and the one every deadline around a
    /// keyring is written for.** A store that is not there says so at once; a *locked* one is a
    /// D-Bus round trip to a daemon that is prompting somebody who may not be at the machine, and it
    /// answers when they type a password or never. A caller that treats "slow" and "absent" the same
    /// way is one that has not been tested against the slow one.
    ///
    /// Reads only. Writing a credential is not on any path that has to finish inside a deadline, and
    /// a mock that stalled everything would be one that costs every unrelated test its time.
    pub(super) fn stalling(how_long: Duration) -> Self {
        Self {
            stall: Some(how_long),
            ..Self::default()
        }
    }

    pub(super) fn operations(&self) -> Vec<SecretOp> {
        lock(&self.operations).clone()
    }

    fn unavailable(&self) -> Option<Error> {
        self.refuse.map(|reason| Error::UnsupportedPlatform {
            capability: "Keyring",
            reason: reason.to_owned(),
        })
    }
}

impl Keyring for Secrets {
    fn secret(&self, service: &str, key: &str) -> Result<Option<String>> {
        if let Some(error) = self.unavailable() {
            return Err(error);
        }

        // Blocking on purpose, and correct here: `Keyring` is a synchronous trait because every real
        // implementation of it blocks, and every caller already reads it off the runtime.
        if let Some(how_long) = self.stall {
            std::thread::sleep(how_long);
        }

        Ok(lock(&self.stored)
            .get(&(service.to_owned(), key.to_owned()))
            .cloned())
    }

    fn set_secret(&self, service: &str, key: &str, secret: &str) -> Result<()> {
        if let Some(error) = self.unavailable() {
            return Err(error);
        }

        lock(&self.stored).insert((service.to_owned(), key.to_owned()), secret.to_owned());
        lock(&self.operations).push(SecretOp::Stored {
            service: service.to_owned(),
            key: key.to_owned(),
        });

        Ok(())
    }

    fn forget_secret(&self, service: &str, key: &str) -> Result<()> {
        if let Some(error) = self.unavailable() {
            return Err(error);
        }

        lock(&self.stored).remove(&(service.to_owned(), key.to_owned()));
        lock(&self.operations).push(SecretOp::Forgotten {
            service: service.to_owned(),
            key: key.to_owned(),
        });

        Ok(())
    }
}

/// A poisoned lock means an assertion already failed on another thread; there is nothing left for
/// this one to report truthfully, so it takes the contents and carries on — as `mock::access` does.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        mutex.clear_poison();
        poisoned.into_inner()
    })
}
