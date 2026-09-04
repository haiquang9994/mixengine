//! Getting an artifact the index names onto this machine — completely, or not at all.
//!
//! # The whole of it is one transaction, and the rename is the commit
//!
//! `.claude/features/runtime-versions.md` states the invariant this module exists to hold: *a
//! half-extracted version must never appear in `list`*. So nothing is written where a reader will
//! look for it. The archive is unpacked into a staging directory beside the destination, everything
//! that could still refuse it happens there — the paths it promised are checked, the binary is made
//! to actually start — and only then is the directory renamed into place, which the operating system
//! does atomically or not at all. Any failure removes the staging directory and leaves the
//! destination as if nothing had been attempted.
//!
//! That ordering is why the post-install check lives here rather than in whatever calls this. Run
//! after the rename it would be a test of something already installed, and its failure would have
//! nothing to undo without deleting a directory a client may already have been told about.
//!
//! # A download survives the process that started it, and an install does not
//!
//! The two halves are deliberately unlike. The `.part` file is named after the hash the index
//! promises, lives in `cache/downloads/` and is *kept* across a failure, a cancellation and a daemon
//! restart, so asking again resumes rather than starts over — which for an eighty-megabyte artifact
//! on a bad connection is the difference between a feature and a wish. The staging directory is the
//! opposite: it belongs to one attempt and is removed by anything that goes wrong, including the
//! next attempt finding one left behind by a daemon that was killed.
//!
//! # What is not here
//!
//! **No job, and no method.** This is the mechanism; `runtime.install` is [T23]'s, on the same split
//! T19a/T19b and T22 used, and [`Watcher`] is shaped exactly like the daemon's `JobHandle` so that
//! wiring the two together is an impl and not an adapter. **No per-runtime knowledge** either: what
//! to run as a smoke test arrives as a [`SmokeTest`], because "which flag prints the version" is a
//! fact about PHP and Node, not about downloading.
//!
//! [T23]: ../../../../.claude/roadmap/phase-2-runtimes.md

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::AsyncWriteExt as _;

use crate::index::Artifact;
use crate::{Error, Result, paths};

pub(crate) mod archive;

/// Where partial downloads live, under the cache directory.
///
/// Its own directory rather than files beside `index.json`, so that emptying it is a repair somebody
/// can be told to perform without being told to be careful about which files.
const DOWNLOADS_DIR_NAME: &str = "downloads";

/// The suffix a download carries until it is known to be complete and correct.
const PART_SUFFIX: &str = ".part";

/// How long a connection may take to be established.
///
/// A download has no *total* budget on purpose — an artifact is tens of megabytes and a slow
/// connection is not a broken one, so a ceiling large enough to be safe would be too large to mean
/// anything. What is bounded instead is silence: this, and [`READ_TIMEOUT`] between two reads. That
/// is what `.claude/standards/rust.md` asks for when it says every network path carries a timeout —
/// the failure being guarded against is a socket that stops answering, not a file that is big.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the transfer may go without producing a byte.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// How many times a transfer that ends early is resumed before the failure is the answer.
///
/// Three because the case this exists for is a connection dropped mid-file, which is transient and
/// usually over by the second try; a server that truncates every response is not something more
/// attempts will fix. Each attempt resumes from what is already on disk, so the cost of one more is
/// bounded by what is left rather than by the size of the artifact.
const ATTEMPTS: u32 = 3;

/// How long the post-install check may take before it counts as a binary that does not start.
///
/// Generous for `php -v`, and it is not really the runtime that is being waited for: a first launch
/// under Defender or Smart App Control is scanned before it is allowed to run, and a machine that
/// has never seen this file pays for that once.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(60);

/// How many times the final rename is attempted.
///
/// Windows refuses to rename a directory while anything holds a handle inside it, and the smoke
/// test has just run a binary from in there: the process has exited, but an antivirus scanner or the
/// search indexer may still be reading what it left. A short retry turns a race that resolves itself
/// into a pause nobody notices.
const RENAME_ATTEMPTS: u32 = 4;

/// How long to wait between those attempts.
const RENAME_PAUSE: Duration = Duration::from_millis(150);

