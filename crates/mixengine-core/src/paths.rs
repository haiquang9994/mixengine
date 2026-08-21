//! Where everything lives inside `MIXENGINE_HOME`.
//!
//! The layout is identical on all three operating systems — only the root differs, and choosing it
//! is the platform layer's job ([`mixengine_platform::HomeDirs`]). Nothing outside this root is
//! ever written except the handful of system files listed in
//! `.claude/architecture/overview.md`, all of them through `mixengine-elevate` — and the
//! directories the user themselves moved with `[paths]`, which are still MixEngine's to remove.

use std::path::{Path, PathBuf};

use mixengine_platform::Host;
use mixengine_proto::ServiceId;

use crate::config::{FILE_NAME as CONFIG_FILE_NAME, PathOverrides};
use crate::{Error, Result};

/// The SQLite database, directly under the root: the single source of truth.
pub const DATABASE_FILE_NAME: &str = "mixengine.db";

/// The single-instance lock, inside `run/`.
///
/// Held open for as long as the daemon runs; its contents are the holder's pid, and its *existence*
/// means nothing — see [`mixengine_platform::lock`].
pub const LOCK_FILE_NAME: &str = "mixengined.lock";

/// The daemon's own log, inside `logs/`.
///
/// Rotated copies sit next to it as `daemon.log.1` … `daemon.log.5`; the daemon owns that naming
/// because it is the only process that writes the file. Service logs are somewhere else entirely
/// — see [`Paths::service_logs`] — because these are `tracing` output, not a program's stdout.
pub const DAEMON_LOG_FILE_NAME: &str = "daemon.log";

/// Where the per-service log directories live, inside `logs/`.
///
/// A directory of its own rather than files beside `daemon.log`, so that a service id can never
/// collide with the daemon's own file and so that everything one service ever wrote — the live file
/// and its rotated copies — can be removed by removing one directory.
const SERVICES_LOG_DIR_NAME: &str = "services";

/// Decide which directory is `MIXENGINE_HOME`.
///
/// `override_` comes from the environment or the command line and wins outright; without one the
/// platform decides. Either way the result is made absolute — the daemon outlives any particular
/// working directory, and a relative root would quietly follow it around.
///
/// The directory is not created and need not exist yet, so this stops short of `canonicalize`,
/// which would both require existence and hand back a `\\?\` path on Windows. What it does do is
/// [`mixengine_platform::paths::in_full`], which is the same answer spelled the way the
/// filesystem spells it — a home reached through an 8.3 alias is a home nginx refuses every
/// file in.
///
/// # Errors
///
/// [`Error::EmptyHome`] when the override is an empty string, [`Error::Platform`] when there is no
/// override and the OS cannot say where user data belongs, and [`Error::Io`] if the path cannot be
/// made absolute.
pub fn resolve_root(override_: Option<&Path>, host: &dyn Host) -> Result<PathBuf> {
    let root = match override_ {
        // A guard at the library boundary, not the daemon's first line of defence: `clap` refuses
        // an empty `--home` and an empty `MIXENGINE_HOME` before either reaches this function, so
        // `mixengined` never gets here. `resolve_root` is public and `core` cannot assume its
        // caller is a `clap` binary — and the one thing that must never happen is treating an
        // empty override as "not given", which would point a sandbox run at the real install.
        Some(path) if path.as_os_str().is_empty() => return Err(Error::EmptyHome),
        Some(path) => path.to_path_buf(),
        None => host.home_dirs().default_home()?,
    };

    let absolute = std::path::absolute(&root).map_err(|source| Error::Io {
        action: "resolve",
        path: root,
        source,
    })?;

    Ok(mixengine_platform::paths::in_full(&absolute))
}

/// Create `path` and every missing parent.
///
/// # Errors
///
/// [`Error::Io`], with the path in the message, when the directory cannot be created — including
/// the case where something that is not a directory is already sitting there.
pub fn create_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|source| Error::Io {
        action: "create directory",
        path: path.to_path_buf(),
        source,
    })
}

/// Every directory and file MixEngine owns, resolved once at startup.
///
/// Built from the root plus the `[paths]` section of `config.toml`, so the rest of the code asks
/// this type where something goes instead of joining strings and re-deciding what "overridden"
/// means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
    bin: PathBuf,
    runtimes: PathBuf,
    packages: PathBuf,
    data: PathBuf,
    etc: PathBuf,
    certs: PathBuf,
    logs: PathBuf,
    extensions: PathBuf,
    blueprints: PathBuf,
    run: PathBuf,
    cache: PathBuf,
    database_file: PathBuf,
    config_file: PathBuf,
    daemon_log_file: PathBuf,
    lock_file: PathBuf,
}

