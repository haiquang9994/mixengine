//! The half of the three launchers that is a decision rather than a call.
//!
//! `windows/prompt.rs`, `macos/prompt.rs` and `linux/prompt.rs` are mapped onto `sys` by `#[path]`,
//! so each is compiled on its own system alone — and so is any test beside it. The part of a
//! launcher most likely to be wrong is its table: which exit code means the person said no, which
//! means there was nobody to ask, how an argument is quoted. Those live here, as pure functions over
//! a code and a string, in a module compiled on all three systems, so that each system's table is
//! tested on every one of them. What stays in `sys::prompt` is the call that can be made nowhere
//! else. See the T40a design, "Testing".

#![allow(
    dead_code,
    reason = "each table here is compiled on all three systems and used on one, which is the \
              module's whole purpose: on Windows the `macos` and `linux` tables are read by the \
              tests below and by nothing else, and the same holds on the other two"
)]

use std::ffi::OsString;
use std::io;
use std::path::Path;

use mixengine_proto::privileged::ElevationOutcome;

use crate::{Error, Result};

/// Refuse a path this crate is not willing to hand to a mechanism that will run it as root.
///
/// Absolute, and an existing file. Checked in one place rather than in each launcher: three
/// implementations of one check is two chances to forget it, and what is being checked is about to
/// become a root process's command line.
///
/// # Errors
///
/// [`Error::Io`] naming the path, with `action` completing "cannot …".
pub(crate) fn usable(action: &'static str, path: &Path) -> Result<()> {
    let complaint = if path.is_absolute() {
        if path.is_file() {
            None
        } else {
            Some(io::Error::new(
                io::ErrorKind::NotFound,
                "there is no file there",
            ))
        }
    } else {
        Some(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path is not absolute",
        ))
    };

    complaint.map_or(Ok(()), |source| {
        Err(Error::Io {
            action,
            path: path.to_path_buf(),
            source,
        })
    })
}

/// Windows' table, and the one argument its mechanism has to be given as a string.
pub(crate) mod windows {
    use super::{Error, OsString, Path, Result, io};

    /// `ERROR_CANCELLED`, which is what `ShellExecuteExW` reports when the person dismisses UAC.
    ///
    /// Spelled out rather than imported, so the table can be read and tested on a machine that is
    /// not Windows. `windows/prompt.rs` asserts it against `windows_sys`' own constant, which is
    /// what stops the two from drifting.
    pub(crate) const ERROR_CANCELLED: u32 = 1223;

    /// The single string `ShellExecuteExW` takes as the child's parameters: the request path, quoted.
    ///
    /// **The quoting is provable here rather than conventional.** `"` is not a legal character in a
    /// Windows path name, so no path can close the quotation early — and a path carrying one anyway
    /// is refused, which is what turns that from a belief into a check. See the T40a design, D4.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the path contains a quotation mark.
    pub(crate) fn parameters(request: &Path) -> Result<OsString> {
        if request.as_os_str().to_string_lossy().contains('"') {
            return Err(Error::Io {
                action: "quote for the elevation prompt",
                path: request.to_path_buf(),
                source: io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a quotation mark is not a legal character in a Windows path name",
                ),
            });
        }

        let mut quoted = OsString::from("\"");
        quoted.push(request.as_os_str());
        quoted.push("\"");

        Ok(quoted)
    }
}

/// macOS' table, and the script that is a constant.
pub(crate) mod macos {
    use super::ElevationOutcome;

    /// The AppleScript, as the three `-e` lines it is given in.
    ///
    /// **A compile-time constant with no value of ours anywhere in it.** The two paths arrive as
    /// `argv` and are quoted by AppleScript's own `quoted form of`, which is the one operator on
    /// that system whose entire job is being right about spaces, quotes and newlines in a shell
    /// word. Interpolating a path into this string instead is the line T40/D9 singles out as the
    /// single most dangerous one in the elevation path on any of the three systems — T40a, D3.
    pub(crate) const SCRIPT: [&str; 3] = [
        "on run argv",
        "do shell script (quoted form of (item 1 of argv)) & \" \" \
         & (quoted form of (item 2 of argv)) with administrator privileges",
        "end run",
    ];

    /// The AppleScript error code trailing `stderr`, when osascript left one.
    ///
    /// `execution error: User canceled. (-128)` — the number in the last parentheses is the whole of
    /// what this system hands back. `do shell script` raises an error rather than returning a
    /// status, so the helper's own exit code arrives here too, as a positive number.
    pub(crate) fn error_code(stderr: &str) -> Option<i32> {
        let tail = stderr.trim_end().strip_suffix(')')?;
        let open = tail.rfind('(')?;

        tail[open + 1..].parse().ok()
    }

