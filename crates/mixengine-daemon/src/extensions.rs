//! `extension.inspect` — roadmap task **T80**.
//!
//! **Not [`crate::php_extensions`]**, which turns a *PHP* extension on for one installed runtime.
//! These are MixEngine's own: Mailpit, phpMyAdmin, MixDB.
//!
//! A façade and nothing more — everything it answers is computed by [`mixengine_core::extensions`],
//! because reading a manifest is business logic and `CLAUDE.md` puts none of that in a client. What
//! belongs *here* is the reason the daemon has to be the one doing it: the render context needs
//! `<root>`, and only the daemon knows where that is.
//!
//! One method, and it is read-only. T80 installs nothing; `extension.install` and the rest of the
//! lifecycle arrive with T81.

use std::path::{Path, PathBuf};

use mixengine_core::Paths;
use mixengine_proto::{Error, ErrorCode, ExtensionInspect, ExtensionInspection};

use crate::error::ToWire as _;

/// Everything `extension.*` needs today: where this home keeps its extensions.
#[derive(Debug)]
pub(crate) struct Extensions {
    /// The home, for the directory an extension would be installed into.
    paths: Paths,
}

impl Extensions {
    /// Build it.
    pub(crate) fn new(paths: Paths) -> Self {
        Self { paths }
    }

    /// Read a manifest and say what installing it here would produce.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::InvalidArgument`] for a path that is not absolute, and whatever
    /// [`mixengine_core::extensions::inspect`] raises about the file itself.
    pub(crate) fn inspect(&self, asked: &ExtensionInspect) -> Result<ExtensionInspection, Error> {
        let path = absolute(&asked.path)?;

        mixengine_core::extensions::inspect(&self.paths, &path).map_err(|error| error.to_wire())
    }
}

/// A path the daemon can act on: absolute, because this daemon has no idea what the client's
/// current directory is and a relative path here would be resolved against the wrong one — which
/// reads the wrong file rather than failing.
fn absolute(given: &str) -> Result<PathBuf, Error> {
    let path = Path::new(given);

    match path.is_absolute() {
        true => Ok(path.to_path_buf()),
        false => Err(Error::new(
            ErrorCode::InvalidArgument,
            format!("{given} is not an absolute path"),
        )
        .with_hint("the client resolves a path against its own directory before sending it")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one thing this type decides for itself.
    #[test]
    fn a_relative_path_is_refused() {
        let home = tempfile::tempdir().expect("a directory");
        let extensions =
            Extensions::new(Paths::new(home.path().to_path_buf(), &Default::default()));

        let outcome = extensions.inspect(&ExtensionInspect {
            path: "mailpit".to_owned(),
        });

        let error = outcome.expect_err("a relative path is not something the daemon can resolve");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }
}
