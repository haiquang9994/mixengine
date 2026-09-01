//! What a gallery signature buys, and what it refuses — roadmap task **T78a**.
//!
//! Out here rather than beside the code because the signing half of minisign is a
//! `mixengine-testkit` dev-dependency and may be nowhere else: a shipped binary holding a signing
//! key would be a signing key on every user's machine. `verify` takes its key as a parameter for
//! exactly this reason — see [`mixengine_core::blueprints::trust`].

use mixengine_core::Error;
use mixengine_core::blueprints::trust::{PUBLIC_KEY, verify};
use mixengine_testkit::Signer;

/// The whole of what a signature buys: these bytes, from that key.
#[test]
fn a_signature_from_the_key_verifies() {
    let signer = Signer::new();
    let document = b"schema = 1\n";

    verify(document, &signer.sign(document), &signer.public_key()).expect("it verifies");
}

/// **One byte changed is a failure, not a warning.** A blueprint whose command was edited after it
/// was signed is the case this exists for.
#[test]
fn a_document_that_was_edited_does_not_verify() {
    let signer = Signer::new();
    let signature = signer.sign(b"schema = 1\n");

    let error = verify(
        b"schema = 1\n# and one more line\n",
        &signature,
        &signer.public_key(),
    )
    .expect_err("it does not verify");

    assert!(
        matches!(error, Error::BlueprintSignature { .. }),
        "{error:?}"
    );
}

/// A signature from somebody else's key fails the same way: what is trusted is the key, not the
/// shape of the file.
#[test]
fn a_signature_from_another_key_does_not_verify() {
    let document = b"schema = 1\n";
    let signature = Signer::new().sign(document);
    let somebody_else = Signer::new();

    assert!(verify(document, &signature, &somebody_else.public_key()).is_err());
}

/// A file that is not a signature at all is refused as one, rather than reaching the verifier as
/// bytes it has to make sense of.
#[test]
fn a_file_that_is_not_a_signature_is_refused_as_one() {
    let signer = Signer::new();

    let error =
        verify(b"schema = 1\n", "not a signature", &signer.public_key()).expect_err("refused");

    assert!(
        matches!(error, Error::BlueprintSignature { .. }),
        "{error:?}"
    );
}

/// The constant this build ships is a key that can be read. It signs nothing here — T79 is what
/// publishes with the private half — and this is what keeps a typo in it from being discovered on
/// somebody else's machine.
#[test]
fn the_compiled_in_key_is_a_key() {
    let signer = Signer::new();
    let document = b"schema = 1\n";

    // Read as a key, and not one that vouches for a stranger's signature.
    let error = verify(document, &signer.sign(document), PUBLIC_KEY).expect_err("not this key");

    assert!(
        matches!(error, Error::BlueprintSignature { .. }),
        "a build whose own key will not parse says so differently: {error:?}"
    );
}
