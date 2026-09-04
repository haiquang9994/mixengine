//! An autostart entry that exists only in memory, and remembers what it was asked to do to it.

use std::sync::Mutex;

use crate::{AutostartMechanism, AutostartPlan, AutostartState, Error, Result, ServiceInstaller};

/// What a test asserts on: the calls, in order, with the plan each one carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutostartOp {
    /// [`ServiceInstaller::enable`] was called with this plan.
    Enabled(AutostartPlan),

    /// [`ServiceInstaller::disable`] was called.
    Disabled,
}

/// The name the mock reports its entry under.
///
/// Deliberately not a plausible path or task name: a test that asserted on
/// `~/Library/LaunchAgents/…` here would be asserting about a machine no part of this run touched.
const LOCATION: &str = "the mock host's autostart entry";

#[derive(Debug, Default)]
pub(super) struct Entry {
    /// The plan currently registered, if any.
    registered: Mutex<Option<AutostartPlan>>,

    /// Every mutation, including the ones that changed nothing.
    operations: Mutex<Vec<AutostartOp>>,

    /// Set by a test that wants to see what the caller does on a machine with no mechanism.
    absent: bool,
}

impl Entry {
    pub(super) fn recording() -> Self {
        Self::default()
    }

    /// A machine with nowhere to register anything — Linux with no systemd user manager.
    pub(super) fn without_a_mechanism() -> Self {
        Self {
            absent: true,
            ..Self::default()
        }
    }

    pub(super) fn operations(&self) -> Vec<AutostartOp> {
        lock(&self.operations).clone()
    }

    fn reading(&self, changed: bool) -> AutostartState {
        let registered = lock(&self.registered);

        AutostartState {
            mechanism: match self.absent {
                true => AutostartMechanism::None,
                false => AutostartMechanism::SystemdUser,
            },
            location: LOCATION.to_owned(),
            enabled: registered.is_some(),
            changed,
            command: registered
                .as_ref()
                .map(|plan| {
                    vec![
                        plan.program.display().to_string(),
                        "--home".to_owned(),
                        plan.home.display().to_string(),
                    ]
                })
                .unwrap_or_default(),
        }
    }
}

impl ServiceInstaller for Entry {
    fn enable(&self, plan: &AutostartPlan) -> Result<AutostartState> {
        // Recorded before anything is decided, and whether or not the entry changes: what a test
        // asserts is that the daemon *asked*, which an idempotent second call still did.
        lock(&self.operations).push(AutostartOp::Enabled(plan.clone()));

        if self.absent {
            return Err(Error::UnsupportedPlatform {
                capability: "ServiceInstaller",
                reason: "this mock has no way to start anything at login".to_owned(),
            });
        }

        let changed = {
            let mut registered = lock(&self.registered);
            let changed = registered.as_ref() != Some(plan);
            *registered = Some(plan.clone());
            changed
        };

        Ok(self.reading(changed))
    }

    fn disable(&self) -> Result<AutostartState> {
        lock(&self.operations).push(AutostartOp::Disabled);

        let changed = lock(&self.registered).take().is_some();

        Ok(self.reading(changed))
    }

    fn state(&self) -> Result<AutostartState> {
        Ok(self.reading(false))
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn plan() -> AutostartPlan {
        AutostartPlan {
            program: PathBuf::from("/usr/bin/mixengined"),
            home: PathBuf::from("/tmp/home"),
        }
    }

    #[test]
    fn a_second_enable_with_the_same_plan_changes_nothing_and_is_still_recorded() {
        let entry = Entry::recording();

        assert!(entry.enable(&plan()).unwrap().changed);
        assert!(!entry.enable(&plan()).unwrap().changed);
        assert_eq!(
            entry.operations(),
            [AutostartOp::Enabled(plan()), AutostartOp::Enabled(plan())]
        );
    }

    #[test]
    fn enabling_from_another_home_replaces_the_entry() {
        let entry = Entry::recording();
        entry.enable(&plan()).unwrap();

        let elsewhere = AutostartPlan {
            home: PathBuf::from("/tmp/other"),
            ..plan()
        };
        let state = entry.enable(&elsewhere).unwrap();

        assert!(state.changed);
        assert_eq!(state.command.last().unwrap(), "/tmp/other");
    }

    #[test]
    fn disabling_what_was_never_enabled_changes_nothing() {
        let entry = Entry::recording();

        let state = entry.disable().unwrap();

        assert!(!state.changed);
        assert!(!state.enabled);
        assert!(state.command.is_empty());
    }

    #[test]
    fn a_machine_with_no_mechanism_reports_rather_than_refusing_a_status() {
        let entry = Entry::without_a_mechanism();

        assert_eq!(entry.state().unwrap().mechanism, AutostartMechanism::None);
        assert!(entry.enable(&plan()).is_err());
    }
}
