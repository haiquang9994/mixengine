//! Enforces the workspace dependency direction described in
//! `.claude/architecture/overview.md`: strictly downward, `core` never depending on `daemon`.
//!
//! The test lives in `mixengine-proto` because proto is the bottom of the graph and therefore the
//! cheapest crate to build — but it checks every member of the workspace, not just this one.

use std::collections::BTreeSet;

use cargo_metadata::MetadataCommand;

/// For each workspace crate, the workspace crates it is allowed to depend on.
///
/// Adding an edge here is an architectural decision. Adding one that points upward (anything to
/// `mixengine-daemon`, say) is the bug this test exists to catch.
const ALLOWED_EDGES: &[(&str, &[&str])] = &[
    ("mixengine-proto", &[]),
    ("mixengine-platform", &["mixengine-proto"]),
    (
        "mixengine-supervisor",
        &["mixengine-platform", "mixengine-proto"],
    ),
    ("mixengine-core", &["mixengine-platform", "mixengine-proto"]),
    (
        "mixengine-elevate",
        &["mixengine-platform", "mixengine-proto"],
    ),
    (
        "mixengine-daemon",
        &[
            "mixengine-core",
            "mixengine-platform",
            "mixengine-proto",
            "mixengine-supervisor",
        ],
    ),
    // `platform` is here for `ipc::Connection` and `HomeDirs` alone — the transport `mix` dials and
    // the OS convention that says which home it dials for (roadmap task T10). Narrow on purpose: a
    // client that reached further into that crate would be doing something to the machine, which is
    // the daemon's job, and the ban on business logic in a client holds either way. Notably absent
    // is `mixengine-core`, which the CLI would otherwise want for `Paths`: it carries `sqlx`, and
    // linking a bundled SQLite into `mix` to learn that `run/` sits under the root is a trade
    // nobody would make. See `home.rs` for the one thing that duplicates instead, and for the test
    // that keeps the two answers together.
    ("mixengine-cli", &["mixengine-platform", "mixengine-proto"]),
];

#[test]
fn dependency_direction_is_downward() {
    let metadata = MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .no_deps()
        .exec()
        .expect("cargo metadata runs inside the workspace");

    let members: BTreeSet<&str> = metadata
        .packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();

    let declared: BTreeSet<&str> = ALLOWED_EDGES.iter().map(|(krate, _)| *krate).collect();
    assert_eq!(
        members, declared,
        "the workspace members and the crates listed in ALLOWED_EDGES have drifted apart"
    );

    for package in &metadata.packages {
        let allowed = ALLOWED_EDGES
            .iter()
            .find(|(krate, _)| *krate == package.name.as_str())
            .map(|(_, allowed)| *allowed)
            .expect("checked above that every member is listed");

        for dependency in &package.dependencies {
            if !members.contains(dependency.name.as_str()) {
                continue; // third-party crates are governed by deny.toml, not by this test
            }
            assert!(
                allowed.contains(&dependency.name.as_str()),
                "{} depends on {}, which the layering does not allow",
                package.name,
                dependency.name
            );
        }
    }
}
