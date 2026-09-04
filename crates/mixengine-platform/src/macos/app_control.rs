//! macOS: no such mechanism — roadmap task **T94**.
//!
//! Gatekeeper is not this question in another accent. It gates a *quarantined download* once, and
//! T86a's **M5** measured that nothing MixEngine's package installs carries `com.apple.quarantine`
//! afterwards — so there is no policy here that judges every image load, and nothing to report.

use crate::{AppControl, AppControlState, Error, Result};

/// This system's answer, which is that the question does not apply here.
#[derive(Debug, Default)]
pub(crate) struct Policy;

impl AppControl for Policy {
    fn state(&self) -> Result<AppControlState> {
        Err(Error::UnsupportedPlatform {
            capability: "AppControl",
            reason: "macOS gates a quarantined download through Gatekeeper rather than refusing \
                     every unsigned image at load, and nothing the package installs is quarantined"
                .to_owned(),
        })
    }
}
