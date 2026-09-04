//! The mock's application control policy — whatever a test said.

use crate::{AppControl, AppControlState, Error, Result};

/// What this mock will answer.
#[derive(Debug)]
pub(crate) struct Policy {
    /// The state this mock reports, or the reason it refuses the question.
    answer: std::result::Result<AppControlState, &'static str>,
}

impl Default for Policy {
    /// The ordinary machine: nothing is refusing images here.
    fn default() -> Self {
        Self {
            answer: Ok(AppControlState::Off),
        }
    }
}

impl Policy {
    /// A machine in `state`.
    pub(crate) fn reporting(state: AppControlState) -> Self {
        Self { answer: Ok(state) }
    }

    /// A machine that cannot be asked, with `reason` — the other two systems' answer.
    pub(crate) fn refusing(reason: &'static str) -> Self {
        Self {
            answer: Err(reason),
        }
    }
}

impl AppControl for Policy {
    fn state(&self) -> Result<AppControlState> {
        self.answer.map_err(|reason| Error::UnsupportedPlatform {
            capability: "AppControl",
            reason: reason.to_owned(),
        })
    }
}
