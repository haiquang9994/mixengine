//! `~/Library/Application Support/MixEngine`.

use std::path::PathBuf;

use directories::BaseDirs;

use crate::{Error, HomeDirs, Result};

#[derive(Debug, Default)]
pub(crate) struct Home;

impl HomeDirs for Home {
    fn default_home(&self) -> Result<PathBuf> {
        let base = BaseDirs::new().ok_or(Error::NoHomeDirectory {
            reason: "$HOME is not set",
        })?;
        Ok(base.data_dir().join("MixEngine"))
    }
}
