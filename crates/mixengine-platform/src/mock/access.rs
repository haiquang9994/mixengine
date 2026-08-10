//! Records what it was asked to restrict; restricts nothing.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::{DirectoryAccess, Error, Result};

#[derive(Debug, Default)]
pub(super) struct Access {
    restricted: Mutex<Vec<PathBuf>>,
    /// Set by a test that wants to see what the caller does when the OS says no.
    refuse: Option<&'static str>,
}

impl Access {
    pub(super) fn recording() -> Self {
        Self::default()
    }

    pub(super) fn refusing(reason: &'static str) -> Self {
        Self {
            restricted: Mutex::new(Vec::new()),
            refuse: Some(reason),
        }
    }

    pub(super) fn restricted(&self) -> Vec<PathBuf> {
        self.lock().clone()
    }

    /// A poisoned lock means an assertion already failed on another thread; there is nothing left
    /// for this one to report truthfully, so it takes the contents and carries on.
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<PathBuf>> {
        self.restricted.lock().unwrap_or_else(|poisoned| {
            self.restricted.clear_poison();
            poisoned.into_inner()
        })
    }
}

impl DirectoryAccess for Access {
    fn restrict_to_owner(&self, path: &Path) -> Result<()> {
        if let Some(reason) = self.refuse {
            return Err(Error::UnsupportedPlatform {
                capability: "DirectoryAccess",
                reason: reason.to_owned(),
            });
        }

        // Recorded even though nothing changes on disk: the assertion worth making is "bootstrap
        // asked for these four, in this order", which a test cannot see by looking at a `TempDir`.
        self.lock().push(path.to_path_buf());

        Ok(())
    }

    fn is_restricted_to_owner(&self, path: &Path) -> Result<bool> {
        if let Some(reason) = self.refuse {
            return Err(Error::UnsupportedPlatform {
                capability: "DirectoryAccess",
                reason: reason.to_owned(),
            });
        }

        Ok(self.lock().iter().any(|restricted| restricted == path))
    }
}
