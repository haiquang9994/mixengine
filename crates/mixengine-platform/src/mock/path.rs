//! A PATH that exists only in memory, and remembers what it was asked to do to it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::{Error, PathIntegration, PathLocation, PathState, Result};

/// What a test asserts on: the calls, in order, with the directory each named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathOp {
    /// [`PathIntegration::add`] was called with this directory.
    Added(PathBuf),

    /// [`PathIntegration::remove`] was called with it.
    Removed(PathBuf),
}

/// The name the mock reports its one location under.
///
/// Deliberately not a plausible file path: a test that asserted on `~/.zprofile` here would be
/// asserting about a machine no part of this run touched.
const LOCATION: &str = "the mock host's PATH";

#[derive(Debug, Default)]
pub(super) struct Env {
    /// Which directories are on it, in the order they were prepended.
    entries: Mutex<Vec<PathBuf>>,

    /// Every mutation, including the ones that changed nothing.
    operations: Mutex<Vec<PathOp>>,

    /// Set by a test that wants to see what the caller does when the OS says no.
    refuse: Option<&'static str>,
}

impl Env {
    pub(super) fn recording() -> Self {
        Self::default()
    }

    pub(super) fn refusing(reason: &'static str) -> Self {
        Self {
            refuse: Some(reason),
            ..Self::default()
        }
    }

    pub(super) fn operations(&self) -> Vec<PathOp> {
        lock(&self.operations).clone()
    }

    fn refused(&self) -> Result<()> {
        match self.refuse {
            Some(reason) => Err(Error::UnsupportedPlatform {
                capability: "PathIntegration",
                reason: reason.to_owned(),
            }),
            None => Ok(()),
        }
    }

    fn state(present: bool, changed: bool) -> PathState {
        PathState {
            locations: vec![PathLocation {
                name: LOCATION.to_owned(),
                present,
                changed,
            }],
        }
    }
}

impl PathIntegration for Env {
    fn add(&self, dir: &Path) -> Result<PathState> {
        self.refused()?;

        // Recorded before the state is consulted, and whether or not anything changes: what a test
        // asserts is that the daemon *asked*, which an idempotent second call still did.
        lock(&self.operations).push(PathOp::Added(dir.to_path_buf()));

        let mut entries = lock(&self.entries);

        if entries.iter().any(|entry| entry == dir) {
            return Ok(Env::state(true, false));
        }

        entries.insert(0, dir.to_path_buf());

        Ok(Env::state(true, true))
    }

    fn remove(&self, dir: &Path) -> Result<PathState> {
        self.refused()?;

        lock(&self.operations).push(PathOp::Removed(dir.to_path_buf()));

        let mut entries = lock(&self.entries);
        let before = entries.len();
        entries.retain(|entry| entry != dir);

        Ok(Env::state(false, entries.len() != before))
    }

    fn state(&self, dir: &Path) -> Result<PathState> {
        self.refused()?;

        Ok(Env::state(
            lock(&self.entries).iter().any(|entry| entry == dir),
            false,
        ))
    }
}

/// A poisoned lock means an assertion already failed on another thread; there is nothing left for
/// this one to report truthfully, so it takes the contents and carries on.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        mutex.clear_poison();
        poisoned.into_inner()
    })
}
