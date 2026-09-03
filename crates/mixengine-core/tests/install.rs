//! The install pipeline against real archives served over a real socket.
//!
//! Everything here downloads from [`mixengine_testkit::MockRegistry`] and unpacks what
//! [`mixengine_testkit::FakePackage`] actually packed — a genuine zip, gzip and zstandard stream, the
//! last of them written by a different implementation than the product reads it with. A double that
//! handed back a directory would answer by construction the three questions this pipeline exists to
//! ask: does the checksum decide anything, does an archive get to choose where its bytes land, and
//! does anything half-finished ever appear where a client will look.
//!
//! The invariant every test here is a restatement of is
//! [runtime-versions.md](../../../.claude/features/runtime-versions.md)'s: **a half-extracted version
//! must never appear in `list`**. So each failing path asserts the absence of the destination, not
//! only the presence of an error.

use std::path::PathBuf;
use std::sync::Mutex;

use mixengine_core::index::{Arch, Artifact, Extensions, Os, Requires};
use mixengine_core::install::{Installed, Installer, SmokeTest, Watcher};
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};

/// The name the fixture's real program is packed under, with whatever suffix this OS needs to be
/// able to spawn it: Windows resolves an extensionless name by appending `.exe`, and a fixture that
/// relied on that would be testing the loader rather than the install.
fn program_name() -> String {
    format!("bin/php{}", std::env::consts::EXE_SUFFIX)
}

/// What a [`Watcher`] saw, and a way to make it ask for a stop part way through.
#[derive(Debug, Default)]
struct Recorder {
    reported: Mutex<Vec<(u8, String)>>,
    /// Report this many times, then answer every `is_cancelled` with `true`.
    cancel_after: Option<usize>,
}

impl Recorder {
    fn cancelling_after(reports: usize) -> Self {
        Self {
            reported: Mutex::new(Vec::new()),
            cancel_after: Some(reports),
        }
    }

    fn percentages(&self) -> Vec<u8> {
        self.reported
            .lock()
            .expect("a test lock")
            .iter()
            .map(|(percent, _)| *percent)
            .collect()
    }
}

impl Watcher for Recorder {
    async fn report(&self, percent: u8, message: &str) {
        self.reported
            .lock()
            .expect("a test lock")
            .push((percent, message.to_owned()));
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_after
            .is_some_and(|after| self.reported.lock().expect("a test lock").len() >= after)
    }
}

/// An index entry for what was packed, for this machine.
fn artifact(url: String, packed: &Packed, provides: &[(&str, &str)]) -> Artifact {
    Artifact {
        os: Os::host().expect("a supported operating system"),
        arch: Arch::host().expect("a supported architecture"),
        url,
        sha256: packed.sha256.clone(),
        size: packed.size(),
        provides: provides
            .iter()
            .map(|(name, path)| ((*name).to_owned(), (*path).to_owned()))
            .collect(),
        requires: Requires::default(),
        extension_dir: None,
        extensions: Extensions::default(),
    }
}

/// A registry, an installer, and a home to install into.
struct Fixture {
    home: tempfile::TempDir,
    registry: MockRegistry,
    installer: Installer,
}

impl Fixture {
    async fn start() -> Self {
        let home = tempfile::tempdir().expect("a temporary home");
        // An index nothing here reads: the install pipeline is handed an `Artifact`, because
        // *choosing* one is `runtime.install`'s job and this is the part underneath it.
        let registry = MockRegistry::start(&serde_json::json!({
            "schema": 1, "generated_at": "2026-08-14T06:55:12Z", "packages": []
        }))
        .await;
        let installer = Installer::new(&home.path().join("cache")).expect("an installer");

        Self {
            home,
            registry,
            installer,
        }
    }

    /// Publish `packed` and answer with the index entry that names it.
    fn publish(&self, packed: &Packed, provides: &[(&str, &str)]) -> Artifact {
        let url = self
            .registry
            .publish_asset(&packed.path(), packed.bytes.clone());
        artifact(url, packed, provides)
    }

    fn target(&self) -> PathBuf {
        self.home.path().join("runtimes/php/8.3.33")
    }