    /// What osascript's answer means, in the three words the trait speaks.
    ///
    /// `-128` is `userCanceledErr`, and it is why T40/D11 said a declined prompt cannot be an exit
    /// code of the helper's. A **positive** code is the helper's own status, reported through an
    /// AppleScript error because that is how `do shell script` reports one — the helper ran, so the
    /// answer is `Completed` and whatever it thought is beside the request. Any other negative code
    /// is AppleScript refusing to do it at all; so is a run that left no code to read, because there
    /// is then nothing to say it ever started.
    pub(crate) fn outcome(code: Option<i32>, stderr: &str) -> ElevationOutcome {
        if code == Some(0) {
            return ElevationOutcome::Completed;
        }

        match error_code(stderr) {
            Some(-128) => ElevationOutcome::Declined,
            Some(status) if status > 0 => ElevationOutcome::Completed,
            _ => ElevationOutcome::Unavailable {
                reason: format!(
                    "macOS would not raise an administrator prompt: {}",
                    stderr.trim()
                ),
            },
        }
    }
}

/// Linux's table, and the environment question the other two systems do not have.
pub(crate) mod linux {
    use super::ElevationOutcome;

    /// What `pkexec` needs on this machine, as far as the environment will say.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct Session {
        /// Is `pkexec` on `PATH`?
        pub(crate) pkexec: bool,

        /// Is there a session bus for polkit to reach an authentication agent over?
        pub(crate) bus: bool,

