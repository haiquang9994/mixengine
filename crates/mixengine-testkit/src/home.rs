//! A `MIXENGINE_HOME` that exists only for the test that made it.
//!
//! Rule 2 in `.claude/standards/testing.md`: every test gets its own home in a `TempDir`, **passed
//! as an argument** rather than through the environment. `std::env::set_var` is `unsafe` in edition
//! 2024 and process-global regardless, so two tests in one binary would rewrite each other's home.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mixengine_platform::ipc::{Connection, Endpoint};
use tempfile::TempDir;

/// How long a freshly started daemon is given to bind its endpoint.
///
/// Generous, because a first start creates the directory tree, runs the migrations and opens SQLite
/// — and because a loaded CI runner is the machine this has to be reliable on. A ceiling, not a
/// wait: every helper below returns as soon as the thing it is waiting for has happened.
pub const STARTUP: Duration = Duration::from_secs(30);

/// How long a daemon that has been asked to stop is given to disappear.
///
/// Longer than the daemon's own two-second grace period for open connections, so a test failing
/// here is reporting a daemon that did not stop rather than one that was merely tidy about it.
pub const SHUTDOWN: Duration = Duration::from_secs(10);

/// How often the waits below look again.
const POLL: Duration = Duration::from_millis(25);

/// The name of the file the single-instance lock is held on, inside [`Home::run_dir`].
///
/// One of the conventions this crate restates rather than reads — see [`Home::run_dir`] for why, and
/// `mixengine_core::paths::LOCK_FILE_NAME` for the definition it is restating.
const LOCK_FILE_NAME: &str = "mixengined.lock";

/// The directory the daemon's own log lives in, and its name inside it.
///
/// Restated for the same reason as [`LOCK_FILE_NAME`] and with less licence: `run/` cannot be moved
/// by a `[paths]` override and `logs/` can, so this pair is only the daemon's answer for a home with
/// default overrides — which every home this fixture makes is. [`Home::daemon_log_file`] says what
/// keeps the two together.
const LOGS_DIR_NAME: &str = "logs";
const DAEMON_LOG_FILE_NAME: &str = "daemon.log";

/// The database file directly under the root, restated from `mixengine_core::paths`.
const DATABASE_FILE_NAME: &str = "mixengine.db";

/// The settings file, restated from `mixengine_core::config::FILE_NAME`.
const CONFIG_FILE_NAME: &str = "config.toml";

/// The one setting written into every home this fixture makes — roadmap task **T44**.
///
/// **Rule 1 in `.claude/standards/testing.md`: no test touches port 53.** Left to its default the
/// daemon's DNS server binds 53 on Windows and 53535 on macOS and Linux, so without this every
/// suite that starts a real daemon would take a port off the machine running it — and two of them
/// in parallel would race for it, the loser silently falling back to hosts-only mode.
///
/// `0` rather than `enabled = false`, because it costs nothing more and tests strictly more: the
/// daemon still binds, still registers both transports and still reports where it is listening, on
/// a port the operating system hands out and nothing else can be wired to.
const DNS_ON_AN_EPHEMERAL_PORT: &str = "[dns]\nport = 0\n";

/// Make the directory a home lives in, somewhere short — roadmap task **T82a**.
///
/// **`/tmp` on Unix rather than `$TMPDIR`, and this is about `sockaddr_un`.** macOS hands each
/// process a per-user temporary directory like
/// `/var/folders/df/djsxfhc17x95674wsm_g8s980000gn/T/`, which resolves through `/private` to 57
/// characters before `tempfile` has added a name — 68 by the time a home exists. A Unix socket path
/// is capped at 103, and a php-fpm pool's activator listens on
/// `run/php-fpm-<instance>.activate.sock`; a `web-app` extension's pool is named after the extension
/// rather than after a version, so `php-fpm-phpmyadmin.activate.sock` is 32 of the remaining 35 and
/// the whole path came to 104. Four tests failed on macOS and on no other system, saying so exactly.
///
/// **What that limit is not is a limit the product has.** A real macOS home is `~/.mixengine`, which
/// leaves fifty characters of room; the refusal those tests met is the right refusal, arriving
/// against this fixture's own path. So the fixture moves, and `/tmp` is where
/// `generate::recipes::space_free_view` already reaches for the neighbouring reason, stated the same
/// way: it is POSIX, it exists on every Unix this runs on, and its name is short.
///
/// Windows keeps the platform's own temporary directory, where there is no such cap and no `/tmp`.
fn temporary_directory() -> TempDir {
    let mut builder = tempfile::Builder::new();

    // Named rather than `.tmpXXXXXX`, because a directory this leaks is one somebody has to
    // recognise — and in a shared `/tmp` rather than a per-user one, that matters more.
    builder.prefix("mixengine-");

    #[cfg(unix)]
    let made = builder.tempdir_in("/tmp");

    #[cfg(not(unix))]
    let made = builder.tempdir();

    made.expect("a temporary home")
}

