//! Whether a blueprint arrived with the gallery's signature on it — roadmap task **T78a**.
//!
//! **A key of its own, and not the index's** (the T78a design, D2). One compromise of the index key
//! would cost the package index; one compromise of a key that also vouches for a `[scaffold]` would
//! cost the right to run arbitrary code on every machine that took a blueprint in. Those are
//! different blast radii, so they are different keys.
//!
//! **What is verified is verified once, when the blueprint arrives** (D1), and the answer becomes
//! the `blueprints.trusted` column. That is where this differs from [`crate::index`], which keeps
//! the signed bytes and checks them again every time it reads them: a blueprint's truth is its row
//! and the file beside it is a *rendering*, so a check made later would be a check against
//! something the signer never saw — and a check that can fail with no tampering behind it is a
//! check somebody eventually turns off.

use mixengine_proto::SignatureCheck;

use crate::{Error, Result};

/// The key the gallery's blueprints are signed with, compiled in.
///
/// Rotating it needs an application release, which is the point: a key the artifact itself could
/// announce would be a key an attacker serving the artifact could announce. The same value is
/// committed as `blueprints.pub` in the packaging repository, whose Actions secrets hold the
/// private half — [`crate::index::PUBLIC_KEY`]'s arrangement, one key along.
pub const PUBLIC_KEY: &str = "RWSBNWAf3DM823XHg3Gc/oTuC+0eCaOvI4x9m+djqCKNTsQMplCGzx2N";

/// Check `document` against a detached minisign `signature`.
///
/// **The key is a parameter rather than read from inside**, on
/// [`Catalogue`](crate::index::Catalogue)'s shape: a test signs a fixture with a key it made and
/// hands the verifying half in, which is the difference between a compiled-in constant that is
/// shipped and one that is exercised.
///
/// # Errors
///
/// [`Error::BlueprintKey`] when `public_key` is not a minisign key — a broken build, since the only
/// caller passes [`PUBLIC_KEY`] — and [`Error::BlueprintSignature`] when the signature does not
/// verify against it: a blueprint edited after it was signed, a signature from another key, or a
/// file that is not a signature at all.
pub fn verify(document: &[u8], signature: &str, public_key: &str) -> Result<()> {
    let key = minisign_verify::PublicKey::from_base64(public_key).map_err(|source| {
        Error::BlueprintKey {
            source: Box::new(source),
        }
    })?;

    let signature = minisign_verify::Signature::decode(signature).map_err(|source| {
        Error::BlueprintSignature {
            source: Box::new(source),
        }
    })?;

    // `false` refuses minisign's legacy algorithm, as `index` does: everything this project
    // publishes is signed with the current one.
    key.verify(document, &signature, false)
        .map_err(|source| Error::BlueprintSignature {
            source: Box::new(source),
        })
}

/// Why a blueprint is or is not trusted, settled once when its row is written — roadmap task
/// **T79b**.
///
/// **One value here, two columns there.** [`crate::blueprints::store::save`] derives both
/// `blueprints.trusted` and `blueprints.signature` from this, so the answer and the reason cannot
/// be set apart and cannot come to disagree — which is worth arranging in the one function that
/// decides whether somebody else's code will ever be offered a run on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// This build's own, or this machine's own: no signature was looked for, and none would have
    /// proved anything. A blueprint compiled in beside the key that would check it proves nothing
    /// the binary has not already proved (T79's D3), and a capture has nobody else to vouch for it
    /// (T78a's D1).
    Inherent,

    /// A signature came with it, and it verified against [`PUBLIC_KEY`].
    Verified,

    /// Nothing came with it — the ordinary case for a blueprint a colleague sent, which is why it
    /// is an answer rather than an error.
    Unsigned,

    /// One came with it and did not verify, which is **not** a refusal (T78a's D3): a file whose
    /// signature is stale is still a file its owner may want. What it loses is the right to have
    /// its `[scaffold]` offered without the louder gesture.
    Rejected,
}

impl Trust {
    /// Whether this build will offer to run the blueprint's own `[scaffold]` command.
    #[must_use]
    pub fn trusted(self) -> bool {
        matches!(self, Self::Inherent | Self::Verified)
    }

    /// What a client is told about the check, or [`None`] where no check happened.
    ///
    /// [`Self::Inherent`] is the one arm with nothing to say: `BlueprintSource` already tells a
    /// person the blueprint is this build's or this machine's, and a second word for it would be
    /// the same fact spelled twice.
    #[must_use]
    pub fn signature(self) -> Option<SignatureCheck> {
        match self {
            Self::Inherent => None,
            Self::Verified => Some(SignatureCheck::Verified),
            Self::Unsigned => Some(SignatureCheck::Missing),
            Self::Rejected => Some(SignatureCheck::Rejected),
        }
    }
}
