//! `<root>/bin` and this user's PATH — the only door into either. Roadmap task **T26**.
//!
//! Two mechanisms with one purpose, which is why they are one type: filling the directory makes
//! `php` a file, putting the directory on the PATH makes `php` a word somebody can type, and either
//! on its own does nothing a person would notice. `path.install` does both and `path.status`
//! reports both.
//!
//! **The two halves have opposite policies about being done without being asked**, and the split is
//! where the change lands.
//!
//! `<root>/bin` is inside the home, so it is refreshed on **every start**, beside the recovery
//! passes that reconcile services and jobs: it is a projection of a table in this binary, exactly as
//! `etc/` is a projection of the database, and a home whose `bin/` was deleted is repaired by
//! starting the daemon. Nothing outside the root is touched by it and there is nothing for a user to
//! consent to.
//!
//! The PATH is **only ever changed when somebody asks**. It is a file in the user's home on Unix and
//! a value in their registry hive on Windows — outside the root, and outside what
//! `.claude/architecture/overview.md` lists as MixEngine's to write on its own account. A daemon
//! that edited `~/.zprofile` because it happened to start at login would be a program that changed
//! the shell of somebody who had only installed it.

use std::path::PathBuf;
use std::sync::Arc;

use mixengine_core::{Paths, shims};
use mixengine_platform::{Host, PathState};
use mixengine_proto::{Error, PathPlace, PathReport};

use crate::error::ToWire as _;

/// The home's `bin/`, the binary that fills it, and the machine whose PATH it goes on.
#[derive(Debug)]
pub(crate) struct Shims {
    /// `<root>/bin`.
    bin: PathBuf,

    /// The program that is running, which is what the shim binary is found beside.
    ///
    /// Held rather than the resolved shim, so that a broken installation is reported by the call
    /// that needs it rather than by refusing to start: a daemon with no `mixengine-shim` next to it
    /// can still supervise every service in this home, and saying so at `path.install` is where a
    /// person can act on it.
    program: PathBuf,

    /// The OS, for the half of this that is not a file inside the home.
    host: Arc<dyn Host>,
}

impl Shims {
    pub(crate) fn new(paths: &Paths, program: PathBuf, host: Arc<dyn Host>) -> Self {
        Self {
            bin: paths.bin().to_path_buf(),
            program,
            host,
        }
    }

    /// Put one copy of the shim in `bin/` per command, and clear out what is not one.
    ///
    /// Called at every start and by [`install`](Self::install). Touches nothing outside the home.
    ///
    /// Answers the **wire** error rather than the domain one, although its only other caller is the
    /// start-up path that logs it: everything below the API boundary in this binary has already
    /// been through [`ToWire`](crate::error::ToWire), and a second error type flowing up through
    /// one method would be one place for a hint to go missing.
    pub(crate) fn refresh(&self) -> Result<shims::Refreshed, Error> {
        let shim = shims::source(&self.program).map_err(|error| error.to_wire())?;

        shims::refresh(&self.bin, &shim).map_err(|error| error.to_wire())
    }

    /// `path.status` — what a terminal opened a minute from now would find.
    ///
    /// Reads `bin/` rather than reporting [`COMMANDS`](mixengine_core::shims::COMMANDS): the
    /// question is what is *there*, and a listing composed from the table would answer it out of
    /// this binary's constants on a machine where the directory had been deleted.
    pub(crate) fn status(&self) -> Result<PathReport, Error> {
        let state = self
            .host
            .path_integration()
            .state(&self.bin)
            .map_err(|error| error.to_wire())?;

        Ok(self.report(state, self.installed(), Vec::new()))
    }

    /// `path.install` — fill `bin/`, then put it on the PATH.
    ///
    /// **That order and not the other**, because the failure that survives has to be the harmless
    /// one: a directory of shims nothing can find is invisible, and a PATH entry naming a directory
    /// that was never filled is a `php` that resolves to nothing.
    pub(crate) fn install(&self) -> Result<PathReport, Error> {
        let refreshed = self.refresh()?;

        let state = self
            .host
            .path_integration()
            .add(&self.bin)
            .map_err(|error| error.to_wire())?;

        Ok(self.report(state, refreshed.commands, refreshed.refused))
    }

    /// `path.uninstall` — take `bin/` off the PATH, and leave it exactly as it is.
    ///
    /// The shims stay. They are inside the home, they cost a few megabytes there, and removing what
    /// makes the home work in order to undo one line in a profile would be an uninstall wearing a
    /// smaller command's name — `.claude/architecture/overview.md` has removing the home remove
    /// them.
    pub(crate) fn uninstall(&self) -> Result<PathReport, Error> {
        let state = self
            .host
            .path_integration()
            .remove(&self.bin)
            .map_err(|error| error.to_wire())?;

        Ok(self.report(state, self.installed(), Vec::new()))
    }

    /// The commands `bin/` answers to right now, read off the directory.
    ///
    /// Best effort, and the empty list is the honest answer for a directory that cannot be read: a
    /// status that invented the table's contents would say `php` is there on a machine where it is
    /// not, which is the one thing this method exists to be able to deny.
    fn installed(&self) -> Vec<String> {
        let known: Vec<String> = shims::COMMANDS.iter().map(shims::file_name).collect();

        let Ok(entries) = std::fs::read_dir(&self.bin) else {
            return Vec::new();
        };

        let present: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();

        // In the table's order rather than the directory's, which is arbitrary on every filesystem
        // and stable on none — a listing somebody scans has to be one the eye can predict, which is
        // `runtime.list_installed`'s own reasoning.
        known
            .into_iter()
            .filter(|name| {
                present.iter().any(|found| match cfg!(windows) {
                    true => found.eq_ignore_ascii_case(name),
                    false => found == name,
                })
            })
            .collect()
    }

    /// The wire shape of an answer, from the OS's half and the directory's.
    fn report(&self, state: PathState, commands: Vec<String>, stale: Vec<String>) -> PathReport {
        PathReport {
            directory: self.bin.display().to_string(),
            on_path: state.complete(),
            places: state
                .locations
                .into_iter()
                .map(|location| PathPlace {
                    name: location.name,
                    present: location.present,
                    changed: location.changed,
                })
                .collect(),
            commands,
            stale,
        }
    }
}
