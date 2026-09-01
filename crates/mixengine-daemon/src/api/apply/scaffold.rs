//! Running a blueprint's own command — roadmap task **T78a**.
//!
//! **This is the first thing MixEngine runs that MixEngine did not write**, and every guard around
//! it is somewhere else: the consent that names the command is checked in
//! [`super`](crate::api::apply), the trust that decides how loudly it is asked for is a column
//! decided at import, and the shell that starts it is `mixengine-platform`'s. What is here is the
//! part that is only true while it runs — where the lines go, what a cancellation does, and what the
//! exit code becomes.
//!
//! Three things it deliberately does not do:
//!
//! - **It never elevates.** The command runs under the account this daemon runs as, and nothing it
//!   does reaches the elevation queue. T78's "one prompt, at the end" is about the hosts file and
//!   the trust store; a blueprint's command is not admitted to it.
//! - **It invents no environment.** The working directory is the project's and `PATH` gains the shim
//!   directory, which is how the blueprint's own `[runtimes]` reaches the command — the shims
//!   resolve a version from the project they are run in. Nothing else is added.
//! - **It has no timeout** (the T78a design, D10). Every number that could be chosen kills a
//!   legitimate `composer install` on a slow line, and a scaffold is by definition somebody else's
//!   program doing an unknown amount of work. The bound is the job: it is visible, its output is
//!   streaming, and `job.cancel` stops it — which kills the process *group*, so the tree a package
//!   manager forked goes with it.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use mixengine_core::Paths;
use mixengine_platform::process::Limits;
use mixengine_proto::{LogLine, LogPolicy, StepResult};
use mixengine_supervisor::logs::Capture;

use crate::jobs::JobHandle;
use crate::services::logs::ServiceLog;

/// How often the command is asked whether it has ended.
///
/// `services::runner`'s own liveness poll, and for its reason: there is nothing to wait *on* that a
/// cancellation could also interrupt, so the wait is a poll and the interval is short enough that a
/// finished command is not sat on.
const POLL: Duration = Duration::from_millis(50);

/// How many of the command's last lines are quoted when it fails.
///
/// A job's log is a ring and no file (D13), so what survives the terminal scrolling past is this.
const LAST_WORDS: usize = 3;

/// Where a running command's output goes.
///
/// A trait so the tests can watch the lines without a job registry behind them, and so this module
/// says out loud that it only ever *writes* — nothing here reads the log back.
pub(crate) trait Sink: Send + Sync {
    /// One line, as it arrived.
    fn line(&self, line: LogLine);
}

impl Sink for ServiceLog {
    fn line(&self, line: LogLine) {
        self.record(line);
    }
}

/// A sink for a test that only wants the outcome.
#[cfg(test)]
pub(crate) struct Discarding;

#[cfg(test)]
impl Sink for Discarding {
    fn line(&self, _line: LogLine) {}
}

/// What the command sees: the project's directory, and the shims in front of this daemon's `PATH`.
///
/// **The shim directory is the whole of how a blueprint's `[runtimes]` reaches the command.** A shim
/// reads the project it is run in and resolves the version pinned there, so `composer` finds the PHP
/// this apply just pinned without anything here computing a path to it. The rest of `PATH` is this
/// daemon's own, because a scaffold needs the `git`, `npm` and `composer` a person installed and
/// inventing that list is not something a daemon can do honestly.
/// **`std::env::join_paths` rather than a separator of our own**, which is what keeps this file free
/// of a `#[cfg(windows)]` the way `.claude/CLAUDE.md` asks of everything above
/// `mixengine-platform`: the standard library already knows what this system puts between two `PATH`
/// entries. A `PATH` this cannot be joined back into — an entry holding the separator itself — leaves
/// the command with the shims alone, which is the half that matters here.
pub(crate) fn environment(paths: &Paths) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let inherited = std::env::var_os("PATH").unwrap_or_default();

    let ahead = std::iter::once(paths.bin().to_path_buf());
    let joined = std::env::join_paths(ahead.chain(std::env::split_paths(&inherited)))
        .unwrap_or_else(|_| paths.bin().as_os_str().to_owned());

    env.insert("PATH".to_owned(), joined.to_string_lossy().into_owned());
    env
}