/// A home directory that exists only for this test, and the endpoint a daemon serving it will bind.
///
/// Removed when it drops, along with whatever the daemon put in it. What this type does *not* do is
/// stop anything: a daemon a client autostarted is nobody's child, and only the test that arranged
/// that knows how to find it. See `crates/mixengine-cli/tests/status.rs`, which does exactly that on
/// top of this.
#[derive(Debug)]
pub struct Home {
    /// The directory itself, kept for the one job it has left: removing the home when this drops.
    #[expect(
        dead_code,
        reason = "every path is taken from `root` below, which is the same directory under the                   name the daemon resolves it to"
    )]
    dir: TempDir,

    /// The same directory, spelled the way `mixengine_core::paths::resolve_root` will spell it.
    ///
    /// **A fixture that skipped this would be one the daemon disagrees with.** Windows keeps an 8.3
    /// alias for most names and hands one out in the temporary directory a runner's account gets, so
    /// a home arrives here under a name the daemon resolves to another the moment it is given it.
    /// Every path this type restates is derived from the resolved spelling, because that is where
    /// the daemon really puts things and what it really names in an answer.
    root: PathBuf,

    endpoint: Endpoint,
}

impl Home {
    /// A new empty home, with nothing created inside it but the one setting no test may leave to a
    /// default.
    ///
    /// # Panics
    ///
    /// If the system has no temporary directory to make one in, or if the resulting path cannot
    /// name an endpoint — on Unix that means a `TMPDIR` deep enough to overflow `sun_path`, which
    /// is worth failing loudly rather than skipping.
    #[must_use]
    pub fn new() -> Self {
        Self::configured("")
    }

    /// The same, with `extra` appended to `config.toml`.
    ///
    /// **The seed [`Home::SEEDED`] anticipated** — roadmap task **T52**. Every home this fixture
    /// makes carries `[dns] port = 0` and nothing could add to it, so a suite that needed a second
    /// setting had no way to ask for one. That absence is what would make a period no test can move
    /// look like the only option available, which is the mistake T51 nearly shipped with the nginx
    /// TLS port.
    ///
    /// # Panics
    ///
    /// For [`Home::new`]'s reasons.
    #[must_use]
    pub fn configured(extra: &str) -> Self {
        let dir = temporary_directory();
        let root = mixengine_platform::paths::in_full(dir.path());
        let endpoint = Endpoint::in_run_dir(&root.join("run")).expect("an endpoint for this home");

        std::fs::write(
            root.join(CONFIG_FILE_NAME),
            format!("{DNS_ON_AN_EPHEMERAL_PORT}{extra}"),
        )
        .expect("a temporary home is writable");

        Self {
            dir,
            root,
            endpoint,
        }
    }

    /// Everything in this home that this fixture put there, and nothing a daemon or a client added.
    ///
    /// The list a test asserts against when its claim is "that command created nothing": since T44
    /// a fresh home is not empty, and a test comparing against `0` would be asserting that the DNS
    /// port fix is absent. Named here so a future seed is one line rather than a hunt through every
    /// suite for a number that quietly stopped meaning "empty".
    pub const SEEDED: [&str; 1] = [CONFIG_FILE_NAME];

