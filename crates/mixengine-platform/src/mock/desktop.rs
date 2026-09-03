//! A desktop application that exists when a test says so, and a record of what was started.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use crate::{DesktopApps, InstalledApp, Located, Result, Started};

/// One launch, as the mock saw it.
///
/// **Names and never values**, on [`SecretOp`](super::SecretOp)'s rule: the environment a launch
/// carried is recorded as the list of variables in it, so an assertion can say "the password was
/// handed over in `MIXENGINE_DB_PASSWORD`" without the password sitting in a recorder afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    /// What was started.
    pub program: PathBuf,

    /// With what, after the application's own fixed arguments.
    pub args: Vec<OsString>,

    /// Which variables were added to its environment, sorted.
    pub env_names: Vec<String>,
}

/// The pid every mock launch reports.
const PID: u32 = 4242;

#[derive(Debug, Default)]
pub(super) struct Apps {
    /// The one application this machine has, for every hint — or none.
    program: Option<PathBuf>,
    launches: Mutex<Vec<Launched>>,
}

impl Apps {
    pub(super) fn installing(program: PathBuf) -> Self {
        Self {
            program: Some(program),
            launches: Mutex::default(),
        }
    }

    pub(super) fn launched(&self) -> Vec<Launched> {
        self.launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl DesktopApps for Apps {
    fn locate(&self, _hint: &str) -> Result<Located> {
        Ok(match &self.program {
            Some(program) => Located::Installed(InstalledApp {
                program: program.clone(),
                args: Vec::new(),
            }),
            None => Located::NotInstalled {
                searched: "the mock's empty table of applications".to_owned(),
            },
        })
    }

    fn launch(
        &self,
        app: &InstalledApp,
        args: &[OsString],
        env: &BTreeMap<String, String>,
    ) -> Result<Started> {
        self.launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Launched {
                program: app.program.clone(),
                args: args.to_vec(),
                env_names: env.keys().cloned().collect(),
            });

        Ok(Started::Running { pid: PID })
    }
}
