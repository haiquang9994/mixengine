//! Linux: no such mechanism — roadmap task **T94**.

use crate::{AppControl, AppControlState, Error, Result};

/// This system's answer, which is that the question does not apply here.
#[derive(Debug, Default)]
pub(crate) struct Policy;

impl AppControl for Policy {
    fn state(&self) -> Result<AppControlState> {
        Err(Error::UnsupportedPlatform {
            capability: "AppControl",
            reason: "Linux has no policy that refuses to load a program for want of a code \
                     signature"
                .to_owned(),
        })
    }
}
