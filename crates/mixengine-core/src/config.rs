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
use std::path::{Component, Path, PathBuf};

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
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

/// Refuse a relocation that names nothing: `""`, `"."`, `"./"`.
///
/// `Path::join("")` hands the original path straight back, so `data = ""` would not fail, it would
/// make `data/` *be* `MIXENGINE_HOME`. Everything downstream would then treat the whole install as
/// the service data directory — and "reset the data directory" is a thing MixEngine offers to do.
/// A user who wants the default deletes the key.
fn relocation<'de, D>(deserializer: D) -> std::result::Result<Option<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    let path = Option::<PathBuf>::deserialize(deserializer)?;

    let Some(candidate) = path.as_deref() else {
        return Ok(path);
    };

    // An empty path has no components at all; `.` and `./` have nothing but `CurDir`. Both resolve
    // to the root itself, which is the case this exists to catch.
    if candidate.components().all(|part| part == Component::CurDir) {
        return Err(serde::de::Error::custom(
            "this names the MixEngine home itself rather than a directory inside or outside it; \
             remove the key to use the default",
        ));
    }

    if is_drive_less_root(candidate) {
        return Err(serde::de::Error::custom(
            "this path starts at the root of a drive without saying which one; write it in full \
             (D:\\bulk\\data) or make it relative to the MixEngine home (bulk/data)",
        ));
    }

    Ok(path)
}

/// Refuse an empty path where a name is required.
///
/// Deliberately *not* [`is_drive_less_root`]: a listening address is not a place data is written
/// to, the platform layer has the final say over what shape it may take (T7), and a socket path
/// resolved against the wrong drive fails loudly at bind time instead of quietly storing a
/// database somewhere nobody will look.
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
