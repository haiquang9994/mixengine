//! Deciding whether a replacement helper deserved the prompt it is about to be installed by —
//! roadmap task **T88a**.
//!
//! **This is the only place in MixEngine where a signature is checked by a process running as
//! root.** Everything else the updater verifies is verified by the daemon, which runs as the user
//! and is, if it has been compromised, the attacker. `.claude/features/updates.md` calls this the
//! single most important rule on its page: an auto-updated binary that runs as root, with no OS
//! signature, is a local privilege-escalation vector — so the copy already installed, in a
//! directory an ordinary account cannot write, is the one that decides.
//!
//! # Read once, verify those bytes, write those bytes
//!
//! [`read_verified`] hands back the bytes it checked, and its caller writes **those**. Nothing
//! re-opens the file. The candidate lives in a directory the caller owns, the caller is the party
//! this binary is written not to trust, and a `verify(path)` followed by a `copy(path, …)` is a
//! check that can be stepped past by swapping the file in between. A refactor that turns the
//! `Vec<u8>` back into a path is the one plausible-looking change that would break this module.
//!
//! # What the signature does not say, and is checked beside it
//!
//! That the bytes are ours does not make them *this machine's*, and a helper that cannot be loaded
//! is a machine with no elevation left and no way back but a reinstall. Nor does it make them
//! *newer*: "only we can sign" bounds an attacker to our own past releases, which is the entire
//! content of a downgrade. Both facts travel in the signed trusted comment, which is what
//! [`HelperStamp`] reads — minisign's global signature covers that comment, and `minisign-verify`
//! hands it over only after the signature has verified.

use std::path::Path;

use mixengine_platform::elevated::others_can_write;
use mixengine_proto::PackageVersion;
use mixengine_proto::privileged::{HelperStamp, OpOutcome};

/// The key every published MixEngine release artifact is signed with, compiled into this binary.
///
/// **A second constant rather than `mixengine_core::updates::PUBLIC_KEY`**, because this crate may
/// not depend on that one — `workspace_layering.rs` — and because *"pinned in the currently
/// installed copy"* is the property `.claude/features/updates.md` asks for and this is what it
/// means. Both constants are checked against the same committed `packaging/updates.pub`, by the
/// same test read at compile time, so the two cannot drift apart without a build failing.
///
/// The same key as the feed's, and not a fourth one: `core::updates`' own module header argues it,
/// and the argument holds here. A key of its own for the one binary that runs as root would be the
/// same secret in the same place under a second name — it splits the label and not the blast
/// radius.
pub(crate) const PUBLIC_KEY: &str = "RWTYXH7WrSyA9PjseTArGpMA1b2uedummCWwZAgBp8PtNh6VDeCwqu+z";

/// The largest candidate this process will read into memory.
///
/// `mixengine-elevate` is under a megabyte on every platform this ships to. The cap is not about
/// the helper; it is about the fact that the size of that file is chosen by the untrusted caller,
/// and a process running as root must not be talked into allocating whatever is on the disk.
pub(crate) const MAX_CANDIDATE: u64 = 128 * 1024 * 1024;

/// Why a candidate will not be installed.
///
/// Two shapes, because they settle the queue differently — the T40b design, D5:
/// [`OpOutcome::Refused`] deletes the row, since the same request will be refused again, and
/// [`OpOutcome::Failed`] keeps it, since trying again may work and nothing about the request is
/// wrong.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The candidate is not one this machine will install, and asking again will not change that.
    Refused(String),

    /// Something about the machine got in the way of finding out.
    Failed(String),
}

impl Refusal {
    /// The outcome the response carries.
    pub(crate) fn into_outcome(self) -> OpOutcome {
        match self {
            Self::Refused(reason) => OpOutcome::Refused { reason },
            Self::Failed(message) => OpOutcome::Failed { message },
        }
    }

    /// `Err(Refused(…))`, spelled once because every refusal below is one.
    fn refuse<T>(why: String) -> Result<T, Self> {
        Err(Self::Refused(why))
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(why) | Self::Failed(why) => f.write_str(why),
        }
    }
}