        /// Is there a graphical session for an agent to draw a prompt in?
        pub(crate) graphical: bool,
    }

    /// What is missing, or `None` when a prompt can be raised.
    ///
    /// `manual` is the command a person could run by hand, and is `Some` only where the caller knows
    /// it. [`Elevation::probe`](crate::Elevation::probe) takes no paths, so it cannot compose one and
    /// its reason stops at what is absent; `run` has both paths and appends the command. The design
    /// asked for the command in the reason without saying which of the two callers could produce one
    /// — this is that resolution, and it is why `manual` is an `Option` rather than a `&str`.
    pub(crate) fn missing(session: Session, manual: Option<&str>) -> Option<String> {
        let absent = if !session.pkexec {
            "pkexec is not on PATH, so polkit is not installed"
        } else if !session.bus {
            "there is no session bus ($DBUS_SESSION_BUS_ADDRESS is unset and $XDG_RUNTIME_DIR has \
             no bus socket), so polkit cannot reach an authentication agent"
        } else if !session.graphical {
            "there is no graphical session ($DISPLAY and $WAYLAND_DISPLAY are both unset), so no \
             authentication agent can show a password prompt"
        } else {
            return None;
        };

        Some(match manual {
            Some(command) => format!("{absent}. Run this by hand instead: {command}"),
            None => absent.to_owned(),
        })
    }

    /// What `pkexec`'s exit code means, in the three words the trait speaks.
    ///
    /// `126` is a dismissed dialog and `127` is "not authorised, or something went wrong" — the two
    /// numbers T40/D2 kept every exit code of the helper's below 125 to stay clear of, and this is
    /// the task that spends them. Anything else is the helper's own status: it ran, and what it
    /// thought is beside the request. A run with no code at all was ended by a signal, which is not
    /// a report that it started.
    pub(crate) fn outcome(code: Option<i32>) -> ElevationOutcome {
        match code {
            Some(126) => ElevationOutcome::Declined,
            Some(127) => ElevationOutcome::Unavailable {
                reason:
                    "pkexec would not authorise this — either no authentication agent answered, \
                         or the request was refused"
                        .to_owned(),
            },
            Some(_) => ElevationOutcome::Completed,
            None => ElevationOutcome::Unavailable {
                reason: "pkexec was killed before it could say anything".to_owned(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path that is absolute and is a file, on whichever system this is.
    fn a_real_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::TempDir::new().expect("the temporary directory is writable");
        let path = directory.path().join("mixengine-elevate");
        std::fs::write(&path, b"not really a binary").expect("a file in a temporary directory");

        (directory, path)
    }

    #[test]
    fn an_absolute_existing_file_is_usable() {
        let (_directory, path) = a_real_file();

        assert!(usable("run as the elevation helper", &path).is_ok());
    }

    #[test]
    fn a_relative_path_is_refused_before_anything_is_run() {
        let error = usable(
            "run as the elevation helper",
            Path::new("mixengine-elevate"),
        )
        .expect_err("a relative path is not run as root");

        assert!(
            error.to_string().contains("run as the elevation helper"),
            "{error}"
        );
    }

    #[test]
    fn a_path_with_no_file_at_it_is_refused() {
        let (directory, _path) = a_real_file();

        assert!(
            usable(
                "run as the elevation helper",
                &directory.path().join("absent")
            )
            .is_err()
        );
    }

    #[test]
    fn windows_quotes_the_request_path() {
        let quoted = windows::parameters(Path::new(r"C:\Program Files\MixEngine\request.json"))
            .expect("a path with a space is exactly what the quoting is for");

        assert_eq!(
            quoted,
            OsString::from("\"C:\\Program Files\\MixEngine\\request.json\"")
        );
    }

    /// D4: the guarantee is enforced rather than assumed. `"` cannot occur in a Windows path name,
    /// and a path carrying one anyway never reaches a command line.
    #[test]
    fn windows_refuses_a_path_carrying_a_quotation_mark() {
        let error = windows::parameters(Path::new("C:\\a\"b\\request.json"))
            .expect_err("a quotation mark would end the quoting early");

        assert!(
            error.to_string().contains("quote for the elevation prompt"),
            "{error}"
        );
    }

    #[test]
    fn macos_reads_the_code_out_of_an_applescript_error() {
        assert_eq!(
            macos::error_code("execution error: User canceled. (-128)"),
            Some(-128)
        );
        assert_eq!(
            macos::error_code("execution error: something went wrong. (65)\n"),
            Some(65)
        );
        assert_eq!(macos::error_code("no code here at all"), None);
    }

    #[test]
    fn macos_maps_its_three_cases() {
        assert_eq!(macos::outcome(Some(0), ""), ElevationOutcome::Completed);
        assert_eq!(
            macos::outcome(Some(1), "execution error: User canceled. (-128)"),
            ElevationOutcome::Declined
        );
        assert_eq!(
            macos::outcome(Some(1), "execution error: refused. (65)"),
            ElevationOutcome::Completed,
            "a positive code is the helper's own status, so the helper ran"
        );
        assert!(matches!(
            macos::outcome(Some(1), "execution error: Not authorized. (-1743)"),
            ElevationOutcome::Unavailable { .. }
        ));
        assert!(matches!(
            macos::outcome(None, ""),
            ElevationOutcome::Unavailable { .. }
        ));
    }

    /// D5, all four states of the probe.
    #[test]
    fn linux_names_whichever_piece_of_polkit_is_missing() {
        let complete = linux::Session {
            pkexec: true,
            bus: true,
            graphical: true,
        };

        assert_eq!(linux::missing(complete, None), None);

        let reason = linux::missing(
            linux::Session {
                pkexec: false,
                ..complete
            },
            None,
        )
        .expect("no pkexec, no prompt");
        assert!(reason.contains("pkexec"), "{reason}");

        let reason = linux::missing(
            linux::Session {
                bus: false,
                ..complete
            },
            None,
        )
        .expect("no bus, no agent");
        assert!(reason.contains("session bus"), "{reason}");

        let reason = linux::missing(
            linux::Session {
                graphical: false,
                ..complete
            },
            None,
        )
        .expect("no graphical session, nowhere to draw a prompt");
        assert!(reason.contains("graphical session"), "{reason}");
    }

    /// The fallback ADR 0005 asks for: a machine that cannot prompt says what to type instead.
    #[test]
    fn linux_puts_the_manual_command_in_the_reason_when_it_has_one() {
        let reason = linux::missing(
            linux::Session {
                pkexec: true,
                bus: true,
                graphical: false,
            },
            Some("pkexec /opt/mixengine/mixengine-elevate /tmp/one/request.json"),
        )
        .expect("no graphical session");

        assert!(
            reason.ends_with("pkexec /opt/mixengine/mixengine-elevate /tmp/one/request.json"),
            "{reason}"
        );
    }

    #[test]
    fn linux_maps_the_two_numbers_pkexec_reserves() {
        assert_eq!(linux::outcome(Some(126)), ElevationOutcome::Declined);
        assert!(matches!(
            linux::outcome(Some(127)),
            ElevationOutcome::Unavailable { .. }
        ));
        assert_eq!(linux::outcome(Some(0)), ElevationOutcome::Completed);
        assert_eq!(
            linux::outcome(Some(65)),
            ElevationOutcome::Completed,
            "65 is the helper's own refusal code, and the helper having run is the answer here"
        );
        assert!(matches!(
            linux::outcome(None),
            ElevationOutcome::Unavailable { .. }
        ));
    }
}
