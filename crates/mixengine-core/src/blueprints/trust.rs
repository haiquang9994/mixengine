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

use crate::{Error, Result};

/// The key the gallery's blueprints are signed with, compiled in.
///
/// Rotating it needs an application release, which is the point: a key the artifact itself could
/// announce would be a key an attacker serving the artifact could announce. The same value is
/// committed as `blueprints.pub` in the packaging repository, whose Actions secrets hold the
/// private half — [`crate::index::PUBLIC_KEY`]'s arrangement, one key along.
pub const PUBLIC_KEY: &str = "RWR9G08NiiSJuYmto9DhdpHUloc/MZQiQClZStt4vmgSfVWozqb/kFGx";

/// Check `document` against a detached minisign `signature`.
///
/// **The key is a parameter rather than read from inside**, on
/// [`Catalog::new`](crate::index::Catalog)'s shape: a test signs a fixture with a key it made and
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
