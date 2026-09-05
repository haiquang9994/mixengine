//! The fixture set is well formed — roadmap task **T89**.
//!
//! What the suite in `mixengine-core` cannot say about itself: that there is anything in the
//! directory at all, that each blob has the readable rendering beside it that makes it reviewable,
//! and that no fixture carries a `-wal` sidecar — which would be a database missing exactly the
//! commits it was captured to hold.
//!
//! The first of those is not hypothetical. `.gitignore` excludes `*.db`, so the first commit of
//! this task added the seed and the tool and **silently dropped the fixture**: `git add` said
//! nothing, a fresh clone would have had an empty directory, and every test in the core suite would
//! have looped over nothing and passed.

use mixengine_testkit::upgrade::Fixture;

#[test]
fn there_is_a_fixture_and_one_of_them_is_at_the_oldest_schema_there_has_ever_been() {
    let fixtures = Fixture::all();

    assert!(
        !fixtures.is_empty(),
        "an empty directory is a suite that reads nothing and passes"
    );

    // A fixture captured at today's schema is `Current` and proves nothing today; it starts
    // carrying evidence the day a migration lands after it. Schema 1 is the one that exercises
    // every migration this build has, on the day it is read.
    assert!(
        fixtures.iter().any(|fixture| fixture.schema() == 1),
        "no fixture at schema 1: {:?}",
        fixtures.iter().map(Fixture::name).collect::<Vec<_>>()
    );
}

#[test]
fn every_fixture_carries_the_seed_it_was_captured_from() {
    for fixture in Fixture::all() {
        assert!(
            fixture.seed_sql().contains("INSERT INTO"),
            "{} has a seed that seeds nothing",
            fixture.name()
        );
    }
}

#[test]
fn no_fixture_carries_a_write_ahead_log_beside_it() {
    for fixture in Fixture::all() {
        assert!(
            fixture.stray_siblings().is_empty(),
            "{} has {:?} beside it — the file alone is not the database",
            fixture.name(),
            fixture.stray_siblings()
        );
    }
}

#[test]
fn a_copy_is_writable_even_where_the_committed_file_is_not() {
    let temp = tempfile::TempDir::new().expect("a temporary directory");

    for fixture in Fixture::all() {
        let copy = fixture.copy_into(&temp.path().join(format!("{}.db", fixture.name())));

        assert!(copy.is_file(), "{} was not copied", fixture.name());
        assert!(
            !copy
                .metadata()
                .expect("the copy has metadata")
                .permissions()
                .readonly(),
            "{} was copied read-only, which is a `VACUUM INTO` that fails looking like a bug in \
             Store::back_up",
            fixture.name()
        );
    }
}
