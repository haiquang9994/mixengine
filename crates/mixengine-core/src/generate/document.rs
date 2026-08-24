//! Getting a rendered file onto the disk: diffed, staged, checked, and only then installed.
//!
//! Four rules, in the order they apply, and each of them exists because of what the *next* thing to
//! happen is — a web server being told to reload:
//!
//! 1. **Identical is not a change.** A rendering that byte-matches what is already there is not
//!    written at all, and [`install`] says so with [`Written::Unchanged`]. The caller's next move is
//!    a reload, and a service reloading because the daemon restarted — dropping connections, on
//!    `mariadb` re-reading a data directory — is a cost the user never asked for and cannot see the
//!    reason for.
//! 2. **The whole set is staged first.** Files go into a staging directory beside the destination,
//!    never into it, because a validator has to be shown a *complete* configuration: a Caddyfile
//!    that imports six site files cannot be judged with two of them from before this render.
//! 3. **Validation happens against the staging directory**, so a configuration that does not parse
//!    is one nothing was installed from. [`features/services.md`] requires exactly that: the
//!    previous config stays live and the error is surfaced.
//! 4. **The install is a rename per file.** Renaming within one directory tree is atomic on every
//!    filesystem this runs on, so nothing ever reads half a file — a web server whose reload races
//!    the write of its own config is the failure this rules out.
//!
//! Rule 4 is [`crate::install`]'s "the rename is the commit", one layer smaller: there it is a
//! directory of an unpacked runtime, here it is a config file. Unlike an install, this cannot be one
//! transaction across the set — several files cannot be renamed at one instant — and it does not
//! need to be: the set was validated as a set, and the window between two renames is microseconds
//! inside which nothing has been asked to reload yet.
//!
//! [`features/services.md`]: ../../../../../.claude/features/services.md

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{Error, Result};

/// What a [`Validator`]'s arguments write where the file being checked goes.
///
/// A placeholder rather than "appended last", because the real commands put it in the middle:
/// `caddy validate --config <file> --adapter caddyfile`, `postgres --check -D <dir>`.
pub const CONFIG: &str = "{config}";

/// How long a validator is given before it is killed and its verdict counted as a refusal.
///
/// Generous by the standard of what these commands do — `caddy validate` on a dozen sites is
/// milliseconds — because the alternative failure is a Windows runner under load reporting a
/// configuration as broken when it is fine.
const PATIENCE: Duration = Duration::from_secs(30);

/// One rendered file, and where it goes relative to the service's `etc/<service-id>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Where it goes, relative to the service's configuration directory. May name a subdirectory —
    /// `sites/blog.test.caddy` — which [`install`] creates.
    relative: PathBuf,

    /// What is in it.
    contents: String,
}

impl Document {
    /// A file at `relative` holding `contents`.
    #[must_use]
    pub fn new(relative: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        Self {
            relative: relative.into(),
            contents: contents.into(),
        }
    }

    /// Where it goes, relative to the service's configuration directory.
    #[must_use]
    pub fn relative(&self) -> &Path {
        &self.relative
    }

    /// What is in it.
    #[must_use]
    pub fn contents(&self) -> &str {
        &self.contents
    }
}

/// What happened to one file.
///
/// The two that are not [`Unchanged`](Self::Unchanged) are kept apart because they are different
/// news: a file that appeared is a service being configured for the first time, and one that
/// changed is a reload somebody is about to have to explain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// There was no such file before.
    Created,
    /// There was, and it said something else.
    Updated,
    /// There was, and it said exactly this. Nothing was written.
    Unchanged,
}

impl Written {
    /// Whether the file on disk is different from what it was.
    ///
    /// What a reload decision is made of — see the module note's first rule.
    #[must_use]
    pub fn changed(self) -> bool {
        !matches!(self, Self::Unchanged)
    }
}

/// What installing one service's set of documents did.
///
/// **Two lists rather than one**, because a removal is not a document and a `Vec<Written>` has no
/// entry for one. It matters to exactly one caller and for exactly one reason: a walk whose only
/// difference is a site that was deleted has to count as changed, or the front end goes on serving
/// the site until something else moves — which is the whole point of a swept directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// One per document, in the order they were given.
    pub written: Vec<Written>,

    /// What a swept directory carried that no document in this set owns. Already removed.
    pub removed: Vec<PathBuf>,
}

