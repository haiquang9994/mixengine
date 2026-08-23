//! `config.toml` — the user's preferences, read once at boot.
//!
//! This file is *not* state. Everything MixEngine decides for itself lives in `mixengine.db`;
//! everything the machine can regenerate lives under `etc/`. What is left — how loudly to log,
//! where the daemon listens, which directories were moved to a bigger disk — is what a user edits
//! by hand, so it stays a small, commented TOML file rather than a hidden database row.
//!
//! Keys arrive with the task that reads them. Declaring a section before anything honours it would
//! be a promise the build does not keep.

use std::io::Write as _;
use std::path::{Component, Path, PathBuf, Prefix};

use serde::{Deserialize, Deserializer};

use crate::{Error, Result};

/// The configuration file's name, directly under `MIXENGINE_HOME`.
pub const FILE_NAME: &str = "config.toml";

/// The commented starting point written on first run.
///
/// Public so `mix doctor` can restore a deleted file and so tests can prove it stays in step with
/// the types below.
pub const TEMPLATE: &str = include_str!("config/template.toml");

/// Everything a user may set.
///
/// `deny_unknown_fields` is deliberate: an unrecognised key fails the load instead of being
/// ignored. A misspelled key that does nothing looks exactly like a key that does not work, and
/// the user has no way to tell which they are looking at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Log verbosity and format.
    pub log: Logging,
    /// Daemon-level settings.
    pub daemon: Daemon,
    /// The built-in DNS server.
    pub dns: Dns,
    /// Overrides for the directories that grow.
    pub paths: PathOverrides,
}

/// How the daemon writes its log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Logging {
    /// How much to log.
    pub level: LogLevel,
    /// How to shape each line.
    pub format: LogFormat,
}

/// Verbosity of the daemon log.
///
/// A closed set: a free-form string would let a typo silence logging entirely, and the process
/// would look perfectly healthy while saying nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// User-visible failures only.
    Error,
    /// Degraded but continuing.
    Warn,
    /// Lifecycle events a user might care about.
    #[default]
    Info,
    /// Developer detail.
    Debug,
    /// Firehose.
    Trace,
}

/// Shape of each log line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable, for a terminal.
    #[default]
    Text,
    /// One JSON object per line, for a collector.
    Json,
}

/// Daemon-level settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Daemon {
    /// Where the daemon listens for clients: a Unix socket path or a Windows named pipe.
    ///
    /// `None` means "wherever the platform layer puts it", which is the answer for almost
    /// everyone. It exists because the default lands under `run/`, and a `MIXENGINE_HOME` on a
    /// filesystem that cannot host a socket (a network share, a path over the 108-byte `sun_path`
    /// limit) needs a way out that is not "move everything".
    #[serde(default, deserialize_with = "named_path")]
    pub ipc_path: Option<PathBuf>,

    /// How long the whole of a `daemon.shutdown` may spend stopping services, in seconds.
    ///
    /// **The budget is on the total, not on each service**, which is the distinction that makes it
    /// worth a key at all: a [`ServiceSpec`](mixengine_proto::ServiceSpec) already says how long
    /// *its* service needs in order to shut down cleanly, and eight services each allowed ten
    /// seconds is eighty seconds a user is waiting for a daemon they asked to stop. Each service
    /// gets what its spec asks for or what is left of this, whichever is less, so the sum is what a
    /// person can plan around and the individual grace periods stay statements about the services.
    ///
    /// Zero is a real answer and means every service is killed at once — recovery on the next start,
    /// deliberately chosen. A generous value only costs anything on the shutdown where something
    /// genuinely will not go, and stays generous up to ten minutes. Past that the file is refused
    /// rather than corrected: the number leaves here as a `Duration` added to an `Instant`, which
    /// panics on overflow, so an unbounded budget crashes the shutdown it was meant to bound.
    ///
    /// **Not what bounds a shutdown the operating system asked for.** A console control event on
    /// Windows runs on a clock the daemon cannot extend
    /// ([`STOP_CEILING`](mixengine_platform::signal::STOP_CEILING)), so that path takes the smaller
    /// of the two; on Unix and over the API this value is the whole of it.
    #[serde(deserialize_with = "shutdown_grace")]
    pub shutdown_grace_seconds: u64,
}

/// The default for [`Daemon::shutdown_grace_seconds`], and the reason [`Daemon`] writes its own
/// [`Default`] rather than deriving one: a derived default here would be zero, which is a real
/// setting meaning "kill everything at once" and is not what an absent key asks for.
const DEFAULT_SHUTDOWN_GRACE_SECONDS: u64 = 10;

