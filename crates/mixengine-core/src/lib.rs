//! The domain: what a project, site, runtime and service *are*.
//!
//! Storage and platform access arrive as injected traits, so every rule in here — version
//! resolution, config rendering, blueprint diffing — is testable without touching the machine.
//! Modules are organised by capability (`sites/`, `runtimes/`, `certs/`), never by layer.
//!
//! `core` never depends on `daemon`.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};

use mixengine_platform::Host;

pub mod config;
pub mod paths;

pub use config::Config;
pub use paths::Paths;

/// An opened MixEngine home: the user's preferences and the directory layout they produce.
#[derive(Debug, Clone)]
pub struct Home {
    /// What the user asked for in `config.toml`.
    pub config: Config,
    /// Where everything lives, with any `[paths]` override already applied.
    pub paths: Paths,
}

/// Open the MixEngine home directory: resolve it, read its configuration, create what is missing.
///
/// This is the first thing a daemon or CLI process does, and the only place the four steps are
/// ordered — they depend on each other. `config.toml` lives *inside* the root, so the root has to
/// be resolved and created before the configuration that may relocate `runtimes/` or `data/` can
/// be read at all.
///
/// `root_override` is `MIXENGINE_HOME` (or `--home`), already read at `main`; `None` means "use
/// what this OS considers the right place". Running it twice is a no-op — every step is
/// idempotent, which is what makes `mix doctor` able to reuse it.
///
/// # Errors
///
/// [`Error::Platform`] when the OS cannot say where user data belongs and no override was given,
/// [`Error::EmptyHome`] when the override is present but empty, [`Error::Config`] when
/// `config.toml` does not parse, and [`Error::Io`] when a directory cannot be created.
pub fn open_home(root_override: Option<&Path>, host: &dyn Host) -> Result<Home> {
    let root = paths::resolve_root(root_override, host)?;
    paths::create_dir(&root)?;

    let config = config::load_or_create(&root.join(config::FILE_NAME))?;
    let paths = Paths::new(root, &config.paths);
    paths.bootstrap()?;

    Ok(Home { config, paths })
}

/// Failure of a domain operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The requested entity does not exist.
    #[error("no such {kind}: {id}")]
    NotFound {
        /// The kind of entity, e.g. `"site"`.
        kind: &'static str,
        /// The identifier that was looked up.
        id: String,
    },

    /// A file or directory under `MIXENGINE_HOME` could not be touched.
    ///
    /// The path is part of the message because "permission denied" on its own has never helped
    /// anybody: on a `[paths]` override pointing at an unmounted disk, the path *is* the answer.
    // The OS error is the `#[source]`, not part of this message: `anyhow` in the binaries and the
    // wire error at the daemon boundary both walk the chain, and a message that repeats its own
    // cause prints it twice.
    #[error("cannot {action} {}", path.display())]
    Io {
        /// What was being attempted, e.g. `"create"`.
        action: &'static str,
        /// The path it was attempted on.
        path: PathBuf,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// `config.toml` is not valid.
    ///
    /// Unknown keys count: a silently ignored typo is a setting the user believes is in effect.
    #[error("{} is not valid configuration", path.display())]
    Config {
        /// The configuration file that failed to parse.
        path: PathBuf,
        /// The parse failure, which carries the line, the column and the accepted keys.
        #[source]
        source: toml::de::Error,
    },

    /// `MIXENGINE_HOME` (or `--home`) was given, but empty.
    ///
    /// Distinct from "not given": the user meant to point somewhere and the value went missing on
    /// the way, so the platform default would be the one place they did *not* ask for.
    #[error("MIXENGINE_HOME is empty — unset it to use this platform's default location")]
    EmptyHome,

    /// The OS refused to answer a question only it can answer.
    #[error(transparent)]
    Platform(#[from] mixengine_platform::Error),
}

/// Result of a domain operation.
pub type Result<T> = std::result::Result<T, Error>;
