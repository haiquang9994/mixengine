//! What has to happen once, before a service is ever started — roadmap task **T33**.
//!
//! A [`Recipe`](super::Recipe) can describe a service completely and still not say "before this ever
//! runs, bootstrap a data directory and set a password in it". MariaDB is the first service that
//! needs it and PostgreSQL (T34) is the second, which is why this is a hook on the trait rather than
//! something inside one recipe.
//!
//! # Two halves, and the reason is the keyring
//!
//! A recipe lives in `mixengine-core`, which has no business reaching an OS credential store; the
//! daemon holds the [`Keyring`](mixengine_platform::Keyring) and is the only thing that should. So a
//! recipe **declares** what it needs — [`SecretSpec`] — the daemon generates the value and stores
//! it, and the value arrives back inside the [`Context`] the recipe is handed to build its steps.
//! One place makes a credential, and no recipe carries a platform call.
//!
//! **Storing comes before touching the disk.** The daemon writes to the keyring first and runs the
//! first step second, so a machine with no credential store fails while nothing has been created —
//! rather than half-way through, leaving a data directory whose root password exists nowhere.
//!
//! # Doing it once, and knowing which "once" this is
//!
//! Two marker files, and the pair is what makes cleaning safe — see [`inspect`].

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use mixengine_proto::Millis;

use super::recipe::Context;
use crate::{Error, Result, paths};

/// The file that says a ritual was begun here.
///
/// Written before the first step and removed by nothing: what takes it away is the directory being
/// cleared, which only ever happens to a directory carrying it and not [`READY_MARKER`].
pub const STARTED_MARKER: &str = ".mixengine-init-started";

/// The file that says one finished. Its contents are the package version that performed it.
///
/// Nothing reads that version yet. The thing that will is a version upgrade needing
/// `mariadb-upgrade`, and a marker written without it would have to be guessed at later.
pub const READY_MARKER: &str = ".mixengine-ready";

/// A credential a ritual needs, declared rather than generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretSpec {
    /// What the recipe reads it back by: `context.secret("root")`.
    pub key: &'static str,

    /// How many characters the generated value has.
    pub length: usize,
}

/// A first-run ritual, as a recipe declares it.
///
/// Declared with no [`Context`] because the daemon has to know whether there *is* a ritual before it
/// generates a secret for one, and a context is not what that question is about. The steps are a
/// function pointer rather than a second trait method so that the declaration and the thing it
/// declares are one value — a recipe cannot say it needs a secret and then have no ritual, or have
/// one that is never asked for.
#[derive(Debug, Clone, Copy)]
pub struct Ritual {
    /// The credentials the daemon generates and stores before the first step runs.
    pub secrets: &'static [SecretSpec],

    /// Builds the steps, from a context that already carries those credentials.
    pub steps: fn(&Context) -> Result<Vec<Step>>,
}

/// One program a ritual runs.
pub struct Step {
    /// What the progress line says: `creating the data directory`.
    pub label: String,

    /// The program. Absolute, for [`ServiceSpec`](mixengine_proto::ServiceSpec)'s reason: a relative
    /// one is whatever the `PATH` happens to say at the moment it runs.
    pub program: PathBuf,

    /// What to pass it.
    pub args: Vec<String>,

    /// Fed to the program's standard input, which is then closed.
    ///
    /// This is how SQL reaches `mariadbd --bootstrap` without a temporary file — which for a
    /// statement that sets a root password would be a plaintext credential on disk.
    pub stdin: Option<String>,

    /// The whole environment, over the platform's own floor.
    pub env: BTreeMap<String, String>,

    /// Where it runs.
    pub cwd: PathBuf,

    /// How long it is given before it is killed and the ritual has failed.
    pub timeout: Millis,
}

/// Written by hand, and [`Step::stdin`] is the reason.
///
/// It carries a generated password. `.claude/standards/rust.md`'s rule is that a struct which
/// *might* hold a secret redacts it rather than trusting every caller that ever writes `{:?}`, and a
/// `tracing` field on a step that failed is one line away at all times. The length stays, because it
/// is what a reader debugging a bootstrap actually needs.
impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Step")
            .field("label", &self.label)
            .field("program", &self.program)
            .field("args", &self.args)
            .field(
                "stdin",
                &self
                    .stdin
                    .as_ref()
                    .map(|input| format!("<{} bytes>", input.len())),
            )
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("cwd", &self.cwd)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// A ritual, bound to the service it is for — what a [`Generated`](super::Generated) carries.
///
/// Holds the [`Context`] rather than the steps, because the steps cannot be built until the daemon
/// has generated the secrets, and the daemon should not generate them until it knows the ritual is
/// needed at all.
#[derive(Debug, Clone)]
pub struct FirstRun {
    data: PathBuf,
    version: String,
    ritual: Ritual,
    context: Context,
}

