//! The frozen databases T89's suite migrates: a `mixengine.db` captured at an older schema,
//! committed, and never regenerated.
//!
//! **There is deliberately no accessor returning a committed fixture's own path.** `Store::open`
//! migrates what it is given, so a suite handed the source would rewrite this repository's fixture
//! on its first run — and every run after that would be judging a database this build had written,
//! which is the whole design undone by one convenience method. [`Fixture::copy_into`] is the only
//! way in.
//!
//! Captured by `cargo run -p mixengine-core --example capture-upgrade-fixture -- <schema>`, which
//! refuses a destination that exists.

use std::path::{Path, PathBuf};

/// Where the committed fixtures live, resolved when this crate is compiled.
///
/// `CARGO_MANIFEST_DIR` rather than a walk up from the test binary: a consumer of this crate runs
/// from its own target directory and has no idea where this source tree is.
const DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/upgrade");

/// One committed database, and the schema version it was captured at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    schema: i64,
    name: String,
    file: PathBuf,
}

impl Fixture {
    /// Every fixture in the directory, oldest schema first.
    ///
    /// A missing directory answers an empty list rather than panicking; the test that fails on it
    /// is `there_is_a_fixture_and_one_of_them_is_at_the_oldest_schema_there_has_ever_been` in this
    /// crate's own suite, which says what is wrong in a sentence rather than in an `io::Error`
    /// about a path.
    #[must_use]
    pub fn all() -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(DIRECTORY) else {
            return Vec::new();
        };

        let mut fixtures: Vec<Self> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "db"))
            .filter_map(|file| {
                let name = file.file_stem()?.to_str()?.to_owned();
                let schema = name.strip_prefix("schema-")?.parse().ok()?;
                Some(Self { schema, name, file })
            })
            .collect();

        fixtures.sort_by_key(|fixture| fixture.schema);
        fixtures
    }

    /// The migration version this database was captured at.
    #[must_use]
    pub fn schema(&self) -> i64 {
        self.schema
    }

    /// `schema-0001` — what a failure names, so a reader knows which fixture it was.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Copy this fixture to `destination` and hand back the path it now lives at.
    ///
    /// **A read-only copy is made writable.** `std::fs::copy` carries a file's attributes across on
    /// Windows, and a read-only `mixengine.db` is a `VACUUM INTO` that fails inside
    /// `Store::back_up` — a failure that reads like a bug in the backup rather than like a checkout
    /// that happened to be read-only. Only when it *is* read-only, because on Unix clearing that
    /// flag sets every write bit there is, and widening a file nobody had narrowed would be a
    /// second bug traded for the first.
    ///
    /// # Panics
    ///
    /// When the fixture cannot be read or the destination cannot be written, which is a broken
    /// checkout rather than a failing assertion.
    pub fn copy_into(&self, destination: &Path) -> PathBuf {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).expect("a directory to copy the fixture into");
        }

        std::fs::copy(&self.file, destination)
            .unwrap_or_else(|error| panic!("copying {}: {error}", self.name));

        let mut permissions = std::fs::metadata(destination)
            .expect("the copy has metadata")
            .permissions();

        if permissions.readonly() {
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "the file was read-only and a database that cannot be written is not a \
                          database; the branch is what keeps this off every other platform"
            )]
            permissions.set_readonly(false);
            std::fs::set_permissions(destination, permissions).expect("the copy is writable");
        }

        destination.to_path_buf()
    }

    /// The committed seed this fixture was captured from — the readable rendering of the blob.
    ///
    /// # Panics
    ///
    /// When there is no `.sql` beside the `.db`. A blob with no rendering is a fixture nobody can
    /// review, which is the one cost of committing a binary at all.
    #[must_use]
    pub fn seed_sql(&self) -> String {
        let seed = self.file.with_extension("sql");
        std::fs::read_to_string(&seed)
            .unwrap_or_else(|error| panic!("reading {}: {error}", seed.display()))
    }

    /// The `-wal` and `-shm` files sitting beside this fixture, which must be none.
    ///
    /// A fixture is one file: a `-wal` beside it holds exactly the commits the main file is missing,
    /// so a copy of the main file alone would be a database without the rows it was captured for.
    #[must_use]
    pub fn stray_siblings(&self) -> Vec<String> {
        ["-wal", "-shm"]
            .into_iter()
            .filter(|suffix| PathBuf::from(format!("{}{suffix}", self.file.display())).exists())
            .map(str::to_owned)
            .collect()
    }
}
