//! Starting a desktop application and judging it — roadmap task **T83**, D8, D9 and D11.
//!
//! One launcher for three systems, over [`crate::process::spawn_detached`]: the application inherits
//! this process's environment — a GUI needs the session, which is the opposite of what a supervised
//! child gets — plus whatever the caller adds, which is one credential or nothing.
//!
//! # The judgement
//!
//! [`JUDGEMENT`] after the spawn, the child is asked whether it has exited. Still up is
//! [`Started::Running`]. A clean exit is [`Started::HandedOn`], because that is what a single-instance
//! application does when a copy is already running: forward `argv` and exit 0. Anything else is
//! [`Started::Failed`].
//!
//! # The reaper
//!
//! On Unix a detached child stays this process's child (`setsid` only), so a daemon that never waits
//! on it leaves a zombie for as long as the daemon runs. One thread, started on the first launch,
//! polls every [`REAP_EVERY`] and drops what has ended. On Windows the same thread closes the handle.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::process::{self, Detached};
use crate::{InstalledApp, Result, Started};

/// How long a started application is watched before it is called running.
pub(crate) const JUDGEMENT: Duration = Duration::from_secs(1);

/// How often the judgement looks.
const GLANCE: Duration = Duration::from_millis(50);

/// How often the reaper looks.
const REAP_EVERY: Duration = Duration::from_secs(2);

/// Every application started and not yet seen to exit.
static ADOPTED: OnceLock<Mutex<Vec<Detached>>> = OnceLock::new();

/// Start `app`, judge it, and hand what is still running to the reaper.
///
/// # Errors
///
/// [`Error::Io`](crate::Error::Io) naming the program when it cannot be started;
/// [`Error::Os`](crate::Error::Os) when the OS will not say whether it has exited.
pub(crate) fn launch(
    app: &InstalledApp,
    args: &[OsString],
    env: &BTreeMap<String, String>,
) -> Result<Started> {
    let mut all = app.args.clone();
    all.extend(args.iter().cloned());

    // The program's own directory: what Explorer and Finder give it, and never this daemon's home,
    // which the application would then pin for its whole life.
    let directory = app
        .program
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);

    let mut child = process::spawn_detached(&app.program, &all, &directory, env)?;
    let pid = child.pid();
    let deadline = Instant::now() + JUDGEMENT;

    loop {
        if let Some(exit) = child.exited()? {
            return Ok(if exit.is_success() {
                Started::HandedOn
            } else {
                Started::Failed {
                    status: exit.to_string(),
                }
            });
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(GLANCE);
    }

    adopt(child);
    tracing::debug!(pid, program = %app.program.display(), "started a desktop application");

    Ok(Started::Running { pid })
}

/// Hand a running child to the reaper, starting it if this is the first.
fn adopt(child: Detached) {
    let adopted = ADOPTED.get_or_init(|| {
        std::thread::Builder::new()
            .name("mixengine-reaper".to_owned())
            .spawn(reap)
            .expect("a thread can be started");
        Mutex::new(Vec::new())
    });

    adopted
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(child);
}

/// Forever: drop every adopted child that has exited.
fn reap() {
    loop {
        std::thread::sleep(REAP_EVERY);

        if let Some(adopted) = ADOPTED.get() {
            adopted
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain_mut(|child| match child.exited() {
                    Ok(None) => true,
                    Ok(Some(exit)) => {
                        tracing::debug!(pid = child.pid(), %exit, "a desktop application ended");
                        false
                    }
                    Err(error) => {
                        tracing::debug!(
                            pid = child.pid(),
                            %error,
                            "a desktop application cannot be waited on"
                        );
                        false
                    }
                });
        }
    }
}

