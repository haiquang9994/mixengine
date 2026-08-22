//! A prompt nobody sees.
//!
//! The surface T40b's tests run against, and the reason `Elevation` is a capability on a `Host`
//! rather than a free function like `lock` or `signal`: a queue whose whole job is to batch pending
//! operations behind **one** prompt has to be testable without one.
//!
//! **Unlike the three real launchers, nothing here refuses a path.** A test asserting what a queue
//! raised should not also have to install a helper binary; the refusals are the launchers' own and
//! are asserted against the real host in `tests/elevation.rs`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mixengine_proto::privileged::ElevationOutcome;

use crate::{Elevation, ElevationSupport, Result};

/// One attempt to raise a prompt, as the mock recorded it.
///
/// Both paths, because the pair is the assertion worth making: one prompt, on the request that was
/// just written, with the helper the daemon was configured with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// The helper that would have been run.
    pub helper: PathBuf,

    /// The request that would have been its only argument.
    pub request: PathBuf,
}

#[derive(Debug)]
pub(super) struct Prompts {
    raised: Mutex<Vec<Prompt>>,

    /// What every attempt answers with. One scripted outcome rather than a queue of them: a caller
    /// that behaves differently on the second prompt of a batch is a caller that has broken the rule
    /// the batch exists for.
    answer: ElevationOutcome,
}

impl Default for Prompts {
    fn default() -> Self {
        Self {
            raised: Mutex::new(Vec::new()),
            answer: ElevationOutcome::Completed,
        }
    }
}

impl Prompts {
    /// A machine where every prompt is accepted.
    pub(super) fn accepting() -> Self {
        Self::default()
    }

    /// A machine where the person at it says no.
    pub(super) fn declining() -> Self {
        Self {
            answer: ElevationOutcome::Declined,
            ..Self::default()
        }
    }

    /// A machine with no way to raise a prompt at all, with `reason`.
    ///
    /// `&str` and not the `&'static str` its neighbours in this directory take: the wire type owns
    /// its reason, so there is nothing for a lifetime to buy here.
    pub(super) fn refusing(reason: &str) -> Self {
        Self {
            answer: ElevationOutcome::Unavailable {
                reason: reason.to_owned(),
            },
            ..Self::default()
        }
    }

    /// Every prompt this host was asked to raise, in order.
    pub(super) fn raised(&self) -> Vec<Prompt> {
        self.raised
            .lock()
            .expect("no test panics while holding this")
            .clone()
    }
}

impl Elevation for Prompts {
    fn probe(&self) -> ElevationSupport {
        match &self.answer {
            ElevationOutcome::Unavailable { reason } => ElevationSupport::Unavailable {
                reason: reason.clone(),
            },
            // A machine where the person declines can still raise a prompt, which is the whole
            // distinction `probe` exists to draw — and the one a caller gets wrong by reading
            // "declined" as "impossible".
            ElevationOutcome::Completed | ElevationOutcome::Declined => ElevationSupport::Available,
        }
    }

    fn run(&self, helper: &Path, request: &Path) -> Result<ElevationOutcome> {
        self.raised
            .lock()
            .expect("no test panics while holding this")
            .push(Prompt {
                helper: helper.to_path_buf(),
                request: request.to_path_buf(),
            });

        Ok(self.answer.clone())
    }
}