/// Read a candidate and its detached signature, and hand back the bytes that verified.
///
/// `not_older_than` is the version the *running* helper is, which is the floor a candidate has to
/// reach. It is a parameter rather than `env!("CARGO_PKG_VERSION")` read in here for the reason
/// `public_key` is one: no test can sign under the compiled-in key, so both facts this function
/// decides against are supplied by its caller — and the caller in production supplies both in one
/// line.
///
/// # Errors
///
/// [`Refusal::Refused`] for everything about the candidate — a bad or missing signature, a trusted
/// comment that is not the grammar, another machine's build, an older release, a symlink, a file a
/// second account can write, or one past [`MAX_CANDIDATE`]. [`Refusal::Failed`] for a machine that
/// would not answer a question about its own filesystem.
pub(crate) fn read_verified(
    candidate: &Path,
    signature: &Path,
    public_key: &str,
    not_older_than: &str,
) -> Result<(Vec<u8>, HelperStamp), Refusal> {
    let bytes = read_plain_file(candidate)?;
    let signature = String::from_utf8(read_plain_file(signature)?).map_err(|_| {
        Refusal::Refused("the signature beside the candidate is not text".to_owned())
    })?;

    let key = minisign_verify::PublicKey::from_base64(public_key).map_err(|error| {
        Refusal::Failed(format!("this build's update key is not a key: {error}"))
    })?;

    let decoded = minisign_verify::Signature::decode(&signature)
        .map_err(|error| Refusal::Refused(format!("that is not a minisign signature: {error}")))?;

    // `false` refuses minisign's legacy algorithm, as both other verifiers in this product do:
    // everything MixEngine publishes is the modern pre-hashed form, and accepting the other one
    // would widen what runs as root in exchange for nothing. The call checks the signature over the
    // trusted comment as well as the one over the bytes, which is what makes the stamp below a fact
    // rather than a claim.
    key.verify(&bytes, &decoded, false).map_err(|error| {
        Refusal::Refused(format!(
            "the candidate is not signed by MixEngine's update key: {error}"
        ))
    })?;

    let stamp = HelperStamp::parse(decoded.trusted_comment()).ok_or_else(|| {
        Refusal::Refused(format!(
            "the signature is MixEngine's but says nothing this build can read about what it \
             covers: {:?}",
            decoded.trusted_comment()
        ))
    })?;

    if !stamp.is_for_host() {
        return Refusal::refuse(format!(
            "that candidate is MixEngine's {}/{} build and this machine is {}/{}; installing it \
             would leave a privileged helper this machine cannot load",
            stamp.os,
            stamp.arch,
            HelperStamp::host_os(),
            HelperStamp::host_arch()
        ));
    }

    let offered = PackageVersion::parse(stamp.version.clone()).map_err(|error| {
        Refusal::Refused(format!("{} is not a version: {error}", stamp.version))
    })?;
    let installed = PackageVersion::parse(not_older_than.to_owned()).map_err(|error| {
        Refusal::Failed(format!("this helper's own version is not one: {error}"))
    })?;

    if offered.cmp_precedence(&installed) == std::cmp::Ordering::Less {
        return Refusal::refuse(format!(
            "that candidate is {} and the helper installed here is {not_older_than}; a signature \
             says who made a binary and not that it is newer, so one of our own past releases is \
             exactly what a replacement has to refuse",
            stamp.version
        ));
    }

    Ok((bytes, stamp))
}

