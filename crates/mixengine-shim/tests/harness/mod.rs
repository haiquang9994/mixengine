//! A home the way an install leaves one, and the shim binary sitting in its `bin/`.
//!
//! It lives here rather than in `mixengine-testkit` for the reason the CLI's own harness gives:
//! none of it is about MixEngine, it is about *this* suite — a fake runtime unpacked where
//! `provides` says it is, and `bin/` filled through the product's own [`shims::refresh`] so that
//! the `php` these tests run is the file a daemon start would have put there.
//!
//! **No `MIXENGINE_HOME` is ever set in this process.** Every case sets it on the child's own
//! `Command`, which is what `.claude/standards/testing.md` requires and what lets these run in
//! parallel: `std::env::set_var` is process-global, and two homes in one binary would overwrite
//! each other.

// Each integration test binary compiles this module separately, so anything `shim.rs` uses and
// `overhead.rs` does not is dead code in one of them. The alternative is two fixtures that build a
// home slightly differently, which is the one thing a benchmark and the suite it is measured
// against must not have.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mixengine_core::runtimes::Installation;
use mixengine_core::{Store, paths, runtimes, shims};
use mixengine_proto::{PackageChannel, PackageVersion, RuntimeKind, Timestamp};

/// A fixed moment: nothing here asserts on time, and a fixture that read the clock would be one
/// more thing that can differ between two runs.
const NOW: Timestamp = Timestamp(1_760_000_000_000);

/// Where inside an install directory the fake runtime's program sits.
///
/// Nested rather than at the root, because that is the shape the Unix artifacts have and it is the
/// one that would break a shim which assumed the executable is the directory itself.
pub(crate) fn published_at() -> String {
    format!("bin/php{}", std::env::consts::EXE_SUFFIX)
}

/// A home with runtimes installed in it, and a `bin/` holding the shim under a real command name.
pub(crate) struct Home {
    root: tempfile::TempDir,
}

impl Home {
    /// A home holding one PHP per version named, the first of them the default — which is what
    /// installing them in this order really does.
    pub(crate) fn with(versions: &[&str]) -> Self {
        let root = tempfile::tempdir().expect("a temporary home");
        let home = Self { root };

        let database = home.path().join(paths::DATABASE_FILE_NAME);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for the fixture's own writes");

        runtime.block_on(async {
            let store = Store::open(&database).await.expect("a database");

            for version in versions {
                let directory = home.runtime_directory(version);
                home.unpack_a_fake_runtime(&directory);

                runtimes::remember(
                    &store,
                    &Installation {
                        kind: RuntimeKind::Php,
                        version: PackageVersion::parse(*version).expect("a version"),
                        channel: PackageChannel::Stable,
                        path: directory,
                        bytes: 41_000_000,
                        url: format!("https://example.invalid/php-{version}.tar.zst"),
                        sha256: "00".to_owned(),
                        provides: [("php".to_owned(), published_at())].into_iter().collect(),
                    },
                    NOW,
                )
                .await
                .expect("a row");
            }

            // Closed rather than dropped, so the write-ahead log is checkpointed and the `-shm`
            // file goes: a shim opening the database read-only afterwards is the case that has to
            // work on a machine where no daemon has run since the last reboot.
            store.close().await;
        });

        home.fill_bin();
        home
    }

    pub(crate) fn path(&self) -> &Path {
        self.root.path()
    }

    pub(crate) fn runtime_directory(&self, version: &str) -> PathBuf {
        self.path().join("runtimes").join("php").join(version)
    }

    /// What an install would have left on disk: the program, where `provides` says it is.
    fn unpack_a_fake_runtime(&self, directory: &Path) {
        let program = directory.join(published_at());
        std::fs::create_dir_all(program.parent().expect("a bin directory")).expect("a directory");

        // Copied rather than linked, and one copy per version, because the whole question is which
        // of two identical programs ran — the answer is the path it ran from.
        std::fs::copy(mixengine_testkit::package::executable_source(), &program).unwrap_or_else(
            |error| panic!("copy the fake runtime to {}: {error}", program.display()),
        );
    }

    /// Fill `bin/` the way a daemon start does — **through the product's own function**.
    ///
    /// Roadmap task T26. It used to copy one file under one name, which proved the shim reads
    /// `argv[0]` and proved nothing whatever about how a real `bin/` comes to exist. Going through
    /// [`shims::refresh`] is what makes this suite the end-to-end claim of Phase 2's milestone
    /// rather than half of it: the directory a person's PATH points at is filled by the code that
    /// fills it, and the `php` run below is the file that code put there.
    pub(crate) fn fill_bin(&self) -> shims::Refreshed {
        shims::refresh(
            &self.path().join("bin"),
            Path::new(env!("CARGO_BIN_EXE_mixengine-shim")),
        )
        .expect("bin/ can be filled in a temporary home")
    }

