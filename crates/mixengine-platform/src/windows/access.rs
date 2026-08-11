//! An explicit DACL, because the inherited one is not private.
//!
//! `%LOCALAPPDATA%` inherits an owner-only ACL and needs nothing. Every other location does: a
//! volume root grants `BUILTIN\Users` read and execute with `(OI)(CI)`, which every directory
//! created below it inherits — so a `MIXENGINE_HOME` on `D:\`, or a `[paths]` override onto a
//! second disk, is readable by every local account until this runs.
//!
//! The work is done by `icacls` rather than by `SetNamedSecurityInfoW`. This crate may lift the
//! workspace ban on `unsafe` per item, so that is not the obstacle; the obstacle is that building a
//! DACL through the Win32 API means hand-computing ACL sizes behind raw pointers, where a mistake
//! produces a *wrong ACL* rather than a crash — a silent failure in the one place that must not
//! fail silently. The crates that wrap it safely (`windows-acl`, `windows-permissions`) have both
//! been frozen on the unmaintained `winapi 0.3` since 2021.
//!
//! `icacls` ships with Windows, is called here with an argument vector rather than an interpolated
//! command line, and names well-known accounts by SID so a localised Windows cannot change what a
//! grant means. Its own answer is checked: a non-zero exit fails the start.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::{DirectoryAccess, Error, Result};

/// `NT AUTHORITY\SYSTEM`. Named by SID: the display name is localised, the SID is not.
const SYSTEM: &str = "S-1-5-18";

/// `BUILTIN\Administrators`, likewise.
const ADMINISTRATORS: &str = "S-1-5-32-544";

/// Full control, inherited by both files and subdirectories.
const FULL: &str = "(OI)(CI)F";

/// The flag `icacls` prints on an ACE that came from the parent rather than from us.
const INHERITED: &str = "(I)";

/// How many ACEs [`Access::restrict_to_owner`] leaves behind: the user, `SYSTEM`, `Administrators`.
///
/// Counting them is what lets the check notice a *fourth* — an explicit grant to somebody else,
/// which carries no `(I)` flag and would otherwise be indistinguishable from one of ours, because
/// `icacls` prints localised names and never a SID.
const GRANTS: usize = 3;

#[derive(Debug, Default)]
pub(crate) struct Access {
    /// Answered once per process: it cannot change while we run, and it costs a subprocess.
    sid: OnceLock<String>,
}

impl DirectoryAccess for Access {
    fn restrict_to_owner(&self, path: &Path) -> Result<()> {
        let sid = self.current_user_sid()?;

        let owner = format!("*{sid}:{FULL}");
        let system = format!("*{SYSTEM}:{FULL}");
        let administrators = format!("*{ADMINISTRATORS}:{FULL}");

        // Two calls, because one cannot do this: `icacls` rejects `/reset` in company
        // ("Invalid parameter /inheritance:r"), and without `/reset` an ACE that is *explicit*
        // rather than inherited survives everything below. A directory someone had already shared,
        // or one restored from a backup with its ACL, would keep granting whoever it granted while
        // reporting itself locked down. `chmod` has no such hole, and neither may this.
        //
        // The order costs a moment in which the directory carries only its inherited ACL — the
        // state it would have been in had it just been created, and no worse than the gap between
        // `create_dir` and getting here.
        run(
            "icacls",
            Some(path),
            [path.as_os_str(), OsStr::new("/reset"), OsStr::new("/q")],
        )?;

        // `/inheritance:r` is the one that matters now. Without it the grants below are merely
        // added to what the volume root handed down, and `BUILTIN\Users` keeps its read access.
        // `/grant:r` replaces an existing grant for the same account rather than accumulating a
        // second one every time the daemon starts.
        run(
            "icacls",
            Some(path),
            [
                path.as_os_str(),
                OsStr::new("/inheritance:r"),
                OsStr::new("/grant:r"),
                OsStr::new(&owner),
                OsStr::new("/grant:r"),
                OsStr::new(&system),
                OsStr::new("/grant:r"),
                OsStr::new(&administrators),
                // Say nothing on success: the daemon's log is not a place for one line per
                // directory per start.
                OsStr::new("/q"),
            ],
        )?;

        Ok(())
    }

