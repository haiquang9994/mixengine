//! Port access that exists only in memory.

use std::path::Path;

use crate::{Error, PortAccessMethod, PortAccessState, PortBinding, Result};

/// What a test says this machine answers.
#[derive(Debug)]
pub(crate) struct Access {
    answer: std::result::Result<Answer, String>,
}

/// The three fields a fixture sets; the bindings are derived from the method, so a test that says
/// `Redirect` gets 8080 without having to spell it.
#[derive(Debug, Clone)]
struct Answer {
    method: PortAccessMethod,
    granted: bool,
    missing: Option<String>,
}

impl Default for Access {
    /// A machine that needs nothing — which is what Windows is, and what every suite written before
    /// T42 should keep looking like: they ask for no prompt and this is why.
    fn default() -> Self {
        Self {
            answer: Ok(Answer {
                method: PortAccessMethod::Direct,
                granted: true,
                missing: None,
            }),
        }
    }
}

impl Access {
    /// A machine using `method`, with the grant already in place.
    pub(crate) fn granting(method: PortAccessMethod) -> Self {
        Self {
            answer: Ok(Answer {
                method,
                granted: true,
                missing: None,
            }),
        }
    }

    /// A machine using `method` with the grant absent, and `missing` saying why.
    pub(crate) fn withholding(method: PortAccessMethod, missing: &str) -> Self {
        Self {
            answer: Ok(Answer {
                method,
                granted: false,
                missing: Some(missing.to_owned()),
            }),
        }
    }

    /// A machine that cannot say, with `reason`.
    pub(crate) fn refusing(reason: &str) -> Self {
        Self {
            answer: Err(reason.to_owned()),
        }
    }
}

impl crate::PortAccess for Access {
    /// The method the fixture was given decides the mapping, and a fixture that cannot probe at all
    /// still maps: the two questions are different, and "this machine cannot say whether the grant
    /// is in place" is not "this machine has no ports".
    fn bindings(&self, answering: &[u16]) -> Vec<PortBinding> {
        let method = self
            .answer
            .as_ref()
            .map_or(PortAccessMethod::Direct, |answer| answer.method);

        answering
            .iter()
            .map(|&answer_port| PortBinding {
                answer: answer_port,
                bind: bind(method, answer_port),
            })
            .collect()
    }

    fn probe(&self, _binary: &Path, answering: &[u16]) -> Result<PortAccessState> {
        let answer = self
            .answer
            .clone()
            .map_err(|reason| Error::UnsupportedPlatform {
                capability: "PortAccess",
                reason,
            })?;

        Ok(PortAccessState {
            method: answer.method,
            bindings: self.bindings(answering),
            granted: answer.granted,
            missing: answer.missing,
        })
    }
}

/// The mock maps ports the way the system each method belongs to does, so a fixture cannot describe
/// a machine none of the three could be.
fn bind(method: PortAccessMethod, answer: u16) -> u16 {
    match (method, answer) {
        (PortAccessMethod::Redirect, 80) => 8080,
        (PortAccessMethod::Redirect, 443) => 8443,
        _ => answer,
    }
}