    /// Another language installed beside the PHPs, publishing what its real artifact publishes.
    ///
    /// Not a second fixture but a method on this one, because what it is here to exercise is a home
    /// with **more than one** language in it: `bin/` is one directory holding shims for all of them,
    /// and a `node` row must not change what `php` resolves to.
    pub(crate) fn install(
        &self,
        kind: RuntimeKind,
        version: &str,
        provides: BTreeMap<String, String>,
    ) {
        let directory = self
            .path()
            .join("runtimes")
            .join(kind.as_str())
            .join(version);

        for published in provides.values() {
            let program = directory.join(published);
            std::fs::create_dir_all(program.parent().expect("a directory")).expect("a directory");
            if program.extension().is_some_and(|kind| kind == "cmd") {
                continue; // written by the case that wants one, since its contents are the point
            }
            std::fs::copy(mixengine_testkit::package::executable_source(), &program)
                .unwrap_or_else(|error| panic!("copy to {}: {error}", program.display()));
        }

        let database = self.path().join(paths::DATABASE_FILE_NAME);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for the fixture's own writes");

        runtime.block_on(async {
            let store = Store::open(&database).await.expect("a database");
            runtimes::remember(
                &store,
                &Installation {
                    kind,
                    version: PackageVersion::parse(version).expect("a version"),
                    channel: PackageChannel::Stable,
                    path: directory,
                    bytes: 37_000_000,
                    url: format!("https://example.invalid/{}-{version}.zip", kind.as_str()),
                    sha256: "00".to_owned(),
                    provides,
                },
                NOW,
            )
            .await
            .expect("a row");
            store.close().await;
        });
    }

    /// A project directory under this home's temporary root, with the manifest it pins with.
    pub(crate) fn project(&self, name: &str, manifest: Option<&str>) -> PathBuf {
        let directory = self.path().join("projects").join(name);
        std::fs::create_dir_all(&directory).expect("a project directory");

        if let Some(body) = manifest {
            std::fs::write(directory.join("mixengine.toml"), body).expect("a manifest");
        }

        directory
    }

    /// The shim in `bin/` that answers to `command`, as a path a `Command` can be built from.
    pub(crate) fn shim(&self, command: &str) -> PathBuf {
        self.path()
            .join("bin")
            .join(format!("{command}{}", std::env::consts::EXE_SUFFIX))
    }

    /// Run `bin/php` from `cwd` and have the program it becomes write down what it was handed.
    ///
    /// **`--touch` is load-bearing rather than decoration.** `fakeservice --dump-env` records the
    /// environment and then goes on to *be a service*, which in a test is not a failure but a hang;
    /// the touch file is what makes it a one-shot, and it doubles as the proof that the arguments
    /// reached the program at all. Every case that expects a program to run goes through here so
    /// that none of them can forget it.
    pub(crate) fn record(
        &self,
        cwd: &Path,
        session: &BTreeMap<&str, String>,
        exit_code: i32,
    ) -> Recorded {
        self.record_command("php", cwd, session, exit_code)
    }

    /// The same for a command that is not `php`, which is what a home with a second language in it
    /// needs: the recording is about the shim and not about PHP.
    pub(crate) fn record_command(
        &self,
        command: &str,
        cwd: &Path,
        session: &BTreeMap<&str, String>,
        exit_code: i32,
    ) -> Recorded {
        let dump = cwd.join(format!("environment-{command}.txt"));
        let touched = cwd.join(format!("ran-{command}.txt"));

        let run = self.run_with(
            command,
            cwd,
            &[
                "--dump-env",
                &dump.display().to_string(),
                "--touch",
                &touched.display().to_string(),
                "--exit-code",
                &exit_code.to_string(),
            ],
            session,
        );

        // A run that refused has no dump to read, and reading one would fail on the file rather
        // than on the assertion the case is about.
        let reached = touched.is_file();

        Recorded {
            environment: if reached {
                dumped(&dump)
            } else {
                BTreeMap::new()
            },
            reached,
            run,
        }
    }

    /// The same, with variables the user's session would have exported.
    pub(crate) fn run_with(
        &self,
        command: &str,
        cwd: &Path,
        arguments: &[&str],
        environment: &BTreeMap<&str, String>,
    ) -> Run {
        let shim = self.shim(command);

        let output = Command::new(&shim)
            .args(arguments)
            .current_dir(cwd)
            // On the child, never on this process: the home is an argument here, exactly as the
            // testing standard requires, and it happens to be spelled as a variable because a shim
            // has nowhere else to be told.
            .env("MIXENGINE_HOME", self.path())
            .envs(environment)
            .output()
            .unwrap_or_else(|error| panic!("run {}: {error}", shim.display()));

        Run { output }
    }
}

/// A run whose program was asked to record what it was handed.
pub(crate) struct Recorded {
    /// The run itself: its status and whatever the shim said on the way out.
    pub(crate) run: Run,

    /// The environment the program was given, or empty if it never ran.
    environment: BTreeMap<String, String>,

    /// Whether the program ran at all, which is what its own arguments arriving proves.
    pub(crate) reached: bool,
}

impl Recorded {
    /// Which runtime really ran: the first entry of the `PATH` the program was given.
    pub(crate) fn ran_from(&self) -> PathBuf {
        let path = self
            .environment
            .iter()
            // Windows spells it `Path`, and a child's block keeps whichever spelling it already had.
            .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("no PATH was recorded: {}", self.run.stderr()));

        std::env::split_paths(&path)
            .next()
            .expect("the PATH has an entry")
    }
}

/// One run of a shim, and what can be asked of it afterwards.
pub(crate) struct Run {
    output: Output,
}

impl Run {
    /// The status a shell would see.
    pub(crate) fn code(&self) -> i32 {
        self.output.status.code().expect("the child exited")
    }

    pub(crate) fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }
}

/// The environment the program was handed, as `fakeservice --dump-env` recorded it.
fn dumped(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("the program did not record its environment: {error}"));

    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}