    fn is_restricted_to_owner(&self, path: &Path) -> Result<bool> {
        // `icacls` does report a missing path, but as a failure indistinguishable from every other
        // failure. Ask the filesystem first, so the caller gets `NotFound` rather than a parse of
        // an English error string.
        std::fs::metadata(path).map_err(|source| Error::Io {
            action: "read the permissions of",
            path: path.to_path_buf(),
            source,
        })?;

        let listing = run("icacls", Some(path), [path.as_os_str()])?;

        Ok(matches_what_we_apply(&listing))
    }
}

impl Access {
    /// The SID of the account this process runs as.
    ///
    /// `whoami /user` rather than the `USERNAME`/`USERDOMAIN` environment pair: those are
    /// inherited, and a daemon started from a shell where they had been edited would hand the
    /// directory to the wrong account. A SID also survives a rename, which a name in an ACL does
    /// not.
    fn current_user_sid(&self) -> Result<&str> {
        if let Some(sid) = self.sid.get() {
            return Ok(sid);
        }

        let output = run(
            "whoami",
            None,
            ["/user", "/fo", "csv", "/nh"].map(OsStr::new),
        )?;

        // `"machine\user","S-1-5-21-…"` — the SID is the last quoted field.
        let sid = output
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .and_then(|line| line.rsplit('"').nth(1))
            .filter(|sid| sid.starts_with("S-1-"))
            .ok_or_else(|| Error::Command {
                command: "whoami",
                path: None,
                status: "unexpected output".to_owned(),
                output: output.trim().to_owned(),
            })?;

        // A second caller may have won the race and stored an identical answer; either is correct.
        Ok(self.sid.get_or_init(|| sid.to_owned()))
    }
}

/// Has every inherited ACE been replaced by one of ours?
///
/// Does this listing show exactly the DACL [`Access::restrict_to_owner`] writes?
///
/// Two things have to hold: nothing is inherited, and there are exactly [`GRANTS`] ACEs. The
/// second is doing real work — an explicit ACE granting a fourth account carries no `(I)` flag,
/// so counting is the only way to see it without resolving every printed name back to a SID, which
/// `icacls` gives no way to do and which localisation makes unsafe to guess at.
///
/// It still cannot say *who* the three are. If T47 needs that, the way out is
/// `GetNamedSecurityInfoW` plus `GetSecurityDescriptorControl` for `SE_DACL_PROTECTED` — the first
/// half of this answer, read from the descriptor instead of from printed text — and `GetAce` with
/// `EqualSid` for the second. That is `unsafe` FFI, which this crate may use per item; it was not
/// worth writing for a function with no caller yet. See T47 in the roadmap.
fn matches_what_we_apply(listing: &str) -> bool {
    let mut aces = 0;

    for line in listing.lines() {
        // Each ACE ends in `<trustee>:(flags)`; the path on the first line and the summary on the
        // last have no such tail. Searching from the right keeps a directory named `D:\Old (I)`
        // from being read as a set of flags.
        let Some(start) = line.rfind(":(") else {
            continue;
        };

        if line[start + 1..].contains(INHERITED) {
            return false;
        }

        aces += 1;
    }

    // Not `>=`: a fourth ACE is the case this exists to catch. Not `!= 0` either — an empty DACL
    // shuts everyone out including us, which is not what we applied and not something `mix doctor`
    // should pass over in silence.
    aces == GRANTS
}

/// Run a Windows tool and hand back its stdout.
///
/// The program is named, never a command line: no quoting rules, no interpolation, nothing for a
/// path containing a space or a quote to break.
fn run<'a>(
    command: &'static str,
    path: Option<&Path>,
    args: impl IntoIterator<Item = &'a OsStr>,
) -> Result<String> {
    let mut process = Command::new(system32(command));
    process.args(args);

    // Or a daemon with no console of its own opens a terminal window for every call — see
    // [`without_a_window`](super::process::without_a_window).
    super::process::without_a_window(&mut process);

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

/// `%SystemRoot%\System32\<tool>.exe`, not whatever `PATH` resolves `<tool>` to.
///
/// A daemon started from a shell whose `PATH` leads with a POSIX toolbox — Git for Windows ships a
/// `whoami.exe` that speaks a completely different language — would otherwise parse the wrong
/// program's output. Falls back to the bare name if Windows will not say where it lives, which is
/// no worse than not having tried.
fn system32(tool: &str) -> PathBuf {
    std::env::var_os("SystemRoot").map_or_else(
        || Path::new(tool).to_path_buf(),
        |root| {
            Path::new(&root)
                .join("System32")
                .join(format!("{tool}.exe"))
        },
    )
}
