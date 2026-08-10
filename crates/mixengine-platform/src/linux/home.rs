//! `$XDG_DATA_HOME/mixengine`, falling back to `~/.local/share/mixengine`.

use std::path::PathBuf;

use directories::BaseDirs;

use crate::{Error, HomeDirs, Result};

#[derive(Debug, Default)]
pub(crate) struct Home;

impl HomeDirs for Home {
    fn default_home(&self) -> Result<PathBuf> {
        // `data_dir()` already implements the XDG fallback, so an unset or relative
        // `$XDG_DATA_HOME` lands on `~/.local/share` the way the spec requires.
        let base = BaseDirs::new().ok_or(Error::NoHomeDirectory {
            reason: "neither $XDG_DATA_HOME nor $HOME is set",
        })?;
        Ok(base.data_dir().join("mixengine"))
    }
}