impl Paths {
    /// Lay out `root`, applying the user's `[paths]` overrides.
    ///
    /// An override may be absolute (`D:\mixengine\runtimes`, a second disk with room for it) or
    /// relative, in which case it is taken relative to the root rather than to the process's
    /// working directory — relative-to-cwd would mean the same config file describing a different
    /// machine depending on where the daemon happened to be started.
    ///
    /// Overrides arrive already validated by [`crate::config`], which is where "relative" is made
    /// to mean what it says. The Windows paths that are neither absolute nor relative to anything
    /// the config file names — `\bulk`, rooted without a drive, and `C:bulk`, a drive without its
    /// root — would both be resolved by `join` against the *current* drive rather than against the
    /// root, so they are refused when the file is read rather than quietly redirected here. So is
    /// an override that resolves back to the root or above it (`""`, `"."`, `".."`).
    #[must_use]
    pub fn new(root: PathBuf, overrides: &PathOverrides) -> Self {
        let under = |name: &str, override_: Option<&PathBuf>| match override_ {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => root.join(path),
            None => root.join(name),
        };

        // The one path built on top of another rather than on the root: moving `logs/` to a second
        // disk has to take `daemon.log` with it, or the override would silently only apply to the
        // service logs.
        let logs = under("logs", overrides.logs.as_ref());

        // The other one, for the same reason in reverse: `run/` cannot be moved by `[paths]`, so the
        // lock is built on it rather than on the root to keep the two from ever disagreeing about
        // which directory the daemon's runtime scratch is.
        let run = under("run", None);

        Self {
            bin: under("bin", None),
            runtimes: under("runtimes", overrides.runtimes.as_ref()),
            packages: under("packages", overrides.packages.as_ref()),
            data: under("data", overrides.data.as_ref()),
            etc: under("etc", None),
            certs: under("certs", None),
            daemon_log_file: logs.join(DAEMON_LOG_FILE_NAME),
            logs,
            extensions: under("extensions", None),
            blueprints: under("blueprints", None),
            cache: under("cache", None),
            lock_file: run.join(LOCK_FILE_NAME),
            run,
            database_file: under(DATABASE_FILE_NAME, None),
            config_file: under(CONFIG_FILE_NAME, None),
            root,
        }
    }

    /// `MIXENGINE_HOME` itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Version-resolving shims: `php`, `node`, `composer` … This is the directory that goes on
    /// `PATH`.
    #[must_use]
    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Installed language runtimes, one directory per `kind/version`.
    #[must_use]
    pub fn runtimes(&self) -> &Path {
        &self.runtimes
    }

    /// Installed servers, databases and caches, one directory per `name/version`.
    #[must_use]
    pub fn packages(&self) -> &Path {
        &self.packages
    }

    /// Per-instance service data — the user's databases. Never regenerated, never deleted.
    #[must_use]
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// Generated configuration. Disposable by design: it is a projection of the database and is
    /// never parsed back into state.
    #[must_use]
    pub fn etc(&self) -> &Path {
        &self.etc
    }

    /// The internal CA and the per-site certificates it issues.
    #[must_use]
    pub fn certs(&self) -> &Path {
        &self.certs
    }

    /// `daemon.log` plus a directory per supervised service.
    #[must_use]
    pub fn logs(&self) -> &Path {
        &self.logs
    }

    /// Installed extensions, one directory per extension id.
    #[must_use]
    pub fn extensions(&self) -> &Path {
        &self.extensions
    }

    /// Captured blueprints, one TOML file each.
    #[must_use]
    pub fn blueprints(&self) -> &Path {
        &self.blueprints
    }

    /// Runtime scratch: pid files, sockets, health markers. Safe to delete while nothing runs.
    #[must_use]
    pub fn run(&self) -> &Path {
        &self.run
    }

