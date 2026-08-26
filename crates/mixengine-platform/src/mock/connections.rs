//! An in-memory count of who is connected.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::{ConnectionCount, Error, Result};

/// What is connected, by port. A port this does not name has nothing connected to it.
///
/// **Behind a `Mutex` where [`Ports`](super::ports::Ports) is not**, and the reason is what the two
/// are for. A listening table is a fact about the machine a test sets up once; this is the thing a
/// service *becomes* idle by, so a test of the sweeper has to change the answer between two readings
/// of one `Host`.
#[derive(Debug, Default)]
pub(super) struct Connections {
    /// How many connections each port has.
    open: Mutex<BTreeMap<u16, usize>>,

    /// Set for a machine that cannot answer the question at all.
    refuse: Option<&'static str>,
}

impl Connections {
    /// A machine with `open` connections, by port.
    pub(super) fn holding(open: BTreeMap<u16, usize>) -> Self {
        Self {
            open: Mutex::new(open),
            refuse: None,
        }
    }

    /// A machine that will not say how many connections a port has, with `reason`.
    ///
    /// **Not the same as a machine with nothing connected**, which is the distinction every caller
    /// of this capability has to get right: one is no measurement and the other is a measurement of
    /// nothing, and only the second may stop a service.
    pub(super) fn refusing(reason: &'static str) -> Self {
        Self {
            open: Mutex::new(BTreeMap::new()),
            refuse: Some(reason),
        }
    }

    /// Change what the next reading of `port` will say.
    pub(super) fn set(&self, port: u16, count: usize) {
        self.locked().insert(port, count);
    }

    /// The table, or a panic naming why it could not be taken.
    ///
    /// A poisoned lock here means a test panicked while holding it, and the useful report is that
    /// panic rather than a second one about a mutex.
    fn locked(&self) -> std::sync::MutexGuard<'_, BTreeMap<u16, usize>> {
        self.open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ConnectionCount for Connections {
    fn established_on(&self, port: u16) -> Result<usize> {
        if let Some(reason) = self.refuse {
            return Err(Error::UnsupportedPlatform {
                capability: "ConnectionCount",
                reason: reason.to_owned(),
            });
        }

        Ok(self.locked().get(&port).copied().unwrap_or_default())
    }
}
