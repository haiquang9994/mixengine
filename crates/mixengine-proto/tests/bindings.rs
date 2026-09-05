//! The published TypeScript contract and the crate it is generated from cannot drift apart.
//!
//! Roadmap task **T56**. `packaging/bindings.sh --check` regenerates the contract and compares the
//! bytes, which answers *"does the file say what the type says"*. This answers the questions that
//! survive a correct regeneration: is every public type in it, does any two of them collide, and is
//! everything left out of it left out on purpose.
//!
//! It is the shape `crates/mixengine-core/tests/packaging.rs` uses on `MIX_BINARIES`, for the same
//! reason: a list nothing forces to agree with the code is a list that will not.
//!
//! **No feature and no `ts-rs`.** These read the committed tree and this crate's own source, so
//! they run in the ordinary `test` job on all three operating systems rather than only in the one
//! that regenerates.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mixengine_proto::ErrorCode;

/// Public types this crate declares that are deliberately not on the wire.
///
/// **A reason each, because the list is the exemption.** Anything not here must be in the contract.
const NOT_ON_THE_WIRE: &[(&str, &str)] = &[
    (
        "ServiceSpecBuilder",
        "a builder: it constructs a ServiceSpec and is never encoded itself",
    ),
    (
        "SpecError",
        "the failure of building one, which happens below the daemon and so cannot be the wire \
         `Error` this crate also defines (see error.rs)",
    ),
    (
        "VersionError",
        "the failure of parsing a PackageVersion, for the same reason as SpecError",
    ),
];

/// The types whose `Deserialize` is written by hand.
///
/// Each of them accepts more than it emits, and the published contract states only what the daemon
/// **writes** — `.claude/decisions/0020-the-published-contract-is-the-shape-the-daemon-writes.md`.
/// A name arriving here is the moment somebody decides what the contract will and will not say, so
/// it is a test failure until they have.
const HAND_WRITTEN_DESERIALIZE: &[&str] = &[
    "EnvValue",
    "ErrorCode",
    "ExtensionId",
    "JobKind",
    "MetricsSubject",
    "Millis",
    "PackageVersion",
    "ServiceId",
    "Version",
    "VersionConstraint",
];

/// The only name in the contract this crate does not declare: `serde_json::Value`'s own binding,
/// which `ts-rs` writes for the five `params`/`result`/`overrides` positions that hold one.
const THIRD_PARTY: &[&str] = &["JsonValue"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root is two levels above this crate")
}

fn sources() -> Vec<PathBuf> {
    fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("src/ is readable") {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }

    let mut found = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    found.sort();
    found
}

/// The identifier a line begins with, after `prefix`.
fn name_after(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?;
    let name: String = rest
        .chars()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Every `pub struct` / `pub enum` declared at the top level of this crate, and which file it is in.
///
/// Column 0 on purpose: nothing in this crate declares a public type inside a module body, and a
/// type inside `#[cfg(test)]` is not part of the contract.
fn declared_types() -> BTreeMap<String, String> {
    let mut declared = BTreeMap::new();

    for source in sources() {
        let file = source
            .file_name()
            .expect("a file")
            .to_string_lossy()
            .into_owned();
        let text = std::fs::read_to_string(&source).expect("a readable source file");

        for line in text.lines() {
            let Some(name) =
                name_after(line, "pub struct ").or_else(|| name_after(line, "pub enum "))
            else {
                continue;
            };

            if let Some(first) = declared.insert(name.clone(), file.clone()) {
                panic!(
                    "two types are called {name} — {first} and {file}. The published contract names \
                     a file after each type, so the second would overwrite the first, and `ts-rs` \
                     merges rather than truncates, so the file would carry both declarations in \
                     whatever order the exporter ran them. Rename one; `#[ts(rename)]` would only \
                     move the collision somewhere a client cannot grep back into this repository."
                );
            }
        }
    }

    declared
}

/// Every declaration in the published contract, by the name of the file that holds it.
fn contract_types() -> BTreeMap<String, PathBuf> {
    fn walk(directory: &Path, found: &mut BTreeMap<String, PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("bindings/ is readable") {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                walk(&path, found);
                continue;
            }

            if path.extension().is_none_or(|extension| extension != "ts") {
                continue;
            }

            let name = path
                .file_stem()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            if name == "index" {
                continue;
            }

            found.insert(name, path);
        }
    }

    let bindings = repository_root().join("bindings");
    assert!(
        bindings.join("index.ts").is_file(),
        "there is no contract at {} — run: bash packaging/bindings.sh",
        bindings.display()
    );

    let mut found = BTreeMap::new();
    walk(&bindings, &mut found);
    found
}

