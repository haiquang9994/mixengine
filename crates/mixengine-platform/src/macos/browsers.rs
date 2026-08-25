//! macOS: MixEngine does not search browser certificate databases here.
//!
//! **This says what MixEngine did and not what Firefox reads** — the T49b design, D2. Firefox on
//! macOS may well read the System keychain, through `security.enterprise_roots`; no machine
//! available to that task had one installed, so nothing here claims it either way. D14 of the
//! design records how to find out, which is one `about:config` entry away for anybody who has one.

use crate::{BrowserSurvey, BrowserTrust, Result};

/// This system's answer.
#[derive(Debug, Default)]
pub(crate) struct Browsers;

impl BrowserTrust for Browsers {
    fn survey(&self, _der: &[u8]) -> Result<BrowserSurvey> {
        Ok(BrowserSurvey::NotSearched {
            because:
                "MixEngine does not search browser certificate databases on macOS; the System \
                      keychain is what the line above reports"
                    .to_owned(),
        })
    }
}
