//! Browsers that exist only in memory.

use std::sync::Mutex;

use crate::{BrowserChange, BrowserSurvey, BrowserTrust, Result};

/// What a test says this machine's browsers hold, and what was asked of them.
///
/// **Records rather than discards**, on the rule this module's header states: once mutations exist,
/// a fixture answers from memory *and* keeps what it was asked to do, so a suite asserts on the
/// recorded sequence instead of on a side effect it cannot see.
#[derive(Debug)]
pub(crate) struct Browsers {
    answer: BrowserSurvey,
    installed: Mutex<Vec<Vec<u8>>>,
    removed: Mutex<Vec<String>>,
}

impl Default for Browsers {
    /// A machine MixEngine does not search.
    ///
    /// **Which is what every suite written before T49b should keep looking like** — the same
    /// reasoning as [`super::trust`]'s default, and the same mistake it exists to prevent: those
    /// suites were written against a home whose browsers nobody had asked about, and a default that
    /// quietly held the authority would rewrite what they assert.
    fn default() -> Self {
        Self::answering(BrowserSurvey::NotSearched {
            because: "this fixture's machine is not one MixEngine searches".to_owned(),
        })
    }
}

impl Browsers {
    /// A machine answering exactly `survey`.
    pub(crate) fn answering(survey: BrowserSurvey) -> Self {
        Self {
            answer: survey,
            installed: Mutex::new(Vec::new()),
            removed: Mutex::new(Vec::new()),
        }
    }

    /// Every certificate this was asked to hold.
    pub(crate) fn installed(&self) -> Vec<Vec<u8>> {
        self.installed
            .lock()
            .expect("the fixture's log is not poisoned")
            .clone()
    }

    /// Every authority this was asked to let go of.
    pub(crate) fn removed(&self) -> Vec<String> {
        self.removed
            .lock()
            .expect("the fixture's log is not poisoned")
            .clone()
    }

    /// The paths the recorded answer says are short, which is what a real install would write into.
    fn lacking(&self) -> Vec<String> {
        self.answer
            .lacking()
            .into_iter()
            .map(|one| one.path.clone())
            .collect()
    }
}

impl BrowserTrust for Browsers {
    fn survey(&self, _der: &[u8]) -> Result<BrowserSurvey> {
        Ok(self.answer.clone())
    }

    fn install(&self, der: &[u8]) -> Result<BrowserChange> {
        self.installed
            .lock()
            .expect("the fixture's log is not poisoned")
            .push(der.to_vec());

        Ok(BrowserChange {
            written: self.lacking(),
            refused: Vec::new(),
        })
    }

    fn remove(&self, key_id: &str) -> Result<BrowserChange> {
        self.removed
            .lock()
            .expect("the fixture's log is not poisoned")
            .push(key_id.to_owned());

        Ok(BrowserChange::default())
    }
}
