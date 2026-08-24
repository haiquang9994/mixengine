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
//!
//! **One thing here is per-OS after all, and it is not the implementation.** Asking *this machine
//! has no store at all* is a question `keyring` answers differently on each of the three: its own
//! `NoStorageAccess` is exactly right on Windows and macOS, and on Linux it names a keyring that is
//! locked while a session with no secret service arrives as a plain platform failure. So the reading
//! lives in `sys::secrets` — three small modules over one `Secrets`, which is the opposite split
//! from three wrappers over one library and is the only one the backends justify.

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
/// A machine with no credential store becomes [`Error::UnsupportedPlatform`] rather than a generic
/// failure, because rule 4 in `.claude/architecture/platform-abstraction.md` says a capability the
/// machine does not have is a normal answer carrying a workaround, not a bug. Everything else is the
/// store refusing, which is a bug — a locked keyring, a dismissed prompt, a vault denying access.
///
/// **Which of the two a failure is, is the one question here that has three answers.** `keyring`'s
/// own `NoStorageAccess` is that answer on Windows and macOS and is the wrong answer on Linux in
/// both directions, so the reading belongs to `sys::secrets` — see that module on whichever system
/// this is compiled for. This function only decides what to do with the verdict.
fn failure(action: &'static str, service: &str, key: &str, source: KeyringError) -> Error {
    if let Some(workaround) = crate::sys::secrets::absent_store(&source) {
        return Error::UnsupportedPlatform {
            capability: "Keyring",
            reason: format!(
                "this session has no credential store to keep {service}/{key} in ({source}) — \
                 {workaround}"
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

/// The characters a generated secret is made of.
///
/// ASCII letters and digits and nothing else. **Not a nod to compatibility — an escaping decision.**
/// A generated password is interpolated into a SQL string literal by the MariaDB recipe's bootstrap
/// step (T33), carried on an environment variable, and never written to a file at all; the first of
/// those is the one that would need quoting, and an alphabet with no quote, no backslash and no
/// newline in it is what makes the interpolation safe without an escaper to get wrong.
///
/// Sixty-two characters is 5.95 bits each, so the 32 the MariaDB recipe asks for is 190 bits.
const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// The first byte that cannot be folded into [`ALPHABET`] without favouring its first characters.
///
/// `62 * 4 = 248`, so bytes `0..248` map evenly and `248..=255` are thrown away and drawn again. The
/// bias this avoids is small and the fix is three lines, which is the wrong trade to skip in the one
/// function in this workspace that makes a credential.
const UNBIASED: u8 = 248;

/// A random secret of `length` characters, from this machine's own entropy.
///
/// **Here rather than in the crate that wants one**, for the reason everything else in this crate is
/// here: randomness is the operating system's — `BCryptGenRandom`, `getentropy`, `getrandom(2)` —
/// and a daemon reaching for it directly would be an OS call outside `mixengine-platform`.
///
/// # Errors
///
/// [`Error::UnsupportedPlatform`] when the operating system will not give out randomness at all,
/// which on the three systems MixEngine supports means a kernel without a CSPRNG. Reported the way
/// every other "this machine cannot" is, rather than as a failure of ours.
pub fn generate_secret(length: usize) -> Result<String> {
    let mut secret = String::with_capacity(length);
    let mut buffer = [0_u8; 64];

    while secret.len() < length {
        getrandom::fill(&mut buffer).map_err(|source| Error::UnsupportedPlatform {
            capability: "Keyring",
            reason: format!(
                "this machine's operating system would not produce random bytes, so there is                  nothing to make a credential out of ({source})"
            ),
        })?;

        for byte in buffer {
            if secret.len() == length {
                break;
            }

            if byte < UNBIASED {
                secret.push(char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]));
            }
        }
    }

    Ok(secret)
}
