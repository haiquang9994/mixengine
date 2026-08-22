//! The record of what ran as root.
//!
//! **Outside `MIXENGINE_HOME`**, because a root-owned file inside a directory the user owns can be
//! renamed or unlinked by that user whatever its own mode says. `%ProgramData%\MixEngine`,
//! `/Library/Logs/MixEngine`, `/var/log/mixengine` — see
//! `mixengine_platform::elevated::audit_directory`.
//!
//! **Created by the helper on first run**, not by the installer: the helper has to work on a machine
//! no installer has touched, which is exactly the machine roadmap task T41a runs it on.
//!
//! **Written only when this process is elevated.** Under an ordinary token there is no privileged
//! directory to create and nothing worth recording — every operation with effects is refused there
//! anyway, by `PrivilegedOp::requires_elevation`.
//!
//! **No rotation.** A machine produces a few dozen lines over the lifetime of an installation, and
//! rotation here is code running as root to solve a problem that does not occur.
//!
//! **And it makes what ran readable, nothing more.** It prevents nothing. It does not stand between
//! an attacker and the binary-replacement path in the threat model — a helper that has been replaced
//! is also the thing writing the log.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mixengine_platform::elevated;
use mixengine_proto::privileged::OpOutcome;

/// The log's name inside the directory the platform layer names.
pub(crate) const FILE_NAME: &str = "elevate.log";

/// Where this helper records what it applied, whether or not it is there yet.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired into main.rs in Task 7 of this series")
)]
pub(crate) fn path() -> Result<PathBuf, String> {
    elevated::audit_directory()
        .map(|directory| directory.join(FILE_NAME))
        .map_err(|error| error.to_string())
}

/// Make sure the log's directory exists and belongs to an account that already had power.
///
/// Refuses rather than repairs: a directory that is already there and is not administrative was put
/// there by somebody, and on Windows `%ProgramData%` lets any account create one.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired into main.rs in Task 7 of this series")
)]
pub(crate) fn prepare(log: &Path) -> Result<(), String> {
    let directory = log
        .parent()
        .ok_or_else(|| format!("{} has no directory", log.display()))?;

    let Ok(metadata) = std::fs::symlink_metadata(directory) else {
        return elevated::create_root_owned_directory(directory).map_err(|error| error.to_string());
    };

    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{} is not a directory", directory.display()));
    }

    let owner = elevated::owner_of(directory).map_err(|error| error.to_string())?;
    if !owner.is_administrative() {
        return Err(format!(
            "{} belongs to {owner}, which is not an account this log may be kept by",
            directory.display()
        ));
    }

    Ok(())
}

/// One line of the log, as a document.
///
/// The calling identity is the one taken from the request file's owner, never from the request
/// document — the same rule the validation runs on.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired into main.rs in Task 7 of this series")
)]
pub(crate) fn entry(caller: &str, nonce: &str, op: &str, outcome: &OpOutcome) -> serde_json::Value {
    let (name, detail) = describe(outcome);

    serde_json::json!({
        "at": millis(),
        "version": env!("CARGO_PKG_VERSION"),
        "caller": caller,
        "nonce": nonce,
        "op": op,
        "outcome": name,
        "detail": detail,
    })
}

/// Append one line. Never a replace: replacing the file whole is the one thing that would destroy
/// the property this file exists to have.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired into main.rs in Task 7 of this series")
)]
pub(crate) fn append(log: &Path, entry: &serde_json::Value) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|source| format!("cannot open {}: {source}", log.display()))?;

    // One `write_all` and not a `writeln!` of two pieces: a single write of a line that fits in a
    // pipe buffer is what keeps two helpers' lines from interleaving inside one another.
    let mut line = entry.to_string();
    line.push('\n');

    file.write_all(line.as_bytes())
        .map_err(|source| format!("cannot write {}: {source}", log.display()))
}

/// Milliseconds since the epoch. A clock set before 1970 reads as 0 rather than failing the run: the
/// log is evidence and not a gate, and refusing to apply a hosts entry over a wrong clock helps
/// nobody.
fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The outcome's wire name, and whatever it carries.
fn describe(outcome: &OpOutcome) -> (&'static str, &str) {
    match outcome {
        OpOutcome::Applied { detail } => ("applied", detail),
        OpOutcome::AlreadyDone => ("already-done", ""),
        OpOutcome::Refused { reason } => ("refused", reason),
        OpOutcome::Unsupported { reason } => ("unsupported", reason),
        OpOutcome::Failed { message } => ("failed", message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_sits_in_the_directory_the_platform_names() {
        let log = path().expect("this OS names a place for it");

        assert_eq!(log.file_name().unwrap(), FILE_NAME);
        assert!(log.is_absolute(), "{}", log.display());
    }

    /// Epoch milliseconds and not a formatted date. Rendering a calendar date needs a date library,
    /// and adding one to a binary that runs as root to make a log line prettier is not a trade this
    /// budget makes — the daemon and `mix doctor` can format it.
    #[test]
    fn an_entry_carries_a_number_rather_than_a_calendar() {
        let line = entry("1000", "n", "probe", &OpOutcome::AlreadyDone);

        assert!(line["at"].is_u64(), "{line}");
        assert_eq!(line["op"], "probe");
        assert_eq!(line["outcome"], "already-done");
        assert_eq!(line["caller"], "1000");
        assert_eq!(line["nonce"], "n");
        assert_eq!(line["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn a_failure_records_what_the_os_said() {
        let line = entry(
            "1000",
            "n",
            "probe",
            &OpOutcome::Failed {
                message: "the disk is full".to_owned(),
            },
        );

        assert_eq!(line["outcome"], "failed");
        assert_eq!(line["detail"], "the disk is full");
    }

    /// One line per operation per invocation, and the file is appended to. Replacing it whole is the
    /// one thing that would destroy the property it exists to have.
    #[test]
    fn appending_twice_keeps_both_lines() {
        let directory = tempfile::TempDir::new().unwrap();
        let log = directory.path().join(FILE_NAME);

        append(&log, &entry("1000", "a", "probe", &OpOutcome::AlreadyDone)).unwrap();
        append(&log, &entry("1000", "b", "probe", &OpOutcome::AlreadyDone)).unwrap();

        let written = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = written.lines().collect();

        assert_eq!(lines.len(), 2, "{written}");
        for line in lines {
            serde_json::from_str::<serde_json::Value>(line)
                .expect("each line is a document of its own");
        }
    }

    /// D7: a directory root appends into is itself a target. One that is already there and belongs
    /// to an ordinary account was arranged by somebody.
    #[test]
    fn a_directory_an_ordinary_account_created_is_refused() {
        let directory = tempfile::TempDir::new().unwrap();
        let log = directory.path().join("MixEngine").join(FILE_NAME);
        std::fs::create_dir(log.parent().unwrap()).unwrap();

        let refused = prepare(&log);

        // Under an administrative token this test's own directory *is* administrative, and the
        // premise does not hold — which is a pass, not a skip: what is asserted is that `prepare`
        // and `is_administrative` agree, whichever way round they come out.
        let owner = elevated::owner_of(log.parent().unwrap()).unwrap();
        assert_eq!(refused.is_ok(), owner.is_administrative(), "{refused:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_where_the_directory_should_be_is_refused() {
        let directory = tempfile::TempDir::new().unwrap();
        let real = directory.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = directory.path().join("MixEngine");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(prepare(&link.join(FILE_NAME)).is_err());
    }
}