impl FirstRun {
    /// The ritual `ritual` declares, for the service `context` describes.
    pub(super) fn new(context: &Context, ritual: Ritual) -> Self {
        Self {
            data: context.data().to_path_buf(),
            version: context.version().to_owned(),
            ritual,
            context: context.clone(),
        }
    }

    /// The data directory whose markers say whether this has been done.
    #[must_use]
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// The package version that is about to perform it, recorded in [`READY_MARKER`].
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Where this service's credential `key` lives in the OS keyring.
    ///
    /// The daemon's half of [`Context::secret_address`], and literally the same composition: this is
    /// what makes "the recipe named it" and "the daemon wrote it" one address rather than two that
    /// happen to match.
    #[must_use]
    pub fn secret_address(&self, key: &str) -> String {
        self.context.secret_address(key)
    }

    /// What the daemon has to generate and store before the first step runs.
    #[must_use]
    pub fn secrets(&self) -> &'static [SecretSpec] {
        self.ritual.secrets
    }

    /// The steps, now that those credentials exist.
    ///
    /// # Errors
    ///
    /// Whatever this recipe cannot answer for this instance — a path this system will not accept, an
    /// executable the install does not publish.
    pub fn steps(&self, secrets: BTreeMap<String, String>) -> Result<Vec<Step>> {
        let mut context = self.context.clone();
        context.set_secrets(secrets);

        (self.ritual.steps)(&context)
    }
}

/// What is in a data directory, as far as whether a ritual has been performed in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataDirectory {
    /// It is not there, or it is there and empty. Bootstrap.
    Empty,

    /// A ritual we began and did not finish. Clear it and bootstrap again.
    Unfinished,

    /// A ritual that finished, by the version this records.
    Ready {
        /// What [`READY_MARKER`] holds.
        version: String,
    },

    /// It has contents and neither marker. **Refuse, and touch nothing.**
    Foreign,
}

/// Read `data` and say which of the four it is.
///
/// `.claude/features/services.md` says a half-finished data directory is "detected and cleaned
/// rather than reused", and [`DataDirectory::Foreign`] is what keeps that sentence from also meaning
/// *MixEngine deletes a database it did not create*. Deleting a data directory is not reversible, so
/// it happens only where we left our own evidence that we were mid-ritual.
///
/// # Errors
///
/// [`Error::Io`] when the directory is there and cannot be read — a permission, a file where a
/// directory belongs. Deliberately not answered as [`DataDirectory::Empty`]: a caller that
/// bootstrapped into a directory it could not read would be one bad path away from writing over
/// something that matters.
pub async fn inspect(data: &Path) -> Result<DataDirectory> {
    let mut entries = match tokio::fs::read_dir(data).await {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DataDirectory::Empty);
        }
        Err(source) => {
            return Err(Error::Io {
                action: "read the data directory at",
                path: data.to_path_buf(),
                source,
            });
        }
    };

    let mut anything = false;
    let mut started = false;

    while let Some(entry) = entries.next_entry().await.map_err(|source| Error::Io {
        action: "read the data directory at",
        path: data.to_path_buf(),
        source,
    })? {
        anything = true;

        if entry.file_name() == STARTED_MARKER {
            started = true;
        }
    }

    // Read rather than listed, because its contents are half the answer.
    match tokio::fs::read_to_string(data.join(READY_MARKER)).await {
        Ok(version) => {
            return Ok(DataDirectory::Ready {
                version: version.trim().to_owned(),
            });
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(Error::Io {
                action: "read the first-run marker at",
                path: data.join(READY_MARKER),
                source,
            });
        }
    }

    Ok(match (anything, started) {
        (false, _) => DataDirectory::Empty,
        (true, true) => DataDirectory::Unfinished,
        (true, false) => DataDirectory::Foreign,
    })
}

/// Record that a ritual is beginning here, creating the directory if it is not there.
///
/// # Errors
///
/// [`Error::Io`] when the directory or the marker cannot be written.
pub async fn mark_started(data: &Path) -> Result<()> {
    paths::create_dir(data)?;

    tokio::fs::write(data.join(STARTED_MARKER), b"")
        .await
        .map_err(|source| Error::Io {
            action: "write the first-run marker at",
            path: data.join(STARTED_MARKER),
            source,
        })
}