/// Reading the two texts an installer leaves behind, on every system.
///
/// Compiled on all three systems so that each reader is tested on every one of them — `prompt`'s
/// arrangement, and for its reason: the part most likely to be wrong is the parse, and a parse only
/// compiled on the system that calls it is a parse only tested there.
#[allow(
    dead_code,
    reason = "each reader here is compiled on all three systems and called on one: `exec_line` by \
              Linux's locator and `unquoted` by Windows', while the tests below read both everywhere"
)]
pub(crate) mod entry {
    /// The program and its fixed arguments out of a desktop entry's `Exec=` value.
    ///
    /// Field codes (`%u`, `%U`, `%f`, `%F`, `%i`, `%c`, `%k`, …) are dropped; `"quoted words"` are
    /// one word; `%%` is a literal `%`. [`None`] for a line with no program in it.
    pub(crate) fn exec_line(value: &str) -> Option<(String, Vec<String>)> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut quoted = false;
        let mut chars = value.trim().chars();

        while let Some(c) = chars.next() {
            match c {
                '"' => quoted = !quoted,
                '\\' if quoted => {
                    if let Some(next) = chars.next() {
                        word.push(next);
                    }
                }
                ' ' | '\t' if !quoted => {
                    if !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                }
                // A field code is dropped with the letter after it; `%%` is a literal.
                '%' => {
                    if chars.next() == Some('%') {
                        word.push('%');
                    }
                }
                other => word.push(other),
            }
        }
        if !word.is_empty() {
            words.push(word);
        }

        let mut words = words.into_iter();
        let program = words.next()?;
        Some((program, words.collect()))
    }

    /// A registry path as an installer wrote it: quotation marks and a trailing `,<icon index>`
    /// removed. `C:\a\b.exe`, `"C:\a\b.exe"` and `"C:\a\b.exe",0` are one path.
    pub(crate) fn unquoted(value: &str) -> String {
        let trimmed = value.trim();

        if let Some(rest) = trimmed.strip_prefix('"') {
            // Quoted: the path ends at the closing quote, and whatever follows — an icon index —
            // is not part of it.
            return rest
                .split_once('"')
                .map_or(rest, |(inner, _)| inner)
                .to_owned();
        }

        // Bare: a trailing `,<number>` is an icon index and not part of the path. A comma
        // followed by anything else stays, since a directory may be named with one.
        trimmed
            .rsplit_once(',')
            .filter(|(_, index)| index.trim().parse::<i32>().is_ok())
            .map_or(trimmed, |(path, _)| path)
            .trim()
            .to_owned()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn an_exec_line_drops_field_codes_and_keeps_fixed_arguments() {
            assert_eq!(exec_line("mixdb %U"), Some(("mixdb".to_owned(), vec![])));
            assert_eq!(
                exec_line("\"/opt/My App/bin/mixdb\" --flag %u"),
                Some((
                    "/opt/My App/bin/mixdb".to_owned(),
                    vec!["--flag".to_owned()]
                ))
            );
            assert_eq!(
                exec_line("env FOO=1 mixdb"),
                Some((
                    "env".to_owned(),
                    vec!["FOO=1".to_owned(), "mixdb".to_owned()]
                ))
            );
            assert_eq!(exec_line("%U"), None);
            assert_eq!(exec_line("   "), None);
            assert_eq!(
                exec_line("a 100%% b"),
                Some(("a".to_owned(), vec!["100%".to_owned(), "b".to_owned()]))
            );
        }

        #[test]
        fn a_registry_path_loses_its_quotes_and_its_icon_index() {
            assert_eq!(unquoted(r#""C:\a\b.exe""#), r"C:\a\b.exe");
            assert_eq!(unquoted(r#""C:\a b\c.exe",0"#), r"C:\a b\c.exe");
            assert_eq!(unquoted(r"C:\a\b.exe,0"), r"C:\a\b.exe");
            assert_eq!(unquoted(r"C:\a\b.exe"), r"C:\a\b.exe");
            assert_eq!(unquoted(r"C:\a,b\c.exe"), r"C:\a,b\c.exe");
        }
    }
}
