//! macOS has no per-program inbound rule table to enumerate — roadmap task **T76**.

use std::path::Path;

use crate::{FirewallRules, Result};

/// Nothing to read here.
#[derive(Debug, Default)]
pub(crate) struct Rules;

impl FirewallRules for Rules {
    /// **[`None`] and not `Ok(0)`.**
    ///
    /// Zero would be the claim *this machine holds no such rule*, which is a statement about a
    /// table that does not exist. [`None`] says the question does not apply here, and `mix doctor`
    /// renders that as a check that ran and explained itself rather than as a clean bill of health.
    ///
    /// macOS' application firewall asks about *applications* and keeps its answers in its own
    /// store; `crate::firewall::unmanaged_on_macos` already says that a listening socket needs no
    /// rule at all here, which is the same fact from the writing side.
    fn naming(&self, _program: &Path) -> Result<Option<usize>> {
        Ok(None)
    }
}