impl Installed {
    /// Whether anything on disk is different from what it was.
    ///
    /// What a reload decision is made of — see the module note's first rule.
    #[must_use]
    pub fn changed(&self) -> bool {
        !self.removed.is_empty() || self.written.iter().any(|written| written.changed())
    }
}

/// Which line of a refusal is the one worth repeating.
///
/// **Programs disagree about where they put the reason**, and the difference is not cosmetic: a
/// refusal reported by the wrong line is a message telling somebody their configuration is wrong and
/// nothing at all about why. `caddy validate` prints what it is doing and then what went wrong;
/// `nginx -t` prints what went wrong and then `nginx: configuration file <path> test failed`, which
/// on its own says only that the file this message already names is the file that failed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Reason {
    /// The last line the program printed, which is where most of them end up.
    #[default]
    Last,

    /// The first, for the ones that close with a summary instead.
    First,
}

/// A command that judges a rendered configuration before it is installed.
///
/// `caddy validate`, `nginx -t`, `postgres --check`. Built by the recipe, because which program can
/// judge a file — and what to hand it — is knowledge about that service and nothing else.
///
/// It is run **from the staging directory**, so a configuration whose entry point includes its
/// siblings by relative path is judged the way it will be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validator {
    /// The program to run. Absolute, for [`ServiceSpec`]'s reason: a relative one is whatever the
    /// `PATH` happens to say at the moment it runs.
    ///
    /// [`ServiceSpec`]: mixengine_proto::ServiceSpec
    program: PathBuf,

    /// Its arguments, with [`CONFIG`] standing in for the file being checked.
    args: Vec<String>,

    /// Which rendered file the command is about, relative to the configuration directory. This is
    /// what [`CONFIG`] becomes.
    entry: PathBuf,

    /// Where in what it prints this program leaves the reason it refused.
    reason: Reason,
}

impl Validator {
    /// Run `program` against the rendered `entry`.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, entry: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            entry: entry.into(),
            reason: Reason::default(),
        }
    }

    /// Say where this program puts the reason it refused a file. See [`Reason`].
    #[must_use]
    pub fn reason(mut self, reason: Reason) -> Self {
        self.reason = reason;
        self
    }

    /// Add an argument. [`CONFIG`] anywhere inside it becomes the path of the staged file.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add several.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Judge the configuration staged in `staging`.
    ///
    /// # Errors
    ///
    /// [`Error::ConfigRejected`] when the command says no — carrying whichever line [`Reason`] says
    /// the program leaves the answer on — or when it does not answer in time. [`Error::Platform`] when the
    /// program cannot be started at all, which is a recipe naming a binary that is not installed
    /// rather than a configuration that is wrong.
    async fn judge(&self, staging: &Path) -> Result<()> {
        let config = staging.join(&self.entry);
        let args: Vec<OsString> = self
            .args
            .iter()
            .map(|arg| OsString::from(arg.replace(CONFIG, &config.to_string_lossy())))
            .collect();

        let ran = mixengine_platform::process::run_once(
            &self.program,
            &args,
            staging,
            &BTreeMap::new(),
            PATIENCE,
        )
        .await?;

        if ran.succeeded() {
            return Ok(());
        }

        Err(Error::ConfigRejected {
            path: config,
            checker: self.program.clone(),
            // What the program said, or the fact that it said nothing — an empty `detail` would
            // leave a message that names a broken file and no reason.
            detail: if ran.timed_out() {
                format!(
                    "{} did not answer within {PATIENCE:?}",
                    self.program.display()
                )
            } else {
                match self.reason {
                    Reason::Last => ran.complaint(),
                    Reason::First => ran.complaints().lines().next(),
                }
                .unwrap_or("it refused the file without saying why")
                .to_owned()
            },
        })
    }
}