/// The largest [`Daemon::shutdown_grace_seconds`] MixEngine accepts: ten minutes.
///
/// A ceiling is needed at all because the number does not stay a number. The daemon turns it into a
/// [`Duration`](std::time::Duration) and adds it to an [`Instant`](std::time::Instant) to work out
/// when the budget runs out, and `impl Add<Duration> for Instant` *panics* on overflow rather than
/// saturating. A budget of `u64::MAX` seconds therefore does not produce a shutdown that waits
/// forever, it produces one that panics immediately — on the task running the shutdown, unwinding
/// past the write-ahead log checkpoint and leaving every service's row still saying `stopping`.
/// That is the exact outcome the budget exists to prevent, reached by asking for more of it.
///
/// Ten minutes because it has to sit above every honest answer and nowhere near the arithmetic. The
/// slowest polite stop MixEngine supervises is a database writing its buffers out — PostgreSQL's
/// own `shutdown_timeout` defaults to sixty seconds, and MariaDB with a large InnoDB pool and
/// `innodb_fast_shutdown = 0` can spend a few minutes on a slow disk — so this clears the real
/// cases several times over while still being a wait a person can be told to expect. Past it the
/// number has stopped being a preference: it is a typo, or it is somebody spelling "never", and
/// "never" is not a budget.
const MAX_SHUTDOWN_GRACE_SECONDS: u64 = 600;

impl Default for Daemon {
    fn default() -> Self {
        Self {
            ipc_path: None,
            shutdown_grace_seconds: DEFAULT_SHUTDOWN_GRACE_SECONDS,
        }
    }
}

/// The built-in DNS server — roadmap task **T44**.
///
/// It answers `A` for every name under a TLD MixEngine manages and refuses everything else, which
/// is what lets a site be created without an elevation prompt: a wildcard needs no record per
/// domain, where a hosts file needs a line per domain and a prompt per line.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Dns {
    /// Whether to run it at all.
    ///
    /// Turning it off is choosing the hosts file explicitly, and it is reported as such rather than
    /// silently: `daemon.status` says which mechanism this home is on and why.
    pub enabled: bool,

    /// The loopback port it listens on, or [`None`] for this system's default.
    ///
    /// The default is **53 on Windows and 53535 elsewhere**, and the split is about what a resolver
    /// rule can express rather than about privilege: Windows' NRPT rule names a nameserver with no
    /// way to state a port, while `/etc/resolver` and `resolvectl` both can. It is deliberately not
    /// 5353, which belongs to mDNS on both of the systems where the number is free.
    ///
    /// A key at all because a machine where something already holds the default needs a way out
    /// that is not "move your home directory" — and because a test cannot legitimately bind the
    /// real one (`.claude/standards/testing.md`).
    ///
    /// **`0` asks the operating system to pick one**, which is what every suite that starts a real
    /// daemon does (`mixengine_testkit::Home`). It is a real setting rather than a special case,
    /// and it is useless to anybody else: a port that changes on every start is a port no resolver
    /// can be wired to.
    pub port: Option<u16>,
}

/// [`Dns`] writes its own [`Default`] for [`Daemon`]'s reason: a derived one would leave the server
/// switched off, and an absent section asks for the ordinary behaviour rather than for none.
impl Default for Dns {
    fn default() -> Self {
        Self {
            enabled: true,
            port: None,
        }
    }
}

/// Relocations for the directories that grow without bound.
///
/// Only these four: runtimes and packages are re-downloadable, `data/` and `logs/` are the two that
/// get large. `bin/`, `etc/`, `certs/`, `extensions/`, `blueprints/`, `run/` and `mixengine.db`
/// stay together next to `config.toml`, because a home directory that can be split into eleven
/// pieces is a home directory no uninstaller can promise to remove.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PathOverrides {
    /// Where installed language runtimes go instead of `<root>/runtimes`.
    #[serde(default, deserialize_with = "relocation")]
    pub runtimes: Option<PathBuf>,
    /// Where installed servers and databases go instead of `<root>/packages`.
    #[serde(default, deserialize_with = "relocation")]
    pub packages: Option<PathBuf>,
    /// Where service data goes instead of `<root>/data`.
    #[serde(default, deserialize_with = "relocation")]
    pub data: Option<PathBuf>,
    /// Where logs go instead of `<root>/logs`.
    #[serde(default, deserialize_with = "relocation")]
    pub logs: Option<PathBuf>,
}

