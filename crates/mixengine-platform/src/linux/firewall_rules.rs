//! Linux firewalls filter by port, not by program — roadmap task **T76**.

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
    /// renders that as a check that ran and explained itself.
    ///
    /// `ufw` and `firewalld` both take a port and neither takes a program, which is the same fact
    /// `crate::firewall::unix_tools` states from the writing side — and the reason a rule MixEngine
    /// wrote there cannot be found again by name either.
    fn naming(&self, _program: &Path) -> Result<Option<usize>> {
        Ok(None)
    }
}