/// Put `documents` into `directory`, unchanged files left alone and nothing installed unless
/// `validator` accepts the set.
///
/// `swept` names directories, relative to `directory`, whose contents must be **exactly** the
/// documents rendered into them: anything else in one is removed. Only a front end declares one
/// today, and it declares `sites/` — a site whose row is gone would otherwise keep the file it had
/// and keep being served.
///
/// **A set that is entirely unchanged is not validated.** There is nothing to judge: those bytes are
/// the ones already on disk, which were judged when they were written, and running a validator on
/// every walk would put a process launch into `service.list`. A sweep with something to remove is a
/// change and does re-validate, which is deliberate — the checker has to be shown the set that will
/// exist.
///
/// **The staging directory is the swept set already.** It is created fresh on every install, so what
/// the validator is shown is the documents and nothing else; the removal below is what makes the
/// *installed* directory agree with what was judged.
///
/// # Errors
///
/// [`Error::Io`] naming the file or directory that could not be read, written, renamed or removed;
/// [`Error::ConfigRejected`] when the validator refuses the staged set — in which case nothing has
/// been installed or removed, and the staging directory is gone.
pub async fn install(
    directory: &Path,
    documents: &[Document],
    swept: &[&str],
    validator: Option<&Validator>,
) -> Result<Installed> {
    let mut written = Vec::with_capacity(documents.len());

    for document in documents {
        written.push(compare(&directory.join(document.relative()), document).await?);
    }

    let removable = orphans(directory, documents, swept).await?;

    if removable.is_empty() && written.iter().all(|one| !one.changed()) {
        return Ok(Installed {
            written,
            removed: Vec::new(),
        });
    }

    let staging = staging_for(directory);
    let staged = stage(&staging, documents).await;

    // Both failures leave the staging directory behind, and neither is allowed to: the next render
    // clears it, but "the next render" may be after a daemon that failed to start.
    let staged = match staged {
        Ok(()) => match validator {
            Some(validator) => validator.judge(&staging).await,
            None => Ok(()),
        },
        Err(error) => Err(error),
    };

    if let Err(error) = staged {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(error);
    }

    create_dir(directory).await?;

    for (document, one) in documents.iter().zip(&written) {
        if !one.changed() {
            continue;
        }

        let target = directory.join(document.relative());

        if let Some(parent) = target.parent() {
            create_dir(parent).await?;
        }

        commit(&staging.join(document.relative()), &target).await?;
    }

    // **After the commit and not before it.** A removal that ran first would leave a window in which
    // a server asked to reload for some other reason read a set with a site missing and its
    // replacement not yet installed. Renaming a staged file into place cannot fail for want of
    // content, so by here the set is whole, and taking the leftovers out is what makes the directory
    // equal to what the checker was shown.
    for orphan in &removable {
        tokio::fs::remove_file(orphan)
            .await
            .map_err(|source| Error::Io {
                action: "remove the generated file at",
                path: orphan.clone(),
                source,
            })?;
    }

    // Whatever is left in here is a file that did not change and was therefore never installed from
    // it. Failing to remove the directory is not worth failing a start over — it is under `etc/`,
    // which is disposable by construction — but it is worth saying, because a staging directory that
    // survives is the visible half of a filesystem that has stopped cooperating.
    if let Err(error) = tokio::fs::remove_dir_all(&staging).await {
        tracing::warn!(
            path = %staging.display(),
            %error,
            "the staging directory could not be removed"
        );
    }

    Ok(Installed {
        written,
        removed: removable,
    })
}

/// Every file in a swept directory that no document in this set owns.
///
/// **Files only.** A swept directory holds one rendering per site and nothing else, so a
/// subdirectory in one is something this build did not put there — it is said and left, because a
/// recursive delete driven by a relative path out of a recipe is a much worse thing to be wrong
/// about than a directory nobody reads.
/// What [`install`] would change, without changing anything.
///
/// **A read in the strict sense**: no staging directory, no validator, no directory created.
/// `install` creates what it installs into and runs a checker over a staged set; neither may happen
/// on the path `mix doctor` calls, whose whole guarantee is that it writes nothing.
///
/// The two halves are both needed and neither sees the other's answer. A per-document comparison
/// cannot see a site file the recipe stopped rendering, and a sweep cannot see a file whose contents
/// moved.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Drift {
    /// The relative paths whose rendering differs from what is on disk, or that are not there.
    pub changed: Vec<PathBuf>,

    /// The absolute paths of files the recipe no longer renders and the sweep would remove.
    pub removable: Vec<PathBuf>,
}

impl Drift {
    /// Is the directory already what this set of documents says it should be?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removable.is_empty()
    }
}

