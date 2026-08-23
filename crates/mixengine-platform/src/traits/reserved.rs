//! Ports this system has taken out of circulation — roadmap task **T47a**.

use crate::Result;

/// A range of ports, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange {
    /// The first port in the range.
    pub start: u16,

    /// The last port in the range, included.
    pub end: u16,
}

impl PortRange {
    /// Is `port` inside this range?
    #[must_use]
    pub fn holds(self, port: u16) -> bool {
        self.start <= port && port <= self.end
    }
}

/// What this system will not let anything bind, whoever asks.
///
/// **Three capabilities in this crate are about ports, and they answer three different questions.**
/// [`PortOwner`](crate::PortOwner) says who got there first. [`PortAccess`](crate::PortAccess) says
/// whether an unprivileged program may bind a *low* port at all. This one says whether the operating
/// system has reserved the range out from under everybody — and a bind into one of these fails with
/// an access error that **looks exactly like a permission problem and is not one**, which is the
/// whole reason it is worth a check of its own: a person who hits it goes looking at elevation, UAC
/// and the firewall, and none of them is the answer.
///
/// **Reads only, and needs no privilege on any system that has the concept at all.**
pub trait ReservedPorts: std::fmt::Debug + Send + Sync {
    /// Every range this system has reserved.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) on a system with no such
    /// concept, which `mix doctor` renders as a check that ran and says why it had nothing to
    /// examine; and [`Error::Io`](crate::Error::Io) when the reader could not be run.
    fn reserved(&self) -> Result<Vec<PortRange>>;
}
