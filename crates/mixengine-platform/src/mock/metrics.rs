//! Readings a test programmed, rather than readings taken from this machine.
//!
//! **Behind a `Mutex` for [`Connections`](super::connections::Connections)' reason**: what a group is
//! spending is the thing a test *changes* between two readings of one `Host`, so a table set up once
//! at construction could not express a service that grew.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::process::StartTime;
use crate::{GroupReading, GroupRoot, ProcessMetrics};

/// What this mock will say about each pid. A pid it does not name has no reading.
#[derive(Debug, Default)]
pub(super) struct Readings {
    /// Each measurable pid, the moment it began, and what it costs.
    programmed: Mutex<BTreeMap<u32, (StartTime, GroupReading)>>,
}

impl Readings {
    /// Make `pid` measurable, as a process that began at `started`.
    pub(super) fn set(&self, pid: u32, started: StartTime, reading: GroupReading) {
        self.locked().insert(pid, (started, reading));
    }

    /// Stop `pid` being measurable — the process ended.
    pub(super) fn clear(&self, pid: u32) {
        self.locked().remove(&pid);
    }

    /// The table, or a panic naming why it could not be taken.
    ///
    /// A poisoned lock here means a test panicked while holding it, and the useful report is that
    /// panic rather than a second one about a mutex.
    fn locked(&self) -> std::sync::MutexGuard<'_, BTreeMap<u32, (StartTime, GroupReading)>> {
        self.programmed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ProcessMetrics for Readings {
    fn measure(&self, roots: &[GroupRoot]) -> Vec<GroupReading> {
        let programmed = self.locked();

        roots
            .iter()
            .filter_map(|root| {
                let (started, reading) = programmed.get(&root.pid)?;

                // A pid the machine handed out again is not the process the caller recorded.
                (*started == root.started).then_some(*reading)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(pid: u32) -> GroupReading {
        GroupReading {
            pid,
            cpu_percent: Some(12.5),
            rss_bytes: 1024,
            processes: 3,
        }
    }

    #[test]
    fn a_programmed_pid_is_measured() {
        let readings = Readings::default();
        readings.set(41, StartTime::from_stored(100), reading(41));

        let measured = readings.measure(&[GroupRoot {
            pid: 41,
            started: StartTime::from_stored(100),
        }]);

        assert_eq!(measured, vec![reading(41)]);
    }

    #[test]
    fn a_pid_whose_start_time_does_not_match_is_not_measured() {
        let readings = Readings::default();
        readings.set(41, StartTime::from_stored(100), reading(41));

        let measured = readings.measure(&[GroupRoot {
            pid: 41,
            started: StartTime::from_stored(999),
        }]);

        assert!(
            measured.is_empty(),
            "a recycled pid must not be measured as the process that was recorded"
        );
    }

    #[test]
    fn a_pid_that_has_ended_is_absent_rather_than_zero() {
        let readings = Readings::default();
        readings.set(41, StartTime::from_stored(100), reading(41));
        readings.clear(41);

        assert!(
            readings
                .measure(&[GroupRoot {
                    pid: 41,
                    started: StartTime::from_stored(100),
                }])
                .is_empty()
        );
    }
}