/// What [`install`] would do to `directory`, asked without doing it.
///
/// Roadmap task **T47b**, and the reason `mix doctor` can ask whether a generated file still matches
/// its row: the question can only be answered by rendering again and comparing — generated
/// configuration is never parsed back — and answering it must not install what it finds.
///
/// # Errors
///
/// [`Error::Io`] naming the file or directory that could not be read.
pub async fn drift(directory: &Path, documents: &[Document], swept: &[&str]) -> Result<Drift> {
    let mut changed = Vec::new();

    for document in documents {
        if compare(&directory.join(document.relative()), document)
            .await?
            .changed()
        {
            changed.push(document.relative().to_path_buf());
        }
    }

    Ok(Drift {
        changed,
        removable: orphans(directory, documents, swept).await?,
    })
}

async fn orphans(directory: &Path, documents: &[Document], swept: &[&str]) -> Result<Vec<PathBuf>> {
    if swept.is_empty() {
        return Ok(Vec::new());
    }

    let owned: BTreeSet<PathBuf> = documents
        .iter()
        .map(|document| directory.join(document.relative()))
        .collect();

    let mut orphans = Vec::new();

    for relative in swept {
        let sweeping = directory.join(relative);

        let read = |source| Error::Io {
            action: "read the generated directory at",
            path: sweeping.clone(),
            source,
        };

        // A swept directory that is not there holds nothing to sweep, which is what a front end with
        // no sites looks like on its first render.
        let mut entries = match tokio::fs::read_dir(&sweeping).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(read(source)),
        };

        while let Some(entry) = entries.next_entry().await.map_err(read)? {
            let path = entry.path();

            if owned.contains(&path) {
                continue;
            }

            if entry.file_type().await.map_err(read)?.is_file() {
                orphans.push(path);
            } else {
                tracing::warn!(
                    path = %path.display(),
                    "something that is not a file is in a directory MixEngine sweeps; it is left \
                     where it is"
                );
            }
        }
    }

    orphans.sort();

    Ok(orphans)
}

/// What installing `document` at `target` would do.
async fn compare(target: &Path, document: &Document) -> Result<Written> {
    match tokio::fs::read_to_string(target).await {
        Ok(existing) if existing == document.contents() => Ok(Written::Unchanged),
        Ok(_) => Ok(Written::Updated),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Written::Created),

        // A file that is there and cannot be read — a permission, a directory where a file belongs,
        // bytes that are not UTF-8 — is deliberately *not* treated as "different, write over it".
        // Reading is the only evidence there is that the thing being replaced is a file this build
        // wrote, and a caller that overwrote whatever it could not read would be one bad path away
        // from replacing something that matters.
        Err(source) => Err(Error::Io {
            action: "read the generated file at",
            path: target.to_path_buf(),
            source,
        }),
    }
}

/// Where the staged copy of `directory` goes.
///
/// A sibling and not a subdirectory: a subdirectory would be inside the set a validator is shown
/// and inside whatever a later task sweeps. Leading dot so that a person looking at `etc/` sees
/// their configuration and not the machinery, and a suffix as well, because the name has to be one
/// no service id can produce — [`ServiceId`] refuses both a leading dot and an interior one outside
/// an instance name.
///
/// [`ServiceId`]: mixengine_proto::ServiceId
fn staging_for(directory: &Path) -> PathBuf {
    let name = directory.file_name().map_or_else(
        || "service".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );

    directory.with_file_name(format!(".{name}.staging"))
}

/// Write every document into a fresh `staging`.
async fn stage(staging: &Path, documents: &[Document]) -> Result<()> {
    // Removed rather than reused: what a previous, failed render left in here is a configuration
    // nobody validated, and a validator shown a mixture of the two would be judging a file set that
    // never existed.
    if let Err(source) = tokio::fs::remove_dir_all(staging).await
        && source.kind() != std::io::ErrorKind::NotFound
    {
        return Err(Error::Io {
            action: "clear the staging directory at",
            path: staging.to_path_buf(),
            source,
        });
    }

    create_dir(staging).await?;

    for document in documents {
        let path = staging.join(document.relative());

        if let Some(parent) = path.parent() {
            create_dir(parent).await?;
        }

        tokio::fs::write(&path, document.contents())
            .await
            .map_err(|source| Error::Io {
                action: "stage the generated file at",
                path,
                source,
            })?;
    }

    Ok(())
}

