//! Finding an installed desktop application through its desktop entry — roadmap task **T83**.
//!
//! Wired here so the capability exists on every system; the XDG walk arrives in its own change.

use std::collections::BTreeMap;
use std::ffi::OsString;

use crate::{DesktopApps, InstalledApp, Located, Result, Started};

#[derive(Debug)]
pub(crate) struct Apps;

impl Apps {
    pub(crate) fn of_this_user() -> Self {
        Self
    }
}

impl DesktopApps for Apps {
    fn locate(&self, hint: &str) -> Result<Located> {
        Ok(Located::NotInstalled {
            searched: format!("{hint} in the XDG applications directories"),
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
