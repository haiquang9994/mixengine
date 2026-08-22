//! Raising the administrator prompt on the one-shot helper.
//!
//! `do shell script … with administrator privileges`, which is Apple's remaining supported way to
//! run one program elevated from a program that is not: `AuthorizationExecuteWithPrivileges` has
//! been deprecated since 10.7, and `SMJobBless` installs a **persistent** privileged helper, which
//! ADR 0005 rules out.
//!
//! **The script is a compile-time constant and the two paths arrive as `argv`** — see
//! `crate::prompt::macos::SCRIPT`, and the T40a design, D3. Interpolating a path into an AppleScript
//! string literal is the line T40/D9 singles out as the single most dangerous one in the elevation
//! path on any of the three systems, and `quoted form of` is AppleScript's own answer to it.

use std::path::Path;
use std::process::Command;

use mixengine_proto::privileged::ElevationOutcome;

use crate::prompt::{self, macos as decide};
use crate::{Elevation, ElevationSupport, Result};

/// Part of the base system since Mac OS X 10.0, and named absolutely so that the daemon's own `PATH`
/// cannot decide what runs.
const OSASCRIPT: &str = "/usr/bin/osascript";

#[derive(Debug)]
pub(crate) struct Prompt;

impl Elevation for Prompt {
    fn probe(&self) -> ElevationSupport {
        if Path::new(OSASCRIPT).is_file() {
            return ElevationSupport::Available;
        }

        // Nearly a constant, and honest about what it cannot see: a session with no window server
        // would still answer `Available` here, and detecting *that* cheaply and correctly is not
        // something this task has a way to do. See T40a, D6.
        ElevationSupport::Unavailable {
            reason: format!(
                "{OSASCRIPT} is not on this machine, so no administrator prompt can be raised"
            ),
        }
    }

    fn run(&self, helper: &Path, request: &Path) -> Result<ElevationOutcome> {
        prompt::usable("run as the elevation helper", helper)?;
        prompt::usable("hand to the elevation helper", request)?;

        let mut osascript = Command::new(OSASCRIPT);
        for line in decide::SCRIPT {
            osascript.arg("-e").arg(line);
        }

        // Both are absolute, so neither can be mistaken for an option by osascript's own argument
        // parsing, and both are quoted by AppleScript rather than by us.
        osascript.arg(helper).arg(request);

        let ran = match osascript.output() {
            Ok(output) => output,
            Err(source) => {
                return Ok(ElevationOutcome::Unavailable {
                    reason: format!("{OSASCRIPT} could not be started ({source})"),
                });
            }
        };

        let complaint = String::from_utf8_lossy(&ran.stderr);
        tracing::debug!(
            code = ?ran.status.code(),
            stderr = %complaint.trim(),
            "osascript ended"
        );

        Ok(decide::outcome(ran.status.code(), &complaint))
    }
}