/// Run one command in `root`, with its output going to `sink` as it arrives.
///
/// **A non-zero exit is a [`StepResult::Failed`] and not an error** (D7 and D8): the apply did what
/// it was asked, the project it made works, and the command's exit code is the command's own news.
/// Returning an error here would spend T78's rollback ledger on a project that is fine.
///
/// A cancellation stops the group and answers [`StepResult::NotRun`], because a cancellation is not
/// a request to undo anything — running the apply again offers the command afresh.
pub(crate) async fn run_command(
    command: &str,
    root: &Path,
    env: &BTreeMap<String, String>,
    sink: &dyn Sink,
    cancelled: Option<&JobHandle>,
) -> StepResult {
    let mut child = match mixengine_platform::process::spawn_shell_supervised(
        command,
        root,
        env,
        &Limits::default(),
    ) {
        Ok(child) => child,

        // The shell itself would not start, which is a machine that cannot run any scaffold rather
        // than a command that failed. Still this step's outcome and not the apply's: the project is
        // made either way.
        Err(error) => {
            return StepResult::Failed {
                why: format!("it could not be started: {error}"),
            };
        }
    };

    // Before anything waits: a pipe nobody drains stops the command writing to it, which looks
    // exactly like a command that has hung. `services::runner`'s obligation, discharged here for
    // the one child this module starts.
    let capture = Capture::start(&mut child, "a blueprint's command", policy(), None);
    let (already_said, mut lines) = capture.read();

    for line in already_said {
        sink.line(line);
    }

    let mut last = Vec::new();

    loop {
        while let Ok(line) = lines.try_recv() {
            keep_last(&mut last, &line);
            sink.line(line);
        }

        if cancelled.is_some_and(JobHandle::is_cancelled) {
            // The group, not the pid: a package manager's children are what would otherwise be left
            // running with nothing owning them.
            let _ = child.stop();

            return StepResult::NotRun {
                why: format!("`{command}` was cancelled; running the apply again offers it again"),
            };
        }

        match child.exited() {
            Ok(Some(exit)) => {
                // Whatever was still in the pipes when it ended.
                while let Ok(line) = lines.try_recv() {
                    keep_last(&mut last, &line);
                    sink.line(line);
                }

                return match exit.is_success() {
                    true => StepResult::Done,
                    false => StepResult::Failed {
                        why: failure(command, exit.code(), &last),
                    },
                };
            }

            Ok(None) => tokio::time::sleep(POLL).await,

            Err(error) => {
                return StepResult::Failed {
                    why: format!("this daemon lost track of `{command}`: {error}"),
                };
            }
        }
    }
}

/// How many of a command's lines the daemon keeps for a reader that connects late.
///
/// Small on purpose: what a client following the job reads is streaming past it, and what is kept
/// is only the backlog somebody arriving mid-command is handed.
pub(crate) const RING_LINES: usize = 200;

/// The ring the capture keeps while the command runs.
fn policy() -> LogPolicy {
    LogPolicy {
        ring_lines: u16::try_from(RING_LINES).unwrap_or(u16::MAX),
        ..LogPolicy::default()
    }
}

/// Keep the last few lines, for the sentence a failure has to write.
fn keep_last(last: &mut Vec<String>, line: &LogLine) {
    if last.len() == LAST_WORDS {
        last.remove(0);
    }

    last.push(line.text.clone());
}

/// What a failed command's outcome says.
///
/// The exit code, and the last of what it printed — because a job's log is memory only, and by the
/// time somebody reads the outcome the lines may be gone.
fn failure(command: &str, code: Option<i32>, last: &[String]) -> String {
    let ended = match code {
        Some(code) => format!("`{command}` exited with {code}"),
        None => format!("`{command}` was ended by a signal"),
    };

    match last.iter().find(|line| !line.trim().is_empty()) {
        Some(_) => format!("{ended} — its last words: {}", last.join(" / ")),
        None => ended,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command that writes a file, on this system. A blueprint's command is its author's, and a
    /// test's is the test's.
    fn writing_a_file() -> &'static str {
        if cfg!(windows) {
            "echo hello> made.txt"
        } else {
            "printf hello > made.txt"
        }
    }

    /// It runs in the project's directory, which is the whole of where a scaffold belongs.
    #[tokio::test]
    async fn a_command_runs_in_the_project_directory() {
        let root = tempfile::tempdir().expect("a directory");

        let result = run_command(
            writing_a_file(),
            root.path(),
            &BTreeMap::new(),
            &Discarding,
            None,
        )
        .await;

        assert_eq!(result, StepResult::Done, "{result:?}");
        assert!(root.path().join("made.txt").is_file());
    }

    /// **A command that fails is a failed step, not a failed job** — roadmap task **T78a**, its
    /// design's D7. The exit code is in the outcome, so somebody reading it afterwards has the one
    /// fact the log may no longer hold.
    #[tokio::test]
    async fn a_command_that_exits_non_zero_is_a_failed_step() {
        let root = tempfile::tempdir().expect("a directory");

        let result = run_command("exit 3", root.path(), &BTreeMap::new(), &Discarding, None).await;

        let StepResult::Failed { why } = result else {
            panic!("a failed step, not {result:?}");
        };

        assert!(why.contains('3'), "{why}");
    }

    /// The output reaches the sink while the command is running, which is what a client following
    /// the job's log is reading.
    #[tokio::test]
    async fn what_the_command_printed_reaches_the_log() {
        use std::sync::Mutex;

        struct Collecting(Mutex<Vec<String>>);

        impl Sink for Collecting {
            fn line(&self, line: LogLine) {
                self.0.lock().expect("the lock").push(line.text);
            }
        }

        let root = tempfile::tempdir().expect("a directory");
        let collected = Collecting(Mutex::new(Vec::new()));

        let result = run_command(
            "echo scaffolding",
            root.path(),
            &BTreeMap::new(),
            &collected,
            None,
        )
        .await;

        assert_eq!(result, StepResult::Done);
        assert!(
            collected
                .0
                .lock()
                .expect("the lock")
                .iter()
                .any(|line| line.contains("scaffolding")),
            "{:?}",
            collected.0.lock().expect("the lock")
        );
    }

    /// The shims go in front of what this daemon inherited, because that is how a blueprint's
    /// `[runtimes]` reaches a command that only ever types `php`.
    #[test]
    fn the_shim_directory_leads_the_path() {
        let home = tempfile::tempdir().expect("a directory");
        let paths = Paths::new(home.path().to_path_buf(), &Default::default());

        let env = environment(&paths);
        let path = env.get("PATH").expect("a PATH");

        assert!(
            path.starts_with(&paths.bin().display().to_string()),
            "{path}"
        );
    }
}
