//! The real reading, taken with `sysinfo` — roadmap task **T71**.
//!
//! **One file for three operating systems**, which is the exception this crate allows itself where a
//! dependency has already done the per-OS work: `sysinfo` reads `/proc` on Linux, `proc_pidinfo` on
//! macOS and a toolhelp snapshot on Windows, and nothing here names which. The `#[cfg]` rule exists
//! to keep operating-system differences out of the crates *above* this one, and this is inside it.
//!
//! **The walk is a pure function over a table.** A test that had to grow a real process tree would
//! be a test that only runs where unsigned children are allowed to start, which on a developer's
//! Windows machine is nowhere — so the table comes in as data and the arithmetic is asserted on
//! that.
//!
//! # A group is a tree, and the alternative was measured against
//!
//! Every supervised service on Windows already runs in a Job Object, and `QueryInformationJobObject`
//! reports the job's total CPU time and peak memory directly — more accurate than this walk and
//! immune to a process that reparents itself. It is deliberately not used: it would measure Windows
//! through a different mechanism than the other two systems, at the moment **T72** is about to hold
//! all three to one threshold, and three numbers that cannot be compared are worse than three
//! numbers wrong in the same direction. This walk overstates shared pages identically everywhere.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::process::StartTime;
use crate::{GroupReading, GroupRoot, ProcessMetrics};

/// One process, as a refresh saw it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Row {
    /// Who started it, where the system still says.
    parent: Option<u32>,

    /// Percentage of one core since the previous refresh.
    cpu_percent: f32,

    /// Resident bytes.
    rss_bytes: u64,
}

/// Everything one refresh saw, beside the pids the refresh before it had seen.
#[derive(Debug, Default)]
struct Snapshot {
    /// This refresh.
    rows: BTreeMap<u32, Row>,

    /// What the previous refresh held, which is the whole of what makes a CPU figure possible.
    previous: BTreeSet<u32>,
}

impl Snapshot {
    /// Sum each root's group out of this snapshot.
    ///
    /// `identify` is asked what the process bearing a pid says about when it began; a root whose
    /// answer differs from what the caller recorded is a pid the system handed round, and produces
    /// no reading at all.
    fn aggregate(
        &self,
        roots: &[GroupRoot],
        identify: &dyn Fn(u32) -> Option<StartTime>,
    ) -> Vec<GroupReading> {
        let children = self.children();

        roots
            .iter()
            .filter(|root| self.rows.contains_key(&root.pid))
            .filter(|root| identify(root.pid) == Some(root.started))
            .map(|root| {
                let mut rss_bytes: u64 = 0;
                let mut cpu = 0.0;
                let mut processes = 0;
                let mut stack = vec![root.pid];

                // Iterative rather than recursive: a process table is a graph this crate did not
                // build, and a cycle in one must not be a stack overflow in the daemon. `seen` is
                // what makes that true rather than hoped for.
                let mut seen = BTreeSet::new();

                while let Some(pid) = stack.pop() {
                    if !seen.insert(pid) {
                        continue;
                    }

                    let Some(row) = self.rows.get(&pid) else {
                        continue;
                    };

                    rss_bytes = rss_bytes.saturating_add(row.rss_bytes);
                    cpu += row.cpu_percent;
                    processes += 1;

                    if let Some(kids) = children.get(&pid) {
                        stack.extend(kids.iter().copied());
                    }
                }

                GroupReading {
                    pid: root.pid,
                    // A group whose root this sampler had not yet seen has no difference to report.
                    cpu_percent: self.previous.contains(&root.pid).then_some(cpu),
                    rss_bytes,
                    processes,
                }
            })
            .collect()
    }

    /// Who each process started, inverted from each row's parent.
    fn children(&self) -> BTreeMap<u32, Vec<u32>> {
        let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();

        for (pid, row) in &self.rows {
            if let Some(parent) = row.parent {
                children.entry(parent).or_default().push(*pid);
            }
        }

        children
    }
}

/// This machine's own answer, with the state a CPU figure is a difference from.
#[derive(Debug)]
pub(crate) struct Sampler {
    /// The `sysinfo` state and the pids the last refresh saw, together because they are only ever
    /// read and written together.
    state: Mutex<(sysinfo::System, BTreeSet<u32>)>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            state: Mutex::new((sysinfo::System::new(), BTreeSet::new())),
        }
    }
}

impl ProcessMetrics for Sampler {
    fn measure(&self, roots: &[GroupRoot]) -> Vec<GroupReading> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let (system, previous) = &mut *state;

        system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

