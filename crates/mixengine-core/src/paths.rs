//! Where everything lives inside `MIXENGINE_HOME`.
//!
//! The layout is identical on all three operating systems — only the root differs, and choosing it
//! is the platform layer's job ([`mixengine_platform::HomeDirs`]). Nothing outside this root is
//! ever written except the handful of system files listed in
//! `.claude/architecture/overview.md`, all of them through `mixengine-elevate` — and the
//! directories the user themselves moved with `[paths]`, which are still MixEngine's to remove.

use std::path::{Path, PathBuf};

use mixengine_platform::Host;

use crate::config::{FILE_NAME as CONFIG_FILE_NAME, PathOverrides};
use crate::{Error, Result};

/// The SQLite database, directly under the root: the single source of truth.
pub const DATABASE_FILE_NAME: &str = "mixengine.db";

/// Decide which directory is `MIXENGINE_HOME`.
///
/// `override_` comes from the environment or the command line and wins outright; without one the
/// platform decides. Either way the result is made absolute — the daemon outlives any particular
/// working directory, and a relative root would quietly follow it around.
///
/// The directory is not created and need not exist yet, so this stops short of `canonicalize`,
/// which would both require existence and hand back a `\\?\` path on Windows.
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

    std::path::absolute(&root).map_err(|source| Error::Io {
        action: "resolve",
        path: root,
        source,
    })
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
    database_file: PathBuf,
    config_file: PathBuf,
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

        Self {
            bin: under("bin", None),
            runtimes: under("runtimes", overrides.runtimes.as_ref()),
            packages: under("packages", overrides.packages.as_ref()),
            data: under("data", overrides.data.as_ref()),
            etc: under("etc", None),
            certs: under("certs", None),
            logs: under("logs", overrides.logs.as_ref()),
            extensions: under("extensions", None),
            blueprints: under("blueprints", None),
            run: under("run", None),
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

    /// Every directory MixEngine owns, root first.
    #[must_use]
    pub fn directories(&self) -> [&Path; 11] {
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
        ]
    }

    /// Create every directory that does not exist yet.
    ///
    /// Idempotent: a complete home is walked, found intact, and left alone. Deleting `etc/` and
    /// starting the daemon is therefore a supported repair, not an accident.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] naming the first directory that could not be created.
    pub fn bootstrap(&self) -> Result<()> {
        for directory in self.directories() {
            create_dir(directory)?;
        }
        Ok(())
    }
}