/// The percentage the download is finished at.
///
/// The four constants below divide the bar between the four things that take time, weighted by how
/// long each actually takes rather than evenly: the download dominates, and a bar that spent a fifth
/// of itself on a checksum would look stuck for the part that matters.
const DOWNLOADED_AT: u8 = 70;

/// The percentage the checksum is reported at.
const VERIFIED_AT: u8 = 78;

/// The percentage unpacking is reported at.
const UNPACKED_AT: u8 = 92;

/// The percentage the post-install check is reported at.
const CHECKED_AT: u8 = 97;

/// What a URL that is not one of the three archive formats means to whoever asked for it — roadmap
/// task **T82**, the design's D3.
///
/// **The package index publishes archives and nothing else** — `mkindex.py`'s `ARCHIVE_SUFFIXES` —
/// so a fourth suffix there names an artifact this build has no decompressor for, and refusing it
/// with a reason is the honest answer. An extension is the other case: Adminer's distribution is one
/// PHP file, which is what a great many small tools ship. The caller says which of the two it is, so
/// neither has to guess from the shape of a URL what the document it came out of meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAnArchive {
    /// Refuse it, with [`Error::ArtifactFormat`]. What the package index asks for.
    Refuse,

    /// Install it as one file, named by the last segment of its URL. What an extension asks for.
    OneFile,
}

/// What the staging directory gets filled with, once the URL has been read.
#[derive(Debug, Clone)]
enum Unpacking {
    /// Decompressed by [`archive::extract`].
    Archive(archive::Format),

    /// Copied in whole, under this name.
    OneFile(String),
}

/// The file name a one-file artifact takes, or [`None`] when its URL does not end in one.
///
/// **A name that came out of a document can never be a path** — which is [`Installer::part_file`]'s
/// own rule, and the reason that function names a `.part` file after a hash. So nothing here may
/// contain a separator, and `.` and `..` are refused by name rather than left for the file system to
/// interpret.
fn one_file_name(url: &str) -> Option<&str> {
    // A query string or fragment is not part of the file name, the same reading
    // `archive::Format::of` takes of the same URL.
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next()?;

    let refused =
        last.is_empty() || last == "." || last == ".." || last.contains('/') || last.contains('\\');

    (!refused).then_some(last)
}

/// What an install reports to, and asks whether it should stop.
///
/// **Shaped after the daemon's `JobHandle` rather than invented here**, which is what keeps this
/// crate from knowing about jobs at all: reporting is one `async fn`, cancellation is one question,
/// and everything else — the row, the event, the state machine — stays where it belongs. A producer
/// that could write its own ending would be a producer that could disagree with the row.
///
/// Cancellation is cooperative for the reason `mixengine_core::jobs` records: a task dropped
/// mid-download leaves a staging directory behind, and nothing dropped mid-`await` removes it. So
/// this is asked between chunks and between steps, and the work returns when it sees.
pub trait Watcher: Sync {
    /// Say how far along this is, and what it is doing.
    ///
    /// Called on a change worth reporting rather than on every read: the daemon publishes these on
    /// the same bounded stream every service transition uses.
    fn report(&self, percent: u8, message: &str) -> impl Future<Output = ()> + Send;

    /// Whether somebody has asked this to stop.
    fn is_cancelled(&self) -> bool;
}

/// The thing to run once the archive is unpacked, to find out whether it runs *here*.
///
/// **This is what the SHA-256 cannot tell you.** A checksum proves the bytes are the ones we
/// published; it says nothing about whether this machine can execute them. Every failure the
/// `requires` field of an [`Artifact`] describes — a missing VC++ redistributable, a glibc older
/// than the build's floor, an architecture running under an emulator that will not load it — is
/// invisible until something tries, and finding out at install time produces a message naming the
/// cause, while finding out later produces a loader error in the middle of somebody's work.
///
/// The executable is named as a key of [`Artifact::provides`] rather than as a path, because the
/// path inside the archive is the publisher's and the name is ours.
#[derive(Debug, Clone)]
pub struct SmokeTest {
    /// Which of the artifact's executables to run, by the name it is published under (`php`).
    pub executable: String,

