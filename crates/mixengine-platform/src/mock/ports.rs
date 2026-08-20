//! An in-memory listening table.
//!
//! The one capability whose mock needs no recorder: nothing here mutates the machine, so what a
//! test asserts on is the answer a caller was given and what it did with it — not the sequence of
//! questions it asked.

use std::collections::BTreeMap;

use crate::{Error, PortHolder, PortOwner, Result};

#[derive(Debug, Default)]
pub(super) struct Ports {
    /// What is listening, by port. Everything else is free.
    held: BTreeMap<u16, PortHolder>,

    /// Set for a machine that cannot answer the question at all.
    refuse: Option<&'static str>,
}

impl Ports {
    /// A machine where `port` is held by `holder`.
    pub(super) fn holding(port: u16, holder: PortHolder) -> Self {
        Self {
            held: BTreeMap::from([(port, holder)]),
            ..Self::default()
        }
    }

    /// A machine that will not say who is listening, with `reason`.
    ///
    /// **Not the same as a machine where nothing is listening**, which is the distinction every
    /// caller of this capability has to get right: one leaves a failure explained as best it can be,
    /// the other would replace a real explanation with silence.
    pub(super) fn refusing(reason: &'static str) -> Self {
        Self {
            refuse: Some(reason),
            ..Self::default()
        }
    }
}

impl PortOwner for Ports {
    fn listening_on(&self, port: u16) -> Result<Option<PortHolder>> {
        if let Some(reason) = self.refuse {
            return Err(Error::UnsupportedPlatform {
                capability: "PortOwner",
                reason: reason.to_owned(),
            });
        }

        Ok(self.held.get(&port).cloned())
    }
}
