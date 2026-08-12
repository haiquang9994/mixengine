//! The credential store, on whichever of the three systems this is.
//!
//! **The one capability with a single implementation instead of three.** Everything else in this
//! crate is a per-OS directory, or `unix/` where two of the three agree; this is a level below that,
//! because the `keyring` crate `.claude/standards/rust.md` names *is* the abstraction — one API over
//! the Windows Credential Manager, the macOS Keychain and a Linux secret service. Writing three
//! wrappers around one library would produce three copies of the same eleven lines and three places
//! for the error mapping to drift.
//!
//! What each OS actually stores into is chosen by a feature in `Cargo.toml` rather than by code
//! here, and the choices are worth knowing:
//!
//! - **Windows** `windows-native` — the Credential Manager, per-user, unlocked with the session.
//! - **macOS** `apple-native` — the login Keychain, which may prompt the first time this binary asks
//!   for an entry it wrote, and again after the binary is replaced by an update.
//! - **Linux** `sync-secret-service` — the D-Bus secret service (`gnome-keyring`, `kwallet`), with
//!   `crypto-rust` so the session is encrypted without an OpenSSL build, and `vendored` so libdbus
//!   is compiled in rather than demanded from the machine that builds the release.
//!
//! The synchronous secret-service backend is deliberate: its async sibling blocks on an executor of
//! its own inside a synchronous call, which panics when that call is made from a runtime worker
//! thread — and every caller here is a daemon that has one. Blocking is the honest shape, and the
//! caller wraps it in `spawn_blocking` where the trait says to.

use keyring::Entry;
use keyring::error::Error as KeyringError;

use crate::{Error, Keyring, Result};

/// The real credential store.
#[derive(Debug, Default)]
pub(crate) struct Secrets;

impl Keyring for Secrets {
    fn secret(&self, service: &str, key: &str) -> Result<Option<String>> {
        match entry(service, key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(source) => Err(failure("read", service, key, source)),
        }
    }

    fn set_secret(&self, service: &str, key: &str, secret: &str) -> Result<()> {
        entry(service, key)?
            .set_password(secret)
            .map_err(|source| failure("store", service, key, source))
    }

    fn forget_secret(&self, service: &str, key: &str) -> Result<()> {
        match entry(service, key)?.delete_credential() {
            // Idempotent by contract: the caller asked for there to be no credential here, and
            // there is none. See `Keyring::forget_secret`.
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(source) => Err(failure("forget", service, key, source)),
        }
    }
}

/// Address one credential.
///
/// Fails on the address itself being unusable — an empty service or key — and **not** on this
/// machine having no store. Building an `Entry` resolves which backend is compiled in, which is a
/// decision made at build time; it opens nothing. The secret-service backend's builder constructs a
/// credential and never touches D-Bus, so a Linux session with no keyring answers at the first
/// read or write rather than here.
fn entry(service: &str, key: &str) -> Result<Entry> {
    Entry::new(service, key).map_err(|source| failure("address", service, key, source))
}

/// Turn a `keyring` failure into this crate's, without ever touching the value.
///
/// `NoStorageAccess` becomes [`Error::UnsupportedPlatform`] rather than a generic failure, because
/// it is the answer of a machine that has no credential store — a headless Linux, a session with no
/// keyring daemon — which rule 4 in `.claude/architecture/platform-abstraction.md` says is a normal
/// answer carrying a workaround, not a bug. Everything else is the store refusing, which is.
fn failure(action: &'static str, service: &str, key: &str, source: KeyringError) -> Error {
    if let KeyringError::NoStorageAccess(_) = source {
        return Error::UnsupportedPlatform {
            capability: "Keyring",
            reason: format!(
                "this session has no credential store to keep {service}/{key} in ({source}) — on \
                 Linux that usually means no secret service (gnome-keyring, kwallet) is running, \
                 and a desktop session or a headless keyring daemon is what provides one"
            ),
        };
    }

    Error::Secret {
        action,
        service: service.to_owned(),
        key: key.to_owned(),
        source: Box::new(source),
    }
}