    fn part_files(&self) -> Vec<PathBuf> {
        let downloads = self.home.path().join("cache/downloads");
        let Ok(entries) = std::fs::read_dir(downloads) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect()
    }

    async fn install(
        &self,
        artifact: &Artifact,
        watcher: &Recorder,
    ) -> mixengine_core::Result<Installed> {
        self.installer
            .install(
                artifact,
                &self.target(),
                None,
                mixengine_core::install::NotAnArchive::Refuse,
                watcher,
            )
            .await
    }
}

/// **An artifact that is not an archive is one file** — roadmap task **T82**, the design's D3.
///
/// The whole transaction is the one an archive goes through — the download, the checksum, the
/// staging directory beside the destination and the atomic rename — with the decompressor replaced
/// by a copy. What proves that is the file being *inside* the renamed directory afterwards, under
/// the name the URL ended with.
#[tokio::test]
async fn a_one_file_artifact_is_installed_under_the_name_its_url_ends_with() {
    let fixture = Fixture::start().await;
    let packed = Packed::one_file("adminer-6.0.1.php", b"<?php // the whole distribution\n");
    let artifact = fixture.publish(&packed, &[]);

    fixture
        .installer
        .install(
            &artifact,
            &fixture.target(),
            None,
            mixengine_core::install::NotAnArchive::OneFile,
            &Recorder::default(),
        )
        .await
        .expect("one file installs");

    assert_eq!(
        std::fs::read_to_string(fixture.target().join("adminer-6.0.1.php"))
            .expect("the file is there"),
        "<?php // the whole distribution\n"
    );
}

/// And the package index keeps its refusal, because there a fourth suffix names a decompressor this
/// build does not have rather than a tool that ships one file.
#[tokio::test]
async fn the_package_index_still_refuses_an_artifact_that_is_not_an_archive() {
    let fixture = Fixture::start().await;
    let packed = Packed::one_file("adminer-6.0.1.php", b"<?php\n");
    let artifact = fixture.publish(&packed, &[]);

    let refusal = fixture
        .install(&artifact, &Recorder::default())
        .await
        .expect_err("the index publishes archives");

    assert!(
        matches!(refusal, mixengine_core::Error::ArtifactFormat { .. }),
        "{refusal:?}"
    );
}

/// A package with a couple of ordinary files, small and quick to build.
fn plain(packing: Packing) -> Packed {
    FakePackage::new(packing)
        .file("php.ini-development", b"; a file the archive carries\n")
        .file("ext/php_curl.so", b"not really a shared object\n")
        .build("php-8.3.33-test")
}

#[tokio::test]
async fn an_artifact_is_downloaded_verified_unpacked_and_renamed_into_place() {
    for packing in Packing::ALL {
        let fixture = Fixture::start().await;
        let packed = plain(packing);
        let artifact = fixture.publish(&packed, &[("php-ini", "php.ini-development")]);

        let recorder = Recorder::default();
        let installed = fixture
            .install(&artifact, &recorder)
            .await
            .unwrap_or_else(|error| panic!("{packing:?} should install: {error}"));

        assert_eq!(installed.path, fixture.target());
        assert_eq!(installed.bytes, packed.size());
        assert_eq!(
            std::fs::read_to_string(fixture.target().join("php.ini-development"))
                .expect("the file the archive carried"),
            "; a file the archive carries\n"
        );
        assert!(fixture.target().join("ext/php_curl.so").is_file());

        assert!(
            fixture.part_files().is_empty(),
            "a completed download has nothing left to resume: {:?}",
            fixture.part_files()
        );
        assert_eq!(
            recorder.percentages().last().copied(),
            Some(100),
            "the last thing a watcher hears is that it is done"
        );
    }
}

/// The staging directory is beside the destination, and neither it nor anything in it survives.
#[tokio::test]
async fn nothing_is_left_beside_the_destination() {
    let fixture = Fixture::start().await;
    let packed = plain(Packing::TarGz);
    let artifact = fixture.publish(&packed, &[]);

    fixture
        .install(&artifact, &Recorder::default())
        .await
        .expect("it installs");

    let siblings: Vec<String> = std::fs::read_dir(fixture.target().parent().expect("a parent"))
        .expect("read the runtime directory")
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .collect();

    assert_eq!(siblings, ["8.3.33"], "the staging directory is gone");
}

