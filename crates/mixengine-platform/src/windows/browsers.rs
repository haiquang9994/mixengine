//! Windows: MixEngine does not search browser certificate databases here.
//!
//! **This says what MixEngine did and not what Firefox reads** — the T49b design, D2. Firefox on
//! Windows may well read the machine's own store, through `security.enterprise_roots`; no machine
//! available to that task had one installed, so nothing here claims it either way. D14 of the
//! design records how to find out, which is one `about:config` entry away for anybody who has one.
//!
//! It is also a mechanism decision and not only a scope decision. Windows ships an unrelated
//! `certutil.exe` in `C:\WINDOWS\system32` — CryptoAPI's, with an entirely different command line —
//! so a `PATH`-resolved `certutil` here finds the wrong program. Confining the search to Linux
//! means that collision never arises.

use crate::{BrowserSurvey, BrowserTrust, Result};

/// This system's answer.
#[derive(Debug, Default)]
pub(crate) struct Browsers;

impl BrowserTrust for Browsers {
    fn survey(&self, _der: &[u8]) -> Result<BrowserSurvey> {
        Ok(BrowserSurvey::NotSearched {
            because: "MixEngine does not search browser certificate databases on Windows; this \
                      machine's own trusted roots are what the line above reports"
                .to_owned(),
        })
    }
}
