//! What signs a MixEngine release — roadmap task **T86**.
//!
//! This module is one constant today, and the constant is the whole point: it is the root of trust
//! for everything the updater will later install. T88 grows the module into the client that reads
//! `latest.json`; what is here is the key that document will be checked against.
//!
//! # A third key, and one key for every artifact
//!
//! [`crate::index::PUBLIC_KEY`] signs the package index and [`crate::blueprints::trust::PUBLIC_KEY`]
//! signs the blueprint gallery. Both are used from `mixnz/mixengine-packages`, by that repository's
//! workflows, with that repository's secrets. A compromise there would cost the index and the
//! gallery; it must not additionally hand somebody the right to sign the `mixengined` a machine runs
//! as itself. Different repository, different workflow, different secret — so, a different key.
//!
//! The same key signs **every** artifact in a release, `mixengine-elevate` included. A key of its
//! own for the one binary that runs as root would be the same secret in the same place under a
//! second name: it splits the label and not the blast radius. What protects the helper is that it is
//! never auto-updated and that its replacement is verified inside the elevated context (T88a).
//!
//! # No verifier here
//!
//! There is deliberately no `verify` function. T88 checks `latest.json` through
//! [`crate::index::Client`], which already verifies before it parses, and checks the payload against
//! the SHA-256 that signed document carries. T88a checks the helper inside the elevated context, and
//! `mixengine-elevate` may not depend on this crate at all — `workspace_layering.rs` says so — so
//! that copy of the key will live in the helper. A verifier here would have exactly one caller: a
//! test of itself.
//!
//! # Rotating it
//!
//! Rotating this key is a one-way door. Every installed copy trusts exactly one key, so a build from
//! before a rotation can never verify a feed signed after it — silently, since T88's client keeps
//! the last document it verified and logs a refusal nobody reads. See
//! [updates.md](../../../.claude/features/updates.md) for what a rotation therefore costs and for
//! the shape of the mitigation nobody has needed yet.

/// The key every published release artifact is signed with, compiled in.
///
/// Rotating it needs an application release, which is the point: a key the release itself could
/// announce would be a key an attacker serving the release could announce. The same value is
/// committed as `packaging/updates.pub` beside the script that signs with it, and the tests below
/// keep the two from drifting apart.
pub const PUBLIC_KEY: &str = "RWTYXH7WrSyA9PjseTArGpMA1b2uedummCWwZAgBp8PtNh6VDeCwqu+z";

#[cfg(test)]
mod tests {
    use super::PUBLIC_KEY;

    /// Read at compile time on purpose: a `packaging/updates.pub` that is deleted or moved is then a
    /// build error, rather than a test that reads nothing and passes.
    const COMMITTED: &str = include_str!("../../../packaging/updates.pub");

    #[test]
    fn the_committed_public_key_is_the_one_this_build_pins() {
        let key = COMMITTED
            .lines()
            .nth(1)
            .expect("packaging/updates.pub carries the key on its second line")
            .trim();

        assert_eq!(
            key, PUBLIC_KEY,
            "packaging/updates.pub and updates::PUBLIC_KEY have drifted apart; the file is what \
             packaging/sign.sh and the preflight job read, and the constant is what an installed \
             MixEngine checks against, so a release cut while they differ is one no installed copy \
             would accept"
        );
    }

    #[test]
    fn the_pinned_key_is_a_key() {
        minisign_verify::PublicKey::from_base64(PUBLIC_KEY)
            .expect("updates::PUBLIC_KEY parses as a minisign public key");
    }

    /// The one drift this design has that every other check passes through.
    ///
    /// A rotation done in a hurry pastes the wrong key from the wrong file: it is a valid key, the
    /// committed file matches it, and every signature made with its private half verifies. The only
    /// thing wrong is *which* key, and this is the only test that looks.
    #[test]
    fn the_three_keys_this_product_pins_are_three_different_keys() {
        let index = crate::index::PUBLIC_KEY;
        let blueprints = crate::blueprints::trust::PUBLIC_KEY;

        assert_ne!(PUBLIC_KEY, index, "the updater key is the package index's");
        assert_ne!(
            PUBLIC_KEY, blueprints,
            "the updater key is the blueprint gallery's"
        );
        assert_ne!(
            index, blueprints,
            "the index key is the blueprint gallery's"
        );
    }
}