/// "It arrived eventually" is also true of a client that downloaded the whole thing twice. What
/// makes this a resume is the `Range` header on the second request.
#[tokio::test]
async fn a_transfer_that_stops_early_is_resumed_rather_than_restarted() {
    let fixture = Fixture::start().await;
    // Large enough that a prefix is a meaningful part of it rather than a rounding error.
    let packed = FakePackage::new(Packing::Zip)
        .file(
            "big",
            &(0..600_000_u32).map(|byte| byte as u8).collect::<Vec<_>>(),
        )
        .build("php-8.3.33-test");
    let artifact = fixture.publish(&packed, &[("big", "big")]);

    let cut = (packed.size() / 3) as usize;
    fixture.registry.cut_next_response_after(cut);

    fixture
        .install(&artifact, &Recorder::default())
        .await
        .expect("a dropped connection is resumed, not fatal");

    let ranges = fixture.registry.asset_ranges();
    assert_eq!(ranges.len(), 2, "one truncated attempt and one resume");
    assert_eq!(ranges[0], None, "the first attempt starts at the beginning");
    assert_eq!(
        ranges[1],
        Some(format!("bytes={cut}-")),
        "the second continues from exactly what is on disk"
    );

    assert!(fixture.target().join("big").is_file());
}

/// A checksum that does not match is not a transfer to retry: the file is deleted, because a
/// `.part` that can never verify would otherwise be resumed forever at the same wrong answer.
#[tokio::test]
async fn an_artifact_that_is_not_what_the_index_promised_is_refused_and_deleted() {
    let fixture = Fixture::start().await;
    let packed = plain(Packing::TarZst);
    let mut artifact = fixture.publish(&packed, &[]);
    artifact.sha256 = "0".repeat(64);

    let refusal = fixture
        .install(&artifact, &Recorder::default())
        .await
        .expect_err("a hash that does not match is not this artifact");

    assert!(
        matches!(refusal, mixengine_core::Error::ArtifactChecksum { .. }),
        "{refusal:?}"
    );
    assert!(!fixture.target().exists(), "nothing was installed");
    assert!(
        fixture.part_files().is_empty(),
        "the download that cannot verify is gone: {:?}",
        fixture.part_files()
    );
}

/// The oldest attack against an installer, and one a correct checksum says nothing about: both a
/// signature and a hash establish the archive is the one we published, and neither says what is in
/// it.
#[tokio::test]
async fn an_archive_that_names_a_path_outside_the_install_is_refused() {
    for packing in Packing::ALL {
        let fixture = Fixture::start().await;
        let packed = FakePackage::new(packing)
            .file("php.ini-development", b"harmless\n")
            .raw_entry("../escaped", b"this must never be written\n")
            .build("php-8.3.33-test");
        let artifact = fixture.publish(&packed, &[]);

        let refusal = fixture
            .install(&artifact, &Recorder::default())
            .await
            .expect_err("an entry names somewhere else");

        assert!(
            matches!(refusal, mixengine_core::Error::UnsafeArchiveEntry { .. }),
            "{packing:?}: {refusal:?}"
        );
        assert!(
            !fixture.target().exists(),
            "{packing:?}: nothing was installed"
        );

        let outside = fixture.target().parent().expect("a parent").join("escaped");
        assert!(
            !outside.exists(),
            "{packing:?}: the entry was written outside the install"
        );
    }
}