/// Record that one finished, and which version performed it.
///
/// # Errors
///
/// [`Error::Io`] when the marker cannot be written.
pub async fn mark_ready(data: &Path, version: &str) -> Result<()> {
    tokio::fs::write(data.join(READY_MARKER), version.as_bytes())
        .await
        .map_err(|source| Error::Io {
            action: "write the first-run marker at",
            path: data.join(READY_MARKER),
            source,
        })
}

/// Empty a data directory a ritual left half-finished.
///
/// **Only ever called for [`DataDirectory::Unfinished`]**, which is the whole safety argument: what
/// is removed is a directory carrying our own in-progress marker and nothing else.
///
/// The directory itself is removed and made again rather than its entries walked, which is one
/// syscall's worth of difference and no ambiguity about what a partial failure left.
///
/// # Errors
///
/// [`Error::Io`] when it cannot be removed or made again.
pub async fn clear(data: &Path) -> Result<()> {
    if let Err(source) = tokio::fs::remove_dir_all(data).await
        && source.kind() != std::io::ErrorKind::NotFound
    {
        return Err(Error::Io {
            action: "clear the half-bootstrapped data directory at",
            path: data.to_path_buf(),
            source,
        });
    }

    paths::create_dir(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A step never prints what it was given to read.
    ///
    /// **The reason [`Step`] writes its own `Debug`.** `stdin` carries the SQL that sets the root
    /// password, so a `tracing` field on a step that failed — one line away at all times — would put
    /// a generated credential in `daemon.log`. The same rule `Surroundings` follows for the same
    /// reason.
    #[test]
    fn a_step_does_not_print_what_it_reads() {
        let step = Step {
            label: "set the root password".to_owned(),
            program: PathBuf::from("/opt/mariadb/bin/mariadbd"),
            args: vec!["--bootstrap".to_owned()],
            stdin: Some("SET PASSWORD FOR 'root'@'localhost' = PASSWORD('hunter2');".to_owned()),
            env: BTreeMap::new(),
            cwd: PathBuf::from("/opt"),
            timeout: Millis::from_secs(300),
        };

        let printed = format!("{step:?}");

        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(
            printed.contains("bytes"),
            "the size is what a reader needs: {printed}"
        );
    }

    /// Nothing there is a directory to bootstrap.
    #[tokio::test]
    async fn a_directory_that_is_not_there_is_bootstrapped() {
        let home = tempfile::tempdir().expect("a directory");

        assert_eq!(
            inspect(&home.path().join("absent"))
                .await
                .expect("an absent directory is readable"),
            DataDirectory::Empty
        );
    }

    /// One we started and did not finish is cleaned and done again.
    #[tokio::test]
    async fn a_half_finished_directory_is_ours_to_clean() {
        let home = tempfile::tempdir().expect("a directory");
        let data = home.path().join("data");

        mark_started(&data).await.expect("the marker is written");
        std::fs::write(data.join("ibdata1"), b"half a database").expect("some contents");

        assert_eq!(
            inspect(&data).await.expect("readable"),
            DataDirectory::Unfinished
        );

        clear(&data).await.expect("ours to clear");

        assert_eq!(
            inspect(&data).await.expect("readable"),
            DataDirectory::Empty
        );
    }

    /// One that finished is left alone, and says what performed it.
    #[tokio::test]
    async fn a_finished_directory_records_the_version_that_made_it() {
        let home = tempfile::tempdir().expect("a directory");
        let data = home.path().join("data");

        mark_started(&data).await.expect("the marker is written");
        mark_ready(&data, "11.4.9").await.expect("and the second");

        assert_eq!(
            inspect(&data).await.expect("readable"),
            DataDirectory::Ready {
                version: "11.4.9".to_owned()
            }
        );
    }

    /// **And somebody else's database is refused rather than cleaned.**
    ///
    /// The case this whole scheme exists for: `services.md` says a half-finished data directory is
    /// cleaned, and without this it would also say MixEngine deletes a directory it never made.
    #[tokio::test]
    async fn a_directory_that_is_not_ours_is_neither_used_nor_cleaned() {
        let home = tempfile::tempdir().expect("a directory");
        let data = home.path().join("data");

        std::fs::create_dir_all(&data).expect("a directory");
        std::fs::write(data.join("ibdata1"), b"somebody's database").expect("contents");

        assert_eq!(
            inspect(&data).await.expect("readable"),
            DataDirectory::Foreign
        );
        assert!(
            data.join("ibdata1").is_file(),
            "inspecting a directory changed it"
        );
    }
}
