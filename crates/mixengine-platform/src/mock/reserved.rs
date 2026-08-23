//! The mock's reserved ranges — whatever a test said, or none.

use crate::{PortRange, ReservedPorts, Result};

/// What this mock will answer.
#[derive(Debug, Default)]
pub(crate) struct Reserved {
    /// The ranges this machine pretends to have reserved.
    pub(crate) ranges: Vec<PortRange>,
}

impl ReservedPorts for Reserved {
    /// **Never `Unsupported`.** A mock that refused the question could not exercise the overlap,
    /// which is the one branch the real read cannot be made to take on demand.
    fn reserved(&self) -> Result<Vec<PortRange>> {
        Ok(self.ranges.clone())
    }
}
