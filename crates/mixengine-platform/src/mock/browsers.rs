//! Browsers that exist only in memory.

use crate::{BrowserSurvey, BrowserTrust, Result};

/// What a test says this machine's browsers hold.
#[derive(Debug)]
pub(crate) struct Browsers {
    answer: BrowserSurvey,
}

impl Default for Browsers {
    /// A machine MixEngine does not search.
    ///
    /// **Which is what every suite written before T49b should keep looking like** — the same
    /// reasoning as [`super::trust`]'s default, and the same mistake it exists to prevent: those
    /// suites were written against a home whose browsers nobody had asked about, and a default that
    /// quietly held the authority would rewrite what they assert.
    fn default() -> Self {
        Self {
            answer: BrowserSurvey::NotSearched {
                because: "this fixture's machine is not one MixEngine searches".to_owned(),
            },
        }
    }
}

impl Browsers {
    /// A machine answering exactly `survey`.
    pub(crate) fn answering(survey: BrowserSurvey) -> Self {
        Self { answer: survey }
    }
}

impl BrowserTrust for Browsers {
    fn survey(&self, _der: &[u8]) -> Result<BrowserSurvey> {
        Ok(self.answer.clone())
    }
}
