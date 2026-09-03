//! Finding an installed desktop application in the registry — roadmap task **T83**.
//!
//! Wired here so the capability exists on every system; the registry lookup arrives in its own
//! change.

use std::collections::BTreeMap;
use std::ffi::OsString;

use crate::{DesktopApps, InstalledApp, Located, Result, Started};

#[derive(Debug)]
pub(crate) struct Apps;

impl Apps {
    pub(crate) fn of_this_machine() -> Self {
        Self
    }
}

impl DesktopApps for Apps {
    fn locate(&self, hint: &str) -> Result<Located> {
        Ok(Located::NotInstalled {
            searched: format!("{hint} under App Paths and in the uninstall table"),
        })
    }

    fn launch(
        &self,
        app: &InstalledApp,
        args: &[OsString],
        env: &BTreeMap<String, String>,
    ) -> Result<Started> {
        crate::desktop::launch(app, args, env)
    }
}