/// The one failure a checksum cannot see. The archive is ours, the bytes are right, and it still
/// does not run here.
#[tokio::test]
async fn a_binary_that_will_not_run_here_is_never_renamed_into_place() {
    let fixture = Fixture::start().await;
    let packed = FakePackage::new(Packing::TarGz)
        .executable(&program_name())
        .build("php-8.3.33-test");
    let artifact = fixture.publish(&packed, &[("php", &program_name())]);

    let smoke = SmokeTest {
        executable: "php".to_owned(),
        // A flag no program of ours accepts, so this fails the way a runtime that cannot start
        // does: it is spawned, and its exit status is not zero.
        args: vec!["--certainly-not-a-flag".to_owned()],
    };

    let refusal = fixture
        .installer
        .install(
            &artifact,
            &fixture.target(),
            Some(&smoke),
            mixengine_core::install::NotAnArchive::Refuse,
            &Recorder::default(),
        )
        .await
        .expect_err("it does not run");

    assert!(
        matches!(refusal, mixengine_core::Error::SmokeTestFailed { .. }),
        "{refusal:?}"
    );
    assert!(
        !fixture.target().exists(),
        "the check runs while the install is still in staging, so there is nothing to undo"
    );
}

/// And the same archive, asked something it can answer, installs — with the mode bits it needs to
/// have been spawnable at all.
#[tokio::test]
async fn an_artifact_that_runs_here_is_installed_and_is_still_executable_afterwards() {
    let fixture = Fixture::start().await;
    let packed = FakePackage::new(Packing::TarGz)
        .executable(&program_name())
        .build("php-8.3.33-test");
    let artifact = fixture.publish(&packed, &[("php", &program_name())]);

    let smoke = SmokeTest {
        executable: "php".to_owned(),
        args: vec!["--help".to_owned()],
    };

    fixture
        .installer
        .install(
            &artifact,
            &fixture.target(),
            Some(&smoke),
            mixengine_core::install::NotAnArchive::Refuse,
            &Recorder::default(),
        )
        .await
        .expect("it runs, so it installs");

    let program = fixture.target().join(program_name());
    assert!(program.is_file());

    // The check above already proves it was executable in staging; this proves the rename did not
    // lose that — a copy through a temporary directory would.
    let ran = std::process::Command::new(&program)
        .arg("--help")
        .output()
        .expect("the installed binary spawns");
    assert!(ran.status.success());
}

/// Cancelling is not a failure of anything, and it does not throw away work: the partial download
/// is what the next attempt resumes from.
#[tokio::test]
async fn a_cancelled_install_leaves_nothing_installed_and_keeps_what_was_downloaded() {
    let fixture = Fixture::start().await;
    let packed = FakePackage::new(Packing::Zip)
        .file(
            "big",
            &(0..900_000_u32).map(|byte| byte as u8).collect::<Vec<_>>(),
        )
        .build("php-8.3.33-test");
    let artifact = fixture.publish(&packed, &[]);

    // One report is the download's opening one, so the very next look at the token sees the ask.
    let recorder = Recorder::cancelling_after(1);
    let refusal = fixture
        .install(&artifact, &recorder)
        .await
        .expect_err("it was asked to stop");

    assert!(
        matches!(refusal, mixengine_core::Error::InstallCancelled),
        "{refusal:?}"
    );
    assert!(!fixture.target().exists(), "nothing was installed");
    assert_eq!(
        fixture.part_files().len(),
        1,
        "somebody who cancels at sixty percent has not asked for those bytes to be thrown away"
    );
}

/// A runtime directory is immutable once it exists, which is what lets a project pin one and be
/// sure of what it pinned.
#[tokio::test]
async fn installing_over_a_version_that_is_already_there_is_refused() {
    let fixture = Fixture::start().await;
    let packed = plain(Packing::TarGz);
    let artifact = fixture.publish(&packed, &[]);

    std::fs::create_dir_all(fixture.target()).expect("an install that is already there");
    std::fs::write(fixture.target().join("mine"), b"do not touch\n").expect("write");

    let refusal = fixture
        .install(&artifact, &Recorder::default())
        .await
        .expect_err("an install never mutates a version that exists");

    assert!(
        matches!(refusal, mixengine_core::Error::AlreadyInstalled { .. }),
        "{refusal:?}"
    );
    assert!(
        fixture.target().join("mine").is_file(),
        "and it is untouched"
    );
    assert!(
        fixture.registry.asset_ranges().is_empty(),
        "nothing was downloaded to find that out"
    );
}

