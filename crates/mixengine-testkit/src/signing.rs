//! A key pair a test owns, and detached signatures made with it — roadmap task **T78a**.
//!
//! **The signing half lives here and can live nowhere else**, which is the same sentence
//! [`MockRegistry`](crate::MockRegistry) is built on: a shipped binary holding a signing key would
//! be a signing key on every user's machine, and the dev-dependency edge is what keeps `minisign`
//! out of one.
//!
//! What it exists for is the pair of questions a compiled-in key cannot answer on its own — does
//! verification accept what this key signed, and does it refuse everything else. Both need a key a
//! test can make, so `mixengine_core::blueprints::trust::verify` takes the verifying half as a
//! parameter and this makes the other one. A name rather than a link: this crate does not depend on
//! `mixengine-core` and must not start, since the layering test is what keeps the signing half out
//! of anything shipped.

use std::io::Cursor;

use minisign::KeyPair;

/// A key pair, and the signatures it makes.
///
/// The `Debug` shows the verifying half only — a secret key printed by an assertion failure would
/// be a secret key in a CI log.
pub struct Signer {
    pair: KeyPair,
}

impl Signer {
    /// Mint one.
    ///
    /// Unencrypted, because nothing here has a password to keep: the pair lives for the length of
    /// one test.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pair: KeyPair::generate_unencrypted_keypair().expect("a key pair"),
        }
    }

    /// The verifying half, base64, as a compiled-in constant would spell it.
    #[must_use]
    pub fn public_key(&self) -> String {
        self.pair.pk.to_base64()
    }

    /// A detached signature over `document`, as the `.minisig` beside a file would hold it.
    #[must_use]
    pub fn sign(&self, document: &[u8]) -> String {
        minisign::sign(None, &self.pair.sk, Cursor::new(document), None, None)
            .expect("a signature")
            .into_string()
    }

    /// A detached signature whose **trusted comment** is `trusted_comment`.
    ///
    /// minisign's global signature covers that comment, which is what makes it the one place a fact
    /// about a signed artifact can travel without being taken on trust — roadmap task **T88a**, and
    /// `mixengine_proto::privileged::HelperStamp` is what reads it back.
    #[must_use]
    pub fn sign_with_comment(&self, document: &[u8], trusted_comment: &str) -> String {
        minisign::sign(
            None,
            &self.pair.sk,
            Cursor::new(document),
            Some(trusted_comment),
            None,
        )
        .expect("a signature")
        .into_string()
    }
}

impl Default for Signer {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}