    /// What to pass it. Something that exits zero quickly and touches the runtime's own machinery —
    /// `-v` for PHP, `--version` for Node.
    pub args: Vec<String>,
}

/// A runtime that is now on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Where it landed.
    pub path: PathBuf,

    /// How large the archive was, as the index declared it and the download proved it.
    pub bytes: u64,
}

/// Downloads what the index names and installs it, or leaves nothing behind.
#[derive(Debug)]
pub struct Installer {
    http: reqwest::Client,
    downloads: PathBuf,
}

impl Installer {
    /// An installer whose partial downloads live under `cache_dir`.
    ///
    /// The same directory the index cache uses, and for the same reason
    /// ([`Paths::cache`](crate::Paths::cache)): a partial download whose whole value is surviving a
    /// restart does not belong in `run/`, which is scratch belonging to the daemon currently
    /// running.
    ///
    /// # Errors
    ///
    /// [`Error::IndexTransport`] if the HTTP client cannot be constructed, which is a broken build
    /// rather than anything a user did — the variant is shared with the index client because both
    /// describe the same one thing that can go wrong before a request exists.
    pub fn new(cache_dir: &Path) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .user_agent(concat!("mixengine/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|source| Error::IndexTransport {
                // Not a document at all: the HTTP client itself would not build, which is a broken
                // build rather than a fetch of anything.
                document: "downloader",
                url: String::new(),
                source: Box::new(source),
            })?;