    /// What is in the home right now, by file name, sorted — for comparing against [`Home::SEEDED`].
    ///
    /// # Panics
    ///
    /// If the home cannot be read, which is a broken fixture rather than a failing assertion.
    #[must_use]
    pub fn contents(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.root)
            .expect("the temporary home is readable")
            .map(|entry| {
                entry
                    .expect("a readable entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    /// The root, which is what a binary is given as `--home`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// `run/`, where the endpoint and the lock live.
    ///
    /// Restated here rather than taken from `mixengine_core::Paths`, and the reason is the one
    /// [`crate`] gives rather than the one `crates/mixengine-cli/src/home.rs` gives: a fixture that
    /// computed this the way the daemon computes it would make a suite agree with itself by
    /// construction. (The other argument — that `core` carries `sqlx` and a test binary has no
    /// business bundling SQLite to find a socket — stopped applying the day [`declare::package`](crate::declare::package)
    /// needed to write a row. What it buys now is one dependency rather than the whole of `core`.)
    ///
    /// It is safe to restate for a reason rather than by luck. `Paths::new` passes `None` for `run`
    /// deliberately, so a `[paths]` override cannot move the one directory the lock and the
    /// endpoint must agree on. The tests that drive a real daemon are what keep the two answers
    /// together — nothing else would notice them drifting apart, so
    /// `the_fixture_and_the_daemon_agree_on_the_paths_it_restates` in
    /// `crates/mixengine-daemon/tests/lifecycle.rs` holds every one of them against `Paths` at once.
    #[must_use]
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    /// The endpoint a daemon serving this home binds.
    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The file the single-instance lock is held on.
    ///
    /// For a test that wants to hold it *instead* of a daemon — which is the only way to observe
    /// what happens between a daemon taking the lock and binding its endpoint.
    #[must_use]
    pub fn lock_file(&self) -> PathBuf {
        self.run_dir().join(LOCK_FILE_NAME)
    }

    /// The pid the daemon holding this home recorded in its lock file, if one has.
    ///
    /// `None` while no daemon has taken the home, and `None` again for a lock file that is there but
    /// empty — which is the state between a daemon taking the lock and writing to it.
    #[must_use]
    pub fn locked_by(&self) -> Option<u32> {
        std::fs::read_to_string(self.lock_file())
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// The daemon's own log, whether or not one has been written yet.
    ///
    /// The third path this crate restates, and the one most worth checking: unlike [`run_dir`] it is
    /// not protected by `Paths::new` refusing to move it, so nothing but
    /// `the_fixture_and_the_daemon_agree_on_the_paths_it_restates` stands between a `logs/` that
    /// moved and every wait below timing out against a file the daemon stopped writing.
    ///
    /// [`run_dir`]: Self::run_dir
    #[must_use]
    pub fn daemon_log_file(&self) -> PathBuf {
        self.root.join(LOGS_DIR_NAME).join(DAEMON_LOG_FILE_NAME)
    }

    /// The SQLite file a daemon serving this home opens.
    ///
    /// The fourth path this crate restates, and it exists for [`mod@crate::declare`]: a test that has to
    /// put a `services` row somewhere until T30 can create one needs to know where. Held against
    /// `Paths::database_file` by the same test as the other three.
    #[must_use]
    pub fn database_file(&self) -> PathBuf {
        self.root.join(DATABASE_FILE_NAME)
    }

    /// Declare `services` in this home, through the daemon serving it.
    ///
    /// A daemon has to be **running**, not merely to have run: `service.create` is what writes the
    /// row now, which is the whole point — a fixture and a user reach the same code. See
    /// [`mod@crate::create`], and [`Service`](crate::Service) for how one says what it is to do.
    ///
    /// # Panics
    ///
    /// As [`crate::create()`].
    pub fn declare(&self, services: &[crate::Service]) {
        crate::create_blocking(self.endpoint(), &self.database_file(), services);
    }

    /// Whatever a daemon wrote to [`daemon_log_file`](Self::daemon_log_file), for a failure message.
    ///
    /// Never an error: this is called from the panic path of a test that is already failing, and a
    /// missing log file is itself the most useful thing it could say there.
    #[must_use]
    pub fn daemon_log(&self) -> String {
        std::fs::read_to_string(self.daemon_log_file())
            .unwrap_or_else(|error| format!("(unreadable: {error})"))
    }

    /// Wait until something is answering on the endpoint.
    ///
    /// # Panics
    ///
    /// After [`STARTUP`], with the daemon's own log attached — which is where the reason will be.
    pub async fn wait_until_listening(&self) {
        self.wait_until(STARTUP, "a daemon started listening", || async {
            Connection::connect(&self.endpoint).await.is_ok()
        })
        .await;
    }

    /// Wait until nothing is answering on the endpoint any more.
    ///
    /// # Panics
    ///
    /// After [`SHUTDOWN`], with the daemon's own log attached.
    pub async fn wait_until_gone(&self) {
        self.wait_until(SHUTDOWN, "the endpoint went quiet", || async {
            Connection::connect(&self.endpoint).await.is_err()
        })
        .await;
    }

    /// Wait until a daemon has written `wanted` to its log.
    ///
    /// The one signal available for a daemon that deliberately never binds anything: the endpoint
    /// says nothing about it, and the lock file names whoever *holds* the lock rather than whoever
    /// just gave up on it.
    ///
    /// # Panics
    ///
    /// After [`STARTUP`], with the log it was reading attached.
    pub async fn wait_until_daemon_log_says(&self, wanted: &str) {
        self.wait_until(STARTUP, &format!("a daemon said {wanted:?}"), || async {
            self.daemon_log().contains(wanted)
        })
        .await;
    }

    /// Poll `happened` until it does, or panic saying what did not.
    async fn wait_until<F, Fut>(&self, within: Duration, what: &str, mut happened: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        let deadline = tokio::time::Instant::now() + within;

        while tokio::time::Instant::now() < deadline {
            if happened().await {
                return;
            }

            tokio::time::sleep(POLL).await;
        }

        panic!(
            "waited {within:?} and {what} never happened, on {}\n--- daemon.log ---\n{}",
            self.endpoint,
            self.daemon_log()
        );
    }
}

impl Default for Home {
    fn default() -> Self {
        Self::new()
    }
}
