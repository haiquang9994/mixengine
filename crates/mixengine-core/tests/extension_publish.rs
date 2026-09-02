//! Generating `extensions.json` — roadmap task **T81a**.
//!
//! `tests/extension_registry.rs` proves the *reading* half against a signed document over a real
//! socket. This is the half that makes the document, and the two meet in one place worth naming:
//! what `assemble` returns is fed straight back through [`Registry::listing`], so a document this
//! build could not read is one it refuses to hand over rather than one it signs.

use std::path::{Path, PathBuf};

use mixengine_core::extensions::registry::{self, Registry};
use mixengine_core::index::Timestamp;

/// A public key file, written the way `minisign -G` writes one.
fn public_key_file(directory: &Path, key: &str) -> PathBuf {
    let path = directory.join("minisign.pub");
    std::fs::write(
        &path,
        format!("untrusted comment: minisign public key\n{key}\n"),
    )
    .expect("write a public key file");
    path
}

/// An empty roster directory beside a `minisign.pub` holding `key`.
fn roster(home: &Path, key: &str) -> (PathBuf, PathBuf) {
    let manifests = home.join("extensions");
    std::fs::create_dir(&manifests).expect("a manifest directory");
    (manifests, public_key_file(home, key))
}

/// A manifest on disk, named after the id it declares.
fn manifest_file(directory: &Path, id: &str, text: &str) {
    std::fs::write(directory.join(format!("{id}.toml")), text).expect("write a manifest");
}

fn generated_at() -> Timestamp {
    "2026-09-02T11:00:00Z".parse().expect("a UTC second")
}

/// The ordinary path: every manifest in the directory becomes an entry, and the document reads back.
#[test]
fn every_manifest_becomes_an_entry() {
    let home = tempfile::tempdir().expect("a directory");
    let (manifests, key) = roster(home.path(), mixengine_core::index::PUBLIC_KEY);

    manifest_file(&manifests, "mailpit", mixengine_testkit::extension::MAILPIT);
    manifest_file(
        &manifests,
        "phpmyadmin",
        mixengine_testkit::extension::PHPMYADMIN,
    );
    // Not a manifest, and not an error either: `data/extensions/README.md` lives beside them, and
    // until T82 it is the only thing in that directory — git does not carry an empty one.
    std::fs::write(manifests.join("README.md"), "# extensions\n").expect("write a readme");

    let registry = registry::assemble(&manifests, &key, generated_at()).expect("assemble");

    assert_eq!(registry.schema, registry::SCHEMA);
    assert_eq!(registry.generated_at, generated_at());

    let listing = registry.listing();
    assert_eq!(
        listing.unreadable, 0,
        "the generator's own output must read back"
    );

    let ids: Vec<&str> = listing
        .extensions
        .iter()
        .map(|manifest| manifest.extension.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["mailpit", "phpmyadmin"],
        "entries are sorted by id, so two runs over one directory differ only in generated_at"
    );
}

/// The day before T82: a directory with no manifest in it is a document with no entry in it, not an
/// error. `tools/blueprints.py` treats an empty gallery as evidence it was pointed at the wrong
/// place; here empty is the state this task ships in, and an empty answer beats a 404.
#[test]
fn an_empty_directory_is_an_empty_document() {
    let home = tempfile::tempdir().expect("a directory");
    let (manifests, key) = roster(home.path(), mixengine_core::index::PUBLIC_KEY);

    let registry = registry::assemble(&manifests, &key, generated_at()).expect("assemble");

    assert!(registry.extensions.is_empty());
    assert_eq!(registry.listing().unreadable, 0);
}

/// The sentence with teeth: a key this repository would sign with that no installed MixEngine
/// checks against is worse than no signature, because it looks published.
#[test]
fn a_key_the_build_does_not_check_against_stops_everything() {
    let home = tempfile::tempdir().expect("a directory");
    let (manifests, key) = roster(home.path(), "RWTnotTheKeyThisBuildCompilesIn");

    manifest_file(&manifests, "mailpit", mixengine_testkit::extension::MAILPIT);

    let refused = registry::assemble(&manifests, &key, generated_at()).expect_err("a stale key");

    let said = refused.to_string();
    assert!(
        said.contains("is not the key"),
        "the message has to say the two disagree: {said}"
    );
    assert!(
        said.contains(mixengine_core::index::PUBLIC_KEY),
        "and it has to print the one that decides: {said}"
    );
}

