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

/// What is appended to a data directory's own name to make the file that says a ritual began in it.
///
/// **Beside the directory rather than inside it, and that cost a failing run to find out.** Windows'
/// `mariadb-install-db` refuses any datadir that is not empty — "Only new or empty existing
/// directories are accepted for --datadir" — so a marker written inside before the first step is a
/// marker that stops the ritual it was meant to record. An *empty* directory is accepted, which is
/// what [`mark_started`] leaves, and the evidence that this bootstrap is ours sits next to it where
/// no bootstrapper looks.
///
/// Written before the first step and removed by nothing: what takes a data directory away is
/// [`clear`], which only ever runs on one carrying this and not [`READY_MARKER`].
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

    /// A file this step needs to exist while it runs, and never a moment longer.
    ///
    /// **MySQL is why this exists** — roadmap task **T34c**. MariaDB sets its root password through
    /// `mariadbd --bootstrap`, which reads SQL on standard input; MySQL removed `--bootstrap` at
    /// 5.7.6 and offers `--init-file` instead, which is a *path*. The three ways to get a statement
    /// carrying a generated password into that server are a file, an argument list every process on
    /// the machine can read, or a temporary server on a port anybody can connect to — and the file
    /// is the only one whose exposure is bounded by something we control.
    ///
    /// So the daemon writes it as narrowly as the OS allows, runs the step, and removes it —
    /// whether the step succeeded, failed or ran out of time. A recipe never touches the disk.
    pub secret_file: Option<SecretFile>,

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

/// A file that carries a credential, written for one step and removed after it.
///
/// Its own type rather than a pair, so that [`Step`]'s hand-written [`Debug`](fmt::Debug) has
/// somewhere to be careful: the path is what a reader debugging a bootstrap needs and the content is
/// what must never reach `daemon.log`.
pub struct SecretFile {
    /// Where it goes. Inside the home, and named by the service it is for.
    pub path: PathBuf,

    /// What it holds — a SQL statement with a generated password in it.
    pub content: String,
}

/// The content is never printed. See [`Step`]'s own reasoning.
impl fmt::Debug for SecretFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretFile")
            .field("path", &self.path)
            .field("content", &format!("<{} bytes>", self.content.len()))
            .finish()
    }
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
            .field("secret_file", &self.secret_file)
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

    /// How long the steps of this ritual ask for in total, measured before any credential exists.
    ///
    /// The daemon has to choose how long to wait for the bootstrap job *before* it generates the
    /// credentials the steps interpolate, so the measurement is taken against stand-ins of the
    /// shape [`secrets`](Self::secrets) declares. It is core that builds them rather than the
    /// caller, because it is core that decides what a recipe will accept: MariaDB refuses a root
    /// password that is empty or not alphanumeric, and a measurement made with an empty map came
    /// back as no steps at all — thirty declared minutes arriving at the daemon as zero.
    ///
    /// **Nothing is ever run with these.** The value measured is the sum of the steps' deadlines and
    /// the steps themselves are dropped; the real ones are built again from the keyring in
    /// [`steps`](Self::steps).
    ///
    /// Zero for a ritual whose steps cannot be built at all — an install missing a command it
    /// needs. That failure is about to be reported by the job itself, and a wait covering only the
    /// slack is the right length for a job that is going to fail immediately.
    #[must_use]
    pub fn budget(&self) -> Millis {
        let steps = self.steps(self.stand_ins()).unwrap_or_default();

        Millis(steps.iter().map(|step| step.timeout.0).sum())
    }

    /// A value of the declared length for every credential this ritual names.
    ///
    /// Alphanumeric because that is what `mixengine_platform::generate_secret` produces and what the
    /// recipes validate against — a stand-in that would be refused would measure nothing, which is
    /// the whole failure this exists to avoid.
    fn stand_ins(&self) -> BTreeMap<String, String> {
        self.ritual
            .secrets
            .iter()
            .map(|spec| (spec.key.to_owned(), "a".repeat(spec.length)))
            .collect()
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

/// Where the file that says a ritual began in `data` is.
///
/// `<data>.mixengine-init-started`, beside it. A directory with no parent or no name of its own —
/// a filesystem root — falls back to a marker inside, which is the honest answer for a path nothing
/// should be bootstrapping into anyway.
fn started_marker(data: &Path) -> PathBuf {
    let (Some(parent), Some(name)) = (data.parent(), data.file_name()) else {
        return data.join(STARTED_MARKER);
    };

    parent.join(format!("{}{STARTED_MARKER}", name.to_string_lossy()))
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

    // One entry is the whole question — whether there is anything here at all — so the listing
    // stops at the first rather than walking a data directory that may hold thousands of files.
    let anything = entries
        .next_entry()
        .await
        .map_err(|source| Error::Io {
            action: "read the data directory at",
            path: data.to_path_buf(),
            source,
        })?
        .is_some();

    let started = started_marker(data).exists();

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
    // Created and left **empty**, which is what the Windows bootstrapper accepts — see
    // [`STARTED_MARKER`].
    paths::create_dir(data)?;

    let marker = started_marker(data);

    tokio::fs::write(&marker, b"")
        .await
        .map_err(|source| Error::Io {
            action: "write the first-run marker at",
            path: marker,
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
/// The marker beside it is deliberately left where it is: the next attempt writes it again, and a
/// run that fails between this and that is still recognisably ours.
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
            secret_file: None,
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

    /// The same rule for a file a step needs: its content never reaches a log either.
    ///
    /// `SHOW` what it is *for* — the path and the size — and nothing of what is in it, because the
    /// only reason [`SecretFile`] exists is that MySQL's `--init-file` takes a statement carrying a
    /// generated password.
    #[test]
    fn a_step_does_not_print_the_file_it_was_given_to_write() {
        let step = Step {
            label: "set the root password".to_owned(),
            program: PathBuf::from("/opt/mysql/bin/mysqld"),
            args: vec!["--init-file=/run/mysql@main/init.sql".to_owned()],
            stdin: None,
            secret_file: Some(SecretFile {
                path: PathBuf::from("/run/mysql@main/init.sql"),
                content: "ALTER USER 'root'@'localhost' IDENTIFIED BY 'hunter2';".to_owned(),
            }),
            env: BTreeMap::new(),
            cwd: PathBuf::from("/opt"),
            timeout: Millis::from_secs(300),
        };

        let printed = format!("{step:?}");

        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(
            printed.contains("init.sql"),
            "the path is what a reader needs: {printed}"
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