/// Read a file that has to be an ordinary one, small enough, and nobody else's to rewrite.
///
/// Three refusals before a byte is read, on `crate::request`'s rules and for its reasons: a symlink
/// is somebody choosing which file root opens after root has decided to trust the name; a file a
/// second local account can write makes "the user's own home" mean nothing; and the size is chosen
/// by the caller.
fn read_plain_file(path: &Path) -> Result<Vec<u8>, Refusal> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| Refusal::Refused(format!("cannot read {}: {error}", path.display())))?;

    if metadata.file_type().is_symlink() {
        return Refusal::refuse(format!("{} is a symlink", path.display()));
    }

    if !metadata.is_file() {
        return Refusal::refuse(format!("{} is not a file", path.display()));
    }

    if metadata.len() > MAX_CANDIDATE {
        return Refusal::refuse(format!(
            "{} is {} bytes and this helper will not read more than {MAX_CANDIDATE}",
            path.display(),
            metadata.len()
        ));
    }

    if others_can_write(path)
        .map_err(|error| Refusal::Failed(format!("cannot read who may write it: {error}")))?
    {
        return Refusal::refuse(format!(
            "{} can be written by an account other than its owner",
            path.display()
        ));
    }

    std::fs::read(path)
        .map_err(|error| Refusal::Failed(format!("cannot read {}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A directory holding a candidate and a signature over it, made by a key this test owns.
    struct Fixture {
        _directory: tempfile::TempDir,
        candidate: PathBuf,
        signature: PathBuf,
        public_key: String,
    }

    fn fixture(bytes: &[u8], trusted_comment: &str) -> Fixture {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let candidate = directory.path().join("mixengine-elevate");
        let signature = directory.path().join("mixengine-elevate.minisig");

        let pair = minisign::KeyPair::generate_unencrypted_keypair().expect("a key pair");
        let signed = minisign::sign(
            None,
            &pair.sk,
            std::io::Cursor::new(bytes),
            Some(trusted_comment),
            None,
        )
        .expect("a signature")
        .into_string();

        std::fs::write(&candidate, bytes).expect("the candidate");
        std::fs::write(&signature, signed).expect("the signature");

        Fixture {
            _directory: directory,
            candidate,
            signature,
            public_key: pair.pk.to_base64(),
        }
    }

    /// The grammar, for this machine, at whatever version the caller wants to talk about.
    fn comment(version: &str) -> String {
        format!(
            "{} {version} {} {}",
            HelperStamp::LABEL,
            HelperStamp::host_os(),
            HelperStamp::host_arch()
        )
    }

    #[test]
    fn a_candidate_this_key_signed_is_read_back_with_its_stamp() {
        let fixture = fixture(b"a helper, allegedly", &comment("0.2.0"));

        let (bytes, stamp) = read_verified(
            &fixture.candidate,
            &fixture.signature,
            &fixture.public_key,
            "0.1.0",
        )
        .expect("a candidate this key signed");

        assert_eq!(bytes, b"a helper, allegedly");
        assert_eq!(stamp.version, "0.2.0");
    }

    #[test]
    fn a_candidate_another_key_signed_is_refused() {
        let fixture = fixture(b"a helper, allegedly", &comment("0.2.0"));
        let stranger = minisign::KeyPair::generate_unencrypted_keypair().expect("a key pair");

        let refusal = read_verified(
            &fixture.candidate,
            &fixture.signature,
            &stranger.pk.to_base64(),
            "0.1.0",
        )
        .expect_err("a signature made with somebody else's key");

        assert!(matches!(refusal, Refusal::Refused(_)), "{refusal:?}");
    }

    /// The bytes changing after they were signed is the whole reason there is a signature.
    #[test]
    fn a_candidate_edited_after_it_was_signed_is_refused() {
        let fixture = fixture(b"a helper, allegedly", &comment("0.2.0"));
        std::fs::write(&fixture.candidate, b"a helper, tampered!").expect("the swap");

        let refusal = read_verified(
            &fixture.candidate,
            &fixture.signature,
            &fixture.public_key,
            "0.1.0",
        )
        .expect_err("bytes that are not the ones signed");

        assert!(matches!(refusal, Refusal::Refused(_)), "{refusal:?}");
    }

    /// The one thing "only we can sign" does not bound: our own past releases.
    #[test]
    fn a_candidate_older_than_this_helper_is_refused() {
        let fixture = fixture(b"an older helper", &comment("0.1.0"));

        let refusal = read_verified(
            &fixture.candidate,
            &fixture.signature,
            &fixture.public_key,
            "0.2.0",
        )
        .expect_err("a downgrade");

        assert!(
            matches!(&refusal, Refusal::Refused(why) if why.contains("0.1.0")),
            "{refusal:?}"
        );
    }

    /// The same release, signed once and installed twice, is not a downgrade — and refusing it
    /// would make a half-finished replacement impossible to finish.
    #[test]
    fn a_candidate_of_this_very_version_is_accepted() {
        let fixture = fixture(b"the same helper", &comment("0.2.0"));

        read_verified(
            &fixture.candidate,
            &fixture.signature,
            &fixture.public_key,
            "0.2.0",
        )
        .expect("the same version is not older than itself");
    }

    #[test]
    fn a_candidate_for_another_machine_is_refused() {
        let fixture = fixture(
            b"somebody else's helper",
            &format!("{} 9.9.9 plan9 s390x", HelperStamp::LABEL),
        );

        let refusal = read_verified(
            &fixture.candidate,
            &fixture.signature,
            &fixture.public_key,
            "0.1.0",
        )
        .expect_err("another machine's build");

        assert!(
            matches!(&refusal, Refusal::Refused(why) if why.contains("plan9")),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_signature_whose_comment_is_not_the_grammar_is_refused() {
        let fixture = fixture(b"a helper", "something else entirely");

        let refusal = read_verified(
            &fixture.candidate,
            &fixture.signature,
            &fixture.public_key,
            "0.1.0",
        )
        .expect_err("a comment nothing can read");

        assert!(matches!(refusal, Refusal::Refused(_)), "{refusal:?}");
    }

    #[test]
    fn a_missing_signature_is_refused_rather_than_ignored() {
        let fixture = fixture(b"a helper", &comment("0.2.0"));
        std::fs::remove_file(&fixture.signature).expect("the signature goes");

        let refusal = read_verified(
            &fixture.candidate,
            &fixture.signature,
            &fixture.public_key,
            "0.1.0",
        )
        .expect_err("no signature is not a pass");

        assert!(matches!(refusal, Refusal::Refused(_)), "{refusal:?}");
    }

    /// The cap is measured before the read, so a crafted file is refused rather than allocated.
    #[test]
    fn a_candidate_past_the_cap_is_refused_by_its_size_alone() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("enormous");
        let file = std::fs::File::create(&path).expect("the file");
        file.set_len(MAX_CANDIDATE + 1).expect("a sparse file");
        drop(file);

        let refusal = read_plain_file(&path).expect_err("past the cap");

        assert!(
            matches!(&refusal, Refusal::Refused(why) if why.contains("will not read more than")),
            "{refusal:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_candidate_a_second_account_can_write_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = fixture(b"a helper", &comment("0.2.0"));
        std::fs::set_permissions(&fixture.candidate, std::fs::Permissions::from_mode(0o666))
            .expect("the mode is set");

        let refusal = read_verified(
            &fixture.candidate,
            &fixture.signature,
            &fixture.public_key,
            "0.1.0",
        )
        .expect_err("a world-writable candidate");

        assert!(
            matches!(&refusal, Refusal::Refused(why) if why.contains("written")),
            "{refusal:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_candidate_that_is_a_symlink_is_refused() {
        let fixture = fixture(b"a helper", &comment("0.2.0"));
        let link = fixture.candidate.with_file_name("linked");
        std::os::unix::fs::symlink(&fixture.candidate, &link).expect("a symlink");

        let refusal = read_verified(&link, &fixture.signature, &fixture.public_key, "0.1.0")
            .expect_err("a symlink is somebody choosing which file root reads");

        assert!(
            matches!(&refusal, Refusal::Refused(why) if why.contains("symlink")),
            "{refusal:?}"
        );
    }

    /// The same drift test `core::updates` carries, for the second copy of the same key. Read at
    /// compile time on purpose: a `packaging/updates.pub` that is deleted or moved is then a build
    /// error, rather than a test that reads nothing and passes.
    #[test]
    fn the_committed_public_key_is_the_one_this_helper_pins() {
        const COMMITTED: &str = include_str!("../../../packaging/updates.pub");

        let key = COMMITTED
            .lines()
            .nth(1)
            .expect("packaging/updates.pub carries the key on its second line")
            .trim();

        assert_eq!(
            key, PUBLIC_KEY,
            "packaging/updates.pub and this helper's pinned key have drifted apart; a release cut \
             while they differ is one no installed helper would accept a replacement from"
        );
    }

    #[test]
    fn the_pinned_key_is_a_key() {
        minisign_verify::PublicKey::from_base64(PUBLIC_KEY).expect("PUBLIC_KEY parses");
    }
}
