//! The mock's inbound rules — whatever a test said, or a machine holding none.

use std::path::Path;

use crate::{FirewallRules, Result};

/// What this mock will answer.
#[derive(Debug)]
pub(crate) struct Rules {
    /// How many rules name whatever it is asked about, or [`None`] for a system with no such table.
    pub(crate) count: Option<usize>,
}

impl Default for Rules {
    /// **`Some(0)` and not `None`.**
    ///
    /// The default has to be the ordinary machine, which is a Windows holding no rule of this kind
    /// — [`None`] is macOS' and Linux' answer, that the question does not apply, and a test about
    /// those says so rather than inheriting it.
    fn default() -> Self {
        Self { count: Some(0) }
    }
}

impl FirewallRules for Rules {
    /// **Never fails, and ignores the program.** A machine that cannot be asked about its own rules
    /// is an OS error the real implementation reports; what a test arranges here is the answer, and
    /// in every real call the path asked about is this daemon's own.
    fn naming(&self, _program: &Path) -> Result<Option<usize>> {
        Ok(self.count)
    }
}