    /// Downloaded answers that can always be asked for again: the signed package index and, later,
    /// partial downloads.
    ///
    /// Not `run/`, although both are disposable: `run/` is scratch belonging to *this* daemon and is
    /// safe to empty between runs, while the whole value of a cached index is that it survives a
    /// reboot — an offline machine that lost its cache on restart would be an offline machine that
    /// can list nothing.
    ///
    /// Not private either. Everything in here is a document we publish to the world, and the
    /// signature is what makes it trustworthy rather than the file permissions; the index is
    /// re-verified on every read for exactly that reason.
    ///
    /// Not relocatable by `[paths]`, on the rule that a key arrives with the task that reads it: an
    /// index measured in kilobytes is not why anyone moves a directory to a second disk.
    #[must_use]
    pub fn cache(&self) -> &Path {
        &self.cache
    }

    /// The SQLite database.
    #[must_use]
    pub fn database_file(&self) -> &Path {
        &self.database_file
    }

    /// The user's `config.toml`.
    #[must_use]
    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    /// The daemon's own log, inside [`logs`](Self::logs) and therefore moved by the same override.
    #[must_use]
    pub fn daemon_log_file(&self) -> &Path {
        &self.daemon_log_file
    }

    /// Where one service's output is written: `logs/services/<service-id>/`.
    ///
    /// Built rather than stored, because there is one of these per service and the set is not known
    /// until something starts one. The directory need not exist — the supervisor creates it when it
    /// opens the file, since it is the process that holds the handle.
    ///
    /// A [`ServiceId`] is checked to be a usable directory name when it is parsed (see
    /// `.claude/architecture/process-supervision.md`), which is what makes this a join rather than
    /// an escaping problem.
    #[must_use]
    pub fn service_logs(&self, service: &ServiceId) -> PathBuf {
        self.logs.join(SERVICES_LOG_DIR_NAME).join(service.as_str())
    }

    /// The lock that makes one daemon per home, inside [`run`](Self::run).
    ///
    /// Deliberately not moveable by `[paths]`: it decides which daemon owns this home, and a home
    /// whose lock could be redirected elsewhere would be a home two daemons could both hold.
    #[must_use]
    pub fn lock_file(&self) -> &Path {
        &self.lock_file
    }

    /// The directories no other account on this machine has any business reading.
    ///
    /// `certs/` holds the CA private key and `data/` the user's databases; `run/` holds the socket
    /// and the API token, which are what stands between a local process and the daemon. The root
    /// is here because it is the parent the rest inherit from on Windows, and because a `[paths]`
    /// override can move any of the other three out from under it.
    ///
    /// `bin/`, `etc/`, `logs/`, `runtimes/`, `packages/`, `extensions/` and `blueprints/` are
    /// deliberately absent: they hold downloaded software and generated configuration, and making
    /// them unreadable would break a user reading their own generated nginx config without
    /// protecting anything.
    #[must_use]
    pub fn private_directories(&self) -> [&Path; 4] {
        [&self.root, &self.certs, &self.data, &self.run]
    }

    /// Every directory MixEngine owns, root first.
    #[must_use]
    pub fn directories(&self) -> [&Path; 12] {
        [
            &self.root,
            &self.bin,
            &self.runtimes,
            &self.packages,
            &self.data,
            &self.etc,
            &self.certs,
            &self.logs,
            &self.extensions,
            &self.blueprints,
            &self.run,
            &self.cache,
        ]
    }

    /// Create every directory that does not exist yet, and shut other users out of the private
    /// ones.
    ///
    /// Idempotent: a complete home is walked, found intact, and left alone. Deleting `etc/` and
    /// starting the daemon is therefore a supported repair, not an accident. The permissions are
    /// re-applied on every start rather than only on the ones that create something — a home from
    /// an older version, or one copied off a USB stick, arrives with whatever the last filesystem
    /// thought and would otherwise keep it forever.
    ///
    /// Permissions are set immediately after each directory is created rather than in a second
    /// pass, so the window in which `certs/` exists and is world-readable is as short as the OS
    /// allows. It cannot be closed entirely from here: creating a directory with a mode is a
    /// platform detail, and [`create_dir`] is deliberately not one.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] naming the first directory that could not be created, and [`Error::Platform`]
    /// when one of them cannot be made private — which fails the start rather than continuing with
    /// the CA key readable by every account on the machine.
    pub fn bootstrap(&self, host: &dyn Host) -> Result<()> {
        let private = self.private_directories();
        let access = host.directory_access();

        for directory in self.directories() {
            create_dir(directory)?;

            if private.contains(&directory) {
                access.restrict_to_owner(directory)?;
            }
        }

        Ok(())
    }
}