        let snapshot = Snapshot {
            rows: system
                .processes()
                .iter()
                .map(|(pid, process)| {
                    (
                        pid.as_u32(),
                        Row {
                            parent: process.parent().map(sysinfo::Pid::as_u32),
                            cpu_percent: process.cpu_usage(),
                            rss_bytes: process.memory(),
                        },
                    )
                })
                .collect(),
            previous: std::mem::take(previous),
        };

        let readings =
            snapshot.aggregate(roots, &|pid| crate::process::started_at(pid).ok().flatten());

        *previous = snapshot.rows.keys().copied().collect();

        readings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(parent: Option<u32>, cpu_percent: f32, rss_bytes: u64) -> Row {
        Row {
            parent,
            cpu_percent,
            rss_bytes,
        }
    }

    /// A root, its two workers, and an unrelated process that must not be counted.
    fn snapshot() -> Snapshot {
        Snapshot {
            rows: BTreeMap::from([
                (10, row(Some(1), 5.0, 100)),
                (11, row(Some(10), 2.5, 50)),
                (12, row(Some(10), 2.5, 50)),
                (99, row(Some(1), 90.0, 9_000)),
            ]),
            previous: BTreeSet::from([10, 11, 12, 99]),
        }
    }

    fn born_at(stored: i64) -> impl Fn(u32) -> Option<StartTime> {
        move |_| Some(StartTime::from_stored(stored))
    }

    fn root(pid: u32, stored: i64) -> GroupRoot {
        GroupRoot {
            pid,
            started: StartTime::from_stored(stored),
        }
    }

    #[test]
    fn a_group_is_the_root_and_everything_under_it() {
        let measured = snapshot().aggregate(&[root(10, 7)], &born_at(7));

        assert_eq!(measured.len(), 1);
        assert_eq!(
            measured[0].rss_bytes, 200,
            "the root and its two workers, and not the unrelated process"
        );
        assert_eq!(measured[0].processes, 3);
        assert_eq!(measured[0].cpu_percent, Some(10.0));
    }

    #[test]
    fn a_group_seen_for_the_first_time_has_no_cpu_figure() {
        let mut snapshot = snapshot();
        snapshot.previous = BTreeSet::new();

        let measured = snapshot.aggregate(&[root(10, 7)], &born_at(7));

        assert_eq!(
            measured[0].cpu_percent, None,
            "a difference needs a previous reading, and a zero would draw an idle service"
        );
        assert_eq!(measured[0].rss_bytes, 200, "memory needs no history");
    }

    #[test]
    fn a_recycled_pid_is_not_measured() {
        let measured = snapshot().aggregate(&[root(10, 7)], &born_at(8));

        assert!(
            measured.is_empty(),
            "the pid is live, but it is not the process the caller recorded"
        );
    }

    #[test]
    fn a_root_that_is_gone_produces_no_reading() {
        let measured = snapshot().aggregate(&[root(4_242, 7)], &born_at(7));

        assert!(measured.is_empty());
    }

    #[test]
    fn a_cycle_in_the_table_is_walked_once_rather_than_forever() {
        // Never seen on a healthy machine. Asserted because the table is the operating system's and
        // a stack overflow in the daemon is not an acceptable way to find that out.
        let snapshot = Snapshot {
            rows: BTreeMap::from([(10, row(Some(11), 1.0, 10)), (11, row(Some(10), 1.0, 10))]),
            previous: BTreeSet::from([10, 11]),
        };

        let measured = snapshot.aggregate(&[root(10, 7)], &born_at(7));

        assert_eq!(measured[0].processes, 2);
        assert_eq!(measured[0].rss_bytes, 20);
    }

    /// What one refresh of every process on this machine costs.
    ///
    /// **Ignored by default and asserting nothing** — a timing taken on a shared runner is not a
    /// fact to fail a build on. It exists because T71 chose to sample a machine nobody is watching,
    /// and the number belongs beside the periods that spend it, which is
    /// [`ProcessMetrics::measure`]'s documentation. Run it with `--ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not an assertion"]
    fn one_refresh_costs() {
        let sampler = Sampler::default();
        let mine = std::process::id();
        let started = crate::process::started_at(mine)
            .expect("this process can be asked about")
            .expect("this process is running");

        // The first call builds the table; the ones after it are what a tick actually pays.
        sampler.measure(&[GroupRoot { pid: mine, started }]);

        let began = std::time::Instant::now();
        for _ in 0..10 {
            sampler.measure(&[GroupRoot { pid: mine, started }]);
        }

        println!("one refresh: {:?}", began.elapsed() / 10);
    }
}