/// A public key file that is not one. Distinct from a mismatch, because nothing was compared.
#[test]
fn a_public_key_file_of_the_wrong_shape_is_named() {
    let home = tempfile::tempdir().expect("a directory");
    let manifests = home.path().join("extensions");
    std::fs::create_dir(&manifests).expect("a manifest directory");
    let key = home.path().join("minisign.pub");
    std::fs::write(&key, "untrusted comment: minisign public key\n").expect("write a stub");

    let refused =
        registry::assemble(&manifests, &key, generated_at()).expect_err("half a key file");

    assert!(refused.to_string().contains("minisign.pub"), "{refused}");
}

/// The one rule the reader cannot have, because it sees one file and not the directory around it.
/// It is also what makes a repeated id impossible: a directory holds one `mailpit.toml`.
#[test]
fn a_file_must_be_named_after_the_id_it_declares() {
    let home = tempfile::tempdir().expect("a directory");
    let (manifests, key) = roster(home.path(), mixengine_core::index::PUBLIC_KEY);

    manifest_file(&manifests, "smtp", mixengine_testkit::extension::MAILPIT);

    let refused =
        registry::assemble(&manifests, &key, generated_at()).expect_err("a stem that lies");

    let said = refused.to_string();
    assert!(said.contains("smtp.toml"), "{said}");
    assert!(said.contains("mailpit"), "{said}");
}

/// The fixture whose id is not its file name, which is the case above arriving by accident rather
/// than on purpose: `sendmail.toml` declares `sendmail-to-mailpit`.
#[test]
fn a_fixture_whose_id_is_not_its_stem_is_refused_too() {
    let home = tempfile::tempdir().expect("a directory");
    let (manifests, key) = roster(home.path(), mixengine_core::index::PUBLIC_KEY);

    manifest_file(
        &manifests,
        "sendmail",
        mixengine_testkit::extension::SENDMAIL,
    );

    let refused =
        registry::assemble(&manifests, &key, generated_at()).expect_err("a stem that lies");

    assert!(
        refused.to_string().contains("sendmail-to-mailpit.toml"),
        "the message names the file it should have been: {refused}"
    );
}

/// A manifest the reader refuses is refused here too, with the reader's own message: the generator
/// has no opinion of its own about what a manifest may say.
#[test]
fn a_manifest_the_reader_refuses_is_reported_as_the_reader_reports_it() {
    let home = tempfile::tempdir().expect("a directory");
    let (manifests, key) = roster(home.path(), mixengine_core::index::PUBLIC_KEY);

    std::fs::write(manifests.join("broken.toml"), "schema = 1\n[extension]\n")
        .expect("write half a manifest");

    let refused =
        registry::assemble(&manifests, &key, generated_at()).expect_err("half a manifest");

    assert!(
        refused.to_string().contains("broken.toml"),
        "the reader points at the file somebody wrote: {refused}"
    );
}

/// What is assembled is what the client reads: the same type, not a JSON blob that resembles it.
#[test]
fn what_is_assembled_is_what_the_client_reads() {
    let home = tempfile::tempdir().expect("a directory");
    let (manifests, key) = roster(home.path(), mixengine_core::index::PUBLIC_KEY);
    manifest_file(&manifests, "mixdb", mixengine_testkit::extension::MIXDB);

    let registry = registry::assemble(&manifests, &key, generated_at()).expect("assemble");
    let written = serde_json::to_string(&registry).expect("serialise");
    let read: Registry = serde_json::from_str(&written).expect("a published document parses");

    let listing = read.listing();
    assert_eq!(listing.extensions.len(), 1);
    assert_eq!(listing.unreadable, 0);
    assert_eq!(listing.extensions[0].extension.id.as_str(), "mixdb");
}
