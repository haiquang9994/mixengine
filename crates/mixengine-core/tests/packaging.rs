//! `packaging/common.sh`'s two arrays, against the names this crate actually looks for.
//!
//! **Roadmap task T85c is what this file exists to stop happening a second time.** `MIX_BINARIES`
//! named three binaries and [`mixengine_core::shims::source`] looked for a fourth beside the
//! running `mixengined`, so every release built from those scripts installed cleanly, started,
//! reported itself healthy, and could not run `php` — the `bin/` it fills was empty and
//! `Error::ShimMissing` was the only sign.
//!
//! The shape is [`mixengine_core::updates`]' own: `include_str!` the committed file, so one that is
//! deleted or moved is a build error rather than a test that reads nothing and passes.

use std::collections::BTreeSet;

/// What `packaging/stage.sh` sources, read at compile time.
const COMMON_SH: &str = include_str!("../../../packaging/common.sh");

/// The workspace manifest, for the membership check below.
const WORKSPACE: &str = include_str!("../../../Cargo.toml");

/// The entries of a one-line bash array declared in `packaging/common.sh`.
///
/// Panics rather than returning an empty set when the declaration is not there: an array that
/// stopped being declared is a packaging pipeline that stopped working, and a test that quietly
/// compared nothing to nothing would be the failure it is meant to catch.
fn declared(array: &str) -> BTreeSet<String> {
    let opening = format!("{array}=(");

    let line = COMMON_SH
        .lines()
        .find(|line| line.starts_with(&opening))
        .unwrap_or_else(|| panic!("packaging/common.sh declares {array} on one line"));

    line[opening.len()..]
        .trim_end_matches(')')
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// Every name a release has to contain is one the packaging scripts put in it.
///
/// Set equality and not "contains", because the failure being prevented is a list that drifted —
/// and a list somebody shortened drifts exactly as badly as one nobody lengthened.
#[test]
fn the_release_ships_every_binary_this_code_looks_for() {
    let expected: BTreeSet<String> = [
        // The CLI. The one of the four with no constant to borrow — nothing in `core` resolves it
        // by name — so it is spelled here and nowhere else.
        "mix".to_owned(),
        mixengine_core::updates::apply::SMOKE_EXECUTABLE.to_owned(),
        mixengine_core::shims::BINARY.to_owned(),
        mixengine_core::updates::apply::KEPT.to_owned(),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        declared("MIX_BINARIES"),
        expected,
        "packaging/common.sh's MIX_BINARIES and the names this crate looks for have drifted \
         apart; the array is what every packaging script stages and checks, so a release cut \
         while they differ is one that installs and then cannot do what it was installed for"
    );
}

/// And every crate the stage builds is one that exists.
///
/// A typo here is otherwise a `cargo build -p` failure seven minutes into a packaging run, on five
/// runners at once.
#[test]
fn every_crate_the_stage_builds_is_a_workspace_member() {
    for name in declared("MIX_CRATES") {
        assert!(
            WORKSPACE.contains(&format!("\"crates/{name}\"")),
            "packaging/common.sh's MIX_CRATES names {name}, which is not a member of this \
             workspace"
        );
    }
}