/// Every public type is in the contract, and the contract holds nothing else.
///
/// A set comparison in both directions rather than a subset: a type deleted from the crate and left
/// behind in `bindings/` is the same failure read the other way round.
#[test]
fn the_contract_is_every_public_type() {
    let exempt: BTreeSet<&str> = NOT_ON_THE_WIRE.iter().map(|(name, _)| *name).collect();

    let expected: BTreeSet<String> = declared_types()
        .into_keys()
        .filter(|name| !exempt.contains(name.as_str()))
        .collect();

    let third_party: BTreeSet<&str> = THIRD_PARTY.iter().copied().collect();
    let found: BTreeSet<String> = contract_types()
        .into_keys()
        .filter(|name| !third_party.contains(name.as_str()))
        .collect();

    let missing: Vec<&String> = expected.difference(&found).collect();
    let extra: Vec<&String> = found.difference(&expected).collect();

    assert!(
        missing.is_empty(),
        "not in the published contract: {missing:?}. Put \
         `#[cfg_attr(feature = \"ts\", derive(ts_rs::TS), ts(export))]` above each and regenerate \
         with `bash packaging/bindings.sh`, or name it in NOT_ON_THE_WIRE with a reason."
    );
    assert!(
        extra.is_empty(),
        "in the published contract and nowhere in this crate: {extra:?}. Regenerate with \
         `bash packaging/bindings.sh`, which starts by deleting the tree."
    );
}

/// Every name in the contract that this crate does not declare is one somebody decided to publish.
#[test]
fn the_contract_borrows_only_what_is_written_down() {
    let declared: BTreeSet<String> = declared_types().into_keys().collect();
    let known: BTreeSet<&str> = THIRD_PARTY.iter().copied().collect();

    for name in contract_types().into_keys() {
        if declared.contains(&name) {
            continue;
        }

        assert!(
            known.contains(name.as_str()),
            "the contract carries {name}, which this crate does not declare and THIRD_PARTY does \
             not name — a `ts-rs` feature was turned on. Decide whether that type belongs in a \
             published contract before adding it here."
        );
    }
}

/// One file, one declaration.
///
/// `ts-rs` *merges* a second type into a file it has already written, in whatever order the harness
/// ran the two — so a file with two declarations is a collision that changes between runs. The
/// duplicate check in `declared_types` is what prevents it; this is what would notice.
#[test]
fn every_file_in_the_contract_declares_one_type() {
    for (name, path) in contract_types() {
        let text = std::fs::read_to_string(&path).expect("a readable binding");
        let declarations = text
            .lines()
            .filter(|line| line.starts_with("export type "))
            .count();

        assert_eq!(
            declarations,
            1,
            "{} declares {declarations} types and should declare one ({name})",
            path.display()
        );
    }
}

/// `ErrorCode`'s union is exactly the codes this build answers with.
///
/// The binding is produced by `#[ts(rename_all = "snake_case")]`, which is a second spelling of
/// [`ErrorCode::as_str`] — this crate's `Serialize` for it is hand-written, so there is no serde
/// attribute for `ts-rs` to read. This is what keeps the two spellings one list.
#[test]
fn the_contract_names_every_error_code_and_no_others() {
    let path = repository_root().join("bindings").join("ErrorCode.ts");
    let text = std::fs::read_to_string(&path).expect("bindings/ErrorCode.ts");

    let declaration = text
        .lines()
        .find(|line| line.starts_with("export type ErrorCode"))
        .expect("a declaration of ErrorCode");

    // The odd fields of a `"`-split are what was inside the quotes.
    let literals: BTreeSet<&str> = declaration.split('"').skip(1).step_by(2).collect();
    let expected: BTreeSet<&str> = ErrorCode::ALL.iter().map(|code| code.as_str()).collect();

    assert_eq!(
        literals, expected,
        "the contract's ErrorCode union and ErrorCode::as_str disagree. They are two spellings of \
         one list: `#[ts(rename_all = \"snake_case\")]` in error.rs, and the match in `as_str`."
    );
}

/// Nobody has added a lenient deserialiser without reading ADR 0020.
#[test]
fn the_hand_written_deserialisers_are_the_ones_that_were_thought_about() {
    let mut found = BTreeSet::new();

    for source in sources() {
        let text = std::fs::read_to_string(&source).expect("a readable source file");
        for line in text.lines() {
            let trimmed = line.trim_start();
            let Some(name) = name_after(trimmed, "impl<'de> serde::Deserialize<'de> for ")
                .or_else(|| name_after(trimmed, "impl<'de> Deserialize<'de> for "))
            else {
                continue;
            };

            found.insert(name);
        }
    }

    let known: BTreeSet<String> = HAND_WRITTEN_DESERIALIZE
        .iter()
        .map(|name| (*name).to_owned())
        .collect();

    assert_eq!(
        found, known,
        "the set of hand-written `Deserialize` impls has changed. Each of these accepts more than \
         it emits, and the published contract states only what the daemon writes — read \
         .claude/decisions/0020-the-published-contract-is-the-shape-the-daemon-writes.md, decide \
         what the binding should say, and then update HAND_WRITTEN_DESERIALIZE."
    );
}
