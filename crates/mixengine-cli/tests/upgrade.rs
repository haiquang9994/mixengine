//! A daemon started on a database an older build wrote — roadmap task **T89**.
//!
//! `crates/mixengine-core/tests/upgrade.rs` proves the schema migrates and the rows survive, inside
//! one process calling `Store::open`. What it cannot answer is whether the **product** starts on
//! the result: whether the daemon's readers cope with rows whose newer columns hold defaults, and
//! whether `mix` can list what was in the old file.
//!
//! One test and not a suite. Every schema claim is cheaper and clearer one layer down, which is
//! `.claude/standards/testing.md`'s rule about which layer owns a behaviour; what only this can
//! prove is that the daemon starts and reads.

mod harness;

use harness::{Home, json, stdout};
use mixengine_testkit::upgrade::Fixture;

/// `schema-0015` and not `schema-0001`: migration `0006` drops `sites` outright, so a database
/// older than it arrives here with no sites at all and the assertion that matters most would be
/// asserting an empty list. See `EMPTIED` in the core suite.
const SCHEMA: i64 = 15;

#[test]
fn a_daemon_starts_on_a_database_an_older_build_wrote_and_mix_lists_what_was_in_it() {
    let home = Home::new();

    let fixture = Fixture::all()
        .into_iter()
        .find(|fixture| fixture.schema() == SCHEMA)
        .unwrap_or_else(|| panic!("no fixture at schema {SCHEMA}"));
    fixture.copy_into(&home.database_file());

    // The upgrade happens here: the daemon's first act is `Store::open`.
    let _daemon = home.start_daemon();

    let status = json(&home.mix(&["status", "--json"]));
    assert!(
        status["daemon"]["pid"].as_u64().is_some(),
        "the daemon did not come up on a migrated database: {status}\n{}",
        home.daemon_log()
    );

    let listed = stdout(&home.mix(&["site", "list"]));
    assert!(
        listed.contains("blog.test"),
        "the site that was in the old database is gone: {listed}"
    );

    // And the safety net is where a person would look for it.
    let backup = format!("mixengine.db.bak-{}", env!("CARGO_PKG_VERSION"));
    assert!(
        home.contents().contains(&backup),
        "no {backup} in {:?}",
        home.contents()
    );
}