        Ok(Self {
            http,
            downloads: cache_dir.join(DOWNLOADS_DIR_NAME),
        })
    }

    /// Install `artifact` at `into`, reporting to `watcher` and stopping if it says so.
    ///
    /// `into` must not exist: an install never mutates a version that is already there, which is
    /// what makes a runtime directory immutable in the sense
    /// [runtime-versions.md](../../../../.claude/features/runtime-versions.md) means it.
    ///
    /// `not_an_archive` says what a URL that names no archive means here — [`NotAnArchive`], roadmap
    /// task **T82**.
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyInstalled`] when something is already at `into`; [`Error::ArtifactFormat`]
    /// for an archive shape this build cannot unpack; [`Error::ArtifactTransport`],
    /// [`Error::ArtifactIncomplete`] and [`Error::ArtifactTooLarge`] from the download;
    /// [`Error::ArtifactChecksum`] when what arrived is not what the index promised;
    /// [`Error::ArchiveUnreadable`] and [`Error::UnsafeArchiveEntry`] from unpacking;
    /// [`Error::MissingFromArtifact`] when the archive does not contain what it said it did;
    /// [`Error::SmokeTestFailed`] when it will not run here; [`Error::InstallCancelled`] when asked
    /// to stop; and [`Error::Io`] when the staging directory or the rename cannot be done.
    ///
    /// Every one of them leaves `into` untouched.
    pub async fn install<W: Watcher>(
        &self,
        artifact: &Artifact,
        into: &Path,
        smoke: Option<&SmokeTest>,
        not_an_archive: NotAnArchive,
        watcher: &W,
    ) -> Result<Installed> {
        if into.exists() {
            return Err(Error::AlreadyInstalled {
                path: into.to_path_buf(),
            });
        }

        // Read before a byte is fetched. An artifact this build cannot unpack is a refusal that
        // costs nothing now and eighty megabytes later.
        let refuse = || Error::ArtifactFormat {
            url: artifact.url.clone(),
        };
        let unpacking = match (archive::Format::of(&artifact.url), not_an_archive) {
            (Some(format), _) => Unpacking::Archive(format),
            (None, NotAnArchive::OneFile) => {
                Unpacking::OneFile(one_file_name(&artifact.url).ok_or_else(refuse)?.to_owned())
            }
            (None, NotAnArchive::Refuse) => return Err(refuse()),
        };

        let part = self.part_file(artifact);
        self.fetch(artifact, &part, watcher).await?;
        self.verify(artifact, &part, watcher).await?;

        let staging = staging_for(into)?;
        // A staging directory that is already there was left by a daemon that stopped part way
        // through this, and its contents are the wrong half of an archive nobody wants.
        discard(&staging).await;
        paths::create_dir(&staging)?;

        let staged = async {
            self.unpack(&part, &unpacking, &staging, watcher).await?;
            present(artifact, &staging)?;
            self.smoke(artifact, &staging, smoke, watcher).await?;
            promote(&staging, into).await
        }
        .await;

        if let Err(refusal) = staged {
            discard(&staging).await;
            return Err(refusal);
        }

        // Only now: until the rename above, this file is the only copy of work worth resuming.
        remove_file(&part).await;
        watcher.report(100, "installed").await;

        tracing::info!(path = %into.display(), bytes = artifact.size, "a runtime was installed");

        Ok(Installed {
            path: into.to_path_buf(),
            bytes: artifact.size,
        })
    }

    /// Where this artifact's partial download lives.
    ///
    /// Named after the hash rather than after the URL, so the same artifact offered by a mirror and
    /// by the default host resumes one download rather than starting two — and so a name that came
    /// out of a document can never be a path, which a file name built from a URL would have to be
    /// escaped to avoid.
    fn part_file(&self, artifact: &Artifact) -> PathBuf {
        self.downloads
            .join(format!("{}{PART_SUFFIX}", artifact.sha256))
    }

    /// Get the whole artifact onto disk, resuming what is already there.
    ///
    /// The last attempt is made outside the loop so that its failure is the one the caller is told
    /// about: the earlier ones are forgiven noise, and a message about attempt one would send
    /// somebody looking for a network fault that had already recovered.
    async fn fetch<W: Watcher>(&self, artifact: &Artifact, part: &Path, watcher: &W) -> Result<()> {
        paths::create_dir(&self.downloads)?;

        for attempt in 1..ATTEMPTS {
            match self.attempt(artifact, part, watcher).await {
                Ok(()) => return Ok(()),
                Err(refusal @ Error::ArtifactIncomplete { .. }) => tracing::warn!(
                    url = %artifact.url,
                    attempt,
                    error = %refusal,
                    "the download ended early; resuming it"
                ),
                Err(other) => return Err(other),
            }
        }

        self.attempt(artifact, part, watcher).await
    }

    /// One transfer, starting where the last one stopped.
    async fn attempt<W: Watcher>(
        &self,
        artifact: &Artifact,
        part: &Path,
        watcher: &W,
    ) -> Result<()> {
        if watcher.is_cancelled() {
            return Err(Error::InstallCancelled);
        }

        let mut have = match tokio::fs::metadata(part).await {
            Ok(metadata) => metadata.len(),
            // Missing, unreadable, a directory: all of them mean "there is nothing to resume".
            Err(_) => 0,
        };

        if have > artifact.size {
            // Longer than the index says the whole artifact is, so it is not a prefix of it. A
            // truncating write would leave the first bytes of whatever this is in place.
            remove_file(part).await;
            have = 0;
        }
        if have == artifact.size && have > 0 {
            // Complete. Whether it is the *right* file is the checksum's question, not this one's.
            return Ok(());
        }

        let transport = |source: reqwest::Error| Error::ArtifactTransport {
            url: artifact.url.clone(),
            source: Box::new(source),
        };

        let mut request = self.http.get(&artifact.url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }

        let response = request.send().await.map_err(transport)?;
        let status = response.status();

        // The server says the range we asked for is past the end of the file — so what is on disk
        // is longer than what it is serving, and cannot be a prefix of it. Start over rather than
        // fail forever: without this, one bad `.part` would refuse every future install of this
        // version, and `error_for_status` below would report it as an HTTP error nobody can act on.
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && have > 0 {
            remove_file(part).await;
            return Err(Error::ArtifactIncomplete {
                url: artifact.url.clone(),
                expected: artifact.size,
                received: have,
            });
        }

        let mut response = response.error_for_status().map_err(transport)?;

        // **A `200` to a ranged request is a server that ignored the range**, which a CDN edge or a
        // proxy that strips headers really does. The body is then the whole file, so appending it
        // to what is on disk would build something that is neither — and the checksum would catch
        // it, after the whole download.
        let resuming = have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;

        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).write(true);
        if resuming {
            options.append(true);
        } else {
            options.truncate(true);
        }
        let mut file = options.open(part).await.map_err(|source| Error::Io {
            action: "write",
            path: part.to_path_buf(),
            source,
        })?;

        let mut written = if resuming { have } else { 0 };
        let mut reported = downloaded(written, artifact.size);
        watcher
            .report(reported, &progress(written, artifact.size))
            .await;

        while let Some(chunk) = response.chunk().await.map_err(transport)? {
            if watcher.is_cancelled() {
                // The part file stays. Somebody who cancels an install at sixty percent and asks
                // again has not asked for those bytes to be thrown away.
                return Err(Error::InstallCancelled);
            }

            written += chunk.len() as u64;
            if written > artifact.size {
                // A body longer than the index declared, which is either a mirror serving something
                // else or a stream that never ends. Bounded here rather than by the checksum, so a
                // disk is not filled to find out.
                remove_file(part).await;
                return Err(Error::ArtifactTooLarge {
                    url: artifact.url.clone(),
                    expected: artifact.size,
                });
            }

            file.write_all(&chunk).await.map_err(|source| Error::Io {
                action: "write",
                path: part.to_path_buf(),
                source,
            })?;

            let now = downloaded(written, artifact.size);
            if now != reported {
                reported = now;
                watcher.report(now, &progress(written, artifact.size)).await;
            }
        }

        // `sync_all` and not just `flush`: the entire point of this file is that it survives the
        // process that wrote it, and bytes the OS has only promised to write do not.
        file.flush().await.map_err(|source| Error::Io {
            action: "write",
            path: part.to_path_buf(),
            source,
        })?;
        file.sync_all().await.map_err(|source| Error::Io {
            action: "write",
            path: part.to_path_buf(),
            source,
        })?;

        if written != artifact.size {
            return Err(Error::ArtifactIncomplete {
                url: artifact.url.clone(),
                expected: artifact.size,
                received: written,
            });
        }

        Ok(())
    }

    /// Check what arrived against the hash the signed index promised.
    ///
    /// A mismatch **deletes the download**, which
    /// [security-model.md](../../../../.claude/architecture/security-model.md) requires and which is
    /// also the only way out of a loop: a `.part` that cannot verify would otherwise be resumed
    /// forever, arriving at the same wrong answer each time.
    async fn verify<W: Watcher>(
        &self,
        artifact: &Artifact,
        part: &Path,
        watcher: &W,
    ) -> Result<()> {
        watcher.report(VERIFIED_AT, "verifying the download").await;

        let path = part.to_path_buf();
        let found = blocking(part, move || sha256(&path)).await?;

        // Hex from a hand-written pipeline, compared against hex we formatted: the index is written
        // lowercase today and a mirror rewriting the case would be a refusal nobody could diagnose.
        if !found.eq_ignore_ascii_case(&artifact.sha256) {
            remove_file(part).await;
            return Err(Error::ArtifactChecksum {
                url: artifact.url.clone(),
                expected: artifact.sha256.clone(),
                found,
            });
        }

        Ok(())
    }

    /// Fill the staging directory from the downloaded file, off the runtime.
    ///
    /// **Both arms go through [`blocking`]**, because copying an artifact is as much disk work as
    /// decompressing one and `.claude/standards/rust.md`'s rule is about the runtime rather than
    /// about compression.
    async fn unpack<W: Watcher>(
        &self,
        part: &Path,
        unpacking: &Unpacking,
        staging: &Path,
        watcher: &W,
    ) -> Result<()> {
        if watcher.is_cancelled() {
            return Err(Error::InstallCancelled);
        }
        watcher.report(UNPACKED_AT, "unpacking").await;

        let (archive, into) = (part.to_path_buf(), staging.to_path_buf());

        match unpacking {
            Unpacking::Archive(format) => {
                let format = *format;
                blocking(part, move || archive::extract(&archive, format, &into)).await
            }
            Unpacking::OneFile(name) => {
                let target = into.join(name);
                blocking(part, move || {
                    std::fs::copy(&archive, &target)
                        .map(|_| ())
                        .map_err(|source| Error::Io {
                            action: "write",
                            path: target,
                            source,
                        })
                })
                .await
            }
        }
    }

    /// Run the artifact once, from the staging directory, before anything is renamed into place.
    async fn smoke<W: Watcher>(
        &self,
        artifact: &Artifact,
        staging: &Path,
        smoke: Option<&SmokeTest>,
        watcher: &W,
    ) -> Result<()> {
        let Some(smoke) = smoke else {
            return Ok(());
        };
        if watcher.is_cancelled() {
            return Err(Error::InstallCancelled);
        }

        let relative =
            artifact
                .provides
                .get(&smoke.executable)
                .ok_or_else(|| Error::MissingFromArtifact {
                    url: artifact.url.clone(),
                    executable: smoke.executable.clone(),
                    path: String::new(),
                })?;
        let program = staging.join(relative);

        watcher.report(CHECKED_AT, "checking it runs here").await;

        let failed = |detail: String| Error::SmokeTestFailed {
            program: program.clone(),
            detail,
        };

        // `current_dir` is the staging directory because that is where the runtime's own files are:
        // a Windows PHP resolves its DLLs from beside the executable, which is the whole reason
        // `provides` carries a path rather than only a name.
        let running = tokio::process::Command::new(&program)
            .args(&smoke.args)
            .current_dir(staging)
            // A check that hung would otherwise outlive the timeout below and hold the staging
            // directory open, which is exactly what the rename cannot tolerate on Windows.
            .kill_on_drop(true)
            .output();

        let output = match tokio::time::timeout(SMOKE_TIMEOUT, running).await {
            Ok(Ok(output)) => output,
            // Where a missing VC++ redistributable, a glibc floor, an unloadable architecture and a
            // machine whose application control policy refuses the image all arrive: the OS refuses
            // to start it and says why. The last of those needs a sentence of its own — T94.
            Ok(Err(source)) => return Err(failed(why_it_would_not_start(&source))),
            Err(_) => {
                return Err(failed(format!(
                    "it did not answer within {} seconds",
                    SMOKE_TIMEOUT.as_secs()
                )));
            }
        };

        if !output.status.success() {
            let complaint = String::from_utf8_lossy(&output.stderr);
            let first = complaint.lines().find(|line| !line.trim().is_empty());
            return Err(failed(match first {
                Some(line) => format!("it exited with {}: {line}", output.status),
                None => format!("it exited with {}", output.status),
            }));
        }

        Ok(())
    }
}

/// What a smoke test says when the operating system would not start the program at all.
///
/// **A function rather than a `to_string()` at the call site**, so the one interesting case can be
/// exercised without a download — roadmap task **T94**. Every other failure here is transient or is
/// the packaging's fault; an application control policy refusing the image is neither, and the OS
/// message for it is a sentence about *this file* that says nothing about why every MixEngine
/// binary meets the same wall.
fn why_it_would_not_start(source: &std::io::Error) -> String {
    if mixengine_platform::refused_by_app_control(source) {
        format!("{source}; {}", mixengine_platform::APP_CONTROL_REFUSAL)
    } else {
        source.to_string()
    }
}

/// Where an install is assembled before it is allowed to exist.
///
/// Beside the destination rather than in a temporary directory, and that is the load-bearing part:
/// a rename is atomic only within one filesystem, and `MIXENGINE_HOME` can be on a disk the system
/// temporary directory is not. Assembling somewhere else would turn the commit into a copy, which
/// can be interrupted half way — the one thing this whole module is arranged to prevent.
fn staging_for(into: &Path) -> Result<PathBuf> {
    let misuse = |what: &'static str| Error::Io {
        action: "install",
        path: into.to_path_buf(),
        source: std::io::Error::other(what),
    };

    let parent = into.parent().ok_or_else(|| misuse("has no parent"))?;
    let name = into
        .file_name()
        .ok_or_else(|| misuse("does not name a directory"))?;

    // Built as an `OsString` rather than through `with_extension`, which would read `8.3.33` as a
    // stem of `8.3` and an extension of `33` and produce a name for a different version.
    let mut staging = OsString::from(".");
    staging.push(name);
    staging.push(".staging");

    Ok(parent.join(staging))
}

/// Everything the artifact said it provides is in the tree, and is a file.
///
/// Checked before the smoke test rather than trusted, because the failure it catches is a *packaging*
/// bug — an archive repacked without a binary the index still lists — and the message it produces
/// names the file, while the same bug found later is a runtime that is missing something at the
/// moment somebody needs it.
fn present(artifact: &Artifact, staging: &Path) -> Result<()> {
    for (executable, relative) in &artifact.provides {
        let path = Path::new(relative);
        let missing = || Error::MissingFromArtifact {
            url: artifact.url.clone(),
            executable: executable.clone(),
            path: relative.clone(),
        };

        // The index is signed, so this cannot be a hostile path — and it is checked anyway, because
        // the alternative is that one field of one document decides which file on this machine gets
        // run as a post-install check.
        if !archive::safe(path) {
            return Err(missing());
        }
        if !staging.join(path).is_file() {
            return Err(missing());
        }
    }

    Ok(())
}

/// Move the staged tree into place, which is the moment the install exists.
///
/// See [`RENAME_ATTEMPTS`] for why this is retried rather than attempted once.
async fn promote(staging: &Path, into: &Path) -> Result<()> {
    for _ in 1..RENAME_ATTEMPTS {
        if tokio::fs::rename(staging, into).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(RENAME_PAUSE).await;
    }

    tokio::fs::rename(staging, into)
        .await
        .map_err(|source| Error::Io {
            action: "install",
            path: into.to_path_buf(),
            source,
        })
}

/// Remove a staging directory, and say so rather than fail if it cannot be.
///
/// **Deliberately infallible.** Every caller is already on its way out with something else to
/// report, and replacing that with "and the cleanup failed too" would bury the reason the install
/// stopped. What is left behind is removed by the next attempt, which is why [`Installer::install`]
/// clears the directory before it uses it.
async fn discard(staging: &Path) {
    let path = staging.to_path_buf();
    let removed = tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&path) {
        Err(source) if source.kind() != std::io::ErrorKind::NotFound => Err((path, source)),
        _ => Ok(()),
    })
    .await;

    match removed {
        Ok(Err((path, source))) => tracing::warn!(
            path = %path.display(),
            %source,
            "a staging directory could not be removed; the next install will clear it"
        ),
        Err(source) => tracing::warn!(%source, "the staging directory could not be removed"),
        Ok(Ok(())) => {}
    }
}

/// Remove a file that is no longer worth keeping, if it is there at all.
async fn remove_file(path: &Path) {
    if let Err(source) = tokio::fs::remove_file(path).await
        && source.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %source, "a partial download could not be removed");
    }
}

/// Run blocking work on a thread that is allowed to block, blaming `path` if the thread itself dies.
///
/// A `JoinError` here means the blocking pool panicked, which nothing in this module does — it is
/// reported as an I/O failure rather than unwrapped because nothing in this crate panics.
async fn blocking<T, F>(path: &Path, work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(outcome) => outcome,
        Err(source) => Err(Error::Io {
            action: "read",
            path: path.to_path_buf(),
            source: std::io::Error::other(source),
        }),
    }
}

/// The SHA-256 of a file, as lowercase hex.
fn sha256(path: &Path) -> Result<String> {
    use sha2::Digest as _;

    let io = |source| Error::Io {
        action: "read",
        path: path.to_path_buf(),
        source,
    };

    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut hasher = sha2::Sha256::new();
    // A megabyte at a time: large enough that the syscall is not what this costs, small enough that
    // hashing an artifact does not depend on how much memory the machine has.
    let mut buffer = vec![0_u8; 1 << 20];

    loop {
        let read = file.read(&mut buffer).map_err(io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        // Cannot fail: writing to a `String` is infallible, and the format has no user input in it.
        let _ = write!(hex, "{byte:02x}");
    }

    Ok(hex)
}

/// Where the download has got to, on the shared bar.
fn downloaded(written: u64, size: u64) -> u8 {
    if size == 0 {
        return DOWNLOADED_AT;
    }
    let scaled = written.saturating_mul(u64::from(DOWNLOADED_AT)) / size;
    u8::try_from(scaled)
        .unwrap_or(DOWNLOADED_AT)
        .min(DOWNLOADED_AT)
}

/// The same thing in words, since the percentage alone does not say how much is left.
fn progress(written: u64, size: u64) -> String {
    const MIB: u64 = 1 << 20;
    format!("downloading, {} of {} MiB", written / MIB, size / MIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_sits_beside_the_destination_and_keeps_the_whole_version_in_its_name() {
        let staging = staging_for(Path::new("/home/runtimes/php/8.3.33")).expect("a target");

        assert_eq!(
            staging,
            Path::new("/home/runtimes/php/.8.3.33.staging"),
            "a rename is atomic only within one filesystem, and `8.3` would be another version"
        );
    }

    /// The last segment of a URL becomes a file name — roadmap task **T82**, the design's D3.
    ///
    /// A query string or a fragment is not part of it, the way [`archive::Format::of`] already
    /// treats them: a mirror that appends one still installs rather than failing to classify.
    #[test]
    fn a_one_file_artifact_takes_its_name_from_the_url() {
        assert_eq!(
            one_file_name("https://example.invalid/adminer-6.0.1.php").expect("a name"),
            "adminer-6.0.1.php"
        );
        assert_eq!(
            one_file_name("https://example.invalid/a/b/adminer.php?v=2#x").expect("a name"),
            "adminer.php"
        );
    }

    /// **A name that came out of a document can never be a path**, which is [`Installer::part_file`]'s
    /// own rule read in the other direction: there it is why a `.part` file is named after a hash,
    /// and here it is why nothing whose last segment is not a bare file name may be installed.
    #[test]
    fn a_one_file_artifact_whose_url_names_no_file_is_refused() {
        for url in [
            "https://example.invalid/",
            "https://example.invalid/adminer/",
            "https://example.invalid/..",
            "https://example.invalid/.",
            "https://example.invalid/a\\b",
        ] {
            assert!(one_file_name(url).is_none(), "{url} is not a file name");
        }
    }

    #[test]
    fn the_bar_is_weighted_towards_the_part_that_takes_the_time() {
        assert_eq!(downloaded(0, 1_000), 0);
        assert_eq!(downloaded(500, 1_000), DOWNLOADED_AT / 2);
        assert_eq!(downloaded(1_000, 1_000), DOWNLOADED_AT);
        // An index that declares nothing is not a division by zero.
        assert_eq!(downloaded(0, 0), DOWNLOADED_AT);
        // And a body longer than declared cannot push the bar past its share on its way to being
        // refused.
        assert_eq!(downloaded(4_000, 1_000), DOWNLOADED_AT);
    }

    /// The ordinary case is unchanged: whatever the operating system said, and nothing added.
    #[test]
    fn a_program_that_is_simply_missing_says_only_what_the_os_said() {
        let source = std::io::Error::from(std::io::ErrorKind::NotFound);

        assert_eq!(why_it_would_not_start(&source), source.to_string());
    }

    /// **A refused image load is a machine-wide condition and no amount of re-running fixes it** —
    /// roadmap task **T94** — so the detail carries the reason nobody would guess from `4551`.
    ///
    /// **Both arms in one test, through `cfg!` as a value.** The same number means nothing on macOS
    /// or Linux, and a `#[cfg(windows)]` here would be this crate compiling a line away by
    /// operating system — which `workspace_layering` refuses, and rightly: the classifier's own
    /// `cfg!(windows)` is the thing under test, so asserting it from the caller's side is what
    /// keeps the two from drifting.
    #[test]
    fn a_refused_image_load_is_named_exactly_where_that_number_can_mean_it() {
        let source =
            std::io::Error::from_raw_os_error(mixengine_platform::APPLICATION_CONTROL_BLOCKED);

        let detail = why_it_would_not_start(&source);

        assert_eq!(
            detail.contains("application control policy"),
            cfg!(windows),
            "{detail}"
        );
    }

    #[test]
    fn a_hash_is_lowercase_hex_of_the_file_and_not_of_its_name() {
        let file = tempfile::NamedTempFile::new().expect("a temporary file");
        std::fs::write(file.path(), b"abc").expect("write");

        assert_eq!(
            sha256(file.path()).expect("hash it"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "the published vector for SHA-256 of \"abc\""
        );
    }
}