/// Refused before the download rather than after it, which costs a round trip instead of an
/// artifact.
#[tokio::test]
async fn an_archive_shape_this_build_cannot_unpack_is_refused_before_anything_is_fetched() {
    let fixture = Fixture::start().await;
    let packed = plain(Packing::Zip);
    let mut artifact = fixture.publish(&packed, &[]);
    artifact.url = artifact.url.replace(".zip", ".7z");

    let refusal = fixture
        .install(&artifact, &Recorder::default())
        .await
        .expect_err("nothing here unpacks a 7z");

    assert!(
        matches!(refusal, mixengine_core::Error::ArtifactFormat { .. }),
        "{refusal:?}"
    );
    assert!(
        fixture.registry.asset_ranges().is_empty(),
        "it did not download eighty megabytes to find that out"
    );
}

/// Bounded while it is happening, rather than by the checksum afterwards: the alternative to a
/// bound here is filling a disk to discover the hash does not match.
#[tokio::test]
async fn a_body_longer_than_the_index_declares_is_refused_during_the_transfer() {
    let fixture = Fixture::start().await;
    let packed = plain(Packing::TarGz);
    let mut artifact = fixture.publish(&packed, &[]);
    artifact.size = 16;

    let refusal = fixture
        .install(&artifact, &Recorder::default())
        .await
        .expect_err("the server offered more than the index declares");

    assert!(
        matches!(refusal, mixengine_core::Error::ArtifactTooLarge { .. }),
        "{refusal:?}"
    );
    assert!(!fixture.target().exists());
    assert!(
        fixture.part_files().is_empty(),
        "and what arrived is not a prefix of anything worth keeping"
    );
}

/// A packaging bug found at install time, naming the file, instead of at the moment somebody needed
/// the binary.
#[tokio::test]
async fn an_archive_missing_what_its_index_entry_lists_is_refused() {
    let fixture = Fixture::start().await;
    let packed = plain(Packing::TarZst);
    let artifact = fixture.publish(&packed, &[("php", "bin/php")]);

    let refusal = fixture
        .install(&artifact, &Recorder::default())
        .await
        .expect_err("the archive does not contain it");

    assert!(
        matches!(
            &refusal,
            mixengine_core::Error::MissingFromArtifact { executable, .. } if executable == "php"
        ),
        "{refusal:?}"
    );
    assert!(!fixture.target().exists());
}

/// What a daemon that was killed mid-install leaves behind, and what the next one does about it.
#[tokio::test]
async fn a_staging_directory_left_by_a_daemon_that_stopped_is_cleared_rather_than_added_to() {
    let fixture = Fixture::start().await;
    let packed = plain(Packing::TarGz);
    let artifact = fixture.publish(&packed, &[]);

    let leftover = fixture
        .target()
        .parent()
        .expect("a parent")
        .join(".8.3.33.staging");
    std::fs::create_dir_all(&leftover).expect("what the last daemon left");
    std::fs::write(leftover.join("half-an-archive"), b"junk\n").expect("write");

    fixture
        .install(&artifact, &Recorder::default())
        .await
        .expect("the leftover is cleared, not built on");

    assert!(!leftover.exists());
    assert!(
        !fixture.target().join("half-an-archive").exists(),
        "the wrong half of an old archive must not be renamed into place with the new one"
    );
}

/// The bar is what a person watches, so it has to move forwards and it has to arrive.
#[tokio::test]
async fn progress_only_moves_forwards() {
    let fixture = Fixture::start().await;
    let packed = FakePackage::new(Packing::TarGz)
        .file(
            "big",
            &(0..400_000_u32).map(|byte| byte as u8).collect::<Vec<_>>(),
        )
        .build("php-8.3.33-test");
    let artifact = fixture.publish(&packed, &[]);

    let recorder = Recorder::default();
    fixture
        .install(&artifact, &recorder)
        .await
        .expect("it installs");

    let percentages = recorder.percentages();
    assert!(
        percentages.windows(2).all(|pair| pair[0] <= pair[1]),
        "a bar that goes backwards is worse than no bar: {percentages:?}"
    );
    assert_eq!(percentages.last().copied(), Some(100));
    assert!(
        percentages.len() > 4,
        "the download is most of the wait and should say so: {percentages:?}"
    );
}
