//! Raising the polkit prompt on the one-shot helper.
//!
//! **`--disable-internal-agent`.** Without it, a `pkexec` that finds no authentication agent falls
//! back to the textual one built into itself, which prompts on the controlling terminal — a terminal
//! a daemon does not have and could not show anybody if it did. With it, `pkexec` fails at once. The
//! environment check below is a heuristic and can be wrong; this flag is what makes being wrong
//! cheap, because the failure is then a fast non-zero exit rather than a process waiting forever on
//! a tty nobody is watching. See the T40a design, D5, and ADR 0005, which calls this the worst
//! failure mode of the three systems.
//!
//! **No polkit action file is shipped.** `pkexec` run against a program with no registered action
//! falls back to `org.freedesktop.policykit.exec`, which asks for an administrator password and
//! caches the credential briefly — the same shape as the other two systems. Installing a `.policy`
//! file into `/usr/share/polkit-1/actions/` is itself a privileged operation, which would mean
//! needing elevation in order to be able to elevate.

use std::path::Path;
use std::process::Command;

use mixengine_proto::privileged::ElevationOutcome;

use crate::prompt::{self, linux as decide};
use crate::{Elevation, ElevationSupport, Result};

/// Resolved through `PATH` rather than spelled `/usr/bin/pkexec`: the check's first question is
/// whether polkit is on this machine at all, and that question is `PATH`'s.
const PKEXEC: &str = "pkexec";

#[derive(Debug)]
pub(crate) struct Prompt;

impl Elevation for Prompt {
    fn probe(&self) -> ElevationSupport {
        decide::missing(session(), None).map_or(ElevationSupport::Available, |reason| {
            ElevationSupport::Unavailable { reason }
        })
    }

    fn run(&self, helper: &Path, request: &Path) -> Result<ElevationOutcome> {
        prompt::usable("run as the elevation helper", helper)?;
        prompt::usable("hand to the elevation helper", request)?;

        // Composed once and used twice: it is the sentence a user is given whether the machine was
        // ruled out before the spawn or the spawn itself failed.
        let manual = format!("{PKEXEC} {} {}", helper.display(), request.display());

        // Rule 3 of platform-abstraction.md — detect, then act. Nothing is spawned on a machine that
        // has already been shown to have nowhere to draw a prompt.
        if let Some(reason) = decide::missing(session(), Some(&manual)) {
            return Ok(ElevationOutcome::Unavailable { reason });
        }

        let ran = Command::new(PKEXEC)
            .arg("--disable-internal-agent")
            .arg(helper)
            .arg(request)
            .status();

        let status = match ran {
            Ok(status) => status,
            Err(source) => {
                return Ok(ElevationOutcome::Unavailable {
                    reason: format!(
                        "{PKEXEC} could not be started ({source}). Run this by hand instead: {manual}"
                    ),
                });
            }
        };

        tracing::debug!(code = ?status.code(), "pkexec ended");

        Ok(decide::outcome(status.code()))
    }
}

/// What this machine's environment says about polkit.
fn session() -> decide::Session {
    decide::Session {
        pkexec: on_path(PKEXEC),
        // Either the address polkit's own client library reads, or the socket systemd puts in the
        // runtime directory — a session started by `systemd --user` has the second without always
        // exporting the first into a daemon's environment.
        bus: std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
            || std::env::var_os("XDG_RUNTIME_DIR")
                .is_some_and(|directory| Path::new(&directory).join("bus").exists()),
        graphical: std::env::var_os("DISPLAY").is_some()
            || std::env::var_os("WAYLAND_DISPLAY").is_some(),
    }
}

/// Is `tool` on this process's `PATH`?
fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join(tool).is_file())
    })
}