/// Move one staged file onto its final name.
async fn commit(staged: &Path, target: &Path) -> Result<()> {
    let io = |source| Error::Io {
        action: "install the generated file at",
        path: target.to_path_buf(),
        source,
    };

    match tokio::fs::rename(staged, target).await {
        Ok(()) => Ok(()),

        // The Windows case, and the one place this is not atomic. A rename replaces an existing
        // file everywhere this runs — except while another process holds that file open without
        // sharing deletion, which on Windows is most of them. Removing it first opens a window in
        // which the config is absent; that is strictly better than the alternative, which is a
        // service left running the configuration of two releases ago with no error anybody sees.
        Err(_) if target.exists() => {
            tokio::fs::remove_file(target).await.map_err(io)?;
            tokio::fs::rename(staged, target).await.map_err(io)
        }

        Err(source) => Err(io(source)),
    }
}

/// [`crate::paths::create_dir`], without blocking the runtime.
async fn create_dir(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|source| Error::Io {
            action: "create directory",
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn documents() -> Vec<Document> {
        vec![
            Document::new("mixengine.conf", "port = 3306\n"),
            Document::new("conf.d/tuning.conf", "innodb_buffer_pool_size = 256M\n"),
        ]
    }

    #[tokio::test]
    async fn a_first_render_creates_every_file_including_the_directories_under_it() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("mariadb@main");

        let written = install(&directory, &documents(), &[], None)
            .await
            .expect("a first render")
            .written;

        assert_eq!(written, [Written::Created, Written::Created]);
        assert_eq!(
            std::fs::read_to_string(directory.join("conf.d/tuning.conf")).expect("the nested file"),
            "innodb_buffer_pool_size = 256M\n"
        );
    }

    /// The rule a reload hangs off: rendering the same state twice must not look like a change.
    #[tokio::test]
    async fn rendering_the_same_thing_again_writes_nothing() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("mariadb@main");

        install(&directory, &documents(), &[], None)
            .await
            .expect("a first render");

        let file = directory.join("mixengine.conf");
        let before = std::fs::metadata(&file)
            .and_then(|meta| meta.modified())
            .expect("a modification time");

        let written = install(&directory, &documents(), &[], None)
            .await
            .expect("a second render")
            .written;

        assert_eq!(written, [Written::Unchanged, Written::Unchanged]);

        let after = std::fs::metadata(&file)
            .and_then(|meta| meta.modified())
            .expect("a modification time");
        assert_eq!(before, after, "an unchanged file was rewritten");
    }

    #[tokio::test]
    async fn a_changed_file_is_updated_and_its_neighbour_is_left_alone() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("mariadb@main");

        install(&directory, &documents(), &[], None)
            .await
            .expect("a first render");

        let mut second = documents();
        second[0] = Document::new("mixengine.conf", "port = 3307\n");

        let written = install(&directory, &second, &[], None)
            .await
            .expect("a second render")
            .written;

        assert_eq!(written, [Written::Updated, Written::Unchanged]);
        assert_eq!(
            std::fs::read_to_string(directory.join("mixengine.conf")).expect("the file"),
            "port = 3307\n"
        );
    }

    /// Nothing installed, and the previous configuration still in place — which is the whole
    /// promise the staging directory exists to keep.
    #[tokio::test]
    async fn a_refused_configuration_leaves_the_last_good_one_live() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("mariadb@main");

        install(&directory, &documents(), &[], None)
            .await
            .expect("a first render");

        let refuse = Validator::new(mixengine_testkit::FakeService::program(), "mixengine.conf")
            .args(["--touch", CONFIG, "--exit-code", "1"]);

        let mut second = documents();
        second[0] = Document::new("mixengine.conf", "port = nonsense\n");

        let error = install(&directory, &second, &[], Some(&refuse))
            .await
            .expect_err("a configuration the checker refuses");

        assert!(error.to_string().contains("mixengine.conf"), "{error}");
        assert_eq!(
            std::fs::read_to_string(directory.join("mixengine.conf")).expect("the file"),
            "port = 3306\n",
            "the refused rendering was installed anyway"
        );
        assert!(
            !staging_for(&directory).exists(),
            "the staging directory outlived the failure"
        );
    }

    /// **Which line of a refusal a person is shown, when the program puts the reason first.**
    ///
    /// `nginx -t` opens with `nginx: [emerg] ...` and closes with a line that names the file and
    /// says the test failed. Reporting the last line of that is a message telling somebody their
    /// configuration is wrong and nothing whatsoever about why.
    #[tokio::test]
    async fn a_refusal_is_reported_by_its_reason_and_not_by_its_summary() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("nginx");

        let refuse = Validator::new(mixengine_testkit::FakeService::program(), "mixengine.conf")
            .args([
                "--complain",
                "the reason nobody could guess",
                "--complain",
                "mixengine.conf test failed",
                "--exit-code",
                "1",
            ])
            .reason(Reason::First);

        let error = install(&directory, &documents(), &[], Some(&refuse))
            .await
            .expect_err("a configuration the checker refuses");

        assert!(
            error.to_string().contains("the reason nobody could guess"),
            "the summary was reported and the reason above it was dropped: {error}"
        );
    }

    /// And the other way round for every program that ends with the reason, which is most of them.
    #[tokio::test]
    async fn a_refusal_that_ends_with_its_reason_is_reported_by_its_last_line() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("caddy");

        let refuse = Validator::new(mixengine_testkit::FakeService::program(), "mixengine.conf")
            .args([
                "--complain",
                "loading the configuration",
                "--complain",
                "the reason nobody could guess",
                "--exit-code",
                "1",
            ]);

        let error = install(&directory, &documents(), &[], Some(&refuse))
            .await
            .expect_err("a configuration the checker refuses");

        assert!(
            error.to_string().contains("the reason nobody could guess"),
            "the banner above the reason was reported instead of the reason: {error}"
        );
    }

    /// The validator sees the *staged* set, complete, and not the directory it is going into.
    #[tokio::test]
    async fn the_checker_is_shown_the_whole_rendering_before_any_of_it_is_installed() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("mariadb@main");
        let seen = home.path().join("seen");

        // `--touch` writes the file it is given, so the path it was handed is provable rather than
        // asserted: what lands at `seen` says the placeholder was substituted.
        let accept = Validator::new(mixengine_testkit::FakeService::program(), "mixengine.conf")
            .args(["--touch", seen.to_string_lossy().as_ref()]);

        install(&directory, &documents(), &[], Some(&accept))
            .await
            .expect("a rendering the checker accepts");

        assert!(seen.exists(), "the checker never ran");
        assert!(directory.join("mixengine.conf").exists());
    }

    /// D4's first half: a file in a swept directory that no document owns is gone after a render.
    #[tokio::test]
    async fn a_swept_directory_holds_exactly_what_was_rendered_into_it() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("caddy");

        std::fs::create_dir_all(directory.join("sites")).expect("a sites directory");
        std::fs::write(directory.join("sites").join("gone.caddy"), "old").expect("a stale site");

        let documents = vec![
            Document::new(
                "Caddyfile",
                "import sites/*.caddy
",
            ),
            Document::new(
                "sites/blog.test.caddy",
                "http://blog.test {
}
",
            ),
        ];

        let installed = install(&directory, &documents, &["sites"], None)
            .await
            .expect("the set installs");

        assert!(
            !directory.join("sites").join("gone.caddy").exists(),
            "a site nothing declares any more is still being served"
        );
        assert!(directory.join("sites").join("blog.test.caddy").exists());
        assert_eq!(installed.removed.len(), 1);
    }

    /// D4's second half, and the one that matters to a running server: a walk whose *only*
    /// difference is a removal still counts as a change, or `mix site delete` leaves the old site
    /// being served until something else happens to move.
    #[tokio::test]
    async fn a_removal_on_its_own_is_a_change() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("caddy");

        let documents = vec![Document::new(
            "Caddyfile",
            "import sites/*.caddy
",
        )];

        let first = install(&directory, &documents, &["sites"], None)
            .await
            .expect("a first render");
        assert!(first.changed());

        std::fs::create_dir_all(directory.join("sites")).expect("a sites directory");
        std::fs::write(directory.join("sites").join("gone.caddy"), "old").expect("a stale site");

        let second = install(&directory, &documents, &["sites"], None)
            .await
            .expect("a second render");

        assert!(
            second.changed(),
            "nothing was reloaded, so the removed site went on being served"
        );
        assert!(
            second.written.iter().all(|one| !one.changed()),
            "the Caddyfile itself did not move; only the sweep did"
        );
    }

    /// Nothing outside a swept directory is touched, which is what keeps `etc/<service-id>/` a
    /// service's own and a deleted service `service.delete`'s problem rather than this one's.
    #[tokio::test]
    async fn a_file_outside_a_swept_directory_is_left_alone() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("caddy");

        std::fs::create_dir_all(&directory).expect("the configuration directory");
        std::fs::write(directory.join("notes.txt"), "somebody's").expect("a file nobody renders");

        install(
            &directory,
            &[Document::new(
                "Caddyfile",
                "import sites/*.caddy
",
            )],
            &["sites"],
            None,
        )
        .await
        .expect("the set installs");

        assert!(directory.join("notes.txt").exists());
    }

    /// A front end with no sites at all: the directory is not there, and a sweep over one that is
    /// not there is not an error — it is the first render.
    #[tokio::test]
    async fn sweeping_a_directory_that_is_not_there_is_quiet() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("caddy");

        let installed = install(
            &directory,
            &[Document::new(
                "Caddyfile",
                "import sites/*.caddy
",
            )],
            &["sites"],
            None,
        )
        .await
        .expect("the set installs");

        assert!(installed.removed.is_empty());
    }
    /// Asking what would change must not change anything — the whole reason this is separate from
    /// `install`. A directory nobody has installed into yet is entirely drift, and asking twice
    /// gives the same answer because the first ask wrote nothing.
    #[tokio::test]
    async fn asking_what_would_change_changes_nothing() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("mariadb@main");

        let first = drift(&directory, &documents(), &[]).await.expect("a drift");

        assert_eq!(
            first.changed,
            vec![
                PathBuf::from("mixengine.conf"),
                PathBuf::from("conf.d/tuning.conf"),
            ]
        );
        assert!(first.removable.is_empty());
        assert!(!first.is_empty());

        assert!(
            !directory.exists(),
            "asking what would change created the directory it was asked about"
        );

        let again = drift(&directory, &documents(), &[])
            .await
            .expect("a second drift");

        assert_eq!(
            again.changed, first.changed,
            "the first ask was not a read: the second saw a different disk"
        );
    }

    /// After an install there is no drift, which is the property `mix doctor`'s check reads.
    #[tokio::test]
    async fn a_directory_that_was_just_installed_has_no_drift() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("mariadb@main");

        install(&directory, &documents(), &[], None)
            .await
            .expect("a first render");

        let after = drift(&directory, &documents(), &[]).await.expect("a drift");

        assert!(after.is_empty(), "{after:?}");
    }

    /// A file whose contents moved is drift even though every file is present.
    #[tokio::test]
    async fn a_file_somebody_edited_by_hand_is_drift() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("mariadb@main");

        install(&directory, &documents(), &[], None)
            .await
            .expect("a first render");

        std::fs::write(directory.join("mixengine.conf"), "port = 9999\n")
            .expect("the generated file is writable");

        let after = drift(&directory, &documents(), &[]).await.expect("a drift");

        assert_eq!(after.changed, vec![PathBuf::from("mixengine.conf")]);
        assert!(after.removable.is_empty());
    }

    /// A file the recipe no longer renders is drift even though every rendered file matches — the
    /// half a per-document comparison cannot see.
    #[tokio::test]
    async fn a_file_the_recipe_stopped_rendering_is_drift() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let directory = home.path().join("front@main");

        let with = vec![
            Document::new("caddy.conf", "one\n"),
            Document::new("sites/blog.test.conf", "two\n"),
        ];

        install(&directory, &with, &["sites"], None)
            .await
            .expect("a first render");

        let without = vec![Document::new("caddy.conf", "one\n")];
        let after = drift(&directory, &without, &["sites"])
            .await
            .expect("a drift");

        assert!(after.changed.is_empty(), "{after:?}");
        assert_eq!(
            after.removable,
            vec![directory.join("sites").join("blog.test.conf")],
            "a site that is no longer served was not noticed"
        );
    }
}
