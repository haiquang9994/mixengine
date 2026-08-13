//! Running one of a service's own programs — a health probe, a shutdown command — and waiting for
//! what it says.
//!
//! `mariadb-admin ping`, `pg_isready`, `redis-cli ping`, `caddy stop`: short-lived programs shipped
//! *with* the service, run for an exit status. The spawn itself is
//! [`mixengine_platform::process::run_once`], because a `Command` on Windows has to be told not to
//! be given a console window and no crate above the platform layer may say so. What this module
//! adds is the one thing the platform call cannot know: **where** such a program is run.
//!
//! # A probe runs where the service runs
//!
//! [`Surroundings`] is a service's working directory and the environment it was actually started
//! with, and handing both to its own commands is not a convenience. `mariadb-admin` finds the socket
//! it should ask about through `HOME` and the same generated config the server read; a credential
//! reaches it through the environment, which is the whole point of `EnvValue::Keyring`
//! (`.claude/decisions/0006-servicespec-in-proto-and-secret-free.md`) and is why it must not travel
//! in an argument list every process table on the machine can read. A
//! probe given the daemon's surroundings instead would be asking a well-formed question about a
//! different server, and answering it would be worse than not asking.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mixengine_platform::process::{self, Ran};

use crate::Result;

/// Where a service's own commands are run: its directory, and the environment it was started with.
///
/// Built once per life of the process — the environment is resolved at spawn time, credentials and
/// all — and kept for as long as that process is supervised, because the alternative is re-reading
/// the OS keyring on every health probe of every service, ten seconds apart, for ever.
#[derive(Clone)]
pub struct Surroundings {
    /// The service's working directory, from its spec.
    directory: PathBuf,

    /// The environment the service itself was given: the spec's, over the platform's floor.
    env: BTreeMap<String, String>,
}

/// Written by hand, and the values are the reason.
///
/// This environment is the resolved one — `EnvValue::Keyring` has already been read by the time it
/// gets here, so `MYSQL_PWD` is in there as the password itself. `.claude/standards/rust.md`'s rule
/// is that a struct which *might* hold a secret redacts it rather than trusting every caller that
/// ever writes `{:?}`, and this type is re-exported from the crate root and held by a
/// `#[derive(Debug)]` runner in the daemon — a `tracing` field on a stop that went wrong is one
/// line away at all times.
///
/// The names stay, because they are what a reader debugging a probe actually needs: whether `HOME`
/// reached it, not what `HOME` was.
impl fmt::Debug for Surroundings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Surroundings")
            .field("directory", &self.directory)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Surroundings {
    /// The place a service that was started with `env` in `directory` runs its own commands in.
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>, env: BTreeMap<String, String>) -> Self {
        Self {
            directory: directory.into(),
            env,
        }
    }

    /// Run one of them and wait for it, for at most `patience`.
    ///
    /// # Errors
    ///
    /// [`crate::Error::Platform`] when the program cannot be started at all — a probe whose binary a
    /// spec names and this machine does not have. **A program that ran and failed is not an error**:
    /// that is the answer, and it arrives in the [`Ran`].
    pub async fn run(&self, program: &Path, args: &[String], patience: Duration) -> Result<Ran> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();

        process::run_once(program, &args, &self.directory, &self.env, patience)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sentence would not have caught this: the derive that used to be here compiled, ran, and
    /// printed the password.
    #[test]
    fn debug_names_the_environment_without_saying_what_is_in_it() {
        let place = Surroundings::new(
            "/srv/mariadb",
            BTreeMap::from([
                ("HOME".to_owned(), "/srv/mariadb".to_owned()),
                ("MYSQL_PWD".to_owned(), "hunter2".to_owned()),
            ]),
        );

        let printed = format!("{place:?}");

        assert!(
            printed.contains("MYSQL_PWD"),
            "the names are what makes this worth printing at all: {printed}"
        );
        assert!(
            !printed.contains("hunter2"),
            "a resolved keyring value must not reach a log: {printed}"
        );
    }
}