/// Refuse a relocation that does not name a directory of its own, or that names one ambiguously.
///
/// `Path::join("")` hands the original path straight back, so `data = ""` would not fail, it would
/// make `data/` *be* `MIXENGINE_HOME`. Everything downstream would then treat the whole install as
/// the service data directory — and "reset the data directory" is a thing MixEngine offers to do.
/// `"."`, `".."`, `"bulk/.."` and `"/"` all end up somewhere just as destructive: the home itself,
/// a directory containing it, or an entire filesystem. A user who wants the default deletes the
/// key.
fn relocation<'de, D>(deserializer: D) -> std::result::Result<Option<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    let path = Option::<PathBuf>::deserialize(deserializer)?;

    let Some(candidate) = path.as_deref() else {
        return Ok(path);
    };

    // "Is it anchored?" before "does it name anything?": both questions are answered by refusing
    // the path, but a half-anchored Windows path has its own diagnosis and deserves to hear it
    // rather than the generic one.
    if is_drive_less_root(candidate) {
        return Err(serde::de::Error::custom(
            "this path starts at the root of a drive without saying which one; write it in full \
             (D:\\bulk\\data) or make it relative to the MixEngine home (bulk/data)",
        ));
    }

    if is_drive_relative(candidate) {
        return Err(serde::de::Error::custom(
            "this path names a drive but does not start at its root, so it would land wherever \
             that drive's current directory happens to be; write it in full (D:\\bulk\\data) or \
             make it relative to the MixEngine home (bulk/data)",
        ));
    }

    if !names_a_directory(candidate) {
        return Err(serde::de::Error::custom(
            "after resolving `.` and `..` this names no directory of its own — it points at the \
             MixEngine home, at a directory containing it, or at the root of a filesystem; remove \
             the key to use the default",
        ));
    }

    Ok(path)
}

/// Refuse an empty path where a name is required.
///
/// Deliberately *not* the checks [`relocation`] runs: a listening address is not a place data is
/// written to, it is never joined to the home, the platform layer has the final say over what shape
/// it may take (T7), and a socket path resolved against the wrong drive fails loudly at bind time
/// instead of quietly storing a database somewhere nobody will look.
fn named_path<'de, D>(deserializer: D) -> std::result::Result<Option<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    let path = Option::<PathBuf>::deserialize(deserializer)?;

    if path
        .as_deref()
        .is_some_and(|path| path.as_os_str().is_empty())
    {
        return Err(serde::de::Error::custom(
            "the path is empty; remove the key to use the default",
        ));
    }

    Ok(path)
}

/// Does anything survive resolving `.` and `..` — is there a directory here at all?
///
/// Counts how deep the path ends up, purely lexically: `bulk/runtimes` is two down and fine,
/// `bulk/..` is back where it started, `..` is above it, `""`, `"."` and `"/"` never went anywhere.
/// A path that ends at depth zero names the MixEngine home, one of its ancestors, or a filesystem
/// root — and "reset this directory" would then take far more than the directory with it.
///
/// A `..` that climbs and then descends again is left alone: `../bulk` is a sibling of the home,
/// which is an ordinary thing to want and contains nothing dangerous.
fn names_a_directory(path: &Path) -> bool {
    let mut depth = 0usize;

    for part in path.components() {
        match part {
            Component::Normal(_) => depth += 1,
            // Climbing above where we started is a no-op here: the components that follow are what
            // decide whether the path names something, and `../bulk` names `bulk`.
            Component::ParentDir => depth = depth.saturating_sub(1),
            // A share *is* a directory — `\\server\share` is somewhere data can go, and Windows
            // folds the whole of it into the prefix, leaving nothing behind for the count. A drive
            // letter is not: `C:\` is a filesystem, and relocating `data/` onto one whole is the
            // case above.
            Component::Prefix(prefix) => {
                if matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..)) {
                    depth += 1;
                }
            }
            Component::CurDir | Component::RootDir => {}
        }
    }

    depth > 0
}

/// `\bulk` or `/bulk` on Windows: rooted, but with no drive to root it to.
///
/// `Path::join` resolves these against the *current* drive rather than against the path it is
/// joined to, so `C:\home\MixEngine`.join("/bulk") is `C:\bulk`. Treating that as "relative to the
/// home" — which is what the surrounding code promises — would be a lie, and treating it as
/// absolute would silently pick a drive the user never named. Refusing is the only honest answer.
///
/// On Unix `has_root()` and `is_absolute()` are the same question, so this is always `false` and
/// the check costs nothing.
fn is_drive_less_root(path: &Path) -> bool {
    path.has_root() && !path.is_absolute()
}

