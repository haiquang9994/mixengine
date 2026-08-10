//! A default root that the test chose, or none at all.

use std::path::PathBuf;

use crate::{Error, HomeDirs, Result};

#[derive(Debug)]
pub(super) struct Home {
    default: Option<PathBuf>,
}

impl Home {
    pub(super) fn answering(default: Option<PathBuf>) -> Self {
        Self { default }
    }
}

impl HomeDirs for Home {
    fn default_home(&self) -> Result<PathBuf> {
        self.default.clone().ok_or(Error::NoHomeDirectory {
            reason: "the mock host was built without one",
        })
    }
}
