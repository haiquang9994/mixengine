//! Running a Windows tool as an argument vector.
//!
//! Here rather than inside either caller because both `access` (behind the `host` feature) and
//! `elevated` need it, and a build that takes one without the other must still compile. The rule it
//! exists to keep is the one in the T40 design, D9: the program is **named**, never a command line —
//! no quoting rules, no interpolation, nothing for a path containing a space or a quote to break.

use std::ffi::OsStr;
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::{Error, Result};

/// Start this child without a console window, wherever in the platform layer it is started from.
///
/// **Every `Command` this crate runs on Windows has to say this, not only the supervised ones.** A
/// process that has no console — a detached `mixengined`, and so every daemon a client autostarts —
/// gives a console subsystem child nothing to inherit, and Windows answers that by creating a
/// console for it. On Windows 11 a new console is handed to the *default terminal application*,
/// which opens a window of its own; with the default setting of "let Windows decide" that is
/// Windows Terminal. So the eight `icacls` calls that make a home private became eight terminal
/// windows on the desktop, one per call, every time a daemon started. Measured, not reasoned about:
/// one `mixengined --detach` produced nine of them.
///
/// `CREATE_NO_WINDOW` is the answer for a child whose output we read: the console is still created,
/// so `.output()` gets its pipes as usual, and no window is ever handed out for it.
pub(crate) fn without_a_window(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Run a Windows tool and hand back its stdout.
///
/// The program is named, never a command line: no quoting rules, no interpolation, nothing for a
/// path containing a space or a quote to break.
pub(crate) fn run<'a>(
    command: &'static str,
    path: Option<&Path>,
    args: impl IntoIterator<Item = &'a OsStr>,
) -> Result<String> {
    let mut process = Command::new(system32(command));
    process.args(args);

    // Or a daemon with no console of its own opens a terminal window for every call — see
    // [`without_a_window`].
    without_a_window(&mut process);

    let output = process.output();

    let output = match output {
        Ok(output) => output,
        // Not `Error::Io`: that variant is about the path, and here it is the tool that is
        // missing, which is a different thing to go and fix.
        Err(source) => {
            return Err(Error::Command {
                command,
                path: path.map(Path::to_path_buf),
                status: "could not be started".to_owned(),
                output: source.to_string(),
            });
        }
    };

    if !output.status.success() {
        return Err(Error::Command {
            command,
            path: path.map(Path::to_path_buf),
            status: output.status.to_string(),
            // `icacls` reports refusals on stdout as often as on stderr; a message that dropped
            // half of them would be worse than useless.
            output: [&output.stderr, &output.stdout]
                .iter()
                .map(|stream| String::from_utf8_lossy(stream).trim().to_owned())
                .filter(|said| !said.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a Windows tool and hand back its stdout **whatever it exited with** — task **T76**.
///
/// [`run`]'s sibling, and the difference is the whole reason it exists: `netsh advfirewall firewall
/// show rule` exits non-zero when nothing matches, which for a *count* is the answer zero rather
/// than a failure. [`run`] would turn the ordinary case into an error.
///
/// Only for a tool whose exit status carries no information the caller needs. Everything that
/// changes the machine goes through [`run`], where a refusal is a refusal.
///
/// # Errors
///
/// [`Error::Command`] where the tool could not be started at all.
pub(crate) fn output_of<'a>(
    command: &'static str,
    args: impl IntoIterator<Item = &'a OsStr>,
) -> Result<String> {
    let mut process = Command::new(system32(command));
    process.args(args);
    without_a_window(&mut process);

    match process.output() {
        Ok(output) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        Err(source) => Err(Error::Command {
            command,
            path: None,
            status: "could not be started".to_owned(),
            output: source.to_string(),
        }),
    }
}

/// `%SystemRoot%\System32\<tool>.exe`, not whatever `PATH` resolves `<tool>` to.
///
/// A daemon started from a shell whose `PATH` leads with a POSIX toolbox — Git for Windows ships a
/// `whoami.exe` that speaks a completely different language — would otherwise parse the wrong
/// program's output. Falls back to the bare name if Windows will not say where it lives, which is
/// no worse than not having tried.
pub(crate) fn system32(tool: &str) -> PathBuf {
    std::env::var_os("SystemRoot").map_or_else(
        || Path::new(tool).to_path_buf(),
        |root| {
            Path::new(&root)
                .join("System32")
                .join(format!("{tool}.exe"))
        },
    )
}