/// `C:bulk` on Windows: a drive named, but not the root of it.
///
/// The mirror image of [`is_drive_less_root`] and just as dishonest. A path carrying a prefix
/// replaces the whole of whatever it is joined to — `C:\home\MixEngine`.join("C:bulk") is plain
/// `C:bulk` — and the OS then resolves it against the *current directory of drive C*, which no
/// part of the configuration file mentions and which changes under the daemon's feet.
///
/// On Unix there are no prefixes, so `C:bulk` is an ordinary directory name and this is `false`.
fn is_drive_relative(path: &Path) -> bool {
    matches!(path.components().next(), Some(Component::Prefix(_))) && !path.is_absolute()
}

/// Refuse a shutdown budget longer than [`MAX_SHUTDOWN_GRACE_SECONDS`].
///
/// Refused rather than quietly lowered, which is the answer this file gives to every other value
/// it will not take: an empty `ipc_path` and a relocation that names nothing are both errors
/// carrying a sentence about what to write instead, and nothing here rewrites a setting behind the
/// user's back. Two things make that the right answer for this key in particular rather than merely
/// the consistent one. A budget is a *promise about how long a stop takes*, so silently turning an
/// hour into ten minutes would be exactly the surprise the key was set to avoid, and it would
/// arrive during a shutdown rather than at the moment the file was edited. And the only place a
/// correction could be announced is the log, which does not exist yet: `config.toml` is what the
/// daemon reads in order to decide how to log at all, so a `warn!` from here is emitted before any
/// subscriber is installed and is seen by nobody.
///
/// Zero needs no floor to go with this. It is a real setting — see
/// [`Daemon::shutdown_grace_seconds`] — and the arithmetic downstream is happy with it.
fn shutdown_grace<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let seconds = u64::deserialize(deserializer)?;

    if seconds > MAX_SHUTDOWN_GRACE_SECONDS {
        return Err(serde::de::Error::custom(format!(
            "a shutdown budget of {seconds} seconds is longer than the \
             {MAX_SHUTDOWN_GRACE_SECONDS} seconds MixEngine accepts; nothing it supervises takes \
             ten minutes to stop, and a budget that never runs out is not one — lower it, or \
             remove the key for the default of {DEFAULT_SHUTDOWN_GRACE_SECONDS}"
        )));
    }

    Ok(seconds)
}

/// Read `config.toml`.
///
/// A missing file is not an error — it means "all defaults", which is exactly what a user who
/// deleted the file is asking for.
///
/// # Errors
///
/// [`Error::Config`] when the file does not parse or contains a key MixEngine does not know;
/// [`Error::Io`] when it exists but cannot be read.
pub fn load(path: &Path) -> Result<Config> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Config::default());
        }
        Err(source) => {
            return Err(Error::Io {
                action: "read",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    toml::from_str(&text).map_err(|source| Error::Config {
        path: path.to_path_buf(),
        source,
    })
}

/// Read `config.toml`, writing the commented template first if there is nothing there.
///
/// # Errors
///
/// As [`load`], plus [`Error::Io`] when the template cannot be written.
pub fn load_or_create(path: &Path) -> Result<Config> {
    write_template(path)?;
    load(path)
}

/// Write [`TEMPLATE`] to `path` unless a file is already there. Returns whether it wrote one.
///
/// An existing file is never touched, not even to add a key introduced by an update: the file
/// belongs to the user. New keys are documented, and their absence means the default, so an old
/// `config.toml` keeps working unchanged.
///
/// # Errors
///
/// [`Error::Io`] when the file cannot be created or written.
pub fn write_template(path: &Path) -> Result<bool> {
    // `create_new` rather than "check, then write": between the check and the write sits every
    // other MixEngine process on this machine, and the loser of that race would truncate a config
    // file the winner had just written.
    let mut file = match std::fs::File::create_new(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(source) => {
            return Err(Error::Io {
                action: "create",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if let Err(source) = file.write_all(TEMPLATE.as_bytes()) {
        // A half-written template is worse than none: it parses, so it looks like a choice the
        // user made, and `create_new` would never replace it. Take it back out of the way and let
        // the next start try again. If even that fails, the write error is still what gets
        // reported — it is the one the user needs.
        drop(file);
        let _ = std::fs::remove_file(path);

        return Err(Error::Io {
            action: "write",
            path: path.to_path_buf(),
            source,
        });
    }

    Ok(true)
}
